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
        return Err("AppWindow must be declared only in app_window.slint".into());
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
    println!("cargo:rerun-if-changed=ui/arrange_dock.slint");
    for ui_source in [
        "ui/audio_settings.slint",
        "ui/mixer/full_mixer.slint",
        "ui/mixer/mixer_strip.slint",
        "ui/editor/piano_roll.slint",
        "ui/editor/sample_editor.slint",
        "ui/editor/sample_editor_adapter.slint",
        "ui/editor/synth_panel.slint",
        "ui/pages/mastering_page.slint",
        "ui/pages/workflow_pages.slint",
    ] {
        println!("cargo:rerun-if-changed={ui_source}");
    }
    println!("cargo:rerun-if-changed=ui/aura_studio_parts/README.md");
    for font in [
        "resources/fonts/IBMPlexSans-Regular.ttf",
        "resources/fonts/IBMPlexSans-SemiBold.ttf",
        "resources/fonts/IBMPlexSans-Bold.ttf",
    ] {
        println!("cargo:rerun-if-changed={font}");
    }
    for asset in [
        "ui/assets/aura-mark.svg",
        "ui/assets/waveform.svg",
        "ui/assets/mixer.svg",
        "ui/assets/synth.svg",
        "ui/assets/mastering.svg",
    ] {
        println!("cargo:rerun-if-changed={asset}");
    }
    let mut source = String::new();
    const PART_COUNT: usize = 4;
    let mut component_exports = 0;
    for index in 1..=PART_COUNT {
        let path = match index {
            1 => "ui/aura_studio_parts/app_window.slint".to_owned(),
            2 => "ui/aura_studio_parts/automation_view.slint".to_owned(),
            3 => "ui/aura_studio_parts/editor_views.slint".to_owned(),
            4 => "ui/aura_studio_parts/dialogs_and_tools.slint".to_owned(),
            _ => unreachable!(),
        };
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
    let temporary_name = format!("ui/.aura_studio.generated.slint.tmp.{}", std::process::id());
    let temporary = Path::new(&temporary_name);
    fs::write(temporary, source)
        .unwrap_or_else(|error| build_failure("write-generated-source", error));
    if let Err(error) = fs::rename(temporary, generated) {
        let _ = fs::remove_file(temporary);
        build_failure("publish-generated-source", error);
    }
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
            validate_part(1, "app_window.slint", "export component AppWindow {}\n").unwrap(),
            1
        );
    }

    #[test]
    fn rejects_experimental_imports() {
        let error = validate_part(
            2,
            "automation_view.slint",
            "import { X } from \"experimental_gate.slint\";",
        )
        .unwrap_err();
        assert!(error.contains("Experimental"));
    }

    #[test]
    fn rejects_duplicate_root() {
        let error = validate_part(
            2,
            "automation_view.slint",
            "export component AppWindow {}\n",
        )
        .unwrap_err();
        assert!(error.contains("app_window"));
    }

    #[test]
    fn rejects_parts_over_six_hundred_lines() {
        let source = (0..601).map(|_| "// line\n").collect::<String>();
        let error = validate_part(3, "editor_views.slint", &source).unwrap_err();
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
