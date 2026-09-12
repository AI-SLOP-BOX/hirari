impl ControlRoomConsole {
    pub fn to_json(&self) -> Result<String, String> {
        if !self.audit() {
            return Err("invalid Control Room state".into());
        }
        serde_json::to_string(self).map_err(|error| error.to_string())
    }

    pub fn from_json(json: &str) -> Result<Self, String> {
        let value: Self = serde_json::from_str(json).map_err(|error| error.to_string())?;
        if value.audit() {
            Ok(value)
        } else {
            Err("invalid Control Room state".into())
        }
    }

    pub fn select_source(&mut self, id: u32) -> bool {
        if !self.sources.iter().any(|source| source.id == id) {
            return false;
        }
        self.active_source = id;
        true
    }

    pub fn select_monitor(&mut self, id: u32) -> bool {
        if !self.monitors.iter().any(|monitor| monitor.id == id) {
            return false;
        }
        self.active_monitor = id;
        true
    }

    pub fn upsert_cue(&mut self, mut cue: CueMix) -> bool {
        cue.name = cue.name.trim().to_owned();
        if !cue.validate() {
            return false;
        }
        if let Some(existing) = self.cues.iter_mut().find(|item| item.id == cue.id) {
            *existing = cue;
        } else if self.cues.len() < 4 {
            self.cues.push(cue);
        } else {
            return false;
        }
        self.cues.sort_by_key(|item| item.id);
        true
    }

    pub fn upsert_downmix(&mut self, mut preset: DownmixPreset) -> bool {
        preset.name = preset.name.trim().to_owned();
        if !preset.validate()
            || !self.monitors.iter().any(|monitor| {
                monitor.id == preset.monitor_id
                    && monitor.channels as usize == preset.output.channels()
            })
        {
            return false;
        }
        if let Some(existing) = self
            .downmix_presets
            .iter_mut()
            .find(|item| item.id == preset.id)
        {
            *existing = preset;
        } else if self.downmix_presets.len() < 32 {
            self.downmix_presets.push(preset);
        } else {
            return false;
        }
        self.downmix_presets.sort_by_key(|item| item.id);
        true
    }

    pub fn select_downmix(&mut self, id: Option<u32>) -> bool {
        if let Some(id) = id {
            let Some(preset) = self.downmix_presets.iter().find(|item| item.id == id) else {
                return false;
            };
            if preset.monitor_id != self.active_monitor {
                return false;
            }
        }
        self.active_downmix = id;
        true
    }

    pub fn set_phones(&mut self, phones: Option<PhonesChannel>) -> bool {
        if phones
            .as_ref()
            .is_some_and(|channel| !channel.validate(self))
        {
            return false;
        }
        self.phones = phones;
        true
    }

    pub fn render_downmix(&self, interleaved: &[f32]) -> Result<Vec<f32>, String> {
        let Some(id) = self.active_downmix else {
            return Ok(interleaved.to_vec());
        };
        let preset = self
            .downmix_presets
            .iter()
            .find(|item| item.id == id)
            .ok_or("active downmix preset is missing")?;
        let inputs = preset.source_channels as usize;
        if !interleaved.len().is_multiple_of(inputs)
            || interleaved.iter().any(|sample| !sample.is_finite())
        {
            return Err("invalid interleaved source audio".into());
        }
        let outputs = preset.output.channels();
        let mut rendered = Vec::with_capacity(interleaved.len() / inputs * outputs);
        for frame in interleaved.chunks_exact(inputs) {
            for row in preset.coefficients.chunks_exact(inputs) {
                rendered.push(
                    frame
                        .iter()
                        .zip(row)
                        .map(|(sample, gain)| sample * gain)
                        .sum(),
                );
            }
        }
        Ok(rendered)
    }

    pub fn set_transport(&mut self, activity: TransportActivity) {
        self.transport = activity;
        if self.talkback_forbidden() {
            self.talkback = false;
            self.talkback_momentary = false;
        }
    }

    pub fn set_talkback(&mut self, enabled: bool, momentary: bool) -> bool {
        if enabled && self.talkback_forbidden() {
            return false;
        }
        self.talkback = enabled;
        self.talkback_momentary = enabled && momentary;
        true
    }

    /// Applies Control Room monitor level, DIM and talkback to a stereo block.
    /// Talkback is mixed after DIM, matching a dedicated monitor path rather
    /// than altering the project master bus.
    pub fn process_monitor_stereo(
        &self,
        main_l: &[f32],
        main_r: &[f32],
        talkback_l: Option<&[f32]>,
        talkback_r: Option<&[f32]>,
    ) -> Option<(Vec<f32>, Vec<f32>)> {
        if !self.audit()
            || main_l.len() != main_r.len()
            || main_l.len() > 16_000_000
            || main_l
                .iter()
                .chain(main_r)
                .any(|sample| !sample.is_finite())
        {
            return None;
        }
        let talkback = match (talkback_l, talkback_r) {
            (Some(left), Some(right))
                if left.len() == main_l.len()
                    && right.len() == main_r.len()
                    && left.iter().chain(right).all(|sample| sample.is_finite()) =>
            {
                Some((left, right))
            }
            (None, None) => None,
            _ => return None,
        };
        let main_gain = 10.0f32.powf((self.effective_main_level_db() / 20.0).clamp(-120.0, 24.0));
        let talkback_gain = 10.0f32.powf((self.talkback_gain_db / 20.0).clamp(-120.0, 24.0));
        let mut left = Vec::with_capacity(main_l.len());
        let mut right = Vec::with_capacity(main_r.len());
        for index in 0..main_l.len() {
            let mut l = main_l[index] * main_gain;
            let mut r = main_r[index] * main_gain;
            if let Some((talk_l, talk_r)) =
                talkback.filter(|_| self.talkback || self.talkback_momentary)
            {
                l += talk_l[index] * talkback_gain;
                r += talk_r[index] * talkback_gain;
            }
            left.push(l.clamp(-16.0, 16.0));
            right.push(r.clamp(-16.0, 16.0));
        }
        Some((left, right))
    }

    pub fn release_momentary_talkback(&mut self) -> bool {
        if !self.talkback_momentary {
            return false;
        }
        self.talkback = false;
        self.talkback_momentary = false;
        true
    }

    pub fn set_listen(&mut self, channel_id: u32, enabled: bool) -> bool {
        if channel_id == 0 {
            return false;
        }
        if enabled {
            if !self.listen_channels.contains(&channel_id) {
                self.listen_channels.push(channel_id);
                self.listen_channels.sort_unstable();
            }
        } else {
            let before = self.listen_channels.len();
            self.listen_channels.retain(|id| *id != channel_id);
            if before == self.listen_channels.len() {
                return false;
            }
        }
        true
    }

    pub fn effective_main_level_db(&self) -> f32 {
        if !self.enabled {
            return -120.0;
        }
        let mut level = if self.reference_level_active {
            self.reference_level_db
        } else {
            self.control_level_db
        };
        if self.dim {
            level += self.main_dim_db;
        }
        if self.talkback {
            level += self.talkback_dim_db;
        }
        if !self.listen_channels.is_empty() {
            level += self.listen_dim_db;
        }
        level.clamp(-120.0, 24.0)
    }

    pub fn effective_listen_level_db(&self) -> Option<f32> {
        (!self.listen_channels.is_empty()).then_some(
            (self.effective_main_level_db() - self.listen_dim_db + self.listen_level_db)
                .clamp(-120.0, 24.0),
        )
    }

    pub fn effective_cue_level_db(&self, cue_id: u32) -> Option<f32> {
        let cue = self
            .cues
            .iter()
            .find(|cue| cue.id == cue_id && cue.enabled)?;
        let dim = if self.talkback && cue.dim_during_talkback {
            self.talkback_dim_db
        } else {
            0.0
        };
        Some((cue.gain_db + dim).clamp(-120.0, 24.0))
    }

    pub fn effective_talkback_send_db(&self, cue_id: u32) -> Option<f32> {
        if !self.talkback {
            return None;
        }
        let cue = self
            .cues
            .iter()
            .find(|cue| cue.id == cue_id && cue.enabled && cue.talkback_enabled)?;
        Some((self.talkback_gain_db + cue.talkback_send_db).clamp(-120.0, 24.0))
    }

    /// Equal-power stereo click gains for a cue channel.
    pub fn cue_click_gains(&self, cue_id: u32) -> Option<[f32; 2]> {
        let cue = self
            .cues
            .iter()
            .find(|cue| cue.id == cue_id && cue.enabled && cue.click_enabled)?;
        let level = 10.0f32.powf(cue.click_level_db / 20.0);
        let angle = (cue.click_pan + 1.0) * std::f32::consts::FRAC_PI_4;
        Some([level * angle.cos(), level * angle.sin()])
    }

    fn talkback_forbidden(&self) -> bool {
        matches!(
            (self.auto_disable_talkback, self.transport),
            (AutoDisableTalkback::Recording, TransportActivity::Recording)
                | (
                    AutoDisableTalkback::PlaybackAndRecording,
                    TransportActivity::Playback | TransportActivity::Recording
                )
        )
    }

    pub fn audit(&self) -> bool {
        if self.sources.is_empty()
            || self.sources.len() > 32
            || self.monitors.is_empty()
            || self.monitors.len() > 4
            || self.cues.len() > 4
            || self.downmix_presets.len() > 32
            || !valid_db(self.control_level_db)
            || !valid_db(self.reference_level_db)
            || !valid_reduction(self.main_dim_db)
            || !valid_db(self.talkback_gain_db)
            || !valid_reduction(self.talkback_dim_db)
            || !valid_db(self.listen_level_db)
            || !valid_reduction(self.listen_dim_db)
        {
            return false;
        }
        let valid_named = |id: u32, name: &str| {
            id != 0 && !name.trim().is_empty() && name.len() <= 128 && !name.contains('\0')
        };
        if !self.sources.iter().all(|source| {
            valid_named(source.id, &source.name) && (1..=16).contains(&source.channels)
        }) || !self.monitors.iter().all(|monitor| {
            valid_named(monitor.id, &monitor.name)
                && (1..=16).contains(&monitor.channels)
                && monitor.device_ports.len() == monitor.channels as usize
                && monitor
                    .device_ports
                    .iter()
                    .all(|port| !port.trim().is_empty() && !port.contains('\0'))
        }) || !self.cues.iter().all(|cue| {
            cue.validate()
                && match cue.source {
                    CueSource::Mix | CueSource::CueSends => true,
                    CueSource::External(id) => self.sources.iter().any(|source| source.id == id),
                }
        }) || !self.downmix_presets.iter().all(|preset| {
            preset.validate()
                && self.monitors.iter().any(|monitor| {
                    monitor.id == preset.monitor_id
                        && monitor.channels as usize == preset.output.channels()
                })
        }) || self
            .phones
            .as_ref()
            .is_some_and(|phones| !phones.validate(self))
        {
            return false;
        }
        let unique = |ids: Vec<u32>| {
            let mut copy = ids;
            copy.sort_unstable();
            copy.windows(2).all(|pair| pair[0] < pair[1])
        };
        if !unique(self.sources.iter().map(|item| item.id).collect())
            || !unique(self.monitors.iter().map(|item| item.id).collect())
            || !unique(self.cues.iter().map(|item| item.id).collect())
            || !unique(self.downmix_presets.iter().map(|item| item.id).collect())
            || !self
                .sources
                .iter()
                .any(|item| item.id == self.active_source)
            || !self
                .monitors
                .iter()
                .any(|item| item.id == self.active_monitor)
            || self.talkback_forbidden() && self.talkback
            || self
                .listen_channels
                .windows(2)
                .any(|pair| pair[0] >= pair[1])
            || self.active_downmix.is_some_and(|id| {
                !self
                    .downmix_presets
                    .iter()
                    .any(|preset| preset.id == id && preset.monitor_id == self.active_monitor)
            })
        {
            return false;
        }
        if self.exclusive_monitor_ports {
            let mut ports = std::collections::BTreeSet::new();
            if !self
                .monitors
                .iter()
                .flat_map(|monitor| &monitor.device_ports)
                .all(|port| ports.insert(port.to_ascii_lowercase()))
            {
                return false;
            }
        }
        true
    }
}
