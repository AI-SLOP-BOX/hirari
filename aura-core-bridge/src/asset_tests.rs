use super::AssetOrchestrator;
use std::fs;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

fn temp_project() -> std::path::PathBuf {
    static NEXT_ID: AtomicU64 = AtomicU64::new(0);
    let id = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
    let path = std::env::temp_dir().join(format!("aura-asset-test-{}-{id}", NEXT_ID.fetch_add(1, Ordering::Relaxed)));
    fs::create_dir(&path).unwrap();
    path
}

#[test]
fn consolidates_referenced_assets_and_records_size() {
    let project = temp_project();
    let source = project.join("aura-asset-source.wav");
    fs::write(&source, b"audio bytes").unwrap();
    let timeline = format!(r#"{{"regions":[{{"file_path":"{}"}}]}}"#, source.display());
    let mut orchestrator = AssetOrchestrator::new();
    assert!(orchestrator.consolidate_assets(project.to_str().unwrap(), timeline.as_bytes()));
    assert_eq!(orchestrator.assets.len(), 1);
    assert_eq!(orchestrator.assets[0].size, 11);
    assert_eq!(fs::read(&orchestrator.assets[0].consolidated_path).unwrap(), b"audio bytes");
    assert!(orchestrator.audit_assets());
    let _ = fs::remove_file(source);
    let _ = fs::remove_dir_all(project);
}

#[test]
fn rejects_missing_references_and_keeps_previous_state() {
    let project = temp_project();
    let mut orchestrator = AssetOrchestrator::new();
    assert!(!orchestrator.consolidate_assets(project.to_str().unwrap(), br#"{"asset_path":"missing.wav"}"#));
    assert!(orchestrator.assets.is_empty());
    let _ = fs::remove_dir_all(project);
}

#[test]
fn same_basenames_use_content_fingerprints_and_leave_no_asset_temp_files() {
    let project = temp_project();
    let first_dir = project.join("one");
    let second_dir = project.join("two");
    fs::create_dir_all(&first_dir).unwrap();
    fs::create_dir_all(&second_dir).unwrap();
    fs::write(first_dir.join("take.wav"), b"first audio").unwrap();
    fs::write(second_dir.join("take.wav"), b"second audio").unwrap();
    let timeline = serde_json::json!({"regions": [{"file_path": "one/take.wav"}, {"file_path": "two/take.wav"}]});
    let mut orchestrator = AssetOrchestrator::new();
    assert!(orchestrator.consolidate_assets(project.to_str().unwrap(), timeline.to_string().as_bytes()));
    assert_eq!(orchestrator.assets.len(), 2);
    assert_ne!(orchestrator.assets[0].consolidated_path, orchestrator.assets[1].consolidated_path);
    assert!(fs::read_dir(project.join("Assets")).unwrap().all(|entry| !entry.unwrap().file_name().to_string_lossy().contains("asset-tmp-")));
    assert!(orchestrator.audit_assets());
    let _ = fs::remove_dir_all(project);
}

#[test]
fn rewrite_api_removes_external_asset_reference() {
    let project = temp_project();
    let source = project.join("outside").join("vocal.wav");
    fs::create_dir_all(source.parent().unwrap()).unwrap();
    fs::write(&source, b"audio bytes").unwrap();
    let timeline = serde_json::json!({"regions": [{"file_path": source.to_string_lossy()}]});
    let mut orchestrator = AssetOrchestrator::new();
    let rewritten = orchestrator.consolidate_assets_and_rewrite_json(project.to_str().unwrap(), timeline.to_string().as_bytes()).expect("rewritten project must be returned");
    let value: serde_json::Value = serde_json::from_slice(&rewritten).unwrap();
    let path = value["regions"][0]["file_path"].as_str().unwrap();
    assert!(path.starts_with("Assets/"));
    assert!(project.join(path).is_file());
    let _ = fs::remove_dir_all(project);
}

#[cfg(unix)]
#[test]
fn rejects_symlinked_assets_before_copying() {
    let project = temp_project();
    let source = project.join("real.wav");
    let link = project.join("linked.wav");
    fs::write(&source, b"audio bytes").unwrap();
    std::os::unix::fs::symlink(&source, &link).unwrap();
    let mut orchestrator = AssetOrchestrator::new();
    assert!(!orchestrator.consolidate_assets(project.to_str().unwrap(), serde_json::json!({"asset_path": "linked.wav"}).to_string().as_bytes()));
    assert!(orchestrator.assets.is_empty());
    assert!(!project.join("Assets").join("linked.wav").exists());
    let _ = fs::remove_dir_all(project);
}
