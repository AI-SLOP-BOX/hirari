//! Hirari's project integration with the Moufu Hub.

use hirari_core_bridge::HirariCore;
use slint::Model;

pub use hirari_moufu_adapter::{
    EntityDescriptor, LinkInfo, LinkStatus, MoufuEvent, MoufuPublisher, PayloadTransport,
    HIRARI_PROJECT_CONTRACT, HIRARI_PROJECT_DATA_TYPE, HIRARI_PROJECT_ENTITY,
};

/// Publish the native project snapshot with UI-owned input choices. The small
/// selection list is captured here; the adapter merges it into project JSON on
/// its worker so the UI avoids a second full-layout parse and serialization.
pub(crate) fn publish_layout(
    publisher: &MoufuPublisher,
    core: &HirariCore,
    tracks: &slint::VecModel<crate::slint_ui::Z_Track>,
    is_transient: bool,
) {
    let recording_input_channels = (0..tracks.row_count())
        .filter_map(|row| tracks.row_data(row))
        .filter(|track| track.r#type == "Audio" || track.r#type == "Vocal")
        .filter_map(|track| {
            let channels =
                crate::ui::track_model::parse_recording_input_channels(track.input.as_str())?;
            let track_id = u32::try_from(track.id).ok()?;
            Some((
                track_id,
                channels
                    .into_iter()
                    .map(|channel| channel.saturating_add(1))
                    .collect(),
            ))
        })
        .collect();
    crate::ui_shared::moufu_layout::publish_project_snapshot(
        publisher,
        core,
        recording_input_channels,
        is_transient,
    );
}
