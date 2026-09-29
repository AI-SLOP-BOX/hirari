use std::ffi::c_void;
use std::ptr;

/// Runs the final per-track stereo output stage on the Rust side.
///
/// # Safety
/// Every non-null channel array must contain the declared number of pointers,
/// each pointing to `frames` writable floats. Per-track DSP handles must stay
/// alive for the duration of this call, and audio calls for each handle must
/// remain serialized by the host callback.
#[no_mangle]
pub unsafe extern "C" fn hirari_track_finalize_output(
    track_id: u32,
    frames: u32,
    start_time: f64,
    volume_points: *const c_void,
    volume_count: usize,
    volume_hint: *mut usize,
    pan_points: *const c_void,
    pan_count: usize,
    pan_hint: *mut usize,
    channel_strip: *const c_void,
    active_channels: *const *mut f32,
    active_channel_count: u32,
    pre_insert_channels: *const *mut f32,
    pre_insert_channel_count: u32,
    send_pre_channels: *const *mut f32,
    send_pre_channel_count: u32,
    send_post_channels: *const *mut f32,
    send_post_channel_count: u32,
    panner: *const c_void,
    spatial_mode: u32,
    spatial_x: f32,
    spatial_y: f32,
    spatial_z: f32,
    low_gain: f32,
    high_gain: f32,
    low_state_l: *mut f32,
    low_state_r: *mut f32,
    pdc_delay: *mut c_void,
    requested_pdc: u32,
    manual_delay: u32,
    pre_fader_channels: *const *mut f32,
    pre_fader_channel_count: u32,
) {
    if frames == 0
        || channel_strip.is_null()
        || panner.is_null()
        || pdc_delay.is_null()
        || active_channels.is_null()
        || active_channel_count < 2
        || low_state_l.is_null()
        || low_state_r.is_null()
    {
        return;
    }
    let active =
        unsafe { std::slice::from_raw_parts(active_channels, active_channel_count as usize) };
    if active[0].is_null() || active[1].is_null() {
        return;
    }

    unsafe {
        crate::track_channel_eq::hirari_track_eq_process(
            active[0],
            active[1],
            frames,
            low_gain,
            high_gain,
            low_state_l,
            low_state_r,
        );
    }

    let mut pre_fader_left = ptr::null_mut();
    let mut pre_fader_right = ptr::null_mut();
    if pre_fader_channel_count >= 2 && !pre_fader_channels.is_null() {
        let pre_fader = unsafe {
            std::slice::from_raw_parts(pre_fader_channels, pre_fader_channel_count as usize)
        };
        pre_fader_left = pre_fader[0];
        pre_fader_right = pre_fader[1];
    }
    if !pre_fader_left.is_null() && !pre_fader_right.is_null() {
        unsafe {
            ptr::copy_nonoverlapping(active[0], pre_fader_left, frames as usize);
            ptr::copy_nonoverlapping(active[1], pre_fader_right, frames as usize);
        }
    }

    let mut send_pre_left = ptr::null_mut();
    let mut send_pre_right = ptr::null_mut();
    if send_pre_channel_count >= 2 && !send_pre_channels.is_null() {
        let send_pre = unsafe {
            std::slice::from_raw_parts(send_pre_channels, send_pre_channel_count as usize)
        };
        send_pre_left = send_pre[0];
        send_pre_right = send_pre[1];
    }
    if !send_pre_left.is_null() && !send_pre_right.is_null() {
        unsafe {
            ptr::copy_nonoverlapping(active[0], send_pre_left, frames as usize);
            ptr::copy_nonoverlapping(active[1], send_pre_right, frames as usize);
        }
    }

    let mut pre_insert_left = ptr::null_mut();
    let mut pre_insert_right = ptr::null_mut();
    if pre_insert_channel_count >= 2 && !pre_insert_channels.is_null() {
        let pre_insert = unsafe {
            std::slice::from_raw_parts(pre_insert_channels, pre_insert_channel_count as usize)
        };
        pre_insert_left = pre_insert[0];
        pre_insert_right = pre_insert[1];
    }
    if volume_count != 0 || pan_count != 0 {
        if volume_count != 0 {
            unsafe { crate::channel_strip::hirari_channel_strip_set_gain(channel_strip, 1.0) };
        }
        if pan_count != 0 {
            unsafe { crate::channel_strip::hirari_channel_strip_set_pan(channel_strip, 0.0) };
        }
        unsafe {
            crate::realtime_automation::hirari_realtime_automation_apply_stereo_block(
                volume_points.cast(),
                volume_count,
                volume_hint,
                pan_points.cast(),
                pan_count,
                pan_hint,
                start_time,
                active[0],
                active[1],
                pre_insert_left,
                pre_insert_right,
                frames,
            );
        }
    }

    unsafe {
        crate::channel_strip::hirari_channel_strip_process_mirror(
            channel_strip,
            active_channels.cast_mut(),
            active_channel_count,
            if pre_insert_left.is_null() || pre_insert_right.is_null() {
                ptr::null_mut()
            } else {
                pre_insert_channels.cast_mut()
            },
            if pre_insert_left.is_null() {
                0
            } else {
                pre_insert_channel_count
            },
            0,
            frames as usize,
        );
    }

    if spatial_mode != 0 {
        unsafe {
            crate::track_holographic_panner::hirari_track_holographic_panner_process(
                panner, active[0], active[1], frames, spatial_x, spatial_y, spatial_z,
            );
        }
    }
    unsafe {
        crate::realtime_vca_manager::hirari_vca_apply_track_gain(
            track_id, active[0], active[1], frames,
        );
    }

    if send_post_channel_count >= 2 && !send_post_channels.is_null() {
        let send_post = unsafe {
            std::slice::from_raw_parts(send_post_channels, send_post_channel_count as usize)
        };
        if !send_post[0].is_null() && !send_post[1].is_null() {
            unsafe {
                ptr::copy_nonoverlapping(active[0], send_post[0], frames as usize);
                ptr::copy_nonoverlapping(active[1], send_post[1], frames as usize);
            }
        }
    }

    let requested = requested_pdc
        .min(8192u32.saturating_sub(manual_delay.min(8192)))
        .saturating_add(manual_delay.min(8192))
        .min(8192);
    unsafe {
        crate::track_pdc_delay::hirari_track_pdc_delay_process(
            pdc_delay,
            active[0],
            active[1],
            pre_fader_left,
            pre_fader_right,
            frames,
            requested,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::hirari_track_finalize_output;

    #[test]
    fn final_track_stage_matches_ordered_rust_dsp_calls() {
        let frames = 4;
        let mut actual_left = [0.25_f32, -0.5, 0.75, -1.0];
        let mut actual_right = [-0.125_f32, 0.375, -0.625, 0.875];
        let mut expected_left = actual_left;
        let mut expected_right = actual_right;
        let mut actual_dry_left = [0.1_f32, 0.2, 0.3, 0.4];
        let mut actual_dry_right = [-0.1_f32, -0.2, -0.3, -0.4];
        let mut expected_dry_left = actual_dry_left;
        let mut expected_dry_right = actual_dry_right;
        let mut actual_pre_fader_left = [0.0; 4];
        let mut actual_pre_fader_right = [0.0; 4];
        let mut expected_pre_fader_left = [0.0; 4];
        let mut expected_pre_fader_right = [0.0; 4];
        let mut actual_send_pre_left = [0.0; 4];
        let mut actual_send_pre_right = [0.0; 4];
        let mut expected_send_pre_left = [0.0; 4];
        let mut expected_send_pre_right = [0.0; 4];
        let mut actual_send_post_left = [0.0; 4];
        let mut actual_send_post_right = [0.0; 4];
        let mut expected_send_post_left = [0.0; 4];
        let mut expected_send_post_right = [0.0; 4];
        let actual_channels = [actual_left.as_mut_ptr(), actual_right.as_mut_ptr()];
        let expected_channels = [expected_left.as_mut_ptr(), expected_right.as_mut_ptr()];
        let actual_dry = [actual_dry_left.as_mut_ptr(), actual_dry_right.as_mut_ptr()];
        let expected_dry = [
            expected_dry_left.as_mut_ptr(),
            expected_dry_right.as_mut_ptr(),
        ];
        let actual_pre_fader = [
            actual_pre_fader_left.as_mut_ptr(),
            actual_pre_fader_right.as_mut_ptr(),
        ];
        let actual_send_pre = [
            actual_send_pre_left.as_mut_ptr(),
            actual_send_pre_right.as_mut_ptr(),
        ];
        let actual_send_post = [
            actual_send_post_left.as_mut_ptr(),
            actual_send_post_right.as_mut_ptr(),
        ];
        unsafe {
            let actual_strip = crate::channel_strip::hirari_channel_strip_create();
            let expected_strip = crate::channel_strip::hirari_channel_strip_create();
            let actual_panner =
                crate::track_holographic_panner::hirari_track_holographic_panner_create();
            let actual_pdc = crate::track_pdc_delay::hirari_track_pdc_delay_create();
            let expected_pdc = crate::track_pdc_delay::hirari_track_pdc_delay_create();
            let mut actual_low_l = 0.0;
            let mut actual_low_r = 0.0;
            let mut expected_low_l = 0.0;
            let mut expected_low_r = 0.0;

            hirari_track_finalize_output(
                2047,
                frames,
                0.0,
                std::ptr::null(),
                0,
                std::ptr::null_mut(),
                std::ptr::null(),
                0,
                std::ptr::null_mut(),
                actual_strip,
                actual_channels.as_ptr(),
                2,
                actual_dry.as_ptr(),
                2,
                actual_send_pre.as_ptr(),
                2,
                actual_send_post.as_ptr(),
                2,
                actual_panner,
                0,
                0.0,
                0.0,
                0.0,
                0.8,
                1.2,
                &mut actual_low_l,
                &mut actual_low_r,
                actual_pdc,
                4,
                2,
                actual_pre_fader.as_ptr(),
                2,
            );

            crate::track_channel_eq::hirari_track_eq_process(
                expected_left.as_mut_ptr(),
                expected_right.as_mut_ptr(),
                frames,
                0.8,
                1.2,
                &mut expected_low_l,
                &mut expected_low_r,
            );
            expected_pre_fader_left.copy_from_slice(&expected_left);
            expected_pre_fader_right.copy_from_slice(&expected_right);
            expected_send_pre_left.copy_from_slice(&expected_left);
            expected_send_pre_right.copy_from_slice(&expected_right);
            crate::channel_strip::hirari_channel_strip_process_mirror(
                expected_strip,
                expected_channels.as_ptr(),
                2,
                expected_dry.as_ptr(),
                2,
                0,
                frames as usize,
            );
            expected_send_post_left.copy_from_slice(&expected_left);
            expected_send_post_right.copy_from_slice(&expected_right);
            crate::track_pdc_delay::hirari_track_pdc_delay_process(
                expected_pdc,
                expected_left.as_mut_ptr(),
                expected_right.as_mut_ptr(),
                expected_pre_fader_left.as_mut_ptr(),
                expected_pre_fader_right.as_mut_ptr(),
                frames,
                6,
            );

            assert_eq!(actual_left, expected_left);
            assert_eq!(actual_right, expected_right);
            assert_eq!(actual_dry_left, expected_dry_left);
            assert_eq!(actual_dry_right, expected_dry_right);
            assert_eq!(actual_pre_fader_left, expected_pre_fader_left);
            assert_eq!(actual_pre_fader_right, expected_pre_fader_right);
            assert_eq!(actual_send_pre_left, expected_send_pre_left);
            assert_eq!(actual_send_pre_right, expected_send_pre_right);
            assert_eq!(actual_send_post_left, expected_send_post_left);
            assert_eq!(actual_send_post_right, expected_send_post_right);
            assert_eq!(actual_low_l, expected_low_l);
            assert_eq!(actual_low_r, expected_low_r);

            crate::channel_strip::hirari_channel_strip_destroy(actual_strip);
            crate::channel_strip::hirari_channel_strip_destroy(expected_strip);
            crate::track_holographic_panner::hirari_track_holographic_panner_destroy(actual_panner);
            crate::track_pdc_delay::hirari_track_pdc_delay_destroy(actual_pdc);
            crate::track_pdc_delay::hirari_track_pdc_delay_destroy(expected_pdc);
        }
    }
}
