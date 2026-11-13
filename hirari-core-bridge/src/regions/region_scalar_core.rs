//! Scalar-field helpers shared by the region editor actions, split out of
//! region_edit_host.rs.
use super::*;

pub(in crate::region_edit_host) fn scalar_field(value: i32) -> Option<HirariRegionScalarField> {
    Some(match value {
        1 => HirariRegionScalarField::ClipGain,
        2 => HirariRegionScalarField::Muted,
        3 => HirariRegionScalarField::Reverse,
        4 => HirariRegionScalarField::PitchSemitones,
        5 => HirariRegionScalarField::LoopCount,
        6 => HirariRegionScalarField::SyncGroup,
        7 => HirariRegionScalarField::Fades,
        8 => HirariRegionScalarField::Locked,
        9 => HirariRegionScalarField::PitchPreserveWarp,
        _ => return None,
    })
}

/// Undo labels, kept identical to the strings the C++ layer used to record.
/// The undo label, or `None` for fields the engine edited without one.
pub(in crate::region_edit_host) fn field_name(field: HirariRegionScalarField) -> Option<&'static [u8]> {
    Some(match field {
        HirariRegionScalarField::ClipGain => b"Region Gain\0",
        HirariRegionScalarField::Muted => b"Mute Region\0",
        HirariRegionScalarField::Reverse => b"Reverse Region\0",
        HirariRegionScalarField::PitchSemitones => b"Flex Pitch\0",
        HirariRegionScalarField::LoopCount => b"Region Loop\0",
        HirariRegionScalarField::SyncGroup => b"Region Sync Group\0",
        HirariRegionScalarField::Fades => b"Region Fades\0",
        HirariRegionScalarField::Locked => return None,
        HirariRegionScalarField::PitchPreserveWarp => b"Preserve Warp Pitch\0",
    })
}

pub(in crate::region_edit_host) fn state_payload(
    track_id: u32,
    region_id: u32,
    op: i32,
    state: HirariRegionState,
) -> HirariRegionEditPayload {
    HirariRegionEditPayload {
        track_id,
        region_id,
        op,
        state,
        ..Default::default()
    }
}

/// Upper bound for a captured marker list. The engine rejects edits above this
/// order of magnitude; the cap only keeps a foreign host from driving an
/// unbounded allocation here.
pub(super) const MAX_WARP_MARKERS: usize = 1_000_000;

pub(in crate::region_edit_host) fn geometry_of(state: HirariRegionState) -> NativeRegionGeometry {
    NativeRegionGeometry {
        start: state.start,
        length: state.len,
        source_length: state.source_length,
        source_offset: state.source_offset,
        base_start: state.base_start,
        base_source_offset: state.base_source_offset,
        base_length: state.base_length,
        loop_count: state.loop_count,
        fade_in: state.fade_in_samples,
        fade_out: state.fade_out_samples,
        warp_ratio: state.warp_ratio,
        source_sample_rate: state.source_sample_rate,
        timeline_sample_rate: state.timeline_sample_rate,
    }
}
