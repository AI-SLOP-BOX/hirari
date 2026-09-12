use super::*;

#[test]
fn version_and_container_encoding_are_stable() {
    assert_eq!(CORE_API_VERSION, "aura.core.v1");
    assert_eq!(WaveContainer::Wav.native_format(), 0);
    assert_eq!(WaveContainer::Wave64.native_format(), 1);
    assert_eq!(
        serde_json::to_string(&WaveContainer::Wave64).unwrap(),
        "\"wave64\""
    );
}

#[test]
fn headless_api_rejects_empty_paths() {
    let api = CoreApiV1::new_headless().expect("offline core should initialize");
    let error = api.open_project("").expect_err("empty path must fail");
    assert_eq!(error.code, "invalid_project_path");
}

#[test]
fn capability_contract_is_versioned_and_actionable() {
    let api = CoreApiV1::new_headless().expect("offline core should initialize");
    let value: serde_json::Value = serde_json::from_str(&api.engine_capabilities_json())
        .expect("capability contract must be valid JSON");
    assert_eq!(value["schema"], "aura.engine-capabilities.v1");
    let capabilities = value["capabilities"]
        .as_array()
        .expect("capabilities array");
    assert!(capabilities
        .iter()
        .any(|entry| entry["id"] == "direct_routing"));
    assert!(capabilities
        .iter()
        .any(|entry| entry["status"] == "integration_required"));
    let vocabulary = value["status_vocabulary"]
        .as_array()
        .expect("status vocabulary must be present");
    for entry in capabilities {
        let status = entry["status"].as_str().expect("capability status");
        assert!(vocabulary.iter().any(|item| item.as_str() == Some(status)));
        if status == "implemented" {
            assert!(entry["verification"].as_str().is_some());
        }
    }
}

#[test]
fn capability_contract_marks_external_integrations_as_unverified() {
    let api = CoreApiV1::new_headless().expect("offline core should initialize");
    let value: serde_json::Value = serde_json::from_str(&api.engine_capabilities_json())
        .expect("capability contract must be valid JSON");
    let capabilities = value["capabilities"]
        .as_array()
        .expect("capabilities array");
    for id in [
        "audio_device_io",
        "vst3_sandbox_worker",
        "ara2_protocol_endpoint",
        "windows_asio_hardware",
    ] {
        let entry = capabilities
            .iter()
            .find(|entry| entry["id"] == id)
            .unwrap_or_else(|| panic!("missing capability {id}"));
        assert_ne!(
            entry["status"], "implemented",
            "{id} must not claim verified integration"
        );
    }
}

#[test]
fn shared_core_publishes_state_changes_to_api_subscribers() {
    let api = CoreApiV1::new_headless().expect("offline core should initialize");
    let track_id = api.core().add_track(0);
    assert_ne!(track_id, 0);
    let batch = api
        .subscribe_events(EventCursor { after: 0 }, 16)
        .expect("event subscription should work");
    assert!(batch.events.iter().any(|event| matches!(
        event.event,
        ProductionEvent::TrackAdded { track_id: id, .. } if id == track_id
    )));
}
