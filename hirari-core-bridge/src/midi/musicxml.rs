//! MusicXML note and meter interchange for the canonical MIDI timeline.

use roxmltree::{Document, Node};
use std::collections::HashMap;
use std::fs;
use std::path::Path;

const MAX_XML_BYTES: usize = 64 * 1024 * 1024;
const MAX_PARTS: usize = 256;
const MAX_NOTES: usize = 1_000_000;
const MAX_SCORE_BEATS: f64 = 1_000_000.0;

#[derive(Clone, Debug, PartialEq)]
pub struct MusicXmlNote {
    pub start_beat: f64,
    pub length_beats: f64,
    pub pitch: u8,
    pub velocity: u8,
    pub voice: u8,
    pub lyric: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MusicXmlPart {
    pub name: String,
    pub notes: Vec<MusicXmlNote>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MusicXmlTimeSignature {
    pub beat: f64,
    pub numerator: u8,
    pub denominator: u8,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MusicXmlTempoEvent {
    pub beat: f64,
    pub bpm: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MusicXmlScore {
    pub title: String,
    pub parts: Vec<MusicXmlPart>,
    pub time_signatures: Vec<MusicXmlTimeSignature>,
    pub tempo_events: Vec<MusicXmlTempoEvent>,
}

fn child<'a, 'input>(node: Node<'a, 'input>, name: &str) -> Option<Node<'a, 'input>> {
    node.children()
        .find(|candidate| candidate.is_element() && candidate.tag_name().name() == name)
}

fn child_text<'a, 'input>(node: Node<'a, 'input>, name: &str) -> Option<&'a str> {
    child(node, name)?.text().map(str::trim)
}

fn parse_meter(attributes: Node<'_, '_>) -> Option<(u8, u8)> {
    let time = child(attributes, "time")?;
    let numerator = child_text(time, "beats")?
        .split('+')
        .map(str::trim)
        .map(str::parse::<u16>)
        .collect::<Result<Vec<_>, _>>()
        .ok()?
        .into_iter()
        .sum::<u16>();
    let denominator = child_text(time, "beat-type")?.parse::<u16>().ok()?;
    if numerator == 0 || numerator > 32 || !matches!(denominator, 1 | 2 | 4 | 8 | 16 | 32) {
        return None;
    }
    Some((numerator as u8, denominator as u8))
}

fn pitch_from_node(pitch: Node<'_, '_>) -> Option<u8> {
    let step = child_text(pitch, "step")?;
    let natural = match step {
        "C" => 0i32,
        "D" => 2,
        "E" => 4,
        "F" => 5,
        "G" => 7,
        "A" => 9,
        "B" => 11,
        _ => return None,
    };
    let alter = child_text(pitch, "alter")
        .unwrap_or("0")
        .parse::<i32>()
        .ok()?;
    if !(-2..=2).contains(&alter) {
        return None;
    }
    let octave = child_text(pitch, "octave")?.parse::<i32>().ok()?;
    u8::try_from((octave + 1) * 12 + natural + alter)
        .ok()
        .filter(|pitch| *pitch <= 127)
}

fn unpitched_display_pitch(note: Node<'_, '_>) -> Option<u8> {
    let step = child_text(note, "display-step")?;
    let natural = match step {
        "C" => 0i32,
        "D" => 2,
        "E" => 4,
        "F" => 5,
        "G" => 7,
        "A" => 9,
        "B" => 11,
        _ => return None,
    };
    let octave = child_text(note, "display-octave")?.parse::<i32>().ok()?;
    u8::try_from((octave + 1) * 12 + natural)
        .ok()
        .filter(|pitch| *pitch <= 127)
}

fn read_note_text(note: Node<'_, '_>) -> String {
    note.children()
        .filter(|node| node.is_element() && node.tag_name().name() == "lyric")
        .find_map(|lyric| child_text(lyric, "text"))
        .unwrap_or_default()
        .chars()
        .filter(|character| !character.is_control() || *character == '\n')
        .take(1024)
        .collect()
}

fn read_velocity(note: Node<'_, '_>) -> u8 {
    let standard_extension = child(note, "velocity")
        .and_then(|node| node.text())
        .and_then(|value| value.trim().parse::<u16>().ok());
    let play_extension = child(note, "play").and_then(|play| {
        play.children()
            .find(|node| {
                node.is_element()
                    && node.tag_name().name() == "other-play"
                    && node.attribute("type") == Some("velocity")
            })
            .and_then(|node| node.text())
            .and_then(|value| value.trim().parse::<u16>().ok())
    });
    standard_extension
        .or(play_extension)
        .unwrap_or(96)
        .clamp(1, 127) as u8
}

fn tie_types(note: Node<'_, '_>) -> (bool, bool) {
    let mut starts = false;
    let mut stops = false;
    for element in note.children().filter(|node| node.is_element()) {
        match (element.tag_name().name(), element.attribute("type")) {
            ("tie", Some("start")) => starts = true,
            ("tie", Some("stop")) => stops = true,
            ("notations", _) => {
                for tied in element
                    .children()
                    .filter(|node| node.is_element() && node.tag_name().name() == "tied")
                {
                    match tied.attribute("type") {
                        Some("start") => starts = true,
                        Some("stop") => stops = true,
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
    (starts, stops)
}

pub fn parse_musicxml(xml: &str) -> Result<MusicXmlScore, String> {
    if xml.is_empty() || xml.len() > MAX_XML_BYTES {
        return Err("MusicXML must be between 1 byte and 64 MiB".into());
    }
    let document = Document::parse(xml).map_err(|error| format!("invalid MusicXML: {error}"))?;
    let root = document.root_element();
    if root.tag_name().name() != "score-partwise" {
        return Err("only MusicXML score-partwise files are supported".into());
    }
    if document.descendants().take(MAX_NOTES * 8 + 1).count() > MAX_NOTES * 8 {
        return Err("MusicXML contains too many XML elements".into());
    }
    let title = child(root, "work")
        .and_then(|work| child_text(work, "work-title"))
        .unwrap_or("Imported Score")
        .chars()
        .filter(|character| !character.is_control())
        .take(256)
        .collect::<String>();
    let mut names = HashMap::new();
    if let Some(part_list) = child(root, "part-list") {
        for metadata in part_list
            .children()
            .filter(|node| node.is_element() && node.tag_name().name() == "score-part")
        {
            let id = metadata.attribute("id").unwrap_or_default();
            let name = child_text(metadata, "part-name").unwrap_or("Part");
            names.insert(id.to_owned(), name.chars().take(64).collect::<String>());
        }
    }
    let xml_parts = root
        .children()
        .filter(|node| node.is_element() && node.tag_name().name() == "part")
        .collect::<Vec<_>>();
    if xml_parts.is_empty() || xml_parts.len() > MAX_PARTS {
        return Err("MusicXML must contain 1–256 parts".into());
    }

    let mut parts = Vec::with_capacity(xml_parts.len());
    let mut score_time_signatures = Vec::new();
    let mut score_tempo_events = Vec::new();
    for (part_index, part) in xml_parts.into_iter().enumerate() {
        let part_name = names
            .get(part.attribute("id").unwrap_or_default())
            .cloned()
            .unwrap_or_else(|| format!("Part {}", part_index + 1));
        let mut divisions = 1u32;
        let mut numerator = 4u8;
        let mut denominator = 4u8;
        let mut measure_start = 0.0f64;
        let mut notes = Vec::<MusicXmlNote>::new();
        let mut tied_notes = HashMap::<(u8, u8), usize>::new();
        let mut part_time_signatures = Vec::new();
        let mut part_tempo_events = Vec::new();
        for measure in part
            .children()
            .filter(|node| node.is_element() && node.tag_name().name() == "measure")
        {
            if let Some(attributes) = measure
                .children()
                .find(|node| node.is_element() && node.tag_name().name() == "attributes")
            {
                if let Some(value) =
                    child_text(attributes, "divisions").and_then(|value| value.parse::<u32>().ok())
                {
                    if value == 0 || value > 1_000_000 {
                        return Err("MusicXML divisions are out of range".into());
                    }
                    divisions = value;
                }
                if let Some((beats, beat_type)) = parse_meter(attributes) {
                    numerator = beats;
                    denominator = beat_type;
                    if part_time_signatures
                        .last()
                        .is_none_or(|event: &MusicXmlTimeSignature| {
                            event.numerator != numerator || event.denominator != denominator
                        })
                    {
                        part_time_signatures.push(MusicXmlTimeSignature {
                            beat: measure_start,
                            numerator,
                            denominator,
                        });
                    }
                }
            }
            let mut cursor = 0.0f64;
            let mut last_note_start = 0.0f64;
            let mut measure_extent = 0.0f64;
            for element in measure.children().filter(Node::is_element) {
                match element.tag_name().name() {
                    "direction" => {
                        if let Some(sound) = child(element, "sound") {
                            if let Some(bpm) = sound
                                .attribute("tempo")
                                .and_then(|value| value.parse::<f64>().ok())
                                .filter(|value| value.is_finite() && (20.0..=300.0).contains(value))
                            {
                                let offset = child_text(element, "offset")
                                    .and_then(|value| value.parse::<i64>().ok())
                                    .unwrap_or(0);
                                let event_beat =
                                    measure_start + cursor + offset as f64 / divisions as f64;
                                if event_beat >= 0.0 && event_beat <= MAX_SCORE_BEATS {
                                    part_tempo_events.push(MusicXmlTempoEvent {
                                        beat: event_beat,
                                        bpm,
                                    });
                                }
                            }
                        }
                    }
                    "backup" | "forward" => {
                        let duration = child_text(element, "duration")
                            .and_then(|value| value.parse::<u64>().ok())
                            .ok_or_else(|| {
                                "MusicXML backup/forward has invalid duration".to_owned()
                            })?;
                        let delta = duration as f64 / divisions as f64;
                        if element.tag_name().name() == "backup" {
                            cursor = (cursor - delta).max(0.0);
                        } else {
                            cursor += delta;
                        }
                    }
                    "note" => {
                        let duration = child_text(element, "duration")
                            .and_then(|value| value.parse::<u64>().ok())
                            .unwrap_or(0);
                        if duration == 0 {
                            continue;
                        }
                        let chord = child(element, "chord").is_some();
                        let note_start = if chord { last_note_start } else { cursor };
                        let length = duration as f64 / divisions as f64;
                        let note_end = note_start + length;
                        if duration > 1_000_000_000
                            || note_end > MAX_SCORE_BEATS
                            || measure_start + note_end > MAX_SCORE_BEATS
                        {
                            return Err(
                                "MusicXML note position or duration exceeds the project limit"
                                    .into(),
                            );
                        }
                        measure_extent = measure_extent.max(note_end);
                        if !chord {
                            last_note_start = note_start;
                        }
                        if child(element, "rest").is_none() {
                            let pitch =
                                child(element, "pitch")
                                    .and_then(pitch_from_node)
                                    .or_else(|| {
                                        child(element, "unpitched")
                                            .and_then(unpitched_display_pitch)
                                    });
                            let Some(pitch) = pitch else {
                                if !chord {
                                    cursor += length;
                                }
                                continue;
                            };
                            let voice_number = child_text(element, "voice")
                                .and_then(|value| value.parse::<u16>().ok())
                                .unwrap_or(1)
                                .clamp(1, 16);
                            let voice = (voice_number - 1) as u8;
                            let start_beat = measure_start + note_start;
                            let (tie_start, tie_stop) = tie_types(element);
                            let key = (pitch, voice);
                            let mut merged = false;
                            if tie_stop {
                                if let Some(index) = tied_notes.get(&key).copied() {
                                    let previous = &mut notes[index];
                                    if (previous.start_beat + previous.length_beats - start_beat)
                                        .abs()
                                        < 0.0001
                                    {
                                        previous.length_beats += length;
                                        if !tie_start {
                                            tied_notes.remove(&key);
                                        }
                                        merged = true;
                                    }
                                }
                            }
                            if !merged {
                                if notes.len() >= MAX_NOTES {
                                    return Err("MusicXML note count exceeds 1,000,000".into());
                                }
                                notes.push(MusicXmlNote {
                                    start_beat,
                                    length_beats: length,
                                    pitch,
                                    velocity: read_velocity(element),
                                    voice,
                                    lyric: read_note_text(element),
                                });
                                if tie_start {
                                    tied_notes.insert(key, notes.len() - 1);
                                }
                            }
                        }
                        if !chord {
                            cursor += length;
                        }
                    }
                    _ => {}
                }
            }
            let implicit = measure.attribute("implicit") == Some("yes");
            let meter_beats = numerator as f64 * 4.0 / denominator as f64;
            let measure_length = if implicit {
                measure_extent
            } else {
                meter_beats.max(measure_extent)
            };
            measure_start += measure_length;
            if measure_start > MAX_SCORE_BEATS {
                return Err("MusicXML score exceeds the project timeline limit".into());
            }
        }
        if part_index == 0 {
            score_time_signatures = part_time_signatures;
            score_tempo_events = part_tempo_events;
        }
        notes.sort_by(|a, b| {
            a.start_beat
                .total_cmp(&b.start_beat)
                .then(a.pitch.cmp(&b.pitch))
        });
        parts.push(MusicXmlPart {
            name: part_name,
            notes,
        });
    }
    if parts.iter().all(|part| part.notes.is_empty()) {
        return Err("MusicXML contains no pitched notes".into());
    }
    score_tempo_events.sort_by(|a, b| a.beat.total_cmp(&b.beat));
    score_tempo_events.dedup_by(|a, b| (a.beat - b.beat).abs() < 0.000001);
    Ok(MusicXmlScore {
        title,
        parts,
        time_signatures: score_time_signatures,
        tempo_events: score_tempo_events,
    })
}

pub fn read_musicxml(path: &Path) -> Result<MusicXmlScore, String> {
    let metadata =
        fs::metadata(path).map_err(|error| format!("could not inspect MusicXML: {error}"))?;
    if metadata.len() == 0 || metadata.len() > MAX_XML_BYTES as u64 {
        return Err("MusicXML must be between 1 byte and 64 MiB".into());
    }
    let xml =
        fs::read_to_string(path).map_err(|error| format!("could not read MusicXML: {error}"))?;
    parse_musicxml(&xml)
}

fn xml_escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn musicxml_type(beats: f64) -> (&'static str, usize) {
    const TYPES: &[(f64, &str, usize)] = &[
        (8.0, "breve", 0),
        (7.0, "whole", 2),
        (6.0, "whole", 1),
        (4.0, "whole", 0),
        (3.5, "half", 2),
        (3.0, "half", 1),
        (2.0, "half", 0),
        (1.75, "quarter", 2),
        (1.5, "quarter", 1),
        (1.0, "quarter", 0),
        (0.875, "eighth", 2),
        (0.75, "eighth", 1),
        (0.5, "eighth", 0),
        (0.4375, "16th", 2),
        (0.375, "16th", 1),
        (0.25, "16th", 0),
        (0.21875, "32nd", 2),
        (0.1875, "32nd", 1),
        (0.125, "32nd", 0),
        (0.109375, "64th", 2),
        (0.09375, "64th", 1),
        (0.0625, "64th", 0),
        (0.03125, "128th", 0),
    ];
    TYPES
        .iter()
        .find(|(value, _, _)| (beats - *value).abs() < 0.002)
        .map(|(_, kind, dots)| (*kind, *dots))
        .unwrap_or_else(|| {
            let kind = if beats >= 4.0 {
                "whole"
            } else if beats >= 2.0 {
                "half"
            } else if beats >= 1.0 {
                "quarter"
            } else if beats >= 0.5 {
                "eighth"
            } else if beats >= 0.25 {
                "16th"
            } else if beats >= 0.125 {
                "32nd"
            } else if beats >= 0.0625 {
                "64th"
            } else {
                "128th"
            };
            (kind, 0)
        })
}

#[derive(Clone, Debug, PartialEq)]
pub struct MusicXmlExportNote {
    pub start_beat: f64,
    pub length_beats: f64,
    pub pitch: u8,
    pub velocity: u8,
    pub voice: u8,
    pub lyric: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MusicXmlExportPart {
    pub name: String,
    pub notes: Vec<MusicXmlExportNote>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MusicXmlExportTempoEvent {
    pub beat: f64,
    pub bpm: f64,
}

pub fn export_musicxml(
    parts: &[MusicXmlExportPart],
    time_signatures: &[MusicXmlTimeSignature],
    tempo_events: &[MusicXmlExportTempoEvent],
    title: &str,
) -> Result<String, String> {
    if parts.is_empty() || parts.len() > MAX_PARTS || title.trim().is_empty() || title.len() > 256 {
        return Err("MusicXML export needs a title and 1–256 parts".into());
    }
    let signatures = if time_signatures.is_empty() {
        vec![MusicXmlTimeSignature {
            beat: 0.0,
            numerator: 4,
            denominator: 4,
        }]
    } else {
        time_signatures.to_vec()
    };
    let mut sorted_signatures = signatures;
    sorted_signatures.sort_by(|a, b| a.beat.total_cmp(&b.beat));
    if sorted_signatures.iter().any(|event| {
        !event.beat.is_finite()
            || event.beat < 0.0
            || event.beat > MAX_SCORE_BEATS
            || event.numerator == 0
            || event.numerator > 32
            || !matches!(event.denominator, 1 | 2 | 4 | 8 | 16 | 32)
    }) {
        return Err("MusicXML time-signature map is invalid".into());
    }
    let mut sorted_tempos = tempo_events.to_vec();
    sorted_tempos.sort_by(|a, b| a.beat.total_cmp(&b.beat));
    if sorted_tempos.len() > 100_000
        || sorted_tempos.iter().any(|event| {
            !event.beat.is_finite()
                || event.beat < 0.0
                || event.beat > MAX_SCORE_BEATS
                || !event.bpm.is_finite()
                || !(20.0..=300.0).contains(&event.bpm)
        })
    {
        return Err("MusicXML tempo map is invalid".into());
    }
    let mut max_end = 0.0f64;
    for part in parts {
        for note in &part.notes {
            if !note.start_beat.is_finite()
                || !note.length_beats.is_finite()
                || note.start_beat < 0.0
                || note.length_beats <= 0.0
                || note.pitch > 127
                || note.start_beat + note.length_beats > MAX_SCORE_BEATS
                || note.velocity == 0
                || note.velocity > 127
                || note.voice > 15
            {
                return Err("MusicXML contains an invalid note".into());
            }
            max_end = max_end.max(note.start_beat + note.length_beats);
        }
    }
    let mut bars = Vec::new();
    let mut beat = 0.0f64;
    let mut signature_index = 0;
    let mut active_num = 4u8;
    let mut active_den = 4u8;
    while beat < max_end.max(1.0) || bars.is_empty() {
        while signature_index < sorted_signatures.len()
            && sorted_signatures[signature_index].beat <= beat + 0.000001
        {
            active_num = sorted_signatures[signature_index].numerator;
            active_den = sorted_signatures[signature_index].denominator;
            signature_index += 1;
        }
        let length = active_num as f64 * 4.0 / active_den as f64;
        if length <= 0.0 || bars.len() >= 100_000 {
            return Err("MusicXML score is too long".into());
        }
        bars.push((beat, length, active_num, active_den));
        beat += length;
    }
    let mut xml = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<score-partwise version=\"3.1\"><work><work-title>{}</work-title></work><part-list>",
        xml_escape(title)
    );
    for (index, part) in parts.iter().enumerate() {
        xml.push_str(&format!(
            "<score-part id=\"P{}\"><part-name>{}</part-name></score-part>",
            index + 1,
            xml_escape(&part.name)
        ));
    }
    xml.push_str("</part-list>");
    const DIVISIONS: u64 = 480;
    for (part_index, part) in parts.iter().enumerate() {
        if part.notes.len() > MAX_NOTES {
            return Err("MusicXML note count exceeds 1,000,000".into());
        }
        xml.push_str(&format!("<part id=\"P{}\">", part_index + 1));
        let mut notes = part.notes.clone();
        notes.sort_by(|a, b| {
            a.start_beat
                .total_cmp(&b.start_beat)
                .then(a.pitch.cmp(&b.pitch))
        });
        for (bar_index, (bar_start, bar_length, numerator, denominator)) in bars.iter().enumerate()
        {
            xml.push_str(&format!("<measure number=\"{}\">", bar_index + 1));
            if bar_index == 0
                || sorted_signatures
                    .iter()
                    .any(|event| (event.beat - bar_start).abs() < 0.000001)
            {
                xml.push_str(&format!(
                    "<attributes><divisions>{DIVISIONS}</divisions><time><beats>{numerator}</beats><beat-type>{denominator}</beat-type></time><clef><sign>G</sign><line>2</line></clef></attributes>"
                ));
            }
            let bar_end = bar_start + bar_length;
            if part_index == 0 {
                for event in sorted_tempos.iter().filter(|event| {
                    event.beat >= *bar_start - 0.000001 && event.beat < bar_end - 0.000001
                }) {
                    let offset =
                        ((event.beat - bar_start).max(0.0) * DIVISIONS as f64).round() as i64;
                    xml.push_str(&format!(
                        "<direction><direction-type><words>Tempo</words></direction-type><offset>{offset}</offset><sound tempo=\"{:.3}\"/></direction>",
                        event.bpm
                    ));
                }
            }
            let mut segments = Vec::new();
            for note in &notes {
                let note_end = note.start_beat + note.length_beats;
                if note.start_beat >= bar_end || note_end <= *bar_start {
                    continue;
                }
                let start = note.start_beat.max(*bar_start);
                let end = note_end.min(bar_end);
                segments.push((
                    note,
                    start,
                    end - start,
                    note.start_beat < start,
                    note_end > end,
                ));
            }
            segments.sort_by(|a, b| a.1.total_cmp(&b.1).then(a.0.pitch.cmp(&b.0.pitch)));
            let mut cursor = 0u64;
            for (note, start, duration_beats, tie_stop, tie_start) in segments {
                let start_tick = ((start - bar_start) * DIVISIONS as f64).round() as u64;
                let duration = (duration_beats * DIVISIONS as f64).round().max(1.0) as u64;
                if start_tick < cursor {
                    xml.push_str(&format!(
                        "<backup><duration>{}</duration></backup>",
                        cursor - start_tick
                    ));
                } else if start_tick > cursor {
                    xml.push_str(&format!(
                        "<forward><duration>{}</duration></forward>",
                        start_tick - cursor
                    ));
                }
                let pitch_class = note.pitch % 12;
                let step = ["C", "C", "D", "D", "E", "F", "F", "G", "G", "A", "A", "B"]
                    [pitch_class as usize];
                let alter = match pitch_class {
                    1 | 3 | 6 | 8 | 10 => "<alter>1</alter>",
                    _ => "",
                };
                let octave = i16::from(note.pitch / 12) - 1;
                let tie_stop_xml = if tie_stop { "<tie type=\"stop\"/>" } else { "" };
                let (note_type, dot_count) = musicxml_type(duration_beats);
                let dots = "<dot/>".repeat(dot_count);
                let notations = if tie_stop || tie_start {
                    format!(
                        "<notations>{}{}</notations>",
                        if tie_stop {
                            "<tied type=\"stop\"/>"
                        } else {
                            ""
                        },
                        if tie_start {
                            "<tied type=\"start\"/>"
                        } else {
                            ""
                        }
                    )
                } else {
                    String::new()
                };
                let lyric = if !tie_stop && !note.lyric.is_empty() {
                    format!("<lyric><text>{}</text></lyric>", xml_escape(&note.lyric))
                } else {
                    String::new()
                };
                let voice = (note.voice + 1).max(1);
                xml.push_str(&format!(
                    "<note><pitch><step>{step}</step>{alter}<octave>{octave}</octave></pitch><duration>{duration}</duration>{tie_stop_xml}<voice>{voice}</voice><type>{note_type}</type>{dots}<play><other-play type=\"velocity\">{}</other-play></play>{notations}{lyric}</note>",
                    note.velocity,
                ));
                cursor = start_tick.saturating_add(duration);
            }
            xml.push_str("</measure>");
        }
        xml.push_str("</part>");
    }
    xml.push_str("</score-partwise>\n");
    if xml.len() > MAX_XML_BYTES {
        return Err("MusicXML export exceeds the 64 MiB limit".into());
    }
    Ok(xml)
}

pub fn write_musicxml(path: &Path, xml: &str) -> Result<(), String> {
    if path.as_os_str().is_empty() || xml.len() > MAX_XML_BYTES {
        return Err("MusicXML output path or document is invalid".into());
    }
    fs::write(path, xml).map_err(|error| format!("could not write MusicXML: {error}"))
}
