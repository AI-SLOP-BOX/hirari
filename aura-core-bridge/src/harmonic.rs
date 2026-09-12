use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ScaleType {
    Major,
    Minor,
    HarmonicMinor,
    MelodicMinor,
    Pentatonic,
    Lydian,
    Mixolydian,
}

pub struct Scale {
    pub name: String,
    pub intervals: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ChordEvent {
    pub tick: u64,
    pub root: u8,
    pub intervals: Vec<u8>,
    pub name: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChordQuality { Major, Minor, Diminished, Augmented, Sus2, Sus4, Dominant7, Major7, Minor7 }

impl ChordQuality {
    fn intervals(self) -> &'static [u8] {
        match self {
            Self::Major => &[0, 4, 7], Self::Minor => &[0, 3, 7], Self::Diminished => &[0, 3, 6],
            Self::Augmented => &[0, 4, 8], Self::Sus2 => &[0, 2, 7], Self::Sus4 => &[0, 5, 7],
            Self::Dominant7 => &[0, 4, 7, 10], Self::Major7 => &[0, 4, 7, 11], Self::Minor7 => &[0, 3, 7, 10],
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProChordEvent {
    pub tick: u64,
    pub root: u8,
    pub quality: ChordQuality,
    /// Additional intervals from the root, for example 14 for a ninth.
    pub tensions: Vec<u8>,
    /// Pitch class for slash chords; `None` links bass to the root.
    pub bass: Option<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChordMidiNote { pub tick: u64, pub duration: u64, pub pitch: u8, pub velocity: u8 }

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProChordTrack { pub events: Vec<ProChordEvent> }

impl ProChordTrack {
    pub fn to_json(&self) -> Result<String, String> { if !self.audit() { return Err("invalid chord track".into()); } serde_json::to_string(self).map_err(|error| error.to_string()) }
    pub fn from_json(json: &str) -> Result<Self, String> { let track: Self = serde_json::from_str(json).map_err(|error| error.to_string())?; if track.audit() { Ok(track) } else { Err("invalid chord track".into()) } }

    pub fn upsert(&mut self, mut event: ProChordEvent) -> bool {
        event.tensions.sort_unstable(); event.tensions.dedup();
        if !event.validate() { return false; }
        if let Some(existing) = self.events.iter_mut().find(|existing| existing.tick == event.tick) { *existing = event; }
        else { self.events.push(event); self.events.sort_by_key(|event| event.tick); }
        true
    }

    /// Render chord events as editable MIDI notes. Event length extends to the
    /// next chord, while the last chord uses `last_duration`.
    pub fn chords_to_midi(&self, octave: u8, velocity: u8, last_duration: u64) -> Result<Vec<ChordMidiNote>, String> {
        if !self.audit() || octave > 9 || !(1..=127).contains(&velocity) || last_duration == 0 { return Err("invalid chord-to-MIDI settings".into()); }
        let mut notes = Vec::new();
        for (index, event) in self.events.iter().enumerate() {
            let duration = self.events.get(index + 1).map(|next| next.tick - event.tick).unwrap_or(last_duration);
            let base = 12u16 * u16::from(octave + 1) + u16::from(event.root);
            let mut pitches = event.quality.intervals().iter().chain(&event.tensions)
                .filter_map(|interval| u8::try_from(base + u16::from(*interval)).ok()).filter(|pitch| *pitch <= 127).collect::<Vec<_>>();
            if let Some(bass) = event.bass.filter(|bass| *bass != event.root) {
                let bass_pitch = 12u16 * u16::from(octave) + u16::from(bass);
                if bass_pitch <= 127 { pitches.push(bass_pitch as u8); }
            }
            pitches.sort_unstable(); pitches.dedup();
            notes.extend(pitches.into_iter().map(|pitch| ChordMidiNote { tick: event.tick, duration, pitch, velocity }));
        }
        Ok(notes)
    }

    /// Recognize a western equal-tempered chord from simultaneous MIDI notes.
    pub fn recognize_midi_chord(tick: u64, pitches: &[u8], include_bass: bool, include_tensions: bool) -> Option<ProChordEvent> {
        if pitches.len() < 3 || pitches.iter().any(|pitch| *pitch > 127) { return None; }
        let bass = *pitches.iter().min()? % 12;
        let mut pcs = pitches.iter().map(|pitch| *pitch % 12).collect::<Vec<_>>(); pcs.sort_unstable(); pcs.dedup();
        let qualities = [ChordQuality::Dominant7, ChordQuality::Major7, ChordQuality::Minor7, ChordQuality::Major,
            ChordQuality::Minor, ChordQuality::Diminished, ChordQuality::Augmented, ChordQuality::Sus2, ChordQuality::Sus4];
        let mut matches = Vec::new();
        for root in 0..12u8 {
            for quality in qualities {
                let core = quality.intervals().iter().map(|interval| (root + interval) % 12).collect::<Vec<_>>();
                if core.iter().all(|pitch| pcs.contains(pitch)) {
                    let extras = pcs.iter().filter(|pitch| !core.contains(pitch)).map(|pitch| 12 + (12 + *pitch - root) % 12).collect::<Vec<_>>();
                    matches.push((extras.len(), usize::MAX - core.len(), root != bass, root, quality, extras));
                }
            }
        }
        matches.sort_by_key(|candidate| (candidate.0, candidate.1, candidate.2, candidate.3));
        let (_, _, _, root, quality, extras) = matches.into_iter().next()?;
        Some(ProChordEvent { tick, root, quality, tensions: if include_tensions { extras } else { Vec::new() }, bass: (include_bass && bass != root).then_some(bass) })
    }

    pub fn audit(&self) -> bool { self.events.len() <= 100_000 && self.events.iter().all(ProChordEvent::validate) && self.events.windows(2).all(|pair| pair[0].tick < pair[1].tick) }
}

impl ProChordEvent {
    fn validate(&self) -> bool {
        self.root < 12 && self.bass.is_none_or(|bass| bass < 12) && self.tensions.len() <= 16
            && self.tensions.iter().all(|interval| (1..=48).contains(interval) && !self.quality.intervals().contains(interval))
            && self.tensions.windows(2).all(|pair| pair[0] < pair[1])
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChordPad {
    pub id: u8,
    pub chord: ProChordEvent,
    pub voicing: i8,
    pub adaptive_voicing: bool,
    pub locked: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChordPadPlayback {
    pub pad_id: u8,
    pub note_ons: Vec<u8>,
    pub note_offs: Vec<u8>,
    pub velocity: u8,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChordPadRack {
    pub pads: Vec<ChordPad>,
    pub remote_start: u8,
    pub remote_end: u8,
    pub latch: bool,
    pub output_octave: u8,
    #[serde(skip)]
    active_pad: Option<u8>,
    #[serde(skip)]
    active_notes: Vec<u8>,
    #[serde(skip)]
    voicing_reference: Vec<u8>,
}

impl Default for ChordPadRack {
    fn default() -> Self {
        Self { pads: Vec::new(), remote_start: 36, remote_end: 47, latch: false,
            output_octave: 4, active_pad: None, active_notes: Vec::new(), voicing_reference: Vec::new() }
    }
}

impl ChordPadRack {
    pub fn to_json(&self) -> Result<String, String> {
        if !self.audit() { return Err("invalid chord pad rack".into()); }
        serde_json::to_string(self).map_err(|error| error.to_string())
    }

    pub fn from_json(json: &str) -> Result<Self, String> {
        let rack: Self = serde_json::from_str(json).map_err(|error| error.to_string())?;
        if rack.audit() { Ok(rack) } else { Err("invalid chord pad rack".into()) }
    }

    pub fn set_remote_range(&mut self, start: u8, end: u8) -> bool {
        if start > end || end > 127 || usize::from(end - start) + 1 > 128 { return false; }
        self.remote_start = start;
        self.remote_end = end;
        self.stop_all();
        true
    }

    /// Cubase assigns each distinct chord-track event once, in timeline order.
    pub fn assign_from_chord_track(&mut self, track: &ProChordTrack) -> bool {
        if !track.audit() { return false; }
        let capacity = usize::from(self.remote_end - self.remote_start) + 1;
        let mut unique: Vec<ProChordEvent> = Vec::new();
        for event in &track.events {
            if !unique.iter().any(|chord| same_chord(chord, event)) { unique.push(event.clone()); }
            if unique.len() == capacity { break; }
        }
        self.pads = unique.into_iter().enumerate().map(|(index, chord)| ChordPad {
            id: index as u8 + 1, chord, voicing: 0, adaptive_voicing: true, locked: false,
        }).collect();
        self.stop_all();
        true
    }

    pub fn set_pad_voicing(&mut self, pad_id: u8, voicing: i8) -> bool {
        if !(-8..=8).contains(&voicing) { return false; }
        let Some(pad) = self.pads.iter_mut().find(|pad| pad.id == pad_id) else { return false; };
        if pad.locked { return false; }
        pad.voicing = voicing;
        pad.adaptive_voicing = false;
        true
    }

    pub fn set_pad_lock(&mut self, pad_id: u8, locked: bool) -> bool {
        let Some(pad) = self.pads.iter_mut().find(|pad| pad.id == pad_id) else { return false; };
        pad.locked = locked;
        if locked { pad.adaptive_voicing = false; }
        true
    }

    pub fn set_adaptive_voicing(&mut self, enabled: bool) {
        for pad in &mut self.pads {
            if !pad.locked { pad.adaptive_voicing = enabled; }
        }
    }

    pub fn trigger_remote(&mut self, note: u8, velocity: u8, pressed: bool) -> Option<ChordPadPlayback> {
        if note < self.remote_start || note > self.remote_end || velocity > 127 { return None; }
        let pad_id = note - self.remote_start + 1;
        let pad = self.pads.iter().find(|pad| pad.id == pad_id)?.clone();
        if !pressed {
            if self.latch || self.active_pad != Some(pad_id) { return None; }
            let note_offs = std::mem::take(&mut self.active_notes);
            self.active_pad = None;
            return Some(ChordPadPlayback { pad_id, note_ons: Vec::new(), note_offs, velocity: 0 });
        }
        if self.latch && self.active_pad == Some(pad_id) {
            let note_offs = std::mem::take(&mut self.active_notes);
            self.active_pad = None;
            return Some(ChordPadPlayback { pad_id, note_ons: Vec::new(), note_offs, velocity: 0 });
        }
        let note_offs = std::mem::take(&mut self.active_notes);
        let note_ons = self.render_pad(&pad);
        self.voicing_reference = note_ons.clone();
        self.active_notes = note_ons.clone();
        self.active_pad = Some(pad_id);
        Some(ChordPadPlayback { pad_id, note_ons, note_offs, velocity })
    }

    pub fn stop_all(&mut self) -> Vec<u8> {
        self.active_pad = None;
        self.voicing_reference.clear();
        std::mem::take(&mut self.active_notes)
    }

    fn render_pad(&self, pad: &ChordPad) -> Vec<u8> {
        let base = 12i16 * i16::from(self.output_octave + 1) + i16::from(pad.chord.root);
        let mut intervals = pad.chord.quality.intervals().iter().copied().chain(pad.chord.tensions.iter().copied()).collect::<Vec<_>>();
        intervals.sort_unstable(); intervals.dedup();
        let len = intervals.len();
        if len == 0 { return Vec::new(); }
        let inversion = pad.voicing.rem_euclid(len as i8) as usize;
        let octave_shift = i16::from(pad.voicing.div_euclid(len as i8)) * 12;
        let mut pitches = (0..len).map(|index| {
            let source = (index + inversion) % len;
            base + i16::from(intervals[source]) + if source < inversion { 12 } else { 0 } + octave_shift
        }).collect::<Vec<_>>();
        if pad.adaptive_voicing && !self.voicing_reference.is_empty() {
            let center = self.voicing_reference.iter().map(|pitch| i32::from(*pitch)).sum::<i32>()
                / self.voicing_reference.len() as i32;
            let current = pitches.iter().map(|pitch| i32::from(*pitch)).sum::<i32>() / pitches.len() as i32;
            let shift = ((center - current) as f32 / 12.0).round() as i16 * 12;
            pitches.iter_mut().for_each(|pitch| *pitch += shift);
        }
        if let Some(bass) = pad.chord.bass.filter(|bass| *bass != pad.chord.root) {
            pitches.push(base - 12 + i16::from(bass));
        }
        let mut rendered = pitches.into_iter().filter_map(|pitch| u8::try_from(pitch).ok())
            .filter(|pitch| *pitch <= 127).collect::<Vec<_>>();
        rendered.sort_unstable(); rendered.dedup(); rendered
    }

    pub fn audit(&self) -> bool {
        self.remote_start <= self.remote_end && self.remote_end <= 127 && self.output_octave <= 9
            && self.pads.len() <= usize::from(self.remote_end - self.remote_start) + 1
            && self.pads.iter().enumerate().all(|(index, pad)| pad.id as usize == index + 1
                && pad.chord.validate() && (-8..=8).contains(&pad.voicing)
                && (!pad.locked || !pad.adaptive_voicing))
            && self.active_notes.iter().all(|note| *note <= 127) && self.voicing_reference.iter().all(|note| *note <= 127)
            && match self.active_pad {
                None => self.active_notes.is_empty(),
                Some(id) => !self.active_notes.is_empty() && self.pads.iter().any(|pad| pad.id == id),
            }
    }
}

fn same_chord(left: &ProChordEvent, right: &ProChordEvent) -> bool {
    left.root == right.root && left.quality == right.quality && left.tensions == right.tensions && left.bass == right.bass
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum SectionPlayMode { ChordPads, Sections, Combination }

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum AdditionalNoteStart { FirstSection, LastSection }

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SubsectionAssignment {
    /// One-based section number, or zero for no assignment.
    pub section: u8,
    pub semitone_offset: i8,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChordSectionPlayer {
    pub mode: SectionPlayMode,
    pub latch_chord_pads: bool,
    pub section_keys: Vec<u8>,
    pub subsection_keys: Vec<u8>,
    pub distribute_from: AdditionalNoteStart,
    /// Number of lowest sections that must contain exactly one note.
    pub force_single_sections: u8,
    pub muted_sections: std::collections::BTreeSet<u8>,
    pub subsections: Vec<SubsectionAssignment>,
}

impl Default for ChordSectionPlayer {
    fn default() -> Self {
        Self { mode: SectionPlayMode::Sections, latch_chord_pads: true,
            section_keys: vec![48, 50, 52, 53, 55], subsection_keys: Vec::new(),
            distribute_from: AdditionalNoteStart::LastSection, force_single_sections: 0,
            muted_sections: std::collections::BTreeSet::new(), subsections: Vec::new() }
    }
}

impl ChordSectionPlayer {
    pub fn to_json(&self) -> Result<String, String> {
        if !self.audit() { return Err("invalid chord section player".into()); }
        serde_json::to_string(self).map_err(|error| error.to_string())
    }

    pub fn from_json(json: &str) -> Result<Self, String> {
        let player: Self = serde_json::from_str(json).map_err(|error| error.to_string())?;
        if player.audit() { Ok(player) } else { Err("invalid chord section player".into()) }
    }

    /// Distributes pitches bottom-to-top. Additional pitches wrap through the
    /// available non-forced sections starting at the selected edge.
    pub fn distribute(&self, chord_notes: &[u8]) -> Result<Vec<Vec<u8>>, String> {
        if !self.audit() || chord_notes.is_empty() || chord_notes.iter().any(|note| *note > 127) {
            return Err("invalid chord section input".into());
        }
        let mut notes = chord_notes.to_vec(); notes.sort_unstable(); notes.dedup();
        let count = self.section_keys.len();
        let mut sections = vec![Vec::new(); count];
        let baseline = count.min(notes.len());
        for index in 0..baseline { sections[index].push(notes[index]); }
        let remaining = &notes[baseline..];
        let forced = usize::from(self.force_single_sections).min(count);
        let distributable = count.saturating_sub(forced);
        if distributable == 0 && !remaining.is_empty() { return Err("forced single sections cannot hold chord".into()); }
        for (index, note) in remaining.iter().enumerate() {
            let slot = match self.distribute_from {
                AdditionalNoteStart::FirstSection => forced + index % distributable,
                AdditionalNoteStart::LastSection => count - 1 - index % distributable,
            };
            sections[slot].push(*note);
        }
        for section in &mut sections { section.sort_unstable(); }
        for muted in &self.muted_sections { sections[usize::from(*muted - 1)].clear(); }
        Ok(sections)
    }

    pub fn notes_for_section(&self, chord_notes: &[u8], section: u8) -> Result<Vec<u8>, String> {
        if section == 0 || usize::from(section) > self.section_keys.len() { return Err("section is unavailable".into()); }
        let mut sections = self.distribute(chord_notes)?;
        Ok(sections.remove(usize::from(section - 1)))
    }

    pub fn notes_for_subsection(&self, chord_notes: &[u8], subsection: u8) -> Result<Vec<u8>, String> {
        if subsection == 0 { return Err("subsection is unavailable".into()); }
        let assignment = *self.subsections.get(usize::from(subsection - 1)).ok_or("subsection is unavailable")?;
        if assignment.section == 0 { return Ok(Vec::new()); }
        let notes = self.notes_for_section(chord_notes, assignment.section)?;
        notes.into_iter().map(|note| {
            let shifted = i16::from(note) + i16::from(assignment.semitone_offset);
            if !(0..=127).contains(&shifted) { return Err("subsection transposition exceeds MIDI range".to_owned()); }
            Ok(shifted as u8)
        }).collect()
    }

    pub fn audit(&self) -> bool {
        !self.section_keys.is_empty() && self.section_keys.len() <= 5 && self.subsection_keys.len() <= 5
            && usize::from(self.force_single_sections) <= self.section_keys.len()
            && unique_midi_keys(&self.section_keys) && unique_midi_keys(&self.subsection_keys)
            && self.section_keys.iter().all(|key| !self.subsection_keys.contains(key))
            && self.muted_sections.iter().all(|section| *section > 0 && usize::from(*section) <= self.section_keys.len())
            && self.subsections.len() == self.subsection_keys.len()
            && self.subsections.iter().all(|item| usize::from(item.section) <= self.section_keys.len()
                && (-48..=48).contains(&item.semitone_offset))
    }
}

fn unique_midi_keys(keys: &[u8]) -> bool {
    keys.iter().enumerate().all(|(index, key)| *key <= 127 && !keys[..index].contains(key))
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum PatternVelocitySource { Pattern, MidiKeyboard }

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChordPatternStep {
    pub tick: u64,
    pub duration: u64,
    /// Zero-based voice index, ordered from the lowest voice upwards.
    pub voice: u8,
    pub octave_offset: i8,
    pub velocity: u8,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChordPatternPlayer {
    pub name: String,
    pub length_ticks: u64,
    pub voices: u8,
    pub velocity_source: PatternVelocitySource,
    pub steps: Vec<ChordPatternStep>,
}

impl ChordPatternPlayer {
    pub fn to_json(&self) -> Result<String, String> {
        if !self.audit() { return Err("invalid chord pattern".into()); }
        serde_json::to_string(self).map_err(|error| error.to_string())
    }

    pub fn from_json(json: &str) -> Result<Self, String> {
        let pattern: Self = serde_json::from_str(json).map_err(|error| error.to_string())?;
        if pattern.audit() { Ok(pattern) } else { Err("invalid chord pattern".into()) }
    }

    /// Renders a MIDI-loop pattern with the pitches of the triggered pad.
    pub fn render(&self, chord_notes: &[u8], start_tick: u64, cycles: u16,
        trigger_velocity: u8) -> Result<Vec<ChordMidiNote>, String> {
        if !self.audit() || cycles == 0 || cycles > 1024 || trigger_velocity > 127
            || chord_notes.len() < usize::from(self.voices) || chord_notes.iter().any(|note| *note > 127) {
            return Err("invalid chord pattern render".into());
        }
        let mut pitches = chord_notes.to_vec(); pitches.sort_unstable(); pitches.dedup();
        if pitches.len() < usize::from(self.voices) { return Err("chord has too few distinct voices".into()); }
        let mut output = Vec::with_capacity(self.steps.len().saturating_mul(usize::from(cycles)));
        for cycle in 0..cycles {
            let cycle_tick = u64::from(cycle).checked_mul(self.length_ticks)
                .and_then(|offset| start_tick.checked_add(offset)).ok_or("pattern tick overflow")?;
            for step in &self.steps {
                let tick = cycle_tick.checked_add(step.tick).ok_or("pattern tick overflow")?;
                let pitch = i16::from(pitches[usize::from(step.voice)]) + i16::from(step.octave_offset) * 12;
                if !(0..=127).contains(&pitch) { return Err("pattern pitch exceeds MIDI range".into()); }
                let velocity = match self.velocity_source {
                    PatternVelocitySource::Pattern => step.velocity,
                    PatternVelocitySource::MidiKeyboard => trigger_velocity,
                };
                output.push(ChordMidiNote { tick, duration: step.duration, pitch: pitch as u8, velocity });
            }
        }
        output.sort_by_key(|note| (note.tick, note.pitch));
        Ok(output)
    }

    pub fn progress(&self, elapsed_ticks: u64) -> Option<f32> {
        self.audit().then_some((elapsed_ticks % self.length_ticks) as f32 / self.length_ticks as f32)
    }

    pub fn audit(&self) -> bool {
        !self.name.trim().is_empty() && self.name.len() <= 128 && !self.name.contains('\0')
            && self.length_ticks > 0 && (3..=5).contains(&self.voices)
            && !self.steps.is_empty() && self.steps.len() <= 1_000_000
            && self.steps.iter().all(|step| step.tick < self.length_ticks && step.duration > 0
                && step.tick.checked_add(step.duration).is_some_and(|end| end <= self.length_ticks)
                && step.voice < self.voices && (-8..=8).contains(&step.octave_offset)
                && (1..=127).contains(&step.velocity))
    }
}

include!("harmonic_orchestrator.rs");

#[cfg(test)]
#[path = "harmonic_tests.rs"]
mod harmonic_tests;
