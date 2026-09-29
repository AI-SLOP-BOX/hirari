//! Allocation-free routing fanout kernels used by the audio callback.

use crate::routing_gains::state as gain_state;
use std::ffi::c_void;

const MAX_ROUTES: usize = 128;
const MAX_CHANNELS: usize = 32;
const MAX_TRACK_DESTINATIONS: usize = 256;

#[repr(C)]
pub struct RoutingTrackDestination {
    pub id: u32,
    pub can_process: u8,
    pub is_bus: u8,
    pub channel_count: u32,
    pub work_channels: *const *mut f32,
    pub bus_context: *mut c_void,
}

pub type BusPreFxAdd = unsafe extern "C" fn(
    context: *mut c_void,
    left: *const f32,
    right: *const f32,
    frames: u32,
    gain: f32,
) -> bool;

/// Applies one processed track's normal routes and parallel bus sends.
/// Track and bus object lifetimes, eligibility, and sample storage remain
/// owned by the C++ host; Rust owns the real-time routing traversal and gain
/// application. The caller holds the Send PDC reader guard for this call.
#[no_mangle]
pub unsafe extern "C" fn hirari_routing_process_track_fanout(
    gains_handle: *const c_void,
    send_pdc_handle: *mut c_void,
    source_id: u32,
    source_channels: *const *const f32,
    source_channel_count: u32,
    source_send_pre_left: *const f32,
    source_send_pre_right: *const f32,
    source_send_post_left: *const f32,
    source_send_post_right: *const f32,
    source_track_delay_samples: u32,
    destinations: *const RoutingTrackDestination,
    destination_count: usize,
    master_left: *mut f32,
    master_right: *mut f32,
    send_pdc_left: *mut f32,
    send_pdc_right: *mut f32,
    frames: u32,
    max_send_frames: u32,
    bus_add: Option<BusPreFxAdd>,
) {
    let Some(gains) = (unsafe { gain_state(gains_handle) }) else {
        return;
    };
    if source_channels.is_null()
        || source_channel_count == 0
        || destination_count > MAX_TRACK_DESTINATIONS
        || (destination_count != 0 && destinations.is_null())
        || master_left.is_null()
        || master_right.is_null()
        || frames == 0
    {
        return;
    }
    let source_planes = unsafe {
        std::slice::from_raw_parts(source_channels, source_channel_count as usize)
    };
    let Some(source_left) = source_planes.first().copied() else {
        return;
    };
    let Some(source_right) = source_planes.get(1).copied() else {
        return;
    };
    if source_left.is_null() || source_right.is_null() {
        return;
    }

    let mut routed = false;
    let destinations = if destination_count == 0 {
        &[][..]
    } else {
        unsafe { std::slice::from_raw_parts(destinations, destination_count) }
    };
    for destination in destinations {
        if destination.id == source_id || destination.can_process == 0 {
            continue;
        }
        let gain = gains.route_gain(source_id, destination.id);
        if !gain.is_finite() || gain <= 0.0 {
            continue;
        }
        if destination.work_channels.is_null() || destination.channel_count == 0 {
            continue;
        }
        let channel_count = (source_channel_count as usize)
            .min(destination.channel_count as usize)
            .min(MAX_CHANNELS);
        for channel in 0..channel_count {
            let source = unsafe { *source_planes.get_unchecked(channel) };
            let output = unsafe { *destination.work_channels.add(channel) };
            if source.is_null() || output.is_null() {
                continue;
            }
            for frame in 0..frames as usize {
                let sample = unsafe { *source.add(frame) };
                unsafe { *output.add(frame) += sample };
            }
        }

        if destination.is_bus != 0 {
            if let Some(add_pre_fx) = bus_add {
                unsafe {
                    add_pre_fx(
                        destination.bus_context,
                        source_left,
                        source_right,
                        frames,
                        gain,
                    );
                }
            }
        }

        if gain != 1.0 && channel_count >= 2 {
            let output_left = unsafe { *destination.work_channels };
            let output_right = unsafe { *destination.work_channels.add(1) };
            if !output_left.is_null() && !output_right.is_null() {
                unsafe {
                    crate::audio_buffer_ops::hirari_audio_route_gain_correction(
                        output_left,
                        output_right,
                        source_left,
                        source_right,
                        frames,
                        gain,
                    );
                }
            }
        }
        routed = true;
    }

    // Sends are parallel taps into bus pre-FX buffers. They advance their PDC
    // histories even at zero gain, matching the host's prior callback path.
    for destination in destinations {
        if destination.id == source_id
            || destination.can_process == 0
            || destination.is_bus == 0
            || !gains.has_send(source_id, destination.id)
        {
            continue;
        }
        let send_gain = gains.send_gain(source_id, destination.id);
        if !send_gain.is_finite() {
            continue;
        }
        let pre_fader = gains.send_is_pre_fader(source_id, destination.id);
        let (input_left, input_right) = if pre_fader {
            (source_send_pre_left, source_send_pre_right)
        } else {
            (source_send_post_left, source_send_post_right)
        };
        if input_left.is_null() || input_right.is_null() {
            continue;
        }

        let processed = if frames <= max_send_frames
            && !send_pdc_handle.is_null()
            && !send_pdc_left.is_null()
            && !send_pdc_right.is_null()
        {
            unsafe {
                crate::send_pdc_manager::hirari_send_pdc_manager_process(
                    send_pdc_handle,
                    source_id,
                    destination.id,
                    input_left,
                    input_right,
                    send_pdc_left,
                    send_pdc_right,
                    frames,
                    source_track_delay_samples,
                    send_gain,
                )
            }
        } else {
            false
        };
        if send_gain > 0.0 {
            if let Some(add_pre_fx) = bus_add {
                unsafe {
                    add_pre_fx(
                        destination.bus_context,
                        if processed { send_pdc_left } else { input_left },
                        if processed { send_pdc_right } else { input_right },
                        frames,
                        if processed { 1.0 } else { send_gain },
                    );
                }
            }
        }
    }

    if !routed {
        for frame in 0..frames as usize {
            unsafe {
                *master_left.add(frame) += *source_left.add(frame);
                *master_right.add(frame) += *source_right.add(frame);
            }
        }
    }
}

/// Fan out stereo audio into caller-owned destination buffers.
///
/// `route_gains[d]` is the already-snapshotted atomic gain for destination
/// `d`. The routine performs no allocation, locking, or callbacks.
#[no_mangle]
pub unsafe extern "C" fn hirari_routing_fanout_stereo(
    gains: *const c_void,
    source: u32,
    left: *const f32,
    right: *const f32,
    destination_left: *const *mut f32,
    destination_right: *const *mut f32,
    destination_ids: *const u32,
    destination_count: u32,
    frames: u32,
) {
    let Some(gains) = gain_state(gains) else {
        return;
    };
    if source >= 128
        || left.is_null()
        || right.is_null()
        || destination_left.is_null()
        || destination_right.is_null()
        || destination_ids.is_null()
        || frames == 0
    {
        return;
    }
    let count = (destination_count as usize).min(MAX_ROUTES);
    for destination in 0..count {
        let destination_id = *destination_ids.add(destination);
        let gain = gains.route_gain(source, destination_id);
        if !gain.is_finite() || gain <= 0.0 {
            continue;
        }
        let out_left = *destination_left.add(destination);
        let out_right = *destination_right.add(destination);
        if out_left.is_null() || out_right.is_null() {
            continue;
        }
        for frame in 0..frames as usize {
            let input_left = *left.add(frame);
            let input_right = *right.add(frame);
            *out_left.add(frame) += (if input_left.is_finite() {
                input_left
            } else {
                0.0
            }) * gain;
            *out_right.add(frame) += (if input_right.is_finite() {
                input_right
            } else {
                0.0
            }) * gain;
        }
    }
}

/// Fan out every source plane to each destination without allocating.
#[no_mangle]
pub unsafe extern "C" fn hirari_routing_fanout_planar(
    gains: *const c_void,
    source: u32,
    source_channels: *const *const f32,
    destination_channels: *const *const *mut f32,
    destination_ids: *const u32,
    destination_count: u32,
    channel_count: u32,
    frames: u32,
) {
    let Some(gains) = gain_state(gains) else {
        return;
    };
    if source_channels.is_null()
        || destination_channels.is_null()
        || destination_ids.is_null()
        || source >= 128
        || frames == 0
        || channel_count == 0
    {
        return;
    }
    let count = (destination_count as usize).min(MAX_ROUTES);
    let channels = (channel_count as usize).min(MAX_CHANNELS);
    for destination in 0..count {
        let destination_id = *destination_ids.add(destination);
        let gain = gains.route_gain(source, destination_id);
        if !gain.is_finite() || gain <= 0.0 {
            continue;
        }
        let destination_plane_list = *destination_channels.add(destination);
        if destination_plane_list.is_null() {
            continue;
        }
        for channel in 0..channels {
            let source = *source_channels.add(channel);
            let output = *destination_plane_list.add(channel);
            if source.is_null() || output.is_null() {
                continue;
            }
            for frame in 0..frames as usize {
                let input = *source.add(frame);
                *output.add(frame) += (if input.is_finite() { input } else { 0.0 }) * gain;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct BusCapture {
        left: [f32; 3],
        right: [f32; 3],
    }

    unsafe extern "C" fn capture_bus(
        context: *mut c_void,
        left: *const f32,
        right: *const f32,
        frames: u32,
        gain: f32,
    ) -> bool {
        if context.is_null() || left.is_null() || right.is_null() || frames > 3 {
            return false;
        }
        let capture = unsafe { &mut *context.cast::<BusCapture>() };
        for frame in 0..frames as usize {
            capture.left[frame] += unsafe { *left.add(frame) } * gain;
            capture.right[frame] += unsafe { *right.add(frame) } * gain;
        }
        true
    }

    fn assert_samples_near(actual: &[f32], expected: &[f32]) {
        assert_eq!(actual.len(), expected.len());
        for (actual, expected) in actual.iter().zip(expected) {
            assert!((actual - expected).abs() <= 1.0e-6, "{actual} != {expected}");
        }
    }

    #[test]
    fn track_fanout_moves_gain_routes_and_bus_pre_fx_addition_into_rust() {
        let gains = crate::routing_gains::RoutingGainState::new();
        assert!(gains.set_route_gain(2, 7, 0.5));
        assert!(gains.set_route_gain(2, 9, 1.25));
        let source_left = [1.0, 0.5, -0.25];
        let source_right = [-1.0, 0.25, 0.75];
        let source_planes = [source_left.as_ptr(), source_right.as_ptr()];
        let mut bus_work_left = [0.1, 0.1, 0.1];
        let mut bus_work_right = [0.2, 0.2, 0.2];
        let mut normal_work_left = [0.3, 0.3, 0.3];
        let mut normal_work_right = [0.4, 0.4, 0.4];
        let bus_work = [bus_work_left.as_mut_ptr(), bus_work_right.as_mut_ptr()];
        let normal_work = [normal_work_left.as_mut_ptr(), normal_work_right.as_mut_ptr()];
        let mut bus = BusCapture {
            left: [0.0; 3],
            right: [0.0; 3],
        };
        let destinations = [
            RoutingTrackDestination {
                id: 7,
                can_process: 1,
                is_bus: 1,
                channel_count: 2,
                work_channels: bus_work.as_ptr(),
                bus_context: (&mut bus as *mut BusCapture).cast(),
            },
            RoutingTrackDestination {
                id: 9,
                can_process: 1,
                is_bus: 0,
                channel_count: 2,
                work_channels: normal_work.as_ptr(),
                bus_context: std::ptr::null_mut(),
            },
        ];
        let mut master_left = [0.0; 3];
        let mut master_right = [0.0; 3];
        unsafe {
            hirari_routing_process_track_fanout(
                (&gains as *const crate::routing_gains::RoutingGainState).cast(),
                std::ptr::null_mut(),
                2,
                source_planes.as_ptr(),
                2,
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                0,
                destinations.as_ptr(),
                destinations.len(),
                master_left.as_mut_ptr(),
                master_right.as_mut_ptr(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                3,
                0,
                Some(capture_bus),
            );
        }
        assert_samples_near(&bus_work_left, &[0.6, 0.35, -0.025]);
        assert_samples_near(&bus_work_right, &[-0.3, 0.325, 0.575]);
        assert_samples_near(&normal_work_left, &[1.55, 0.925, -0.0125]);
        assert_samples_near(&normal_work_right, &[-0.85, 0.7125, 1.3375]);
        assert_samples_near(&bus.left, &[0.5, 0.25, -0.125]);
        assert_samples_near(&bus.right, &[-0.5, 0.125, 0.375]);
        assert_eq!(master_left, [0.0; 3]);
        assert_eq!(master_right, [0.0; 3]);
    }

    #[test]
    fn send_only_fanout_keeps_master_route_and_uses_uncompensated_fallback() {
        let gains = crate::routing_gains::RoutingGainState::new();
        assert!(gains.set_send(2, 7, 0.25, true));
        let source_left = [2.0, -1.0, 0.5];
        let source_right = [-2.0, 1.0, -0.5];
        let source_planes = [source_left.as_ptr(), source_right.as_ptr()];
        let send_pre_left = [1.0, 0.5, 0.25];
        let send_pre_right = [-1.0, -0.5, -0.25];
        let mut bus_work_left = [0.0; 3];
        let mut bus_work_right = [0.0; 3];
        let bus_work = [bus_work_left.as_mut_ptr(), bus_work_right.as_mut_ptr()];
        let mut bus = BusCapture {
            left: [0.0; 3],
            right: [0.0; 3],
        };
        let destination = RoutingTrackDestination {
            id: 7,
            can_process: 1,
            is_bus: 1,
            channel_count: 2,
            work_channels: bus_work.as_ptr(),
            bus_context: (&mut bus as *mut BusCapture).cast(),
        };
        let mut master_left = [0.0; 3];
        let mut master_right = [0.0; 3];
        unsafe {
            hirari_routing_process_track_fanout(
                (&gains as *const crate::routing_gains::RoutingGainState).cast(),
                std::ptr::null_mut(),
                2,
                source_planes.as_ptr(),
                2,
                send_pre_left.as_ptr(),
                send_pre_right.as_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                9,
                &destination,
                1,
                master_left.as_mut_ptr(),
                master_right.as_mut_ptr(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                3,
                8,
                Some(capture_bus),
            );
        }
        assert_eq!(bus.left, [0.25, 0.125, 0.0625]);
        assert_eq!(bus.right, [-0.25, -0.125, -0.0625]);
        assert_eq!(master_left, source_left);
        assert_eq!(master_right, source_right);
        assert_eq!(bus_work_left, [0.0; 3]);
        assert_eq!(bus_work_right, [0.0; 3]);
    }

    #[test]
    fn track_ids_outside_the_routing_table_still_fall_back_to_master() {
        let gains = crate::routing_gains::RoutingGainState::new();
        let source_left = [0.5, -0.25];
        let source_right = [-0.5, 0.25];
        let source_planes = [source_left.as_ptr(), source_right.as_ptr()];
        let mut master_left = [0.0; 2];
        let mut master_right = [0.0; 2];
        unsafe {
            hirari_routing_process_track_fanout(
                (&gains as *const crate::routing_gains::RoutingGainState).cast(),
                std::ptr::null_mut(),
                130,
                source_planes.as_ptr(),
                2,
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                0,
                std::ptr::null(),
                0,
                master_left.as_mut_ptr(),
                master_right.as_mut_ptr(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                2,
                8,
                Some(capture_bus),
            );
        }
        assert_eq!(master_left, source_left);
        assert_eq!(master_right, source_right);
    }

    #[test]
    fn stereo_fanout_applies_route_gains_and_sanitizes_inputs() {
        let gains = crate::routing_gains::RoutingGainState::new();
        assert!(gains.set_route_gain(4, 6, 0.5));
        let left = [1.0, f32::NAN, -0.5];
        let right = [-1.0, f32::INFINITY, 0.25];
        let mut first_left = [0.25, 0.25, 0.25];
        let mut first_right = [0.5, 0.5, 0.5];
        let mut second_left = [0.0; 3];
        let mut second_right = [0.0; 3];
        let destination_left = [first_left.as_mut_ptr(), second_left.as_mut_ptr()];
        let destination_right = [first_right.as_mut_ptr(), second_right.as_mut_ptr()];
        let destination_ids = [6, 7];
        unsafe {
            hirari_routing_fanout_stereo(
                (&gains as *const crate::routing_gains::RoutingGainState).cast(),
                4,
                left.as_ptr(),
                right.as_ptr(),
                destination_left.as_ptr(),
                destination_right.as_ptr(),
                destination_ids.as_ptr(),
                destination_ids.len() as u32,
                left.len() as u32,
            );
        }
        assert_eq!(first_left, [0.75, 0.25, 0.0]);
        assert_eq!(first_right, [0.0, 0.5, 0.625]);
        assert_eq!(second_left, [0.0; 3]);
        assert_eq!(second_right, [0.0; 3]);
    }

    #[test]
    fn planar_fanout_obeys_channel_limit_and_null_planes() {
        let gains = crate::routing_gains::RoutingGainState::new();
        assert!(gains.set_route_gain(2, 8, 2.0));
        let left = [0.5, -0.5];
        let right = [1.0, -1.0];
        let source_channels = [left.as_ptr(), right.as_ptr(), std::ptr::null()];
        let mut destination_a_left = [0.0; 2];
        let mut destination_a_right = [0.25; 2];
        let mut destination_b_left = [0.0; 2];
        let mut destination_b_right = [0.0; 2];
        let destination_a = [
            destination_a_left.as_mut_ptr(),
            destination_a_right.as_mut_ptr(),
        ];
        let destination_b = [
            destination_b_left.as_mut_ptr(),
            destination_b_right.as_mut_ptr(),
        ];
        let destination_channels = [destination_a.as_ptr(), destination_b.as_ptr()];
        let destination_ids = [8, 9];
        unsafe {
            hirari_routing_fanout_planar(
                (&gains as *const crate::routing_gains::RoutingGainState).cast(),
                2,
                source_channels.as_ptr(),
                destination_channels.as_ptr(),
                destination_ids.as_ptr(),
                destination_ids.len() as u32,
                source_channels.len() as u32,
                left.len() as u32,
            );
        }
        assert_eq!(destination_a_left, [1.0, -1.0]);
        assert_eq!(destination_a_right, [2.25, -1.75]);
        assert_eq!(destination_b_left, [0.0; 2]);
        assert_eq!(destination_b_right, [0.0; 2]);
    }
}
