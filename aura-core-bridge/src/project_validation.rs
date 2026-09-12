impl ProjectDocument {
    pub fn validate(&self) -> Result<()> {
        if self.schema_version != PROJECT_SCHEMA_VERSION {
            bail!("unsupported project schema version {}", self.schema_version);
        }
        if self.metadata.version != PROJECT_SCHEMA_VERSION {
            bail!("metadata version does not match project schema");
        }
        if self.contract_version != PROJECT_CONTRACT_VERSION {
            bail!(
                "unsupported project contract version {}",
                self.contract_version
            );
        }
        if Uuid::parse_str(&self.project_id).is_err() {
            bail!("project id must be a UUID");
        }
        if self.metadata.name.trim().is_empty() {
            bail!("project name must not be empty");
        }
        if !self.metadata.bpm.is_finite() || !(20.0..=300.0).contains(&self.metadata.bpm) {
            bail!("project BPM is outside the supported range");
        }
        if !self.sample_rate.is_finite()
            || self.sample_rate.fract() != 0.0
            || !(1.0..=384_000.0).contains(&self.sample_rate)
        {
            bail!("project sample rate is invalid");
        }
        if !self.master_gain.is_finite() || !(0.0..=2.0).contains(&self.master_gain) {
            bail!("project master gain is invalid");
        }
        if let Some(control_room) = &self.control_room {
            if !control_room.validate() {
                bail!("project control room state is invalid");
            }
        }
        if self.cycle_enabled && self.cycle_start_sample >= self.cycle_end_sample {
            bail!("project cycle range is invalid");
        }
        for vocal in &self.openutau_vocals {
            crate::openutau::validate_source(&vocal.source_path)
                .map_err(|error| anyhow::anyhow!(error))?;
            crate::openutau::validate_render(&vocal.rendered_audio_path)
                .map_err(|error| anyhow::anyhow!(error))?;
            let tuning = [
                vocal.tuning.scoop,
                vocal.tuning.vibrato,
                vocal.tuning.dynamics,
                vocal.tuning.consonants,
            ];
            if tuning.iter().any(|value| !value.is_finite() || !(0.0..=1.0).contains(value)) {
                bail!("OpenUtau vocal tuning values are invalid");
            }
        }
        if self.metadata.tracks_count as usize != self.tracks.len() {
            bail!("metadata track count does not match track data");
        }
        if self.tracks.len() > MAX_PROJECT_TRACKS {
            bail!("project contains too many tracks");
        }
        if self.regions.len() > MAX_PROJECT_REGIONS {
            bail!("project contains too many regions");
        }
        if self.plugin_instances.len() > MAX_PROJECT_PLUGIN_INSTANCES {
            bail!("project contains too many plugin instances");
        }
        if self.comp_takes.len() > MAX_PROJECT_COMP_TAKES {
            bail!("project contains too many comp takes");
        }
        if self.comp_segments.len() > MAX_PROJECT_COMP_SEGMENTS {
            bail!("project contains too many comp segments");
        }
        if self.freeze_artifacts.len() > MAX_PROJECT_TRACKS {
            bail!("project contains too many freeze artifacts");
        }
        if self.sidechain_routes.len() > MAX_PROJECT_PLUGIN_INSTANCES {
            bail!("project contains too many sidechain routes");
        }
        if self.feedback_routes.len() > 16 {
            bail!("project contains too many feedback routes");
        }
        let track_ids: HashSet<u32> = self.tracks.iter().map(|track| track.id).collect();
        if self.audio_routes.len() > MAX_PROJECT_TRACKS {
            bail!("project contains too many audio routes");
        }
        let mut audio_route_keys = HashSet::with_capacity(self.audio_routes.len());
        for route in &self.audio_routes {
            if route.source_id == route.destination_id
                || !track_ids.contains(&route.source_id)
                || !track_ids.contains(&route.destination_id)
                || !route.gain.is_finite()
                || !(0.0..=2.0).contains(&route.gain)
                || !audio_route_keys.insert((route.source_id, route.destination_id))
            {
                bail!("invalid or duplicate audio route");
            }
        }
        let mut sidechain_keys = HashSet::with_capacity(self.sidechain_routes.len());
        for route in &self.sidechain_routes {
            if route.source_id == 0
                || route.destination_id == 0
                || route.source_id == route.destination_id
                || !track_ids.contains(&route.source_id)
                || !track_ids.contains(&route.destination_id)
                || route.plugin_index > 65_535
                || route.tap_point > 2
                || !sidechain_keys.insert((route.destination_id, route.plugin_index))
            {
                bail!("invalid or duplicate sidechain route");
            }
        }
        let mut feedback_keys = HashSet::with_capacity(self.feedback_routes.len());
        for route in &self.feedback_routes {
            if route.source_id == 0
                || route.destination_id == 0
                || route.source_id == route.destination_id
                || !track_ids.contains(&route.source_id)
                || !track_ids.contains(&route.destination_id)
                || !route.gain.is_finite()
                || !(0.0..=2.0).contains(&route.gain)
                || !feedback_keys.insert((route.source_id, route.destination_id))
            {
                bail!("invalid or duplicate feedback route");
            }
        }
        let mut freeze_track_ids = HashSet::with_capacity(self.freeze_artifacts.len());
        let mut total_freeze_bytes = 0u64;
        for artifact in &self.freeze_artifacts {
            if artifact.track_id == 0
                || !track_ids.contains(&artifact.track_id)
                || !freeze_track_ids.insert(artifact.track_id)
                || artifact.project_generation == 0
                || artifact.audio_generation == 0
                || artifact.total_samples == 0
                || artifact.total_samples > MAX_FREEZE_SAMPLES
                || artifact.sample_rate == 0
                || artifact.path.trim().is_empty()
                || artifact.path.len() > 4096
                || artifact.path.contains('\0')
                || artifact.content_checksum == 0
            {
                bail!("invalid freeze artifact metadata for track {}", artifact.track_id);
            }
            total_freeze_bytes = total_freeze_bytes
                .checked_add(artifact.total_samples.saturating_mul(8))
                .ok_or_else(|| anyhow::anyhow!("freeze artifact size overflows"))?;
            if total_freeze_bytes > MAX_FREEZE_CACHE_BYTES {
                bail!("freeze cache budget exceeded");
            }
        }
        let mut comp_take_ids = HashSet::with_capacity(self.comp_takes.len());
        for take in &self.comp_takes {
            take.validate()?;
            if !comp_take_ids.insert(take.id) {
                bail!("duplicate comp take id {}", take.id);
            }
        }
        let mut sorted_segments = self.comp_segments.clone();
        sorted_segments.sort_by_key(|segment| segment.start_sample);
        for segment in &sorted_segments {
            segment.validate()?;
            if !comp_take_ids.contains(&segment.take_id) {
                bail!("comp segment references unknown take {}", segment.take_id);
            }
        }
        if sorted_segments.windows(2).any(|pair| {
            pair[0].start_sample.checked_add(pair[0].length_samples)
                .is_none_or(|end| end > pair[1].start_sample)
        }) {
            bail!("comp segments overlap");
        }

        let mut total_state_bytes = 0usize;
        let mut total_string_bytes = self.metadata.name.len() + self.project_id.len();
        for track in &self.tracks {
            total_string_bytes = total_string_bytes
                .checked_add(track.name.len())
                .and_then(|value| {
                    track
                        .sandbox_plugin_paths
                        .iter()
                        .try_fold(value, |sum, path| sum.checked_add(path.len()))
                })
                .ok_or_else(|| anyhow::anyhow!("project string data size overflow"))?;
            for state in track.plugin_states.iter().chain(track.sandbox_plugin_states.iter()) {
                total_state_bytes = total_state_bytes
                    .checked_add(state.len())
                    .ok_or_else(|| anyhow::anyhow!("project plugin state size overflow"))?;
            }
        }
        for region in &self.regions {
            total_string_bytes = total_string_bytes
                .checked_add(region.name.len())
                .and_then(|value| value.checked_add(region.path.len()))
                .ok_or_else(|| anyhow::anyhow!("project string data size overflow"))?;
        }
        for plugin in &self.plugin_instances {
            total_string_bytes = total_string_bytes
                .checked_add(plugin.instance_id.len())
                .and_then(|value| value.checked_add(plugin.bundle_path.len()))
                .and_then(|value| value.checked_add(plugin.plugin_id.len()))
                .and_then(|value| value.checked_add(plugin.binary_hash.len()))
                .and_then(|value| value.checked_add(plugin.capability.len()))
                .and_then(|value| value.checked_add(plugin.plugin_version.len()))
                .and_then(|value| value.checked_add(plugin.architecture.len()))
                .and_then(|value| {
                    plugin.component_ids.iter().try_fold(value, |total, component| {
                        total.checked_add(component.len())
                    })
                })
                .ok_or_else(|| anyhow::anyhow!("project string data size overflow"))?;
            total_state_bytes = total_state_bytes
                .checked_add(plugin.state_blob.len())
                .and_then(|value| value.checked_add(plugin.gui_state.len()))
                .ok_or_else(|| anyhow::anyhow!("project plugin state size overflow"))?;
            if plugin.parameter_ids.len() > MAX_PLUGIN_PARAMETERS {
                bail!("plugin parameter metadata is too large");
            }
            if plugin.parameter_values.len() > MAX_PLUGIN_PARAMETERS
                || plugin
                    .parameter_values
                    .iter()
                    .any(|value| !value.is_finite() || !(0.0..=1.0).contains(value))
            {
                bail!("plugin parameter values are invalid");
            }
        }
        if total_state_bytes > MAX_PROJECT_STATE_BYTES {
            bail!("project plugin state data exceeds aggregate limit");
        }
        if total_string_bytes > MAX_PROJECT_STRING_BYTES {
            bail!("project string data exceeds aggregate limit");
        }
        if self.midi_events.len() > MAX_PROJECT_REGIONS {
            bail!("project MIDI event count exceeds aggregate limit");
        }
        if self.midi_events.iter().any(|event| !event.validate()) {
            bail!("project contains invalid MIDI, SysEx, or MIDI 2.0 event");
        }
        let midi_payload_bytes = self
            .midi_events
            .iter()
            .try_fold(0usize, |total, event| {
                let payload = match &event.kind {
                    MIDIEventKind::SysEx { data } => data.len(),
                    _ => 0,
                };
                total.checked_add(payload)
            })
            .ok_or_else(|| anyhow::anyhow!("project MIDI payload size overflow"))?;
        if midi_payload_bytes > MAX_PROJECT_MIDI_BYTES {
            bail!("project MIDI payload exceeds aggregate limit");
        }

        let mut track_ids = HashSet::with_capacity(self.tracks.len());
        for track in &self.tracks {
            if track.id == 0 {
                bail!("track id must be non-zero");
            }
            if !track_ids.insert(track.id) {
                bail!("duplicate track id {}", track.id);
            }
            if track.plugin_types.len() != track.plugin_states.len() {
                bail!("track {} plugin type/state counts differ", track.id);
            }
            if track.plugin_bypasses.len() != track.plugin_types.len() {
                bail!("track {} plugin bypass/type counts differ", track.id);
            }
            if track.plugin_parameter_values.len() != track.plugin_types.len() {
                bail!("track {} plugin parameter/type counts differ", track.id);
            }
            if track.plugin_gui_states.len() != track.plugin_types.len() {
                bail!("track {} plugin GUI state/type counts differ", track.id);
            }
            if track.plugin_gui_states.iter().any(|state| state.len() > 1024 * 1024) {
                bail!("track {} contains oversized plugin GUI state", track.id);
            }
            if track.plugin_parameter_values.iter().flatten().any(|value| {
                !value.is_finite() || !(0.0..=1.0).contains(value)
            }) {
                bail!("track {} contains invalid plugin parameter values", track.id);
            }
            if track.plugin_state_versions.len() != track.plugin_states.len() {
                bail!("track {} plugin state version counts differ", track.id);
            }
            if track
                .plugin_state_versions
                .iter()
                .any(|version| *version != PLUGIN_STATE_SCHEMA_VERSION)
            {
                bail!(
                    "track {} contains an unsupported plugin state version",
                    track.id
                );
            }
            if track
                .plugin_states
                .iter()
                .any(|state| state.len() > 4 * 1024 * 1024)
            {
                bail!("track {} contains oversized plugin state", track.id);
            }
            if track.sandbox_plugin_paths.len() != track.sandbox_plugin_states.len() {
                bail!("track {} sandbox path/state counts differ", track.id);
            }
            if track.sandbox_plugin_state_versions.len() != track.sandbox_plugin_states.len() {
                bail!(
                    "track {} sandbox plugin state version counts differ",
                    track.id
                );
            }
            if track
                .sandbox_plugin_state_versions
                .iter()
                .any(|version| *version != PLUGIN_STATE_SCHEMA_VERSION)
            {
                bail!(
                    "track {} contains an unsupported sandbox plugin state version",
                    track.id
                );
            }
            let sandbox_slots = track
                .plugin_types
                .iter()
                .filter(|plugin_type| **plugin_type == u32::MAX)
                .count();
            if sandbox_slots != track.sandbox_plugin_paths.len() {
                bail!(
                    "track {} sandbox plugin metadata does not match plugin types",
                    track.id
                );
            }
            if track
                .sandbox_plugin_states
                .iter()
                .any(|state| state.len() > 4 * 1024 * 1024)
            {
                bail!("track {} contains oversized sandbox plugin state", track.id);
            }
            if track
                .sandbox_plugin_paths
                .iter()
                .any(|path| path.trim().is_empty() || path.contains('\0'))
            {
                bail!("track {} contains an invalid sandbox plugin path", track.id);
            }
            if track.name.trim().is_empty()
                || track.name.contains('\0')
                || !track.volume.is_finite()
                || !(0.0..=2.0).contains(&track.volume)
                || !track.pan.is_finite()
                || !(-1.0..=1.0).contains(&track.pan)
                || track.track_delay_samples > 8192
            {
                bail!("track {} contains invalid data", track.id);
            }
            Self::native_track_type_code(&track.track_type)?;
        }

        let mut stack_ids = HashSet::with_capacity(self.track_stacks.len());
        for stack in &self.track_stacks {
            if stack.id == 0 || !stack_ids.insert(stack.id) || stack.name.trim().is_empty()
                || stack.name.len() > 256 || !stack.master_gain.is_finite()
                || !(0.0..=2.0).contains(&stack.master_gain)
            {
                bail!("track stack metadata is invalid");
            }
            let mut members = HashSet::with_capacity(stack.member_track_ids.len());
            if stack.member_track_ids.iter().any(|track_id| {
                *track_id == 0 || !track_ids.contains(track_id) || !members.insert(*track_id)
            }) {
                bail!("track stack {} contains invalid or duplicate members", stack.id);
            }
        }
        if self.markers.len() > 65_536 {
            bail!("too many arrangement markers");
        }
        let mut marker_ids = HashSet::with_capacity(self.markers.len());
        for marker in &self.markers {
            marker.validate()?;
            if !marker_ids.insert(marker.id) {
                bail!("duplicate arrangement marker id {}", marker.id);
            }
        }
        if self.vca_groups.len() > 2048 {
            bail!("too many VCA groups");
        }
        let mut aux_ids = HashSet::with_capacity(self.aux_track_ids.len());
        for aux_id in &self.aux_track_ids {
            if *aux_id == 0 || !aux_ids.insert(*aux_id) || !track_ids.contains(aux_id) {
                bail!("Aux track metadata references an invalid or duplicate track");
            }
            let track = self.tracks.iter().find(|track| track.id == *aux_id).expect("track id checked");
            if !matches!(track.track_type.as_str(), "Aux" | "Bus") {
                bail!("Aux metadata references a non-bus track {}", aux_id);
            }
        }
        let mut vca_ids = HashSet::with_capacity(self.vca_groups.len());
        for group in &self.vca_groups {
            group.validate()?;
            if !vca_ids.insert(group.id) || group.track_ids.iter().any(|id| !track_ids.contains(id)) {
                bail!("VCA group {} contains an invalid or duplicate track", group.id);
            }
        }

        // Region IDs cross the FFI boundary without a track namespace. Keep
        // them globally unique so a reload cannot bind a command to the
        // wrong region when tracks are reordered or hydrated incrementally.
        let mut region_ids = HashSet::with_capacity(self.regions.len());
        for region in &self.regions {
            if region.id == 0 {
                bail!("region id must be non-zero");
            }
            if !track_ids.contains(&region.track_id) {
                bail!("region {} references an unknown track", region.id);
            }
            if region.path.trim().is_empty() || region.path.contains('\0') || region.length == 0 {
                bail!("region {} has invalid media data", region.id);
            }
            if region.start > u64::MAX - region.length {
                bail!("region {} exceeds the project timeline", region.id);
            }
            if region.base_length == 0 {
                if region.source_offset != 0 || region.base_source_offset != 0 {
                    bail!("region {} has source trim without a base length", region.id);
                }
            } else if region.source_offset < region.base_source_offset
                || region
                    .source_offset
                    .checked_sub(region.base_source_offset)
                    .and_then(|offset| offset.checked_add(region.length))
                    .is_none_or(|end| end > region.base_length)
            {
                bail!("region {} source trim exceeds its source bounds", region.id);
            }
            if region.fade_in_samples > region.length
                || region.fade_out_samples > region.length
                || !region.clip_gain.is_finite()
                || !(0.0..=2.0).contains(&region.clip_gain)
                || !region.warp_ratio.is_finite()
                || !(0.5..=2.0).contains(&region.warp_ratio)
                || !region.pitch_semitones.is_finite()
                || !(-24.0..=24.0).contains(&region.pitch_semitones)
                || !(1..=1024).contains(&region.loop_count)
            {
                bail!("region {} contains invalid envelope data", region.id);
            }
            if !region_ids.insert(region.id) {
                bail!("duplicate region id {}", region.id,);
            }
        }
        let mut plugin_slots = HashSet::with_capacity(self.plugin_instances.len());
        for plugin in &self.plugin_instances {
            if plugin.track_id != 0 {
                let Some(track) = self.tracks.iter().find(|track| track.id == plugin.track_id) else {
                    bail!(
                        "plugin instance {} references unknown track {}",
                        plugin.instance_id,
                        plugin.track_id
                    );
                };
                if plugin.slot_index as usize >= track.plugin_types.len() {
                    bail!(
                        "plugin instance {} references missing slot {} on track {}",
                        plugin.instance_id,
                        plugin.slot_index,
                        plugin.track_id
                    );
                }
                if !plugin_slots.insert((plugin.track_id, plugin.slot_index)) {
                    bail!(
                        "duplicate plugin slot {} on track {}",
                        plugin.slot_index,
                        plugin.track_id
                    );
                }
            }
        }
        let automation_points = self.tracks.iter().try_fold(0usize, |total, track| {
            let count = track.volume_automation.len()
                .checked_add(track.pan_automation.len())
                .and_then(|count| count.checked_add(track.track_delay_automation.len()))
                .ok_or_else(|| anyhow::anyhow!("automation point count overflow"))?;
            total.checked_add(count).ok_or_else(|| anyhow::anyhow!("automation point count overflow"))
        })?;
        if automation_points > MAX_PROJECT_REGIONS {
            bail!("project contains too many automation points");
        }
        for track in &self.tracks {
            for points in [&track.volume_automation, &track.pan_automation, &track.track_delay_automation] {
                let mut previous = None;
                for point in points {
                    point.validate()?;
                    if previous.is_some_and(|time| point.time <= time) {
                        bail!("track {} automation points are not strictly ordered", track.id);
                    }
                    previous = Some(point.time);
                }
            }
        }
        if self.midi_notes.len() > MAX_PROJECT_REGIONS {
            bail!("project contains too many MIDI notes");
        }
        for note in &self.midi_notes {
            note.validate()?;
            if note.probability > 100 || note.repeat_count == 0 {
                bail!("MIDI note probability/repeat attributes are invalid");
            }
            if note.lyric.len() > 1_024 || note.lyric.contains('\0') {
                bail!("MIDI note lyric must be at most 1024 bytes and contain no NUL");
            }
            if !track_ids.contains(&note.track_id) {
                bail!("MIDI note references unknown track {}", note.track_id);
            }
        }
        if self.chord_track.len() > MAX_PROJECT_REGIONS {
            bail!("project contains too many chord events");
        }
        let mut previous_chord_tick = None;
        for chord in &self.chord_track {
            if previous_chord_tick.is_some_and(|tick| chord.tick < tick)
                || chord.root > 127
                || chord.intervals.len() > 32
                || chord.intervals.iter().any(|interval| *interval > 127)
                || chord.name.trim().is_empty()
                || chord.name.chars().count() > 128
                || chord.name.contains('\0')
            {
                bail!("invalid chord-track event");
            }
            previous_chord_tick = Some(chord.tick);
        }
        if self.tempo_events.len() > 65_536 {
            bail!("project contains too many tempo events");
        }
        let mut previous_beat = None;
        for event in &self.tempo_events {
            event.validate()?;
            if previous_beat.is_some_and(|beat| event.beat <= beat) {
                bail!("tempo events are not strictly ordered");
            }
            previous_beat = Some(event.beat);
        }
        if self.time_signature_events.len() > 65_536 {
            bail!("project contains too many time signature events");
        }
        let mut previous_signature_beat = None;
        for event in &self.time_signature_events {
            event.validate()?;
            if previous_signature_beat.is_some_and(|beat| event.beat <= beat) {
                bail!("time signature events are not strictly ordered");
            }
            previous_signature_beat = Some(event.beat);
        }
        validate_contracts(
            &self.plugin_instances,
            &self.midi_learn_mappings,
            &self.macro_mappings,
            &self.warp_markers,
            &self.render_targets,
        )?;
        if self.hardware_inserts.len() > 256 {
            bail!("project contains too many hardware inserts");
        }
        for insert in &self.hardware_inserts {
            insert.validate().map_err(|error| anyhow::anyhow!(error))?;
        }
        Ok(())
    }

}
