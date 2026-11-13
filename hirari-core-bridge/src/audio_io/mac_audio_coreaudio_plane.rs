//! Planar buffer list layout and the render callback.
use super::*;

pub(super) struct PlanarBufferList {
    // ABI prefix of `AudioBufferList` (mNumberBuffers); read by the AU host
    // under the cast, never on the Rust side. Layout is load-bearing.
    #[allow(dead_code)]
    number_buffers: u32,
    buffers: [sys::AudioBuffer; MAX_CHANNELS],
}

pub(super) fn silence_buffers(data: *mut sys::AudioBufferList, frames: u32) {
    if data.is_null() {
        return;
    }
    unsafe {
        let list = &mut *data;
        for index in 0..list.mNumberBuffers as usize {
            let buffer = &mut *list.mBuffers.as_mut_ptr().add(index);
            if buffer.mData.is_null() || buffer.mNumberChannels == 0 {
                continue;
            }
            ptr::write_bytes(
                buffer.mData.cast::<f32>(),
                0,
                frames as usize * buffer.mNumberChannels as usize,
            );
        }
    }
}

/// Fan a planar device callback into the engine in bounded chunks. Returns
/// whether the process callback ran. Callers already own the in-flight guard.
pub(crate) unsafe fn dispatch_planar_outputs(
    state: &RealtimeState,
    engine_inputs: *const *const f32,
    engine_input_channels: u32,
    base_outputs: &[*mut f32; MAX_CHANNELS],
    output_channels: u32,
    frames: u32,
) -> bool {
    let Some(process) = state.process_callback() else {
        return false;
    };
    let context = state.process_context.load(Ordering::Acquire) as *mut c_void;
    let chunk_limit = state.buffer_size.load(Ordering::Acquire).max(1);
    let mut dispatched = false;

    for (offset, length) in split_frames(frames, chunk_limit) {
        let mut outputs: [*mut f32; MAX_CHANNELS] = [ptr::null_mut(); MAX_CHANNELS];
        for channel in 0..output_channels as usize {
            outputs[channel] = base_outputs[channel].add(offset as usize);
        }
        let mut inputs: [*const f32; MAX_CHANNELS] = [ptr::null(); MAX_CHANNELS];
        if engine_input_channels > 0 && !engine_inputs.is_null() {
            for channel in 0..engine_input_channels as usize {
                let input = *engine_inputs.add(channel);
                if !input.is_null() {
                    inputs[channel] = input.add(offset as usize);
                }
            }
        }
        process(
            if engine_input_channels > 0 {
                inputs.as_ptr()
            } else {
                ptr::null()
            },
            engine_input_channels,
            outputs.as_ptr(),
            output_channels,
            length,
            context,
        );
        dispatched = true;
    }

    if dispatched {
        let mut peak = 0.0f32;
        for channel in 0..output_channels as usize {
            let output = base_outputs[channel];
            for frame in 0..frames as usize {
                peak = peak.max(output.add(frame).read().abs());
            }
        }
        state.stats.store_peak(peak);
        state.stats.record_callback();
    }
    dispatched
}

pub(super) unsafe extern "C" fn audio_unit_render_callback(
    ref_con: *mut c_void,
    action_flags: *mut sys::AudioUnitRenderActionFlags,
    time_stamp: *const sys::AudioTimeStamp,
    _bus_number: u32,
    frames: u32,
    data: *mut sys::AudioBufferList,
) -> sys::OSStatus {
    if ref_con.is_null() || data.is_null() {
        return 0;
    }
    let state = &*(ref_con as *const RealtimeState);
    state.callbacks_in_flight.fetch_add(1, Ordering::Acquire);
    struct CallbackGuard<'a>(&'a RealtimeState);
    impl Drop for CallbackGuard<'_> {
        fn drop(&mut self) {
            self.0.callbacks_in_flight.fetch_sub(1, Ordering::Release);
        }
    }
    let _guard = CallbackGuard(state);

    let list = &mut *data;
    if list.mNumberBuffers == 0 {
        return 0;
    }

    let mut engine_inputs: [*const f32; MAX_CHANNELS] = [ptr::null(); MAX_CHANNELS];
    let mut engine_input_channels = 0u32;

    let input_channels = state.input_channels.load(Ordering::Acquire) as usize;
    let input_capacity = state.input_buffer_frames.load(Ordering::Acquire);
    let input_buffer = state.input_buffer.load(Ordering::Acquire);
    let unit = state.unit.load(Ordering::Acquire).cast::<sys::ComponentInstanceRecord>();
    if !state.stopping.load(Ordering::Acquire)
        && state.running.load(Ordering::Acquire)
        && !input_buffer.is_null()
        && !unit.is_null()
        && input_channels > 0
        && frames <= input_capacity
    {
        let mut input_data = PlanarBufferList {
            number_buffers: input_channels as u32,
            buffers: [sys::AudioBuffer::default(); MAX_CHANNELS],
        };
        for channel in 0..input_channels {
            input_data.buffers[channel] = sys::AudioBuffer {
                mNumberChannels: 1,
                mDataByteSize: frames * size_of::<f32>() as u32,
                mData: input_buffer
                    .add(channel * input_capacity as usize)
                    .cast::<c_void>(),
            };
        }
        let render_status = sys::AudioUnitRender(
            unit,
            action_flags,
            time_stamp,
            1,
            frames,
            (&mut input_data as *mut PlanarBufferList).cast::<sys::AudioBufferList>(),
        );
        if render_status == 0 {
            let mut channels: [*const f32; MAX_CHANNELS] = [ptr::null(); MAX_CHANNELS];
            for channel in 0..input_channels {
                channels[channel] = input_data.buffers[channel].mData.cast::<f32>();
            }
            engine_inputs = channels;
            engine_input_channels = input_channels as u32;

            let _ = state.queue.push_planar(
                channels.as_ptr(),
                input_channels as u32,
                frames,
            );
            let sink = state.input_sink.load(Ordering::Acquire);
            if !sink.is_null() {
                if let Some(callback) = (*sink).callback {
                    callback(
                        (*sink).context,
                        channels.as_ptr(),
                        input_channels as u32,
                        frames,
                        time_stamp.cast::<c_void>(),
                    );
                }
            }
        }
    }

    if state.stopping.load(Ordering::Acquire)
        || !state.running.load(Ordering::Acquire)
        || state.process_callback().is_none()
    {
        silence_buffers(data, frames);
        return 0;
    }

    if frames > MAX_FRAMES_PER_SLICE {
        silence_buffers(data, frames);
        return 0;
    }

    let output_channels = state.output_channels.load(Ordering::Acquire);
    if output_channels > 0 && list.mNumberBuffers >= output_channels {
        let mut base_outputs: [*mut f32; MAX_CHANNELS] = [ptr::null_mut(); MAX_CHANNELS];
        let mut planar = true;
        for channel in 0..output_channels as usize {
            let buffer = &*list.mBuffers.as_ptr().add(channel);
            if buffer.mData.is_null() || buffer.mNumberChannels != 1 {
                planar = false;
                break;
            }
            base_outputs[channel] = buffer.mData.cast::<f32>();
        }
        if planar {
            let _ = dispatch_planar_outputs(
                state,
                engine_inputs.as_ptr(),
                engine_input_channels,
                &base_outputs,
                output_channels,
                frames,
            );
        } else {
            silence_buffers(data, frames);
        }
    } else if !list.mBuffers[0].mData.is_null() {
        let buffer = &mut *list.mBuffers.as_mut_ptr();
        ptr::write_bytes(
            buffer.mData.cast::<f32>(),
            0,
            frames as usize * buffer.mNumberChannels.max(1) as usize,
        );
    }

    0
}
