use std::path::Path;

fn validate_part(index: usize, path: &str, part: &str) -> Result<usize, String> {
    let line_count = part.lines().count();
    if line_count > 600 {
        return Err(format!(
            "Slint part exceeds the repository limit ({line_count} lines): {path}"
        ));
    }
    if part.contains("experimental_gate.slint") {
        return Err(format!(
            "Experimental Slint imports are not allowed in production parts: {path}"
        ));
    }
    if index != 1 && part.contains("export component AppWindow") {
        return Err("AppWindow must be declared only in part_01.slint".into());
    }
    Ok(part.matches("export component AppWindow").count())
}

fn validate_window_contract(part: &str) -> Result<(), String> {
    for required in [
        "preferred-width: 1600px",
        "preferred-height: 1000px",
        "min-width: 1080px",
        "min-height: 680px",
        "forward-focus:",
    ] {
        if !part.contains(required) {
            return Err(format!(
                "AppWindow is missing required UI contract: {required}"
            ));
        }
    }
    Ok(())
}

fn build_failure(stage: &str, detail: impl std::fmt::Display) -> ! {
    // Keep build failures actionable in both Cargo and IDE output. A bare
    // panic loses whether the source read, generated-file publication, or
    // Slint compiler was responsible for the failure.
    println!("cargo:warning=Aura UI build failure [{stage}]: {detail}");
    eprintln!("Aura UI build failure [{stage}]: {detail}");
    std::process::exit(1);
}

fn main() {
    use std::fs;

    println!("cargo:rerun-if-changed=ui/aura_studio.slint");
    println!("cargo:rerun-if-changed=ui/aura_studio_parts/README.md");
    let mut source = String::new();
    const PART_COUNT: usize = 4;
    let mut component_exports = 0;
    for index in 1..=PART_COUNT {
        let path = format!("ui/aura_studio_parts/part_{index:02}.slint");
        println!("cargo:rerun-if-changed={path}");
        let part = fs::read_to_string(&path)
            .unwrap_or_else(|error| build_failure("read-source", format!("{path}: {error}")));
        if index == 1 {
            validate_window_contract(&part)
                .unwrap_or_else(|error| build_failure("validate-source", error));
        }
        component_exports += validate_part(index, &path, &part)
            .unwrap_or_else(|error| build_failure("validate-source", error));
        source.push_str(&part);
        source.push('\n');
    }
    if component_exports != 1 {
        build_failure(
            "validate-source",
            format!("expected exactly one AppWindow contract, found {component_exports}"),
        );
    }

    let generated = Path::new("ui/.aura_studio.generated.slint");
    let temporary = Path::new("ui/.aura_studio.generated.slint.tmp");
    let _ = fs::remove_file(temporary);
    fs::write(temporary, source)
        .unwrap_or_else(|error| build_failure("write-generated-source", error));
    fs::rename(temporary, generated)
        .unwrap_or_else(|error| build_failure("publish-generated-source", error));
    let generated_path = generated
        .to_str()
        .unwrap_or_else(|| build_failure("compile-slint", "generated path is not UTF-8"));
    let result = slint_build::compile(generated_path);
    let _ = fs::remove_file(generated);
    if let Err(error) = result {
        build_failure("compile-slint", error);
    }
}

#[cfg(test)]
mod tests {
    use super::{validate_part, validate_window_contract};

    #[test]
    fn accepts_a_valid_part() {
        assert_eq!(
            validate_part(1, "part_01.slint", "export component AppWindow {}\n").unwrap(),
            1
        );
    }

    #[test]
    fn rejects_experimental_imports() {
        let error = validate_part(
            2,
            "part_02.slint",
            "import { X } from \"experimental_gate.slint\";",
        )
        .unwrap_err();
        assert!(error.contains("Experimental"));
    }

    #[test]
    fn rejects_duplicate_root() {
        let error =
            validate_part(2, "part_02.slint", "export component AppWindow {}\n").unwrap_err();
        assert!(error.contains("part_01"));
    }

    #[test]
    fn rejects_parts_over_six_hundred_lines() {
        let source = (0..601).map(|_| "// line\n").collect::<String>();
        let error = validate_part(3, "part_03.slint", &source).unwrap_err();
        assert!(error.contains("600 lines"));
    }

    #[test]
    fn requires_responsive_window_and_focus_contract() {
        let source = "preferred-width: 1600px; preferred-height: 1000px; min-width: 1080px; min-height: 680px; forward-focus: fc;";
        assert!(validate_window_contract(source).is_ok());
        let error = validate_window_contract("preferred-width: 1600px;").unwrap_err();
        assert!(error.contains("UI contract"));
    }
}
