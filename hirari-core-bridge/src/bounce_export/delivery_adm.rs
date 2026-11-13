//! ADM authoring model: elements, trims, keyframes and the authoring
//! project, split out of delivery.rs.
use super::*;

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
        self.position
            .iter()
            .all(|value| value.is_finite() && (-1.0..=1.0).contains(value))
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
                AdmElementKind::Object => {
                    self.object_bus_id.is_some_and(|id| id > 0) && !self.keyframes.is_empty()
                }
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
        self.surround_trim_db.is_finite()
            && (-12.0..=0.0).contains(&self.surround_trim_db)
            && self.height_trim_db.is_finite()
            && (-12.0..=0.0).contains(&self.height_trim_db)
            && self.overhead_balance.is_finite()
            && (-1.0..=1.0).contains(&self.overhead_balance)
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
        if !bounded_text(&self.profile, 128, true) {
            errors.push("ADM profile is invalid".into());
        }
        if self.sample_rate != 48_000 {
            errors.push("Dolby Atmos ADM requires 48 kHz".into());
        }
        if self.duration_samples == 0 {
            errors.push("ADM duration is empty".into());
        }
        if self.elements.is_empty() || self.elements.len() > 129 {
            errors.push("ADM element count is invalid".into());
        }
        if !self.trim_downmix.validate() {
            errors.push("ADM trim/downmix settings are invalid".into());
        }
        let beds = self
            .elements
            .iter()
            .filter(|element| element.kind == AdmElementKind::Bed)
            .count();
        let object_channels: usize = self
            .elements
            .iter()
            .filter(|element| element.kind == AdmElementKind::Object)
            .map(|element| usize::from(element.source_channels))
            .sum();
        if beds != 1 {
            errors.push("ADM project must contain exactly one bed".into());
        }
        if object_channels > 128 {
            errors.push("ADM object channel count exceeds 128".into());
        }
        for (index, element) in self.elements.iter().enumerate() {
            if !element.validate() {
                errors.push(format!("ADM element {} is invalid", element.id));
            }
            if self.elements[..index]
                .iter()
                .any(|previous| previous.id == element.id)
            {
                errors.push(format!("duplicate ADM element id {}", element.id));
            }
            if self.elements[..index]
                .iter()
                .any(|previous| previous.source_track_id == element.source_track_id)
            {
                errors.push(format!(
                    "source track {} is assigned more than once",
                    element.source_track_id
                ));
            }
            if element
                .keyframes
                .iter()
                .any(|keyframe| keyframe.sample >= self.duration_samples)
            {
                errors.push(format!(
                    "ADM element {} has a keyframe outside the program",
                    element.id
                ));
            }
        }
        let mut buses = HashSet::new();
        if self
            .elements
            .iter()
            .filter_map(|element| element.object_bus_id)
            .any(|bus| !buses.insert(bus))
        {
            errors.push("ADM object bus assignments are not unique".into());
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }

    pub fn requires_rf64(&self, bytes_per_sample: u16) -> bool {
        let channels: u64 = self
            .elements
            .iter()
            .map(|element| u64::from(element.source_channels))
            .sum();
        self.duration_samples
            .saturating_mul(channels)
            .saturating_mul(u64::from(bytes_per_sample))
            > u64::from(u32::MAX).saturating_sub(4096)
    }
}
