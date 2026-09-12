use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashSet;

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum LoudnessStandard { EbuR128, AtscA85, Streaming }
impl LoudnessStandard { pub fn target_lufs(self) -> f32 { match self { Self::EbuR128 => -23.0, Self::AtscA85 => -24.0, Self::Streaming => -14.0 } } }

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct BroadcastMetadata { pub timecode_start: String, pub reel_name: String, pub country: String, pub adm_profile: Option<String> }
impl BroadcastMetadata { pub fn validate(&self) -> bool { !self.timecode_start.trim().is_empty() && self.timecode_start.len() <= 32 && !self.timecode_start.contains('\0') && !self.reel_name.trim().is_empty() && self.reel_name.len() <= 128 && !self.reel_name.contains('\0') && self.country.len() <= 8 && !self.country.contains('\0') && self.adm_profile.as_ref().is_none_or(|v| !v.trim().is_empty() && v.len() <= 128 && !v.contains('\0')) } }
impl BroadcastMetadata { pub fn parse_timecode_start(&self) -> Option<(u8,u8,u8,u8)> { if !self.validate() { return None; } let mut parts = self.timecode_start.split(':').map(|p| p.parse::<u8>().ok()); let value = (parts.next()??, parts.next()??, parts.next()??, parts.next()??); (value.0 < 24 && value.1 < 60 && value.2 < 60 && value.3 < 60 && parts.next().is_none()).then_some(value) } }

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct AdmObject { pub id: u32, pub name: String, pub channel: u16, pub gain_db: f32, pub position: [f32; 3] }

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct AdmMetadata { pub profile: String, pub beds: Vec<String>, pub objects: Vec<AdmObject> }

impl AdmMetadata {
    pub fn validate(&self) -> bool {
        !self.profile.trim().is_empty() && self.profile.len() <= 128 && !self.profile.contains('\0')
            && self.beds.len() <= 64 && self.beds.iter().all(|bed| !bed.trim().is_empty() && bed.len() <= 128 && !bed.contains('\0'))
            && self.objects.len() <= 1024 && self.objects.iter().enumerate().all(|(index, object)| object.id != 0 && object.channel > 0 && object.channel <= 256 && !object.name.trim().is_empty() && object.name.len() <= 128 && !object.name.contains('\0') && object.gain_db.is_finite() && (-120.0..=24.0).contains(&object.gain_db) && object.position.iter().all(|value| value.is_finite() && (-1.0..=1.0).contains(value)) && self.objects[..index].iter().all(|previous| previous.id != object.id))
    }

    /// Emits a bounded ADM-like XML sidecar suitable for an ADM/BWF writer.
    /// Binary BWF chunk insertion remains a renderer concern, but the object
    /// graph and metadata are fully deterministic and independently verifiable.
    pub fn to_xml(&self) -> Option<String> {
        if !self.validate() { return None; }
        let escape = |value: &str| value.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;");
        let mut xml = format!("<adm profile=\"{}\"><beds>", escape(self.profile.trim()));
        for bed in &self.beds { xml.push_str(&format!("<bed name=\"{}\"/>", escape(bed.trim()))); }
        xml.push_str("</beds><objects>");
        for object in &self.objects { xml.push_str(&format!("<object id=\"{}\" name=\"{}\" channel=\"{}\" gain-db=\"{:.4}\" x=\"{:.4}\" y=\"{:.4}\" z=\"{:.4}\"/>", object.id, escape(object.name.trim()), object.channel, object.gain_db, object.position[0], object.position[1], object.position[2])); }
        xml.push_str("</objects></adm>");
        (xml.len() <= 4 * 1024 * 1024).then_some(xml)
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum DeliveryFormat { Wav, Flac, Mp3, Aac, DdpImage }
impl DeliveryFormat { pub fn extension(self) -> &'static str { match self { Self::Wav => "wav", Self::Flac => "flac", Self::Mp3 => "mp3", Self::Aac => "m4a", Self::DdpImage => "ddp" } } }

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct DeliveryTarget { pub name: String, pub format: DeliveryFormat, pub sample_rate: u32, pub bit_depth: u16, pub loudness_lufs: Option<f32> }
impl DeliveryTarget { pub fn normalized_filename(&self) -> Option<String> { if !self.validate() { return None; } let stem=self.name.rsplit_once('.').map(|(s,_)|s).unwrap_or(&self.name).trim(); if stem.is_empty() || stem.contains('/') || stem.contains('\\') { return None; } Some(format!("{}.{}", stem, self.format.extension())) } pub fn validate(&self) -> bool { !self.name.trim().is_empty() && self.name.len() <= 128 && !self.name.contains('\0') && (8_000..=384_000).contains(&self.sample_rate) && matches!(self.bit_depth, 16 | 24 | 32) && self.loudness_lufs.is_none_or(|v| v.is_finite() && (-60.0..=0.0).contains(&v)) } }

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct DeliveryManifest { pub album_title: String, pub artist: String, pub catalog_number: String, pub targets: Vec<DeliveryTarget> }
impl DeliveryManifest {
    pub fn target(&self, name: &str) -> Option<&DeliveryTarget> { self.targets.iter().find(|t| t.name.eq_ignore_ascii_case(name.trim())) }
    pub fn validate(&self) -> bool {
        let fields_ok = [&self.album_title, &self.artist, &self.catalog_number].into_iter().all(|v| !v.trim().is_empty() && v.len() <= 256 && !v.contains('\0'));
        let mut names = HashSet::new();
        fields_ok && !self.targets.is_empty() && self.targets.len() <= 128 && self.targets.iter().all(DeliveryTarget::validate) && self.targets.iter().all(|t| names.insert(t.name.to_ascii_lowercase()))
    }
    pub fn formats(&self) -> Vec<DeliveryFormat> { let mut out: Vec<_> = self.targets.iter().map(|t| t.format).collect(); out.sort_by_key(|f| *f as u8); out.dedup(); out }
}
pub fn verify_delivery_outputs(manifest: &DeliveryManifest, completed_names: &[String]) -> bool {
    if !manifest.validate() || completed_names.len() != manifest.targets.len() { return false; }
    let expected: std::collections::HashSet<_> = manifest.targets.iter().filter_map(DeliveryTarget::normalized_filename).collect();
    let actual: std::collections::HashSet<_> = completed_names.iter().cloned().collect();
    expected.len() == manifest.targets.len() && actual == expected
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum DeliveryJobStatus { Pending, Rendering, Completed, Failed }

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct DeliveryJob { pub id: u64, pub target: DeliveryTarget, pub status: DeliveryJobStatus, pub output_name: Option<String>, pub error: Option<String> }

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct DeliveryQueue { pub jobs: Vec<DeliveryJob>, #[serde(default = "default_queue_next_id")] next_id: u64 }

fn default_queue_next_id() -> u64 { 1 }

impl DeliveryQueue {
    pub fn enqueue(&mut self, target: DeliveryTarget) -> Option<u64> {
        if !target.validate() || self.jobs.len() >= 4096 || self.jobs.iter().any(|job| job.target.name.eq_ignore_ascii_case(target.name.trim())) { return None; }
        let id = self.next_id.max(1);
        self.next_id = id.checked_add(1)?;
        self.jobs.push(DeliveryJob { id, target, status: DeliveryJobStatus::Pending, output_name: None, error: None });
        Some(id)
    }
    pub fn begin(&mut self, id: u64) -> bool {
        let Some(job) = self.jobs.iter_mut().find(|job| job.id == id && job.status == DeliveryJobStatus::Pending) else { return false; };
        job.status = DeliveryJobStatus::Rendering;
        true
    }
    pub fn complete(&mut self, id: u64, output_name: &str) -> bool {
        let Some(job) = self.jobs.iter_mut().find(|job| job.id == id && job.status == DeliveryJobStatus::Rendering) else { return false; };
        let Some(expected) = job.target.normalized_filename() else { return false; };
        if output_name.trim() != expected { return false; }
        job.output_name = Some(expected);
        job.error = None;
        job.status = DeliveryJobStatus::Completed;
        true
    }
    pub fn fail(&mut self, id: u64, error: &str) -> bool {
        if error.trim().is_empty() || error.len() > 1024 || error.contains('\0') { return false; }
        let Some(job) = self.jobs.iter_mut().find(|job| job.id == id && job.status == DeliveryJobStatus::Rendering) else { return false; };
        job.error = Some(error.trim().to_owned());
        job.status = DeliveryJobStatus::Failed;
        true
    }
    pub fn retry_failed(&mut self, id: u64) -> bool {
        let Some(job) = self.jobs.iter_mut().find(|job| job.id == id && job.status == DeliveryJobStatus::Failed) else { return false; };
        job.error = None;
        job.status = DeliveryJobStatus::Pending;
        true
    }
    pub fn is_complete(&self) -> bool { !self.jobs.is_empty() && self.jobs.iter().all(|job| job.status == DeliveryJobStatus::Completed) }
    pub fn validate(&self) -> bool {
        self.jobs.len() <= 4096 && self.next_id > 0 && self.jobs.iter().enumerate().all(|(index, job)| job.id > 0 && job.id < self.next_id && job.target.validate() && self.jobs[..index].iter().all(|previous| previous.id != job.id && !previous.target.name.eq_ignore_ascii_case(&job.target.name)) && match job.status { DeliveryJobStatus::Completed => job.output_name.as_deref() == job.target.normalized_filename().as_deref() && job.error.is_none(), DeliveryJobStatus::Failed => job.error.as_ref().is_some_and(|error| !error.trim().is_empty() && error.len() <= 1024), DeliveryJobStatus::Pending | DeliveryJobStatus::Rendering => job.output_name.is_none() })
    }
}
pub fn loudness_within_standard(measured_lufs: f32, standard: LoudnessStandard, tolerance_lufs: f32) -> bool { measured_lufs.is_finite() && tolerance_lufs.is_finite() && tolerance_lufs >= 0.0 && (measured_lufs - standard.target_lufs()).abs() <= tolerance_lufs }

/// Applies a bounded loudness-normalization gain to interleaved PCM.  The
/// requested LUFS correction is automatically limited by the true-peak
/// ceiling, so queue exports cannot introduce clipping while normalizing.
pub fn normalize_loudness_interleaved(
    samples: &[f32], measured_lufs: f32, target_lufs: f32, max_true_peak_dbtp: f32,
) -> Option<(Vec<f32>, f32)> {
    if samples.is_empty() || samples.len() > 64 * 1024 * 1024 || samples.iter().any(|sample| !sample.is_finite())
        || !measured_lufs.is_finite() || !target_lufs.is_finite() || !(-70.0..=0.0).contains(&measured_lufs)
        || !(-70.0..=0.0).contains(&target_lufs) || !max_true_peak_dbtp.is_finite() || !(-20.0..=0.0).contains(&max_true_peak_dbtp) { return None; }
    let peak = samples.iter().map(|sample| sample.abs()).fold(0.0f32, f32::max);
    let requested_db = target_lufs - measured_lufs;
    let peak_db = if peak > 0.0 { 20.0 * peak.log10() } else { -120.0 };
    let applied_db = requested_db.min(max_true_peak_dbtp - peak_db).clamp(-60.0, 24.0);
    let gain = 10.0f32.powf(applied_db / 20.0);
    if !gain.is_finite() { return None; }
    let output = samples.iter().map(|sample| *sample * gain).collect::<Vec<_>>();
    (output.iter().all(|sample| sample.is_finite()) && applied_db.is_finite()).then_some((output, applied_db))
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct DdpTrack { pub number: u16, pub title: String, pub performer: String, pub start_sector: u64, pub length_sectors: u64, pub isrc: Option<String> }
impl DdpTrack { pub fn validate(&self) -> bool { self.number > 0 && !self.title.trim().is_empty() && self.title.len() <= 256 && !self.title.contains('\0') && self.performer.len() <= 256 && !self.performer.contains('\0') && self.length_sectors > 0 && self.start_sector.checked_add(self.length_sectors).is_some() && self.isrc.as_ref().is_none_or(|v| v.len() <= 32 && !v.contains('\0')) } pub fn end_sector(&self) -> Option<u64> { self.validate().then(|| self.start_sector.checked_add(self.length_sectors)).flatten() } }

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct DdpImageManifest { pub album_title: String, pub catalog_number: String, pub tracks: Vec<DdpTrack> }
impl DdpImageManifest {
    pub fn validate(&self) -> bool { !self.album_title.trim().is_empty() && self.album_title.len() <= 256 && !self.album_title.contains('\0') && self.catalog_number.len() <= 64 && !self.catalog_number.contains('\0') && !self.tracks.is_empty() && self.tracks.len() <= 99 && self.tracks.iter().enumerate().all(|(i,t)| t.validate() && t.number as usize == i + 1 && (i == 0 || t.start_sector >= self.tracks[i-1].start_sector.saturating_add(self.tracks[i-1].length_sectors))) }
    pub fn total_sectors(&self) -> Option<u64> { if !self.validate() { None } else { self.tracks.last().and_then(|t| t.end_sector()) } }
    pub fn to_json(&self) -> Result<String,String> { if !self.validate() { return Err("invalid DDP image manifest".into()); } serde_json::to_string(self).map_err(|e| e.to_string()) }
    pub fn from_json(json: &str) -> Result<Self,String> { let v: Self = serde_json::from_str(json).map_err(|e| e.to_string())?; if !v.validate() { Err("invalid DDP image manifest".into()) } else { Ok(v) } }
    pub fn track(&self, number: u16) -> Option<&DdpTrack> { self.tracks.iter().find(|track| track.number == number) }
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
            && matches!(self.frame_rate, 23.976 | 24.0 | 25.0 | 29.97 | 30.0 | 50.0 | 59.94 | 60.0)
            && self.tempo_bpm.is_none_or(|tempo| tempo.is_finite() && (1.0..=999.0).contains(&tempo))
            && !self.channel_roles.is_empty()
            && self.channel_roles.len() <= 256
    }

    pub fn stereo(project: &str, author: &str, frame_rate: f64, dual_mono: bool) -> Self {
        let channel_roles = if dual_mono {
            vec![IxmlChannelRole::LeftMix, IxmlChannelRole::RightMix]
        } else {
            vec![IxmlChannelRole::Left, IxmlChannelRole::Right]
        };
        Self { project: project.into(), author: author.into(), frame_rate, tempo_bpm: None, channel_roles }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum WaveContainer {
    Wave,
    WaveExtensible,
    Rf64,
}

pub fn select_wave_container(data_bytes: u64, channels: u16, speaker_metadata: bool) -> Option<WaveContainer> {
    if channels == 0 || channels > 256 { return None; }
    if data_bytes > u64::from(u32::MAX).saturating_sub(4096) {
        Some(WaveContainer::Rf64)
    } else if channels > 2 || speaker_metadata {
        Some(WaveContainer::WaveExtensible)
    } else {
        Some(WaveContainer::Wave)
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
pub struct LoudnessRequirements {
    pub integrated_lufs: f32,
    pub integrated_tolerance_lu: f32,
    pub max_true_peak_dbtp: f32,
    pub max_short_term_lufs: Option<f32>,
    pub max_momentary_lufs: Option<f32>,
    pub max_loudness_range_lu: Option<f32>,
}

impl LoudnessRequirements {
    pub fn ebu_r128() -> Self {
        Self { integrated_lufs: -23.0, integrated_tolerance_lu: 1.0, max_true_peak_dbtp: -1.0,
            max_short_term_lufs: None, max_momentary_lufs: None, max_loudness_range_lu: None }
    }

    pub fn validate(self) -> bool {
        self.integrated_lufs.is_finite() && (-70.0..=0.0).contains(&self.integrated_lufs)
            && self.integrated_tolerance_lu.is_finite() && (0.0..=20.0).contains(&self.integrated_tolerance_lu)
            && self.max_true_peak_dbtp.is_finite() && (-20.0..=0.0).contains(&self.max_true_peak_dbtp)
            && optional_level(self.max_short_term_lufs, -70.0, 0.0)
            && optional_level(self.max_momentary_lufs, -70.0, 0.0)
            && optional_level(self.max_loudness_range_lu, 0.0, 70.0)
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
pub struct LoudnessMeasurements {
    pub integrated_lufs: f32,
    pub max_short_term_lufs: f32,
    pub max_momentary_lufs: f32,
    pub loudness_range_lu: f32,
    pub max_true_peak_dbtp: f32,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct DeliveryQcReport {
    pub passed: bool,
    pub failures: Vec<String>,
}

pub fn audit_loudness(measurements: LoudnessMeasurements, requirements: LoudnessRequirements) -> DeliveryQcReport {
    let mut failures = Vec::new();
    if !requirements.validate() || !measurements_valid(measurements) {
        failures.push("invalid loudness measurements or requirements".into());
    } else {
        if (measurements.integrated_lufs - requirements.integrated_lufs).abs() > requirements.integrated_tolerance_lu {
            failures.push("integrated loudness is outside tolerance".into());
        }
        if measurements.max_true_peak_dbtp > requirements.max_true_peak_dbtp {
            failures.push("true peak exceeds limit".into());
        }
        if requirements.max_short_term_lufs.is_some_and(|limit| measurements.max_short_term_lufs > limit) {
            failures.push("short-term loudness exceeds limit".into());
        }
        if requirements.max_momentary_lufs.is_some_and(|limit| measurements.max_momentary_lufs > limit) {
            failures.push("momentary loudness exceeds limit".into());
        }
        if requirements.max_loudness_range_lu.is_some_and(|limit| measurements.loudness_range_lu > limit) {
            failures.push("loudness range exceeds limit".into());
        }
    }
    DeliveryQcReport { passed: failures.is_empty(), failures }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct DeliveryArtifact {
    pub filename: String,
    pub byte_len: u64,
    pub sha256: String,
}

impl DeliveryArtifact {
    pub fn from_bytes(filename: &str, bytes: &[u8]) -> Option<Self> {
        if !safe_delivery_filename(filename) || bytes.is_empty() { return None; }
        Some(Self { filename: filename.into(), byte_len: bytes.len() as u64,
            sha256: format!("{:x}", Sha256::digest(bytes)) })
    }

    pub fn verify(&self, bytes: &[u8]) -> bool {
        self.byte_len == bytes.len() as u64
            && self.sha256.len() == 64
            && self.sha256.bytes().all(|byte| byte.is_ascii_hexdigit())
            && self.sha256.eq_ignore_ascii_case(&format!("{:x}", Sha256::digest(bytes)))
    }
}

fn bounded_text(value: &str, max: usize, required: bool) -> bool {
    (!required || !value.trim().is_empty()) && value.len() <= max && !value.contains('\0')
}

fn valid_date(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.len() != 10 || bytes[4] != b'-' || bytes[7] != b'-' { return false; }
    let Ok(year) = value[0..4].parse::<u16>() else { return false; };
    let Ok(month) = value[5..7].parse::<u8>() else { return false; };
    let Ok(day) = value[8..10].parse::<u8>() else { return false; };
    if year == 0 || !(1..=12).contains(&month) { return false; }
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let max_day = match month { 2 if leap => 29, 2 => 28, 4 | 6 | 9 | 11 => 30, _ => 31 };
    (1..=max_day).contains(&day)
}

fn valid_time(value: &str) -> bool {
    let parts: Vec<_> = value.split(':').collect();
    parts.len() == 3
        && parts[0].parse::<u8>().is_ok_and(|hour| hour < 24)
        && parts[1].parse::<u8>().is_ok_and(|minute| minute < 60)
        && parts[2].parse::<u8>().is_ok_and(|second| second < 60)
}

fn optional_level(value: Option<f32>, min: f32, max: f32) -> bool {
    value.is_none_or(|value| value.is_finite() && (min..=max).contains(&value))
}

fn measurements_valid(value: LoudnessMeasurements) -> bool {
    [value.integrated_lufs, value.max_short_term_lufs, value.max_momentary_lufs,
        value.max_true_peak_dbtp].into_iter().all(|level| level.is_finite() && (-200.0..=24.0).contains(&level))
        && value.loudness_range_lu.is_finite() && (0.0..=200.0).contains(&value.loudness_range_lu)
}

fn safe_delivery_filename(value: &str) -> bool {
    !value.trim().is_empty() && value.len() <= 512 && !value.contains('\0') && !value.contains('/')
        && !value.contains('\\') && value != "." && value != ".."
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum AdmElementKind {
    Bed,
    Object,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct AdmPositionKeyframe {
    pub sample: u64,
    pub position: [f32; 3],
    pub gain_db: f32,
}

impl AdmPositionKeyframe {
    fn validate(&self) -> bool {
        self.position.iter().all(|value| value.is_finite() && (-1.0..=1.0).contains(value))
            && self.gain_db.is_finite()
            && (-120.0..=24.0).contains(&self.gain_db)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct AdmAuthoringElement {
    pub id: u16,
    pub name: String,
    pub kind: AdmElementKind,
    pub source_track_id: u32,
    pub source_channels: u16,
    pub object_bus_id: Option<u16>,
    pub keyframes: Vec<AdmPositionKeyframe>,
}

impl AdmAuthoringElement {
    fn validate(&self) -> bool {
        self.id > 0
            && bounded_text(&self.name, 128, true)
            && self.source_track_id > 0
            && (1..=16).contains(&self.source_channels)
            && self.keyframes.len() <= 1_000_000
            && self.keyframes.iter().enumerate().all(|(index, keyframe)| {
                keyframe.validate()
                    && (index == 0 || keyframe.sample > self.keyframes[index - 1].sample)
            })
            && match self.kind {
                AdmElementKind::Bed => self.object_bus_id.is_none() && self.keyframes.is_empty(),
                AdmElementKind::Object => self.object_bus_id.is_some_and(|id| id > 0) && !self.keyframes.is_empty(),
            }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
pub struct AdmTrimDownmix {
    pub surround_trim_db: f32,
    pub height_trim_db: f32,
    pub overhead_balance: f32,
    pub stereo_direct: bool,
}

impl AdmTrimDownmix {
    fn validate(self) -> bool {
        self.surround_trim_db.is_finite() && (-12.0..=0.0).contains(&self.surround_trim_db)
            && self.height_trim_db.is_finite() && (-12.0..=0.0).contains(&self.height_trim_db)
            && self.overhead_balance.is_finite() && (-1.0..=1.0).contains(&self.overhead_balance)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct AdmAuthoringProject {
    pub profile: String,
    pub sample_rate: u32,
    pub duration_samples: u64,
    pub elements: Vec<AdmAuthoringElement>,
    pub trim_downmix: AdmTrimDownmix,
}

impl AdmAuthoringProject {
    pub fn validate(&self) -> Result<(), Vec<String>> {
        let mut errors = Vec::new();
        if !bounded_text(&self.profile, 128, true) { errors.push("ADM profile is invalid".into()); }
        if self.sample_rate != 48_000 { errors.push("Dolby Atmos ADM requires 48 kHz".into()); }
        if self.duration_samples == 0 { errors.push("ADM duration is empty".into()); }
        if self.elements.is_empty() || self.elements.len() > 129 { errors.push("ADM element count is invalid".into()); }
        if !self.trim_downmix.validate() { errors.push("ADM trim/downmix settings are invalid".into()); }
        let beds = self.elements.iter().filter(|element| element.kind == AdmElementKind::Bed).count();
        let object_channels: usize = self.elements.iter().filter(|element| element.kind == AdmElementKind::Object)
            .map(|element| usize::from(element.source_channels)).sum();
        if beds != 1 { errors.push("ADM project must contain exactly one bed".into()); }
        if object_channels > 128 { errors.push("ADM object channel count exceeds 128".into()); }
        for (index, element) in self.elements.iter().enumerate() {
            if !element.validate() { errors.push(format!("ADM element {} is invalid", element.id)); }
            if self.elements[..index].iter().any(|previous| previous.id == element.id) {
                errors.push(format!("duplicate ADM element id {}", element.id));
            }
            if self.elements[..index].iter().any(|previous| previous.source_track_id == element.source_track_id) {
                errors.push(format!("source track {} is assigned more than once", element.source_track_id));
            }
            if element.keyframes.iter().any(|keyframe| keyframe.sample >= self.duration_samples) {
                errors.push(format!("ADM element {} has a keyframe outside the program", element.id));
            }
        }
        let mut buses = HashSet::new();
        if self.elements.iter().filter_map(|element| element.object_bus_id).any(|bus| !buses.insert(bus)) {
            errors.push("ADM object bus assignments are not unique".into());
        }
        if errors.is_empty() { Ok(()) } else { Err(errors) }
    }

    pub fn requires_rf64(&self, bytes_per_sample: u16) -> bool {
        let channels: u64 = self.elements.iter().map(|element| u64::from(element.source_channels)).sum();
        self.duration_samples.saturating_mul(channels).saturating_mul(u64::from(bytes_per_sample))
            > u64::from(u32::MAX).saturating_sub(4096)
    }
}

#[cfg(test)]
#[path = "delivery_tests.rs"]
mod delivery_tests;
