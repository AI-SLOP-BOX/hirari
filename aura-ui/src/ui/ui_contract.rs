//! Pure UI contract tests kept outside the Slint event wiring.

#[cfg(test)]
mod tests {
    use crate::slint_ui::{
        bounce_state_label, clamp_selection_index, format_bytes, inspect_rendered_wav,
        render_progress_status, ui_error_message, ui_error_with_action, BounceState, UiErrorKind,
    };

    #[test]
    fn selection_index_is_safe_for_empty_and_out_of_range_models() {
        assert_eq!(clamp_selection_index(-10, 0), 0);
        assert_eq!(clamp_selection_index(-10, 3), 0);
        assert_eq!(clamp_selection_index(1, 3), 1);
        assert_eq!(clamp_selection_index(99, 3), 2);
    }

    #[test]
    fn byte_labels_are_stable_at_unit_boundaries() {
        assert_eq!(format_bytes(0), "0 B");
        assert_eq!(format_bytes(1023), "1023 B");
        assert_eq!(format_bytes(1024), "1.0 KB");
        assert_eq!(format_bytes(1024 * 1024), "1.0 MB");
    }

    #[test]
    fn bounce_states_have_stable_labels_and_unknown_fallback() {
        assert_eq!(BounceState::from(1), BounceState::Queued);
        assert_eq!(bounce_state_label(2), "RENDERING");
        assert_eq!(bounce_state_label(5), "CANCELLED");
        assert_eq!(bounce_state_label(999), "ENGINE_STATUS_UNKNOWN");
    }

    #[test]
    fn render_progress_status_has_determinate_and_indeterminate_forms() {
        assert_eq!(render_progress_status(2, true, 0.375, 4), "RENDERING · 38%");
        assert_eq!(
            render_progress_status(2, false, f32::NAN, 12),
            "RENDERING · 12s elapsed · progress pending"
        );
        assert_eq!(render_progress_status(3, false, 0.0, 0), "COMPLETE");
    }

    #[test]
    fn error_messages_have_stable_user_facing_categories() {
        assert_eq!(
            ui_error_message(UiErrorKind::AudioDevice, "device not ready"),
            "Audio device: device not ready"
        );
        assert_eq!(ui_error_message(UiErrorKind::Render, ""), "Render");
        assert_eq!(
            ui_error_message(UiErrorKind::Plugin, "scan failed"),
            "Plugin: scan failed"
        );
        assert_eq!(
            ui_error_message(UiErrorKind::Engine, "render failed"),
            "Engine: render failed"
        );
    }

    #[test]
    fn actionable_errors_keep_category_and_add_recovery_guidance() {
        assert_eq!(
            ui_error_with_action(
                UiErrorKind::AudioDevice,
                "reconnect failed",
                "Open Audio Settings"
            ),
            "Audio device: reconnect failed · Open Audio Settings"
        );
        assert_eq!(
            ui_error_with_action(UiErrorKind::Plugin, "scan failed", ""),
            "Plugin: scan failed"
        );
    }

    #[test]
    fn render_inspector_rejects_missing_and_corrupt_outputs() {
        let missing = std::env::temp_dir().join(format!(
            "aura-missing-render-{}-{}.wav",
            std::process::id(),
            crate::slint_ui::unix_time_millis()
        ));
        assert_eq!(
            inspect_rendered_wav(&missing),
            "出力ファイルを読み込めません。保存先の権限と空き容量を確認してください"
        );

        let corrupt = std::env::temp_dir().join(format!(
            "aura-corrupt-render-{}-{}.wav",
            std::process::id(),
            crate::slint_ui::unix_time_millis()
        ));
        std::fs::write(&corrupt, b"not a wav").expect("write corrupt fixture");
        assert_eq!(
            inspect_rendered_wav(&corrupt),
            "出力ファイルは有効なWAVではありません。別の形式または保存先を確認してください"
        );
        let _ = std::fs::remove_file(corrupt);
    }

    #[test]
    fn render_inspector_reports_float_wav_payloads() {
        let path = std::env::temp_dir().join(format!(
            "aura-float-inspector-{}-{}.wav",
            std::process::id(),
            crate::slint_ui::unix_time_millis()
        ));
        aura_core_bridge::export::write_wav_float32(&path, &[0.25, -0.5], 48_000, 1)
            .expect("float fixture must be writable");
        let summary = inspect_rendered_wav(&path);
        assert!(summary.contains("48000 Hz · 1 ch · 32 bit float"));
        assert!(summary.contains("2 frames"));
        assert!(summary.contains("-6.0 dBFS"));
        std::fs::remove_file(path).expect("float fixture cleanup must succeed");
    }

    #[test]
    fn render_inspector_accepts_rf64_with_ds64_data_size() {
        let path = std::env::temp_dir().join(format!(
            "aura-rf64-inspector-{}-{}.wav",
            std::process::id(),
            crate::slint_ui::unix_time_millis()
        ));
        let mut bytes = vec![0u8; 76];
        bytes[0..4].copy_from_slice(b"RF64");
        bytes[8..12].copy_from_slice(b"WAVE");
        bytes[12..16].copy_from_slice(b"ds64");
        bytes[16..20].copy_from_slice(&16u32.to_le_bytes());
        bytes[28..36].copy_from_slice(&8u64.to_le_bytes());
        bytes[36..40].copy_from_slice(b"fmt ");
        bytes[40..44].copy_from_slice(&16u32.to_le_bytes());
        bytes[44..46].copy_from_slice(&1u16.to_le_bytes());
        bytes[46..48].copy_from_slice(&2u16.to_le_bytes());
        bytes[48..52].copy_from_slice(&48_000u32.to_le_bytes());
        bytes[56..58].copy_from_slice(&4u16.to_le_bytes());
        bytes[58..60].copy_from_slice(&16u16.to_le_bytes());
        bytes[60..64].copy_from_slice(b"data");
        bytes[64..68].copy_from_slice(&u32::MAX.to_le_bytes());
        bytes[68..76].copy_from_slice(&[0, 0, 0, 0, 0, 0, 0, 0]);
        std::fs::write(&path, bytes).expect("RF64 fixture must be writable");
        let summary = inspect_rendered_wav(&path);
        assert!(summary.contains("48000 Hz · 2 ch · 16 bit"));
        assert!(summary.contains("2 frames"));
        std::fs::remove_file(path).expect("RF64 fixture cleanup must succeed");
    }
}
