use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct CustomMeter {
    pub id: String,
    pub title: String,
    pub source: String,
    #[serde(default = "default_meter_min")]
    pub min_db: f32,
    #[serde(default = "default_meter_max")]
    pub max_db: f32,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct CustomPanel {
    pub id: String,
    pub title: String,
    pub controls: Vec<String>,
    #[serde(default)]
    pub meters: Vec<CustomMeter>,
}

fn default_meter_min() -> f32 {
    -60.0
}
fn default_meter_max() -> f32 {
    6.0
}

impl CustomPanel {
    pub fn validate(&self) -> Result<(), String> {
        if !valid_id(&self.id) || self.title.trim().is_empty() || self.title.len() > 128 {
            return Err("custom panel id/title is invalid".into());
        }
        if self.controls.len() > 64 || self.meters.len() > 32 {
            return Err("custom panel exceeds control or meter limit".into());
        }
        if self.controls.iter().any(|control| !valid_id(control)) {
            return Err("custom panel contains an invalid control id".into());
        }
        for meter in &self.meters {
            if !valid_id(&meter.id)
                || meter.title.trim().is_empty()
                || meter.title.len() > 128
                || meter.title.contains('\0')
                || meter.source.trim().is_empty()
                || meter.source.len() > 256
                || meter.source.contains('\0')
                || !meter.min_db.is_finite()
                || !meter.max_db.is_finite()
                || meter.min_db >= meter.max_db
                || meter.min_db < -180.0
                || meter.max_db > 24.0
            {
                return Err("custom meter definition is invalid".into());
            }
        }
        Ok(())
    }
}

fn valid_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn validates_bounded_custom_panel_and_meter_schema() {
        let panel = CustomPanel {
            id: "vocal-meter".into(),
            title: "Vocal Meter".into(),
            controls: vec!["gain".into()],
            meters: vec![CustomMeter {
                id: "lufs".into(),
                title: "LUFS".into(),
                source: "mix.lufs".into(),
                min_db: -60.0,
                max_db: 6.0,
            }],
        };
        assert!(panel.validate().is_ok());
        let mut invalid = panel;
        invalid.meters[0].max_db = f32::NAN;
        assert!(invalid.validate().is_err());
    }
}
