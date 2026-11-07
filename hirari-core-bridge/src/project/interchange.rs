use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct EdlEvent {
    pub number: u32,
    pub reel: String,
    pub track: char,
    pub edit: char,
    pub source_in: String,
    pub source_out: String,
    pub record_in: String,
    pub record_out: String,
}

impl EdlEvent {
    pub fn validate(&self) -> bool {
        self.number > 0
            && self.number <= 9999
            && !self.reel.trim().is_empty()
            && self.reel.len() <= 16
            && matches!(self.track, 'V' | 'A' | 'B')
            && matches!(self.edit, 'C' | 'D' | 'W' | 'K')
            && [
                &self.source_in,
                &self.source_out,
                &self.record_in,
                &self.record_out,
            ]
            .iter()
            .all(|tc| valid_timecode(tc))
    }
    pub fn to_cmx_line(&self) -> Option<String> {
        self.validate().then(|| {
            format!(
                "{:03}  {:<8} {}  {} {} {} {} {}",
                self.number,
                self.reel,
                self.track,
                self.edit,
                self.source_in,
                self.source_out,
                self.record_in,
                self.record_out
            )
        })
    }
}

pub fn validate_edl(events: &[EdlEvent]) -> bool {
    events.len() <= 100_000
        && events
            .iter()
            .enumerate()
            .all(|(i, e)| e.validate() && (i == 0 || events[i - 1].number < e.number))
}
pub fn export_cmx3600(title: &str, events: &[EdlEvent]) -> Option<String> {
    if title.trim().is_empty() || title.len() > 128 || !validate_edl(events) {
        return None;
    }
    let mut out = format!("TITLE: {}\nFCM: NON-DROP FRAME\n\n", title.trim());
    for event in events {
        out.push_str(&event.to_cmx_line()?);
        out.push('\n');
    }
    Some(out)
}
fn valid_timecode(value: &str) -> bool {
    let p: Vec<_> = value.split(':').collect();
    p.len() == 4
        && p.iter()
            .all(|part| part.len() == 2 && part.bytes().all(|b| b.is_ascii_digit()))
        && p[1].parse::<u8>().map(|v| v < 60).unwrap_or(false)
        && p[2].parse::<u8>().map(|v| v < 60).unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn exports_valid_cmx_edl() {
        let e = EdlEvent {
            number: 1,
            reel: "REEL1".into(),
            track: 'V',
            edit: 'C',
            source_in: "00:00:00:00".into(),
            source_out: "00:00:01:00".into(),
            record_in: "00:00:00:00".into(),
            record_out: "00:00:01:00".into(),
        };
        let text = export_cmx3600("Scene", &[e]).unwrap();
        assert!(text.contains("TITLE: Scene"));
    }
}
