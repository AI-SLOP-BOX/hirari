//! Small, deterministic state-to-text helpers used by the UI telemetry.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub(crate) enum BounceState {
    Idle = 0,
    Queued = 1,
    Rendering = 2,
    Complete = 3,
    Failed = 4,
    Cancelled = 5,
    Paused = 6,
    Unknown = u32::MAX,
}

impl From<u32> for BounceState {
    fn from(value: u32) -> Self {
        match value {
            0 => Self::Idle,
            1 => Self::Queued,
            2 => Self::Rendering,
            3 => Self::Complete,
            4 => Self::Failed,
            5 => Self::Cancelled,
            6 => Self::Paused,
            _ => Self::Unknown,
        }
    }
}

pub(crate) fn bounce_state_label(state: u32) -> &'static str {
    match BounceState::from(state) {
        BounceState::Idle => "IDLE",
        BounceState::Queued => "QUEUED",
        BounceState::Rendering => "RENDERING",
        BounceState::Complete => "COMPLETE",
        BounceState::Failed => "FAILED",
        BounceState::Cancelled => "CANCELLED",
        BounceState::Paused => "PAUSED",
        BounceState::Unknown => "ENGINE_STATUS_UNKNOWN",
    }
}

pub(crate) fn render_progress_status(
    state: u32,
    progress_available: bool,
    progress: f32,
    elapsed_seconds: u64,
) -> String {
    if matches!(
        BounceState::from(state),
        BounceState::Rendering | BounceState::Paused
    ) {
        if progress_available && progress.is_finite() {
            let label = if BounceState::from(state) == BounceState::Paused {
                "PAUSED"
            } else {
                "RENDERING"
            };
            return format!("{label} · {:.0}%", progress.clamp(0.0, 1.0) * 100.0);
        }
        if BounceState::from(state) == BounceState::Paused {
            return "PAUSED · render checkpoint retained".to_owned();
        }
        return format!(
            "RENDERING · {}s elapsed · progress pending",
            elapsed_seconds
        );
    }
    bounce_state_label(state).to_owned()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum UiErrorKind {
    AudioDevice,
    Engine,
    Plugin,
    Render,
    Project,
}

pub(crate) fn ui_error_message(kind: UiErrorKind, detail: &str) -> String {
    let prefix = match kind {
        UiErrorKind::AudioDevice => "Audio device",
        UiErrorKind::Engine => "Engine",
        UiErrorKind::Plugin => "Plugin",
        UiErrorKind::Render => "Render",
        UiErrorKind::Project => "Project",
    };
    let detail = detail.trim();
    if detail.is_empty() {
        prefix.to_owned()
    } else {
        format!("{prefix}: {detail}")
    }
}

pub(crate) fn ui_error_with_action(kind: UiErrorKind, detail: &str, action: &str) -> String {
    let message = ui_error_message(kind, detail);
    let action = action.trim();
    if action.is_empty() {
        message
    } else {
        format!("{message} · {action}")
    }
}

pub(crate) fn format_bytes(bytes: u64) -> String {
    const MB: u64 = 1024 * 1024;
    const KB: u64 = 1024;
    if bytes >= MB {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.1} KB", bytes as f64 / KB as f64)
    } else {
        format!("{} B", bytes)
    }
}

pub(crate) fn clamp_selection_index(index: i32, row_count: usize) -> usize {
    if row_count == 0 {
        return 0;
    }
    (index.max(0) as usize).min(row_count - 1)
}
