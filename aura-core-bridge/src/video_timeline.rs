use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct VideoClip {
    pub id: u32,
    pub path: String,
    pub start_frame: i64,
    pub duration_frames: u64,
    pub fps: f64,
    pub audio_offset_samples: i64,
}
impl VideoClip {
    pub fn validate(&self) -> bool {
        self.id != 0
            && !self.path.trim().is_empty()
            && self.path.len() <= 4096
            && !self.path.contains('\0')
            && self.start_frame >= 0
            && self.duration_frames > 0
            && self.duration_frames <= 10_000_000
            && self.fps.is_finite()
            && (1.0..=240.0).contains(&self.fps)
    }
    pub fn end_frame(&self) -> Option<i64> {
        self.start_frame.checked_add(self.duration_frames as i64)
    }
}
impl VideoClip {
    pub fn contains_frame(&self, frame: i64) -> bool {
        frame >= self.start_frame && self.end_frame().is_some_and(|end| frame < end)
    }
    pub fn audio_sample_at_frame(&self, frame: i64, sample_rate: u32) -> Option<i64> {
        if !self.validate() || sample_rate == 0 || !self.contains_frame(frame) {
            return None;
        }
        let rel = (frame - self.start_frame) as f64 * sample_rate as f64 / self.fps;
        rel.is_finite()
            .then_some(rel.round().clamp(i64::MIN as f64, i64::MAX as f64) as i64)
            .map(|sample| sample.saturating_add(self.audio_offset_samples))
    }
}

pub fn frame_to_timecode(frame: u64, fps: u32) -> Option<(u32, u32, u32, u32)> {
    if fps == 0 || fps > 120 {
        return None;
    }
    let sec = frame / fps as u64;
    Some((
        (sec / 3600) as u32,
        (sec / 60 % 60) as u32,
        (sec % 60) as u32,
        (frame % fps as u64) as u32,
    ))
}
pub fn timecode_to_frame(
    hours: u32,
    minutes: u32,
    seconds: u32,
    frames: u32,
    fps: u32,
) -> Option<u64> {
    if fps == 0 || fps > 120 || minutes >= 60 || seconds >= 60 || frames >= fps {
        return None;
    }
    let total_seconds = (hours as u64)
        .checked_mul(3600)?
        .checked_add((minutes as u64) * 60)?
        .checked_add(seconds as u64)?;
    total_seconds
        .checked_mul(fps as u64)?
        .checked_add(frames as u64)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn validates_frame_sync() {
        let c = VideoClip {
            id: 1,
            path: "movie.mov".into(),
            start_frame: 0,
            duration_frames: 100,
            fps: 24.0,
            audio_offset_samples: 0,
        };
        assert!(c.validate());
        assert_eq!(c.end_frame(), Some(100));
        assert_eq!(timecode_to_frame(1, 2, 3, 4, 24), Some(89356));
        assert_eq!(frame_to_timecode(89356, 24), Some((1, 2, 3, 4)));
    }
}
