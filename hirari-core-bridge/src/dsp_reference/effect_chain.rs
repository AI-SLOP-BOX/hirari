use std::ffi::c_void;

type NodeInfo = Option<unsafe extern "C" fn(*mut c_void, u32, *mut bool, *mut bool) -> bool>;
type NodeProcess = Option<unsafe extern "C" fn(*mut c_void, u32, bool, *mut f32) -> bool>;

#[no_mangle]
#[allow(clippy::too_many_arguments)]
pub unsafe extern "C" fn hirari_effect_chain_process_reference(
    user_data: *mut c_void,
    node_count: u32,
    channels: *mut *mut f32,
    channel_count: u32,
    parallel_channels: *mut *mut f32,
    parallel_channel_count: u32,
    parallel_capacity: u32,
    frames: u32,
    node_info: NodeInfo,
    process_node: NodeProcess,
) {
    let (Some(node_info), Some(process_node)) = (node_info, process_node) else {
        return;
    };
    if channels.is_null() || channel_count == 0 || frames == 0 {
        return;
    }
    for index in 0..node_count {
        let mut bypassed = false;
        let mut parallel = false;
        if !node_info(user_data, index, &mut bypassed, &mut parallel) || bypassed {
            continue;
        }
        let use_parallel = parallel
            && channel_count <= 2
            && channel_count <= parallel_channel_count
            && frames <= parallel_capacity
            && !parallel_channels.is_null();
        if use_parallel {
            for channel in 0..channel_count as usize {
                let source = *channels.add(channel);
                let destination = *parallel_channels.add(channel);
                if !source.is_null() && !destination.is_null() {
                    std::ptr::copy_nonoverlapping(source, destination, frames as usize);
                }
            }
        }
        let mut mix = 1.0f32;
        if !process_node(user_data, index, use_parallel, &mut mix) {
            for channel in 0..channel_count as usize {
                let samples = *channels.add(channel);
                if !samples.is_null() {
                    std::slice::from_raw_parts_mut(samples, frames as usize).fill(0.0);
                }
            }
            continue;
        }
        if use_parallel {
            mix = mix.clamp(0.0, 1.0);
            for channel in 0..channel_count as usize {
                let dry = *channels.add(channel);
                let wet = *parallel_channels.add(channel);
                if dry.is_null() || wet.is_null() {
                    continue;
                }
                for frame in 0..frames as usize {
                    *dry.add(frame) = *dry.add(frame) * (1.0 - mix) + *wet.add(frame) * mix;
                }
            }
        }
        for channel in 0..channel_count as usize {
            let samples = *channels.add(channel);
            if samples.is_null() {
                continue;
            }
            for frame in 0..frames as usize {
                if !(*samples.add(frame)).is_finite() {
                    *samples.add(frame) = 0.0;
                }
            }
        }
    }
}
