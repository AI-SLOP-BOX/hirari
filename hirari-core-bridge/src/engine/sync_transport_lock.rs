#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncLockState {
    Idle,
    Acquiring,
    Locked,
    DroppedOut,
    Inhibited,
}

#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct TimecodeLockPreferences {
    pub lock_frames: u16,
    pub drop_out_frames: u16,
    pub inhibit_restart_ms: u32,
    pub auto_detect_frame_rate: bool,
    pub project_fps: u8,
}

impl Default for TimecodeLockPreferences {
    fn default() -> Self {
        Self {
            lock_frames: 16,
            drop_out_frames: 10,
            inhibit_restart_ms: 500,
            auto_detect_frame_rate: false,
            project_fps: 30,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TimecodeLockEngine {
    pub preferences: TimecodeLockPreferences,
    pub state: SyncLockState,
    pub detected_fps: Option<u8>,
    pub current: Option<Timecode>,
    consecutive_frames: u16,
    missing_frames: u16,
    last_frame_index: Option<u64>,
    inhibit_until_ms: u64,
}

impl TimecodeLockEngine {
    pub fn new(preferences: TimecodeLockPreferences) -> Result<Self, &'static str> {
        if !preferences.validate() {
            return Err("invalid timecode lock preferences");
        }
        Ok(Self {
            preferences,
            state: SyncLockState::Idle,
            detected_fps: None,
            current: None,
            consecutive_frames: 0,
            missing_frames: 0,
            last_frame_index: None,
            inhibit_until_ms: 0,
        })
    }

    /// Feed one complete MTC frame. Lock is acquired only after the configured
    /// number of consecutive frames; discontinuities restart acquisition.
    pub fn ingest(
        &mut self,
        timecode: Timecode,
        now_ms: u64,
    ) -> Result<SyncLockState, &'static str> {
        timecode.validate()?;
        if now_ms < self.inhibit_until_ms {
            self.state = SyncLockState::Inhibited;
            return Ok(self.state);
        }
        if timecode.fps != self.preferences.project_fps {
            if self.preferences.auto_detect_frame_rate {
                self.detected_fps = Some(timecode.fps);
            } else {
                self.state = SyncLockState::Idle;
                return Err("incoming timecode frame rate mismatch");
            }
        } else {
            self.detected_fps = Some(timecode.fps);
        }
        let index = timecode.frame_index();
        let consecutive = self
            .last_frame_index
            .is_none_or(|previous| index == previous.saturating_add(1));
        if consecutive {
            self.consecutive_frames = self.consecutive_frames.saturating_add(1);
        } else {
            self.consecutive_frames = 1;
        }
        self.last_frame_index = Some(index);
        self.current = Some(timecode);
        self.missing_frames = 0;
        self.state = if self.consecutive_frames >= self.preferences.lock_frames {
            SyncLockState::Locked
        } else {
            SyncLockState::Acquiring
        };
        Ok(self.state)
    }

    /// Called by the host when expected frames did not arrive. Crossing the
    /// dropout threshold stops transport and inhibits immediate relocking.
    pub fn report_missing_frames(&mut self, frames: u16, now_ms: u64) -> SyncLockState {
        if self.state == SyncLockState::Idle {
            return self.state;
        }
        self.missing_frames = self.missing_frames.saturating_add(frames);
        if self.missing_frames > self.preferences.drop_out_frames {
            self.state = SyncLockState::DroppedOut;
            self.consecutive_frames = 0;
            self.last_frame_index = None;
            self.inhibit_until_ms =
                now_ms.saturating_add(u64::from(self.preferences.inhibit_restart_ms));
        }
        self.state
    }

    pub fn position_samples(&self, sample_rate: f64) -> Option<u64> {
        if !sample_rate.is_finite() || sample_rate <= 0.0 {
            return None;
        }
        let timecode = self.current?;
        let samples = timecode.frame_index() as f64 * sample_rate / f64::from(timecode.fps);
        (samples.is_finite() && samples <= u64::MAX as f64).then_some(samples.round() as u64)
    }

    pub fn audit(&self) -> bool {
        self.preferences.validate()
            && self.detected_fps.is_none_or(valid_sync_fps)
            && self
                .current
                .is_none_or(|timecode| timecode.validate().is_ok())
    }
}

impl TimecodeLockPreferences {
    fn validate(self) -> bool {
        self.lock_frames > 0
            && self.lock_frames <= 10_000
            && self.drop_out_frames > 0
            && self.drop_out_frames <= 10_000
            && self.inhibit_restart_ms <= 60_000
            && valid_sync_fps(self.project_fps)
    }
}

fn valid_sync_fps(fps: u8) -> bool {
    matches!(fps, 24 | 25 | 29 | 30)
}

#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct MidiClockPreferences {
    pub follows_project_position: bool,
    pub always_send_start: bool,
    pub send_clock_in_stop: bool,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct MidiClockDestination {
    pub port_id: u32,
    pub enabled: bool,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct SyncProjectSettings {
    pub source: SyncSource,
    pub protocol: SyncProtocol,
    pub enabled: bool,
    pub timecode: TimecodeLockPreferences,
    pub midi_clock: MidiClockPreferences,
    pub midi_destinations: Vec<MidiClockDestination>,
    pub mmc_device_id: u8,
}

impl SyncProjectSettings {
    pub fn validate(&self) -> bool {
        self.source.validate()
            && self.timecode.validate()
            && self.mmc_device_id <= 0x7f
            && self.midi_destinations.len() <= 256
            && self
                .midi_destinations
                .iter()
                .all(|destination| destination.port_id != 0)
            && self
                .midi_destinations
                .iter()
                .enumerate()
                .all(|(index, destination)| {
                    self.midi_destinations[..index]
                        .iter()
                        .all(|previous| previous.port_id != destination.port_id)
                })
            && (!self.enabled
                || self.source != SyncSource::Internal
                || self.protocol == SyncProtocol::MidiClock)
    }

    pub fn to_json(&self) -> Result<String, String> {
        if !self.validate() {
            return Err("invalid synchronization settings".into());
        }
        serde_json::to_string(self).map_err(|error| error.to_string())
    }

    pub fn from_json(json: &str) -> Result<Self, String> {
        let value: Self = serde_json::from_str(json).map_err(|error| error.to_string())?;
        if value.validate() {
            Ok(value)
        } else {
            Err("invalid synchronization settings".into())
        }
    }

    pub fn create_midi_clock_master(&self) -> Option<MidiClockMaster> {
        (self.validate() && self.protocol == SyncProtocol::MidiClock).then(|| MidiClockMaster {
            preferences: self.midi_clock,
            destinations: self.midi_destinations.clone(),
            running: false,
            beat: 0.0,
        })
    }

    pub fn create_timecode_lock(&self) -> Result<TimecodeLockEngine, &'static str> {
        if !self.validate() || !matches!(self.protocol, SyncProtocol::Mtc | SyncProtocol::Ltc) {
            return Err("settings do not select timecode synchronization");
        }
        TimecodeLockEngine::new(self.timecode)
    }
}
