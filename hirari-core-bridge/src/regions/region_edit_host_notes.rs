//! Note edits on region note tables (F0/anchors/formants), split out of region_edit_host.rs.
use super::*;

pub(in crate::region_edit_host) struct NoteEditContext {
    inner: *mut c_void,
    mutate: HirariRegionNoteEditFn,
    before: *mut HirariRegionVariAudioStore,
    after: *mut HirariRegionVariAudioStore,
}

unsafe extern "C" fn note_edit_trampoline(
    context: *mut c_void,
    store: *mut HirariRegionVariAudioStore,
) -> bool {
    let context = &mut *(context as *mut NoteEditContext);
    if store.is_null() {
        return false;
    }
    context.before = vari_store_clone(store);
    if !(context.mutate)(context.inner, store) {
        return false;
    }
    context.after = vari_store_clone(store);
    true
}

pub(in crate::region_edit_host) fn note_store_payload(
    track_id: u32,
    region_id: u32,
    snapshot: *mut HirariRegionVariAudioStore,
) -> HirariRegionEditPayload {
    HirariRegionEditPayload {
        track_id,
        region_id,
        op: HIRARI_REGION_EDIT_NOTE_STORE,
        blob: snapshot as *const u8,
        ..Default::default()
    }
}

pub(in crate::region_edit_host) fn note_action_name(prefix: &str, track_id: u32, region_id: u32) -> Vec<u8> {
    let mut name = format!("{prefix} {track_id}:{region_id}").into_bytes();
    name.push(0);
    name
}

/// Runs `mutate` under the host's region lock, then records the before/after
/// tables as one undo entry. The host takes ownership of both snapshots.
pub(in crate::region_edit_host) fn note_edit_with(
    ops: &HirariRegionHostOps,
    host: *mut c_void,
    track_id: u32,
    region_id: u32,
    name: &[u8],
    inner: *mut c_void,
    mutate: HirariRegionNoteEditFn,
    coalesce: bool,
) -> bool {
    let (Some(note_edit), Some(find)) = (ops.note_edit, ops.find_region) else {
        return false;
    };
    let mut state = HirariRegionState::default();
    if !unsafe { find(host, track_id, region_id, &mut state) } {
        return false;
    }
    let mut context = NoteEditContext {
        inner,
        mutate,
        before: std::ptr::null_mut(),
        after: std::ptr::null_mut(),
    };
    let applied = unsafe {
        note_edit(
            host,
            track_id,
            region_id,
            &mut context as *mut NoteEditContext as *mut c_void,
            note_edit_trampoline,
        )
    };
    if !applied || !can_record(ops) {
        unsafe {
            vari_store_release(context.before);
            vari_store_release(context.after);
        }
        return applied;
    }
    let undo = note_store_payload(track_id, region_id, context.before);
    let redo = note_store_payload(track_id, region_id, context.after);
    record(ops, host, name, &[undo], &[redo], coalesce);
    true
}

#[repr(C)]
pub(in crate::region_edit_host) struct AnchorContext {
    segment_start: f64,
    position: f64,
    pitch_cents: f64,
    formant_cents: f64,
}

unsafe extern "C" fn mutate_anchor(
    context: *mut c_void,
    store: *mut HirariRegionVariAudioStore,
) -> bool {
    let context = &*(context as *const AnchorContext);
    hirari_region_vari_audio_store_upsert_anchor(
        store,
        context.segment_start,
        context.position,
        context.pitch_cents,
        context.formant_cents,
    )
}

#[repr(C)]
pub(in crate::region_edit_host) struct ComponentContext {
    segment_start: f64,
    position: f64,
    cents: f64,
    formant: bool,
}

unsafe extern "C" fn mutate_component_anchor(
    context: *mut c_void,
    store: *mut HirariRegionVariAudioStore,
) -> bool {
    let context = &*(context as *const ComponentContext);
    let Some(view) = vari_store_view(store, context.segment_start) else {
        return false;
    };
    if context.position < view.start || context.position > view.end {
        return false;
    }
    let mut anchor = HirariAudioNoteAnchor::default();
    if !hirari_audio_note_curve_component_anchor(
        view.anchors.as_ptr(),
        view.anchors.len(),
        view.start,
        view.end,
        context.position,
        context.cents,
        if context.formant { 1 } else { 0 },
        view.pitch_offset,
        view.formant_offset,
        &mut anchor,
    ) {
        return false;
    }
    hirari_region_vari_audio_store_upsert_anchor(
        store,
        context.segment_start,
        anchor.position_seconds,
        anchor.pitch_cents,
        anchor.formant_cents,
    )
}

#[repr(C)]
pub(in crate::region_edit_host) struct RemoveContext {
    segment_start: f64,
}

unsafe extern "C" fn mutate_remove(
    context: *mut c_void,
    store: *mut HirariRegionVariAudioStore,
) -> bool {
    let context = &*(context as *const RemoveContext);
    hirari_region_vari_audio_store_remove(store, context.segment_start)
}

unsafe extern "C" fn mutate_clear(
    _context: *mut c_void,
    store: *mut HirariRegionVariAudioStore,
) -> bool {
    hirari_region_vari_audio_store_clear_segments(store)
}

#[repr(C)]
pub(in crate::region_edit_host) struct WarpContext {
    segment_start: f64,
    new_start: f64,
    new_end: f64,
}

unsafe extern "C" fn mutate_warp(
    context: *mut c_void,
    store: *mut HirariRegionVariAudioStore,
) -> bool {
    let context = &*(context as *const WarpContext);
    hirari_region_vari_audio_store_warp(
        store,
        context.segment_start,
        context.new_start,
        context.new_end,
    )
}

pub(in crate::region_edit_host) fn note_anchor_with(
    ops: &HirariRegionHostOps,
    host: *mut c_void,
    track_id: u32,
    region_id: u32,
    segment_start: f64,
    position: f64,
    pitch_cents: f64,
    formant_cents: f64,
) -> bool {
    if !segment_start.is_finite()
        || segment_start < 0.0
        || !position.is_finite()
        || position < 0.0
        || !pitch_cents.is_finite()
        || pitch_cents.abs() > 4800.0
        || !formant_cents.is_finite()
        || formant_cents.abs() > 2400.0
    {
        return false;
    }
    let name = note_action_name("Edit Audio Pitch Curve", track_id, region_id);
    let mut context = AnchorContext {
        segment_start,
        position,
        pitch_cents,
        formant_cents,
    };
    note_edit_with(
        ops,
        host,
        track_id,
        region_id,
        &name,
        &mut context as *mut AnchorContext as *mut c_void,
        mutate_anchor,
        true,
    )
}

pub(in crate::region_edit_host) fn note_pitch_anchor_with(
    ops: &HirariRegionHostOps,
    host: *mut c_void,
    track_id: u32,
    region_id: u32,
    segment_start: f64,
    position: f64,
    pitch_cents: f64,
) -> bool {
    if !segment_start.is_finite()
        || segment_start < 0.0
        || !position.is_finite()
        || position < 0.0
        || !pitch_cents.is_finite()
        || pitch_cents.abs() > 4800.0
    {
        return false;
    }
    let name = note_action_name("Edit Audio Pitch Curve", track_id, region_id);
    let mut context = ComponentContext {
        segment_start,
        position,
        cents: pitch_cents,
        formant: false,
    };
    note_edit_with(
        ops,
        host,
        track_id,
        region_id,
        &name,
        &mut context as *mut ComponentContext as *mut c_void,
        mutate_component_anchor,
        true,
    )
}

pub(in crate::region_edit_host) fn note_formant_anchor_with(
    ops: &HirariRegionHostOps,
    host: *mut c_void,
    track_id: u32,
    region_id: u32,
    segment_start: f64,
    position: f64,
    formant_cents: f64,
) -> bool {
    if !segment_start.is_finite()
        || segment_start < 0.0
        || !position.is_finite()
        || position < 0.0
        || !formant_cents.is_finite()
        || formant_cents.abs() > 2400.0
    {
        return false;
    }
    let name = note_action_name("Edit Audio Formant Curve", track_id, region_id);
    let mut context = ComponentContext {
        segment_start,
        position,
        cents: formant_cents,
        formant: true,
    };
    note_edit_with(
        ops,
        host,
        track_id,
        region_id,
        &name,
        &mut context as *mut ComponentContext as *mut c_void,
        mutate_component_anchor,
        true,
    )
}

pub(in crate::region_edit_host) fn note_remove_with(
    ops: &HirariRegionHostOps,
    host: *mut c_void,
    track_id: u32,
    region_id: u32,
    segment_start: f64,
) -> bool {
    if !segment_start.is_finite() || segment_start < 0.0 {
        return false;
    }
    let mut context = RemoveContext { segment_start };
    note_edit_with(
        ops,
        host,
        track_id,
        region_id,
        b"Remove Audio Pitch Segment\0",
        &mut context as *mut RemoveContext as *mut c_void,
        mutate_remove,
        false,
    )
}

pub(in crate::region_edit_host) fn note_clear_with(
    ops: &HirariRegionHostOps,
    host: *mut c_void,
    track_id: u32,
    region_id: u32,
) -> bool {
    note_edit_with(
        ops,
        host,
        track_id,
        region_id,
        b"Clear Audio Pitch Segments\0",
        std::ptr::null_mut(),
        mutate_clear,
        false,
    )
}

pub(in crate::region_edit_host) fn note_warp_with(
    ops: &HirariRegionHostOps,
    host: *mut c_void,
    track_id: u32,
    region_id: u32,
    segment_start: f64,
    new_start: f64,
    new_end: f64,
) -> bool {
    if !segment_start.is_finite()
        || segment_start < 0.0
        || !new_start.is_finite()
        || new_start < 0.0
        || !new_end.is_finite()
        || new_end <= new_start
        || new_end - new_start > 24.0 * 60.0
    {
        return false;
    }
    let mut context = WarpContext {
        segment_start,
        new_start,
        new_end,
    };
    note_edit_with(
        ops,
        host,
        track_id,
        region_id,
        b"Warp Audio Pitch Segment\0",
        &mut context as *mut WarpContext as *mut c_void,
        mutate_warp,
        false,
    )
}

#[repr(C)]
pub(in crate::region_edit_host) struct SegmentContext {
    start: f64,
    end: f64,
    pitch_cents: f64,
    formant_cents: f64,
}

unsafe extern "C" fn mutate_segment(
    context: *mut c_void,
    store: *mut HirariRegionVariAudioStore,
) -> bool {
    let context = &*(context as *const SegmentContext);
    // Re-editing the same span keeps its detected pitch and curve anchors, the
    // way the C++ version reused the existing segment before overwriting the
    // offsets.
    let existing = vari_store_view(store, context.start)
        .filter(|view| (view.end - context.end).abs() < 1e-9);
    let (detected_pitch, anchors) = match existing {
        Some(view) => (view.detected_pitch, view.anchors),
        None => (0.0, Vec::new()),
    };
    let segment = HirariAudioNoteSegment {
        start_seconds: context.start,
        end_seconds: context.end,
        detected_pitch_cents: detected_pitch,
        pitch_offset_cents: context.pitch_cents.clamp(-4800.0, 4800.0),
        formant_offset_cents: context.formant_cents.clamp(-2400.0, 2400.0),
    };
    hirari_region_vari_audio_store_upsert(store, segment, anchors.as_ptr(), anchors.len())
}

#[repr(C)]
pub(in crate::region_edit_host) struct MoveAnchorContext {
    segment_start: f64,
    old_position: f64,
    new_position: f64,
    value_cents: f64,
    formant: bool,
}

unsafe extern "C" fn mutate_move_anchor(
    context: *mut c_void,
    store: *mut HirariRegionVariAudioStore,
) -> bool {
    let context = &*(context as *const MoveAnchorContext);
    let Some(view) = vari_store_view(store, context.segment_start) else {
        return false;
    };
    if context.new_position < view.start || context.new_position > view.end {
        return false;
    }
    let mut anchors = view.anchors;
    let moved = hirari_audio_note_curve_move_anchor(
        anchors.as_mut_ptr(),
        anchors.len(),
        view.start,
        view.end,
        context.old_position,
        context.new_position,
        context.value_cents,
        if context.formant { 1 } else { 0 },
        view.pitch_offset,
        view.formant_offset,
    );
    if moved > anchors.len() {
        return false;
    }
    anchors.truncate(moved);
    let segment = HirariAudioNoteSegment {
        start_seconds: view.start,
        end_seconds: view.end,
        detected_pitch_cents: view.detected_pitch,
        pitch_offset_cents: view.pitch_offset,
        formant_offset_cents: view.formant_offset,
    };
    hirari_region_vari_audio_store_upsert(store, segment, anchors.as_ptr(), anchors.len())
}

pub(in crate::region_edit_host) fn note_segment_with(
    ops: &HirariRegionHostOps,
    host: *mut c_void,
    track_id: u32,
    region_id: u32,
    start: f64,
    end: f64,
    pitch_cents: f64,
    formant_cents: f64,
) -> bool {
    if !start.is_finite()
        || !end.is_finite()
        || end <= start
        || end - start > 24.0 * 60.0
        || !pitch_cents.is_finite()
        || !formant_cents.is_finite()
        || pitch_cents.abs() > 4800.0
        || formant_cents.abs() > 2400.0
    {
        return false;
    }
    let name = note_action_name("Edit Audio Pitch Segments", track_id, region_id);
    let mut context = SegmentContext {
        start,
        end,
        pitch_cents,
        formant_cents,
    };
    note_edit_with(
        ops,
        host,
        track_id,
        region_id,
        &name,
        &mut context as *mut SegmentContext as *mut c_void,
        mutate_segment,
        true,
    )
}

pub(in crate::region_edit_host) fn note_move_anchor_with(
    ops: &HirariRegionHostOps,
    host: *mut c_void,
    track_id: u32,
    region_id: u32,
    segment_start: f64,
    old_position: f64,
    new_position: f64,
    value_cents: f64,
    formant: bool,
) -> bool {
    let max_cents = if formant { 2400.0 } else { 4800.0 };
    if !segment_start.is_finite()
        || segment_start < 0.0
        || !old_position.is_finite()
        || old_position < 0.0
        || !new_position.is_finite()
        || new_position < 0.0
        || !value_cents.is_finite()
        || value_cents.abs() > max_cents
    {
        return false;
    }
    let prefix = if formant {
        "Move Audio Formant Anchor"
    } else {
        "Move Audio Pitch Anchor"
    };
    let name = note_action_name(prefix, track_id, region_id);
    let mut context = MoveAnchorContext {
        segment_start,
        old_position,
        new_position,
        value_cents,
        formant,
    };
    note_edit_with(
        ops,
        host,
        track_id,
        region_id,
        &name,
        &mut context as *mut MoveAnchorContext as *mut c_void,
        mutate_move_anchor,
        true,
    )
}

#[no_mangle]
pub unsafe extern "C" fn hirari_region_edit_note_segment(
    host: *mut c_void,
    track_id: u32,
    region_id: u32,
    start: f64,
    end: f64,
    pitch_cents: f64,
    formant_cents: f64,
) -> bool {
    let Some(ops) = host_ops() else {
        return false;
    };
    note_segment_with(
        &ops,
        host,
        track_id,
        region_id,
        start,
        end,
        pitch_cents,
        formant_cents,
    )
}

#[no_mangle]
pub unsafe extern "C" fn hirari_region_edit_note_move_anchor(
    host: *mut c_void,
    track_id: u32,
    region_id: u32,
    segment_start: f64,
    old_position: f64,
    new_position: f64,
    value_cents: f64,
    formant: bool,
) -> bool {
    let Some(ops) = host_ops() else {
        return false;
    };
    note_move_anchor_with(
        &ops,
        host,
        track_id,
        region_id,
        segment_start,
        old_position,
        new_position,
        value_cents,
        formant,
    )
}

#[no_mangle]
pub unsafe extern "C" fn hirari_region_edit_note_anchor(
    host: *mut c_void,
    track_id: u32,
    region_id: u32,
    segment_start: f64,
    position: f64,
    pitch_cents: f64,
    formant_cents: f64,
) -> bool {
    let Some(ops) = host_ops() else {
        return false;
    };
    note_anchor_with(
        &ops,
        host,
        track_id,
        region_id,
        segment_start,
        position,
        pitch_cents,
        formant_cents,
    )
}

#[no_mangle]
pub unsafe extern "C" fn hirari_region_edit_note_pitch_anchor(
    host: *mut c_void,
    track_id: u32,
    region_id: u32,
    segment_start: f64,
    position: f64,
    pitch_cents: f64,
) -> bool {
    let Some(ops) = host_ops() else {
        return false;
    };
    note_pitch_anchor_with(
        &ops,
        host,
        track_id,
        region_id,
        segment_start,
        position,
        pitch_cents,
    )
}

#[no_mangle]
pub unsafe extern "C" fn hirari_region_edit_note_formant_anchor(
    host: *mut c_void,
    track_id: u32,
    region_id: u32,
    segment_start: f64,
    position: f64,
    formant_cents: f64,
) -> bool {
    let Some(ops) = host_ops() else {
        return false;
    };
    note_formant_anchor_with(
        &ops,
        host,
        track_id,
        region_id,
        segment_start,
        position,
        formant_cents,
    )
}

#[no_mangle]
pub unsafe extern "C" fn hirari_region_edit_note_remove(
    host: *mut c_void,
    track_id: u32,
    region_id: u32,
    segment_start: f64,
) -> bool {
    let Some(ops) = host_ops() else {
        return false;
    };
    note_remove_with(&ops, host, track_id, region_id, segment_start)
}

#[no_mangle]
pub unsafe extern "C" fn hirari_region_edit_note_clear(
    host: *mut c_void,
    track_id: u32,
    region_id: u32,
) -> bool {
    let Some(ops) = host_ops() else {
        return false;
    };
    note_clear_with(&ops, host, track_id, region_id)
}

#[no_mangle]
pub unsafe extern "C" fn hirari_region_edit_note_warp(
    host: *mut c_void,
    track_id: u32,
    region_id: u32,
    segment_start: f64,
    new_start: f64,
    new_end: f64,
) -> bool {
    let Some(ops) = host_ops() else {
        return false;
    };
    note_warp_with(
        &ops,
        host,
        track_id,
        region_id,
        segment_start,
        new_start,
        new_end,
    )
}

#[no_mangle]
pub unsafe extern "C" fn hirari_region_note_store_restore(
    snapshot: *mut c_void,
    store: *mut HirariRegionVariAudioStore,
) -> bool {
    vari_store_restore(store, snapshot as *mut HirariRegionVariAudioStore)
}
