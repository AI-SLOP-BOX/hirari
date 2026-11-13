//! Region restore, sidechains/feedback/audio routes and freeze caches,
//! split out of hirari_core_project_loading.rs.
use super::*;

impl HirariCore {
    pub(super) fn restore_project_elements(
        &self,
        document: &ProjectDocument,
        engine: &crate::AudioEngine,
        sample_rate: u32,
        resolved_region_paths: &[String],
        native_track_ids: &std::collections::HashMap<u32, u32>,
        project_parent: &std::path::Path,
        rollback_error: &dyn Fn(anyhow::Error) -> anyhow::Error,
    ) -> anyhow::Result<()> {
        // Region IDs are allocated by the native engine. Since the project was
        // reset immediately above, the first imported region is deterministic;
        // use the native layout after each import to obtain its actual ID.
        let mut used_native_region_ids = std::collections::HashSet::new();
        for (region, region_path) in document.regions.iter().zip(resolved_region_paths.iter()) {
            let native_track_id = *native_track_ids
                .get(&region.track_id)
                .ok_or_else(|| anyhow::anyhow!("region references an unknown track"))
                .map_err(rollback_error)?;
            let source_trim = region
                .source_offset
                .saturating_sub(region.base_source_offset);
            // Import at the desired clip start first. The decoder reports the
            // actual source rate through the native layout, which also lets
            // legacy project files without this metadata hydrate correctly.
            if !engine.add_region(native_track_id, region_path, region.start as f64) {
                return Err(rollback_error(anyhow::anyhow!(
                    "native engine failed to import region {}",
                    region.id
                )));
            }
            let layout = engine.project_layout_json();
            let native_regions: Vec<NativeLayoutTrack> = serde_json::from_str(&layout)
                .context("native engine returned malformed project layout")
                .map_err(rollback_error)?;
            let native_region = native_regions
                .into_iter()
                .filter(|track| track.id == native_track_id)
                .flat_map(|track| track.regions)
                .find(|candidate| {
                    candidate.path == region_path.as_str()
                        && candidate.start == region.start
                        && (region.base_length > 0 || candidate.len == region.length)
                        && !used_native_region_ids.contains(&candidate.id)
                })
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "native engine did not preserve region position for {}",
                        region.id
                    )
                })
                .map_err(rollback_error)?;
            used_native_region_ids.insert(native_region.id);
            let source_rate = if native_region.source_sample_rate == 0 {
                if region.source_sample_rate == 0 {
                    sample_rate as f64
                } else {
                    f64::from(region.source_sample_rate)
                }
            } else {
                f64::from(native_region.source_sample_rate)
            };
            let source_per_timeline = region.warp_ratio * source_rate / sample_rate as f64;
            let timeline_trim = (source_trim as f64 / source_per_timeline).ceil();
            if !timeline_trim.is_finite() || timeline_trim < 0.0 ||
                timeline_trim > region.start as f64 {
                return Err(rollback_error(anyhow::anyhow!(
                    "region {} trim exceeds its timeline position", region.id
                )));
            }
            let base_start = region.start - timeline_trim as u64;
            if !engine.move_region(native_track_id, native_region.id, base_start as f64) {
                return Err(rollback_error(anyhow::anyhow!(
                    "native engine failed to restore region origin {}", region.id
                )));
            }
            let source_length = if region.source_length > 0 {
                region.source_length
            } else {
                let inferred = (region.length as f64 * source_rate / sample_rate as f64).round();
                if !inferred.is_finite() || inferred < 1.0 || inferred > u64::MAX as f64 {
                    return Err(rollback_error(anyhow::anyhow!(
                        "region {} source duration is out of range", region.id
                    )));
                }
                inferred as u64
            };
            let trim_base_length = if region.base_length > 0 {
                region.base_length
            } else {
                native_region.source_length
            };
            let trim_base_source_offset = if region.base_length > 0 {
                region.base_source_offset
            } else {
                0
            };
            let warp_markers = document
                .warp_markers
                .iter()
                .filter(|marker| marker.region_id == region.id)
                .collect::<Vec<_>>();
            let warp_sources = warp_markers
                .iter()
                .map(|marker| marker.source_sample.saturating_sub(region.source_offset))
                .collect::<Vec<_>>();
            let warp_timeline = warp_markers
                .iter()
                .map(|marker| marker.timeline_sample.saturating_sub(region.start))
                .collect::<Vec<_>>();
            let warp_transients = warp_markers
                .iter()
                .map(|marker| u8::from(marker.transient))
                .collect::<Vec<_>>();
            let trim_ok = if trim_base_length > 0 {
                let source_start = region.source_offset.saturating_sub(trim_base_source_offset);
                let start_norm = source_start as f64 / trim_base_length as f64;
                let end_norm = source_start.saturating_add(source_length) as f64
                    / trim_base_length as f64;
                engine.set_region_trim(
                    native_track_id,
                    native_region.id,
                    start_norm as f32,
                    end_norm as f32,
                )
            } else {
                true
            };
            // The document stores fades as absolute samples, but the native
            // setter takes timeline-normalized fractions (matching how it
            // derives them from its own state: fadeInSamples / len).
            // Passing raw samples rejected every region whose default
            // 64-sample fades exceeded the 0..1 range, so no
            // natively-created project could be reloaded at all.
            let fade_base = region.length.max(1) as f64;
            let fade_in = (region.fade_in_samples as f64 / fade_base).clamp(0.0, 1.0) as f32;
            let fade_out =
                (region.fade_out_samples as f64 / fade_base).clamp(0.0, 1.0) as f32;
            // Each envelope setter is checked separately so a failure names
            // the step instead of the whole region: a single combined
            // boolean hid which of the eleven calls rejected the restore.
            macro_rules! restore_step {
                ($label:literal, $call:expr) => {
                    if !$call {
                        return Err(rollback_error(anyhow::anyhow!(
                            "native engine failed to restore region envelope {} at {}",
                            region.id,
                            $label
                        )));
                    }
                };
            }
            restore_step!(
                "gain",
                engine.set_region_gain(native_track_id, native_region.id, region.clip_gain)
            );
            restore_step!(
                "muted",
                engine.set_region_muted(native_track_id, native_region.id, region.muted)
            );
            restore_step!(
                "fades",
                engine.set_region_fades(native_track_id, native_region.id, fade_in, fade_out)
            );
            if !trim_ok {
                return Err(rollback_error(anyhow::anyhow!(
                    "native engine failed to restore region envelope {} at trim",
                    region.id
                )));
            }
            restore_step!(
                "reverse",
                engine.set_region_reverse(native_track_id, native_region.id, region.reverse)
            );
            restore_step!(
                "warp_ratio",
                engine.set_region_warp_ratio(
                    native_track_id,
                    native_region.id,
                    region.warp_ratio
                )
            );
            restore_step!(
                "pitch_preserve_warp",
                engine.set_region_pitch_preserve_warp(
                    native_track_id,
                    native_region.id,
                    region.pitch_preserve_warp
                )
            );
            restore_step!(
                "pitch_semitones",
                engine.set_region_pitch_semitones(
                    native_track_id,
                    native_region.id,
                    region.pitch_semitones
                )
            );
            restore_step!(
                "loop_count",
                engine.set_region_loop_count(
                    native_track_id,
                    native_region.id,
                    region.loop_count
                )
            );
            restore_step!(
                "sync_group",
                engine.set_region_sync_group(
                    native_track_id,
                    native_region.id,
                    region.sync_group
                )
            );
            restore_step!(
                "warp_markers",
                engine.set_region_warp_markers(
                    native_track_id,
                    native_region.id,
                    &warp_sources,
                    &warp_timeline,
                    &warp_transients
                )
            );
        }

        // Sidechain routes are logical project data; their runtime buffers
        // must be recreated only after every referenced track and plugin has
        // been hydrated. A failure rolls the whole hydration back instead of
        // leaving a project with a visually connected but silent edge.
        for route in &document.sidechain_routes {
            let native_source_id = *native_track_ids
                .get(&route.source_id)
                .ok_or_else(|| anyhow::anyhow!("sidechain source references an unknown track"))
                .map_err(rollback_error)?;
            let native_destination_id = *native_track_ids
                .get(&route.destination_id)
                .ok_or_else(|| anyhow::anyhow!("sidechain destination references an unknown track"))
                .map_err(rollback_error)?;
            if !engine.set_sidechain_link(
                native_source_id,
                native_destination_id,
                route.plugin_index,
                route.tap_point,
                true,
            ) {
                return Err(rollback_error(anyhow::anyhow!(
                    "native engine rejected persisted sidechain route"
                )));
            }
        }

        for route in &document.feedback_routes {
            let native_source_id = *native_track_ids
                .get(&route.source_id)
                .ok_or_else(|| anyhow::anyhow!("feedback source references an unknown track"))
                .map_err(rollback_error)?;
            let native_destination_id = *native_track_ids
                .get(&route.destination_id)
                .ok_or_else(|| anyhow::anyhow!("feedback destination references an unknown track"))
                .map_err(rollback_error)?;
            if !engine.set_feedback_route(native_source_id, native_destination_id, route.gain, true)
            {
                return Err(rollback_error(anyhow::anyhow!(
                    "native engine rejected persisted feedback route"
                )));
            }
        }

        for route in &document.audio_routes {
            let native_source_id = *native_track_ids
                .get(&route.source_id)
                .ok_or_else(|| anyhow::anyhow!("audio route source references an unknown track"))
                .map_err(rollback_error)?;
            let native_destination_id = *native_track_ids
                .get(&route.destination_id)
                .ok_or_else(|| {
                    anyhow::anyhow!("audio route destination references an unknown track")
                })
                .map_err(rollback_error)?;
            let accepted = if route.send {
                engine.set_send_route(native_source_id, native_destination_id, route.gain,
                                      route.pre_fader, true)
            } else {
                engine.set_route_gain(native_source_id, native_destination_id, route.gain, true)
            };
            if !accepted {
                return Err(rollback_error(anyhow::anyhow!(
                    "native engine rejected persisted audio route or send"
                )));
            }
        }

        // Freeze caches are immutable render artifacts on disk, but the
        // native graph owns the runtime buffer. Rebuild that buffer only
        // after the complete track/plugin/region graph has been hydrated;
        // otherwise a partial project could become audible as a frozen track.
        for artifact in &document.freeze_artifacts {
            let native_track_id = *native_track_ids
                .get(&artifact.track_id)
                .ok_or_else(|| anyhow::anyhow!("freeze artifact references an unknown track"))
                .map_err(rollback_error)?;
            let cache_path = std::path::Path::new(&artifact.path);
            let resolved_cache_path = if cache_path.is_absolute() {
                cache_path.to_path_buf()
            } else {
                project_parent.join(cache_path)
            };
            if !engine.restore_track_freeze_from_file(
                native_track_id,
                resolved_cache_path.to_string_lossy().as_ref(),
                artifact.total_samples,
                sample_rate,
            ) {
                return Err(rollback_error(anyhow::anyhow!(
                    "native engine failed to restore freeze artifact for track {}",
                    artifact.track_id
                )));
            }
        }
        Ok(())
    }
}
