//! Engine-bound shell methods for the AmeUI app, split out of ameui/mod.rs.
use crate::ameui::*;

impl HirariAmeApp {
    /// Consumes finished waveform decodes and queues missing clip peaks,
    /// mirroring the telemetry queue: at most 8 detached decodes in
    /// flight, generation-guarded results, failures remembered per
    /// generation. Runs on the layout cadence, never on the UI event path.
    pub(in crate::ameui) fn poll_waveforms(&mut self, core: &HirariCore) {
        const MAX_REQUESTS: usize = 8;
        let revision = core.project_state_revision();
        let generation = if revision == self.gen_revision {
            self.gen_value
        } else {
            let generation = core.project_generation();
            self.gen_revision = revision;
            self.gen_value = generation;
            generation
        };
        if generation != self.last_gen {
            self.last_gen = generation;
            self.clip_peaks.set(HashMap::new());
            self.failed_waveforms.clear();
        }
        let tracks = self.tracks.get();
        let completed: Vec<(u64, u32, u32, u64)> = self
            .waveform_requests
            .iter()
            .map(|(id, &(track, clip, generation))| (*id, track, clip, generation))
            .collect();
        for (request, track_id, clip_id, request_generation) in completed {
            if core.region_waveform_pending(request) {
                continue;
            }
            let waveform = core.poll_region_waveform(request);
            if request_generation == generation && !waveform.is_empty() {
                let still_exists = tracks.iter().any(|track| {
                    track.id == track_id && track.clips.iter().any(|clip| clip.id == clip_id as i32)
                });
                if still_exists {
                    let mut peaks = self.clip_peaks.get();
                    peaks.insert((track_id, clip_id as i32), waveform);
                    self.clip_peaks.set(peaks);
                }
            } else if request_generation == generation {
                self.failed_waveforms
                    .insert((generation, track_id, clip_id));
            }
            self.waveform_requests.remove(&request);
        }
        self.failed_waveforms
            .retain(|(gen, _, _)| *gen == generation);
        'queue: for track in &tracks {
            for clip in &track.clips {
                if clip.midi || clip.id <= 0 {
                    continue;
                }
                let key = (generation, track.id, clip.id as u32);
                if self.clip_peaks.get().contains_key(&(track.id, clip.id))
                    || self.failed_waveforms.contains(&key)
                    || self.waveform_requests.values().any(
                        |&(queued_track, queued_clip, queued_gen)| {
                            queued_gen == generation
                                && queued_track == track.id
                                && queued_clip == clip.id as u32
                        },
                    )
                {
                    continue;
                }
                if self.waveform_requests.len() >= MAX_REQUESTS {
                    break 'queue;
                }
                let request = core.queue_region_waveform(track.id, clip.id as u32);
                if request != 0 {
                    self.waveform_requests
                        .insert(request, (track.id, clip.id as u32, generation));
                }
            }
        }
    }
}
