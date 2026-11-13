use serde::{Deserialize, Serialize};
use std::collections::HashSet;

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum LoudnessStandard {
    EbuR128,
    AtscA85,
    Streaming,
}
impl LoudnessStandard {
    pub fn target_lufs(self) -> f32 {
        match self {
            Self::EbuR128 => -23.0,
            Self::AtscA85 => -24.0,
            Self::Streaming => -14.0,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct BroadcastMetadata {
    pub timecode_start: String,
    pub reel_name: String,
    pub country: String,
    pub adm_profile: Option<String>,
}
impl BroadcastMetadata {
    pub fn validate(&self) -> bool {
        !self.timecode_start.trim().is_empty()
            && self.timecode_start.len() <= 32
            && !self.timecode_start.contains('\0')
            && !self.reel_name.trim().is_empty()
            && self.reel_name.len() <= 128
            && !self.reel_name.contains('\0')
            && self.country.len() <= 8
            && !self.country.contains('\0')
            && self
                .adm_profile
                .as_ref()
                .is_none_or(|v| !v.trim().is_empty() && v.len() <= 128 && !v.contains('\0'))
    }
}
impl BroadcastMetadata {
    pub fn parse_timecode_start(&self) -> Option<(u8, u8, u8, u8)> {
        if !self.validate() {
            return None;
        }
        let mut parts = self.timecode_start.split(':').map(|p| p.parse::<u8>().ok());
        let value = (
            parts.next()??,
            parts.next()??,
            parts.next()??,
            parts.next()??,
        );
        (value.0 < 24 && value.1 < 60 && value.2 < 60 && value.3 < 60 && parts.next().is_none())
            .then_some(value)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct AdmObject {
    pub id: u32,
    pub name: String,
    pub channel: u16,
    pub gain_db: f32,
    pub position: [f32; 3],
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct AdmMetadata {
    pub profile: String,
    pub beds: Vec<String>,
    pub objects: Vec<AdmObject>,
}

impl AdmMetadata {
    pub fn validate(&self) -> bool {
        !self.profile.trim().is_empty()
            && self.profile.len() <= 128
            && !self.profile.contains('\0')
            && self.beds.len() <= 64
            && self
                .beds
                .iter()
                .all(|bed| !bed.trim().is_empty() && bed.len() <= 128 && !bed.contains('\0'))
            && self.objects.len() <= 1024
            && self.objects.iter().enumerate().all(|(index, object)| {
                object.id != 0
                    && object.channel > 0
                    && object.channel <= 256
                    && !object.name.trim().is_empty()
                    && object.name.len() <= 128
                    && !object.name.contains('\0')
                    && object.gain_db.is_finite()
                    && (-120.0..=24.0).contains(&object.gain_db)
                    && object
                        .position
                        .iter()
                        .all(|value| value.is_finite() && (-1.0..=1.0).contains(value))
                    && self.objects[..index]
                        .iter()
                        .all(|previous| previous.id != object.id)
            })
    }

    /// Emits a bounded ADM-like XML sidecar suitable for an ADM/BWF writer.
    /// Binary BWF chunk insertion remains a renderer concern, but the object
    /// graph and metadata are fully deterministic and independently verifiable.
    pub fn to_xml(&self) -> Option<String> {
        if !self.validate() {
            return None;
        }
        let escape = |value: &str| {
            value
                .replace('&', "&amp;")
                .replace('<', "&lt;")
                .replace('>', "&gt;")
                .replace('"', "&quot;")
        };
        let mut xml = format!("<adm profile=\"{}\"><beds>", escape(self.profile.trim()));
        for bed in &self.beds {
            xml.push_str(&format!("<bed name=\"{}\"/>", escape(bed.trim())));
        }
        xml.push_str("</beds><objects>");
        for object in &self.objects {
            xml.push_str(&format!("<object id=\"{}\" name=\"{}\" channel=\"{}\" gain-db=\"{:.4}\" x=\"{:.4}\" y=\"{:.4}\" z=\"{:.4}\"/>", object.id, escape(object.name.trim()), object.channel, object.gain_db, object.position[0], object.position[1], object.position[2]));
        }
        xml.push_str("</objects></adm>");
        (xml.len() <= 4 * 1024 * 1024).then_some(xml)
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum DeliveryFormat {
    Wav,
    Flac,
    Mp3,
    Aac,
    DdpImage,
}
impl DeliveryFormat {
    pub fn extension(self) -> &'static str {
        match self {
            Self::Wav => "wav",
            Self::Flac => "flac",
            Self::Mp3 => "mp3",
            Self::Aac => "m4a",
            Self::DdpImage => "ddp",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct DeliveryTarget {
    pub name: String,
    pub format: DeliveryFormat,
    pub sample_rate: u32,
    pub bit_depth: u16,
    pub loudness_lufs: Option<f32>,
}
impl DeliveryTarget {
    pub fn normalized_filename(&self) -> Option<String> {
        if !self.validate() {
            return None;
        }
        let stem = self
            .name
            .rsplit_once('.')
            .map(|(s, _)| s)
            .unwrap_or(&self.name)
            .trim();
        if stem.is_empty() || stem.contains('/') || stem.contains('\\') {
            return None;
        }
        Some(format!("{}.{}", stem, self.format.extension()))
    }
    pub fn validate(&self) -> bool {
        !self.name.trim().is_empty()
            && self.name.len() <= 128
            && !self.name.contains('\0')
            && (8_000..=384_000).contains(&self.sample_rate)
            && matches!(self.bit_depth, 16 | 24 | 32)
            && self
                .loudness_lufs
                .is_none_or(|v| v.is_finite() && (-60.0..=0.0).contains(&v))
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct DeliveryManifest {
    pub album_title: String,
    pub artist: String,
    pub catalog_number: String,
    pub targets: Vec<DeliveryTarget>,
}
impl DeliveryManifest {
    pub fn target(&self, name: &str) -> Option<&DeliveryTarget> {
        self.targets
            .iter()
            .find(|t| t.name.eq_ignore_ascii_case(name.trim()))
    }
    pub fn validate(&self) -> bool {
        let fields_ok = [&self.album_title, &self.artist, &self.catalog_number]
            .into_iter()
            .all(|v| !v.trim().is_empty() && v.len() <= 256 && !v.contains('\0'));
        let mut names = HashSet::new();
        fields_ok
            && !self.targets.is_empty()
            && self.targets.len() <= 128
            && self.targets.iter().all(DeliveryTarget::validate)
            && self
                .targets
                .iter()
                .all(|t| names.insert(t.name.to_ascii_lowercase()))
    }
    pub fn formats(&self) -> Vec<DeliveryFormat> {
        let mut out: Vec<_> = self.targets.iter().map(|t| t.format).collect();
        out.sort_by_key(|f| *f as u8);
        out.dedup();
        out
    }
}
pub fn verify_delivery_outputs(manifest: &DeliveryManifest, completed_names: &[String]) -> bool {
    if !manifest.validate() || completed_names.len() != manifest.targets.len() {
        return false;
    }
    let expected: std::collections::HashSet<_> = manifest
        .targets
        .iter()
        .filter_map(DeliveryTarget::normalized_filename)
        .collect();
    let actual: std::collections::HashSet<_> = completed_names.iter().cloned().collect();
    expected.len() == manifest.targets.len() && actual == expected
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum DeliveryJobStatus {
    Pending,
    Rendering,
    Completed,
    Failed,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct DeliveryJob {
    pub id: u64,
    pub target: DeliveryTarget,
    pub status: DeliveryJobStatus,
    pub output_name: Option<String>,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct DeliveryQueue {
    pub jobs: Vec<DeliveryJob>,
    #[serde(default = "default_queue_next_id")]
    next_id: u64,
}

fn default_queue_next_id() -> u64 {
    1
}

impl DeliveryQueue {
    pub fn enqueue(&mut self, target: DeliveryTarget) -> Option<u64> {
        if !target.validate()
            || self.jobs.len() >= 4096
            || self
                .jobs
                .iter()
                .any(|job| job.target.name.eq_ignore_ascii_case(target.name.trim()))
        {
            return None;
        }
        let id = self.next_id.max(1);
        self.next_id = id.checked_add(1)?;
        self.jobs.push(DeliveryJob {
            id,
            target,
            status: DeliveryJobStatus::Pending,
            output_name: None,
            error: None,
        });
        Some(id)
    }
    pub fn begin(&mut self, id: u64) -> bool {
        let Some(job) = self
            .jobs
            .iter_mut()
            .find(|job| job.id == id && job.status == DeliveryJobStatus::Pending)
        else {
            return false;
        };
        job.status = DeliveryJobStatus::Rendering;
        true
    }
    pub fn complete(&mut self, id: u64, output_name: &str) -> bool {
        let Some(job) = self
            .jobs
            .iter_mut()
            .find(|job| job.id == id && job.status == DeliveryJobStatus::Rendering)
        else {
            return false;
        };
        let Some(expected) = job.target.normalized_filename() else {
            return false;
        };
        if output_name.trim() != expected {
            return false;
        }
        job.output_name = Some(expected);
        job.error = None;
        job.status = DeliveryJobStatus::Completed;
        true
    }
    pub fn fail(&mut self, id: u64, error: &str) -> bool {
        if error.trim().is_empty() || error.len() > 1024 || error.contains('\0') {
            return false;
        }
        let Some(job) = self
            .jobs
            .iter_mut()
            .find(|job| job.id == id && job.status == DeliveryJobStatus::Rendering)
        else {
            return false;
        };
        job.error = Some(error.trim().to_owned());
        job.status = DeliveryJobStatus::Failed;
        true
    }
    pub fn retry_failed(&mut self, id: u64) -> bool {
        let Some(job) = self
            .jobs
            .iter_mut()
            .find(|job| job.id == id && job.status == DeliveryJobStatus::Failed)
        else {
            return false;
        };
        job.error = None;
        job.status = DeliveryJobStatus::Pending;
        true
    }
    pub fn is_complete(&self) -> bool {
        !self.jobs.is_empty()
            && self
                .jobs
                .iter()
                .all(|job| job.status == DeliveryJobStatus::Completed)
    }
    pub fn validate(&self) -> bool {
        self.jobs.len() <= 4096
            && self.next_id > 0
            && self.jobs.iter().enumerate().all(|(index, job)| {
                job.id > 0
                    && job.id < self.next_id
                    && job.target.validate()
                    && self.jobs[..index].iter().all(|previous| {
                        previous.id != job.id
                            && !previous.target.name.eq_ignore_ascii_case(&job.target.name)
                    })
                    && match job.status {
                        DeliveryJobStatus::Completed => {
                            job.output_name.as_deref()
                                == job.target.normalized_filename().as_deref()
                                && job.error.is_none()
                        }
                        DeliveryJobStatus::Failed => job
                            .error
                            .as_ref()
                            .is_some_and(|error| !error.trim().is_empty() && error.len() <= 1024),
                        DeliveryJobStatus::Pending | DeliveryJobStatus::Rendering => {
                            job.output_name.is_none()
                        }
                    }
            })
    }
}
pub fn loudness_within_standard(
    measured_lufs: f32,
    standard: LoudnessStandard,
    tolerance_lufs: f32,
) -> bool {
    measured_lufs.is_finite()
        && tolerance_lufs.is_finite()
        && tolerance_lufs >= 0.0
        && (measured_lufs - standard.target_lufs()).abs() <= tolerance_lufs
}

/// Applies a bounded loudness-normalization gain to interleaved PCM.  The
/// requested LUFS correction is automatically limited by the true-peak
/// ceiling, so queue exports cannot introduce clipping while normalizing.
pub fn normalize_loudness_interleaved(
    samples: &[f32],
    measured_lufs: f32,
    target_lufs: f32,
    max_true_peak_dbtp: f32,
) -> Option<(Vec<f32>, f32)> {
    if samples.is_empty()
        || samples.len() > 64 * 1024 * 1024
        || samples.iter().any(|sample| !sample.is_finite())
        || !measured_lufs.is_finite()
        || !target_lufs.is_finite()
        || !(-70.0..=0.0).contains(&measured_lufs)
        || !(-70.0..=0.0).contains(&target_lufs)
        || !max_true_peak_dbtp.is_finite()
        || !(-20.0..=0.0).contains(&max_true_peak_dbtp)
    {
        return None;
    }
    let peak = samples
        .iter()
        .map(|sample| sample.abs())
        .fold(0.0f32, f32::max);
    let requested_db = target_lufs - measured_lufs;
    let peak_db = if peak > 0.0 {
        20.0 * peak.log10()
    } else {
        -120.0
    };
    let applied_db = requested_db
        .min(max_true_peak_dbtp - peak_db)
        .clamp(-60.0, 24.0);
    let gain = 10.0f32.powf(applied_db / 20.0);
    if !gain.is_finite() {
        return None;
    }
    let output = samples
        .iter()
        .map(|sample| *sample * gain)
        .collect::<Vec<_>>();
    (output.iter().all(|sample| sample.is_finite()) && applied_db.is_finite())
        .then_some((output, applied_db))
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct DdpTrack {
    pub number: u16,
    pub title: String,
    pub performer: String,
    pub start_sector: u64,
    pub length_sectors: u64,
    pub isrc: Option<String>,
}
impl DdpTrack {
    pub fn validate(&self) -> bool {
        self.number > 0
            && !self.title.trim().is_empty()
            && self.title.len() <= 256
            && !self.title.contains('\0')
            && self.performer.len() <= 256
            && !self.performer.contains('\0')
            && self.length_sectors > 0
            && self.start_sector.checked_add(self.length_sectors).is_some()
            && self
                .isrc
                .as_ref()
                .is_none_or(|v| v.len() <= 32 && !v.contains('\0'))
    }
    pub fn end_sector(&self) -> Option<u64> {
        self.validate()
            .then(|| self.start_sector.checked_add(self.length_sectors))
            .flatten()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct DdpImageManifest {
    pub album_title: String,
    pub catalog_number: String,
    pub tracks: Vec<DdpTrack>,
}
impl DdpImageManifest {
    pub fn validate(&self) -> bool {
        !self.album_title.trim().is_empty()
            && self.album_title.len() <= 256
            && !self.album_title.contains('\0')
            && self.catalog_number.len() <= 64
            && !self.catalog_number.contains('\0')
            && !self.tracks.is_empty()
            && self.tracks.len() <= 99
            && self.tracks.iter().enumerate().all(|(i, t)| {
                t.validate()
                    && t.number as usize == i + 1
                    && (i == 0
                        || t.start_sector
                            >= self.tracks[i - 1]
                                .start_sector
                                .saturating_add(self.tracks[i - 1].length_sectors))
            })
    }
    pub fn total_sectors(&self) -> Option<u64> {
        if !self.validate() {
            None
        } else {
            self.tracks.last().and_then(|t| t.end_sector())
        }
    }
    pub fn to_json(&self) -> Result<String, String> {
        if !self.validate() {
            return Err("invalid DDP image manifest".into());
        }
        serde_json::to_string(self).map_err(|e| e.to_string())
    }
    pub fn from_json(json: &str) -> Result<Self, String> {
        let v: Self = serde_json::from_str(json).map_err(|e| e.to_string())?;
        if !v.validate() {
            Err("invalid DDP image manifest".into())
        } else {
            Ok(v)
        }
    }
    pub fn track(&self, number: u16) -> Option<&DdpTrack> {
        self.tracks.iter().find(|track| track.number == number)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct BroadcastWaveChunk {
    pub description: String,
    pub originator: String,
    pub originator_reference: String,
    pub origination_date: String,
    pub origination_time: String,
    pub time_reference_samples: u64,
    pub coding_history: String,
}

impl BroadcastWaveChunk {
    pub fn validate(&self) -> bool {
        bounded_text(&self.description, 256, true)
            && bounded_text(&self.originator, 32, true)
            && bounded_text(&self.originator_reference, 32, true)
            && valid_date(&self.origination_date)
            && valid_time(&self.origination_time)
            && bounded_text(&self.coding_history, 4096, false)
    }

    pub fn timecode_samples(&self) -> (u32, u32) {
        (
            self.time_reference_samples as u32,
            (self.time_reference_samples >> 32) as u32,
        )
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum IxmlChannelRole {
    Left,
    Right,
    LeftMix,
    RightMix,
    Center,
    Lfe,
    SurroundLeft,
    SurroundRight,
    Unspecified,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct IxmlChunk {
    pub project: String,
    pub author: String,
    pub frame_rate: f64,
    pub tempo_bpm: Option<f64>,
    pub channel_roles: Vec<IxmlChannelRole>,
}

impl IxmlChunk {
    pub fn validate(&self) -> bool {
        bounded_text(&self.project, 256, true)
            && bounded_text(&self.author, 256, true)
            && self.frame_rate.is_finite()
            && matches!(
                self.frame_rate,
                23.976 | 24.0 | 25.0 | 29.97 | 30.0 | 50.0 | 59.94 | 60.0
            )
            && self
                .tempo_bpm
                .is_none_or(|tempo| tempo.is_finite() && (1.0..=999.0).contains(&tempo))
            && !self.channel_roles.is_empty()
            && self.channel_roles.len() <= 256
    }

    pub fn stereo(project: &str, author: &str, frame_rate: f64, dual_mono: bool) -> Self {
        let channel_roles = if dual_mono {
            vec![IxmlChannelRole::LeftMix, IxmlChannelRole::RightMix]
        } else {
            vec![IxmlChannelRole::Left, IxmlChannelRole::Right]
        };
        Self {
            project: project.into(),
            author: author.into(),
            frame_rate,
            tempo_bpm: None,
            channel_roles,
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum WaveContainer {
    Wave,
    WaveExtensible,
    Rf64,
}

pub fn select_wave_container(
    data_bytes: u64,
    channels: u16,
    speaker_metadata: bool,
) -> Option<WaveContainer> {
    if channels == 0 || channels > 256 {
        return None;
    }
    if data_bytes > u64::from(u32::MAX).saturating_sub(4096) {
        Some(WaveContainer::Rf64)
    } else if channels > 2 || speaker_metadata {
        Some(WaveContainer::WaveExtensible)
    } else {
        Some(WaveContainer::Wave)
    }
}

#[path = "delivery_loudness.rs"]
mod loudness;
#[path = "delivery_adm.rs"]
mod adm;
pub use loudness::*;
pub use adm::*;

#[cfg(test)]
#[path = "delivery_tests.rs"]
mod delivery_tests;
