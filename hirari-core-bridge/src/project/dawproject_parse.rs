//! XML/JSON primitives and helpers for DawProject read/write, split out of dawproject.rs.
use super::*;

pub(in crate::dawproject) fn is_dawproject_plugin_device(node: roxmltree::Node<'_, '_>) -> bool {
    matches!(
        node.tag_name().name(),
        "Vst2Plugin"
            | "Vst3Plugin"
            | "ClapPlugin"
            | "AuPlugin"
            | "Device"
            | "BuiltinDevice"
            | "Equalizer"
            | "Compressor"
            | "NoiseGate"
            | "Limiter"
    )
}

/// Recover a clip-level expression only when it is constant across the clip.
/// Hirari's region model stores gain and pitch as static values, so flattening
/// a changing DAWproject expression to one point would silently destroy edits.
pub(in crate::dawproject) fn parse_clip_expression_value(
    clip: roxmltree::Node<'_, '_>,
    expression: &str,
    target_unit: &str,
) -> Result<Option<f32>> {
    for points in clip
        .descendants()
        .filter(|node| node.has_tag_name("Points"))
    {
        let target = points.children().find(|node| node.has_tag_name("Target"));
        if target.and_then(|node| node.attribute("expression")) != Some(expression) {
            continue;
        }
        let unit = points.attribute("unit").unwrap_or("linear");
        if unit != target_unit && !(expression == "gain" && unit == "decibel") {
            continue;
        }
        let mut constant_value = None::<f32>;
        for point in points
            .children()
            .filter(|node| node.has_tag_name("RealPoint"))
        {
            let mut value = required_number(&point, "value")? as f32;
            if !value.is_finite() {
                bail!("DAWproject clip expression value is not finite");
            }
            if expression == "gain" && unit == "decibel" {
                value = 10.0f32.powf(value / 20.0);
            }
            if let Some(previous) = constant_value {
                if (previous - value).abs() > 1e-5 {
                    return Ok(None);
                }
            } else {
                constant_value = Some(value);
            }
        }
        if constant_value.is_some() {
            return Ok(constant_value);
        }
    }
    Ok(None)
}

pub(in crate::dawproject) fn read_zip_text<R: Read + Seek>(
    archive: &mut ZipArchive<R>,
    name: &str,
    limit: u64,
) -> Result<String> {
    let entry = archive
        .by_name(name)
        .with_context(|| format!("DAWproject is missing {name}"))?;
    if entry.size() > limit {
        bail!("DAWproject {name} exceeds the supported size limit");
    }
    let mut bytes = Vec::with_capacity(entry.size() as usize);
    entry.take(limit + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        bail!("DAWproject {name} exceeds the supported size limit");
    }
    String::from_utf8(bytes).with_context(|| format!("DAWproject {name} is not UTF-8"))
}

pub(in crate::dawproject) fn read_zip_text_optional<R: Read + Seek>(
    archive: &mut ZipArchive<R>,
    name: &str,
    limit: u64,
) -> Result<Option<String>> {
    if !archive.file_names().any(|entry| entry == name) {
        return Ok(None);
    }
    read_zip_text(archive, name, limit).map(Some)
}

pub(in crate::dawproject) fn validate_archive_media_path(path: &str) -> Result<()> {
    if path.trim().is_empty() || path.len() > 4096 || path.contains('\0') || path.contains('\\') {
        bail!("DAWproject media path is invalid");
    }
    let path = Path::new(path);
    if path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, std::path::Component::Normal(_)))
    {
        bail!("DAWproject media path escapes the archive");
    }
    Ok(())
}

pub(in crate::dawproject) fn parse_bool(value: &str) -> Option<bool> {
    match value.trim() {
        "true" | "1" => Some(true),
        "false" | "0" => Some(false),
        _ => None,
    }
}

pub(in crate::dawproject) fn bounded_utf8(value: &str, max_bytes: usize) -> String {
    let end = value
        .char_indices()
        .take_while(|(index, character)| index + character.len_utf8() <= max_bytes)
        .map(|(index, character)| index + character.len_utf8())
        .last()
        .unwrap_or(0);
    value[..end].to_owned()
}

pub(in crate::dawproject) fn optional_number(node: &roxmltree::Node<'_, '_>, attribute: &str) -> Option<f64> {
    node.attribute(attribute)
        .and_then(|value| value.parse::<f64>().ok())
        .filter(|value| value.is_finite())
}

pub(in crate::dawproject) fn required_number(node: &roxmltree::Node<'_, '_>, attribute: &str) -> Result<f64> {
    optional_number(node, attribute).with_context(|| {
        format!(
            "DAWproject element {} has invalid {attribute}",
            node.tag_name().name()
        )
    })
}

pub(in crate::dawproject) fn parse_tempo_events(
    root: roxmltree::Node<'_, '_>,
    _base_bpm: f64,
) -> Result<Vec<TempoEventContract>> {
    let Some(automation) = root
        .descendants()
        .find(|node| node.has_tag_name("TempoAutomation"))
    else {
        return Ok(Vec::new());
    };
    if automation
        .children()
        .filter(|node| node.has_tag_name("RealPoint"))
        .count()
        > 65_536
    {
        bail!("DAWproject contains too many tempo automation points");
    }
    automation
        .children()
        .filter(|node| node.has_tag_name("RealPoint"))
        .map(|point| {
            let beat = required_number(&point, "time")?;
            let bpm = required_number(&point, "value")?;
            let interpolation = point.attribute("interpolation").unwrap_or("hold");
            if !matches!(interpolation, "hold" | "linear") {
                bail!("DAWproject tempo interpolation is unsupported: {interpolation}");
            }
            Ok(TempoEventContract {
                beat,
                bpm,
                ramp: interpolation == "linear",
            })
        })
        .collect()
}

pub(in crate::dawproject) fn normalize_tempo_events(
    mut events: Vec<TempoEventContract>,
    base_bpm: f64,
) -> Result<Vec<TempoEventContract>> {
    events.sort_by(|left, right| left.beat.total_cmp(&right.beat));
    let mut normalized = Vec::<TempoEventContract>::with_capacity(events.len() + 1);
    for event in events {
        event.validate()?;
        if let Some(previous) = normalized.last_mut() {
            if (previous.beat - event.beat).abs() <= f64::EPSILON {
                *previous = event;
                continue;
            }
        }
        normalized.push(event);
    }
    if normalized.first().is_none_or(|event| event.beat > 0.0) {
        normalized.insert(
            0,
            TempoEventContract {
                beat: 0.0,
                bpm: base_bpm,
                ramp: false,
            },
        );
    }
    if normalized.is_empty() {
        normalized.push(TempoEventContract {
            beat: 0.0,
            bpm: base_bpm,
            ramp: false,
        });
    }
    Ok(normalized)
}

pub(in crate::dawproject) fn parse_time_signature_events(
    root: roxmltree::Node<'_, '_>,
    base_numerator: u8,
    base_denominator: u8,
) -> Result<Vec<TimeSignatureEventContract>> {
    let Some(automation) = root
        .descendants()
        .find(|node| node.has_tag_name("TimeSignatureAutomation"))
    else {
        return Ok(Vec::new());
    };
    if automation
        .children()
        .filter(|node| node.has_tag_name("TimeSignaturePoint"))
        .count()
        > 65_536
    {
        bail!("DAWproject contains too many time signature points");
    }
    let mut events = Vec::new();
    for point in automation
        .children()
        .filter(|node| node.has_tag_name("TimeSignaturePoint"))
    {
        let event = TimeSignatureEventContract {
            beat: required_number(&point, "time")?,
            numerator: point
                .attribute("numerator")
                .and_then(|value| value.parse().ok())
                .context("time signature numerator is invalid")?,
            denominator: point
                .attribute("denominator")
                .and_then(|value| value.parse().ok())
                .context("time signature denominator is invalid")?,
        };
        event.validate()?;
        events.push(event);
    }
    normalize_time_signature_events(events, base_numerator, base_denominator)
}

pub(in crate::dawproject) fn normalize_time_signature_events(
    mut events: Vec<TimeSignatureEventContract>,
    base_numerator: u8,
    base_denominator: u8,
) -> Result<Vec<TimeSignatureEventContract>> {
    let base = TimeSignatureEventContract {
        beat: 0.0,
        numerator: base_numerator,
        denominator: base_denominator,
    };
    base.validate()?;
    events.sort_by(|left, right| left.beat.total_cmp(&right.beat));
    let mut normalized = Vec::<TimeSignatureEventContract>::with_capacity(events.len() + 1);
    for event in events {
        event.validate()?;
        if let Some(previous) = normalized.last_mut() {
            if (previous.beat - event.beat).abs() <= f64::EPSILON {
                *previous = event;
                continue;
            }
        }
        normalized.push(event);
    }
    if normalized.first().is_none_or(|event| event.beat > 0.0) {
        normalized.insert(0, base.clone());
    }
    if normalized.is_empty() {
        normalized.push(base);
    }
    Ok(normalized)
}

pub(in crate::dawproject) fn duration_to_samples(
    start: f64,
    duration: f64,
    unit: &str,
    sample_rate: f64,
    tempo_events: &[TempoEventContract],
    base_bpm: f64,
) -> Result<u64> {
    if !duration.is_finite() || duration < 0.0 {
        bail!("DAWproject fade duration is invalid");
    }
    let start_samples =
        timeline_position_to_samples(start, unit, sample_rate, tempo_events, base_bpm)?;
    let end_samples =
        timeline_position_to_samples(start + duration, unit, sample_rate, tempo_events, base_bpm)?;
    Ok(end_samples.saturating_sub(start_samples))
}
