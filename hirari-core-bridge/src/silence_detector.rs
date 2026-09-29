//! Deterministic, non-destructive silence analysis for audio-event editing.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SilenceRange {
    pub start: usize,
    pub length: usize,
}

pub fn detect_silence(samples: &[f32], threshold: f32, min_length: usize) -> Vec<SilenceRange> {
    if samples.is_empty()
        || !threshold.is_finite()
        || !(0.0..=1.0).contains(&threshold)
        || min_length == 0
    {
        return Vec::new();
    }
    let mut ranges = Vec::new();
    let mut start = None;
    for (index, sample) in samples.iter().enumerate() {
        let silent = sample.is_finite() && sample.abs() <= threshold;
        match (start, silent) {
            (None, true) => start = Some(index),
            (Some(begin), false) => {
                if index - begin >= min_length {
                    ranges.push(SilenceRange {
                        start: begin,
                        length: index - begin,
                    });
                }
                start = None;
            }
            _ => {}
        }
    }
    if let Some(begin) = start {
        if samples.len() - begin >= min_length {
            ranges.push(SilenceRange {
                start: begin,
                length: samples.len() - begin,
            });
        }
    }
    ranges
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_only_minimum_length_silent_runs() {
        let ranges = detect_silence(&[0.0, 0.01, 0.8, 0.0, 0.0], 0.02, 2);
        assert_eq!(
            ranges,
            vec![
                SilenceRange {
                    start: 0,
                    length: 2
                },
                SilenceRange {
                    start: 3,
                    length: 2
                },
            ]
        );
    }
}
