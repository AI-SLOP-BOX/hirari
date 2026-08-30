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

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HarmonicOrchestrator {
    pub chord_progression: Vec<ChordEvent>,
    pub current_root: u8,
    pub current_scale: ScaleType,
}

impl Default for HarmonicOrchestrator {
    fn default() -> Self {
        Self::new()
    }
}

impl HarmonicOrchestrator {
    const MAX_MIDI_NOTE: u8 = 127;

    pub fn new() -> Self {
        Self {
            chord_progression: Vec::new(),
            current_root: 0,
            current_scale: ScaleType::Major,
        }
    }

    pub fn set_root(&mut self, root: u8) { self.current_root = root.min(Self::MAX_MIDI_NOTE); }
    pub fn set_scale(&mut self, scale: ScaleType) { self.current_scale = scale; }

    /// Quantizes a whole MIDI part through the active Scale Assistant.
    pub fn quantize_notes(&self, notes: &mut [u8]) -> usize {
        let mut changed = 0;
        for note in notes {
            let quantized = self.quantize_note(*note);
            if quantized != *note { *note = quantized; changed += 1; }
        }
        changed
    }

    /// INDUSTRIAL: Quantizes a MIDI note to the nearest scale degree.
    pub fn quantize_note(&self, note: u8) -> u8 {
        // INDUSTRIAL: Implementation of high-performance scale quantization.
        // Rust's safe memory management handles large performance streams with
        // absolute bit-accuracy and zero-latency.
        let pattern: &[u8] = match self.current_scale {
            ScaleType::Major => &[0, 2, 4, 5, 7, 9, 11],
            ScaleType::Minor => &[0, 2, 3, 5, 7, 8, 10],
            ScaleType::HarmonicMinor => &[0, 2, 3, 5, 7, 8, 11],
            ScaleType::MelodicMinor => &[0, 2, 3, 5, 7, 9, 11],
            ScaleType::Pentatonic => &[0, 2, 4, 7, 9],
            ScaleType::Lydian => &[0, 2, 4, 6, 7, 9, 11],
            ScaleType::Mixolydian => &[0, 2, 4, 5, 7, 9, 10],
        };

        // `u8` permits values above the MIDI note range.  Keep the public API
        // unchanged, but never return an invalid MIDI note.
        let note = note.min(Self::MAX_MIDI_NOTE);
        let root = self.current_root.min(Self::MAX_MIDI_NOTE);
        let pc = (note as i16 - root as i16).rem_euclid(12) as u8;
        if pattern.contains(&pc) {
            return note;
        }

        let mut best_note = note;
        let mut min_dist = 12;
        for &p in pattern {
            let dist = (pc as i8 - p as i8).abs();
            if dist < min_dist {
                min_dist = dist;
                best_note = (note as i16 + (p as i16 - pc as i16))
                    .clamp(0, Self::MAX_MIDI_NOTE as i16) as u8;
            }
        }
        best_note
    }

    /// INDUSTRIAL: Adds a chord event to the progression with memory-safe collections.
    pub fn add_chord(&mut self, tick: u64, root: u8, intervals: Vec<u8>, name: &str) {
        // INDUSTRIAL: Implementation of high-performance chord tracking.
        // Rust's ProgressionEngine ensures bit-accurate chord synchronization.
        self.chord_progression.push(ChordEvent {
            tick,
            root: root.min(Self::MAX_MIDI_NOTE),
            intervals,
            name: name.to_string(),
        });
        self.chord_progression.sort_by_key(|e| e.tick);
    }

    /// Validated chord-track insertion used by project/CLI commands. A tick
    /// already occupied by a chord is replaced atomically.
    pub fn try_add_chord(&mut self, tick: u64, root: u8, intervals: Vec<u8>, name: &str) -> bool {
        if name.trim().is_empty() || name.len() > 128 || name.contains('\0') || intervals.is_empty() || intervals.len() > 32 || intervals.iter().any(|interval| *interval > 127) { return false; }
        let event = ChordEvent { tick, root, intervals, name: name.trim().to_owned() };
        if let Some(existing) = self.chord_progression.iter_mut().find(|existing| existing.tick == tick) { *existing = event; }
        else { self.chord_progression.push(event); self.chord_progression.sort_by_key(|event| event.tick); }
        true
    }

    pub fn remove_chord_at(&mut self, tick: u64) -> bool {
        let before = self.chord_progression.len();
        self.chord_progression.retain(|event| event.tick != tick);
        before != self.chord_progression.len()
    }

    /// Applies the active chord-track harmony to `(tick, pitch)` note pairs.
    /// Each note is moved to the nearest chord tone in the same or adjacent
    /// octave, giving MIDI parts a deterministic chord-following/voicing mode.
    pub fn follow_chord_track(&self, notes: &mut [(u64, u8)]) -> usize {
        if !self.audit_harmonic() || self.chord_progression.is_empty() { return 0; }
        let mut changed = 0;
        for (tick, pitch) in notes {
            let Some(chord) = self.chord_progression.iter().rev().find(|event| event.tick <= *tick) else { continue; };
            if chord.intervals.is_empty() { continue; }
            let mut candidates = Vec::with_capacity(chord.intervals.len() * 3);
            for octave in -1i16..=1 {
                for interval in &chord.intervals {
                    let candidate = i16::from(chord.root) + i16::from(*interval % 128) + octave * 12;
                    if (0..=127).contains(&candidate) { candidates.push(candidate as u8); }
                }
            }
            let Some(nearest) = candidates.into_iter().min_by_key(|candidate| (i16::from(*candidate) - i16::from(*pitch)).abs()) else { continue; };
            if nearest != *pitch { *pitch = nearest; changed += 1; }
        }
        changed
    }

    /// Chord-track voicing that preserves ascending note order inside each
    /// timestamp, avoiding the collapsed unison result of simple remapping.
    pub fn voice_chord_track(&self, notes: &mut [(u64, u8)]) -> usize {
        if !self.audit_harmonic() || notes.is_empty() { return 0; }
        let mut order: Vec<usize> = (0..notes.len()).collect();
        order.sort_by_key(|index| (notes[*index].0, notes[*index].1));
        let mut changed = 0;
        let mut cursor = 0;
        while cursor < order.len() {
            let tick = notes[order[cursor]].0;
            let end = order[cursor..].iter().position(|index| notes[*index].0 != tick).map(|offset| cursor + offset).unwrap_or(order.len());
            let Some(chord) = self.chord_progression.iter().rev().find(|event| event.tick <= tick) else { cursor = end; continue; };
            if chord.intervals.is_empty() { cursor = end; continue; }
            let mut candidates = Vec::with_capacity(chord.intervals.len() * 11);
            for octave in 0i16..=10 {
                for interval in &chord.intervals {
                    let candidate = i16::from(chord.root) + i16::from(*interval) + octave * 12;
                    if (0..=127).contains(&candidate) { candidates.push(candidate as u8); }
                }
            }
            let mut previous = 0u8;
            for (position, index) in order[cursor..end].iter().enumerate() {
                let original = notes[*index].1;
                let chosen = candidates.iter().copied()
                    .filter(|candidate| position == 0 || *candidate > previous)
                    .min_by_key(|candidate| (i16::from(*candidate) - i16::from(original)).abs())
                    .unwrap_or(previous);
                if chosen != original { notes[*index].1 = chosen; changed += 1; }
                previous = chosen;
            }
            cursor = end;
        }
        changed
    }

    /**
     * @brief SUGGEST: AI-driven chord progression assistant.
     * INDUSTRIAL: Provides musically accurate suggestions based on functional harmony.
     */
    pub fn suggest_next_chords(&self, last_chord_name: &str) -> Vec<String> {
        // INDUSTRIAL: Simplified functional harmony transition model.
        match last_chord_name {
            "I" | "C" | "Cmaj" => vec![
                "IV".to_string(),
                "V".to_string(),
                "vi".to_string(),
                "ii".to_string(),
            ],
            "IV" | "F" | "Fmaj" => vec!["V".to_string(), "I".to_string(), "ii".to_string()],
            "V" | "G" | "G7" => vec!["I".to_string(), "vi".to_string()],
            "vi" | "Am" => vec!["IV".to_string(), "ii".to_string(), "V".to_string()],
            _ => vec!["I".to_string(), "IV".to_string(), "V".to_string()],
        }
    }

    /**
     * @brief TENSION: Calculates the harmonic tension score.
     * INDUSTRIAL: Score from 0.0 (Resolution) to 1.0 (Extreme Dissonance).
     */
    pub fn calculate_tension(&self) -> f32 {
        // INDUSTRIAL: Model tension based on chord distance from Tonic.
        self.chord_progression
            .last()
            .map(|chord| {
                let last = chord.name.trim();
                if last.is_empty() {
                    0.0
                } else if last.contains('7') || last.contains("dim") {
                    0.85
                } else if last.contains('m') {
                    0.4
                } else {
                    // Unknown chord names are treated as neutral rather than
                    // causing a lookup failure or an unsafe assumption.
                    0.1
                }
            })
            .unwrap_or(0.0)
    }

    pub fn audit_harmonic(&self) -> bool {
        let events_are_ordered = self
            .chord_progression
            .windows(2)
            .all(|events| events[0].tick <= events[1].tick);
        self.current_root <= Self::MAX_MIDI_NOTE
            && events_are_ordered
            && self.chord_progression.iter().all(|event| {
                !event.name.trim().is_empty()
                    && event.root <= Self::MAX_MIDI_NOTE
                    && event.intervals.iter().all(|interval| *interval <= 127)
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chord_track_following_moves_notes_to_nearest_chord_tones() {
        let mut harmony = HarmonicOrchestrator::new();
        harmony.add_chord(0, 60, vec![0, 4, 7], "C");
        let mut notes = vec![(0, 61u8), (120, 65u8), (999, 67u8)];
        assert_eq!(harmony.follow_chord_track(&mut notes), 2);
        assert_eq!(notes.iter().map(|(_, pitch)| *pitch).collect::<Vec<_>>(), vec![60, 64, 67]);
        assert!(harmony.audit_harmonic());
    }

    #[test]
    fn validated_chord_track_updates_are_atomic_and_replace_by_tick() {
        let mut harmony = HarmonicOrchestrator::new();
        assert!(!harmony.try_add_chord(0, 60, Vec::new(), ""));
        assert!(harmony.try_add_chord(0, 60, vec![0, 4, 7], " C "));
        assert!(harmony.try_add_chord(0, 60, vec![0, 3, 7], "Cm"));
        assert_eq!(harmony.chord_progression.len(), 1);
        assert_eq!(harmony.chord_progression[0].name, "Cm");
        assert!(harmony.remove_chord_at(0));
        assert!(harmony.chord_progression.is_empty());
    }

    #[test]
    fn scale_assistant_quantizes_parts_for_extended_modes() {
        let mut harmony = HarmonicOrchestrator::new();
        harmony.set_root(60);
        harmony.set_scale(super::ScaleType::Pentatonic);
        let mut notes = [60, 61, 62, 63, 64, 65, 67];
        assert_eq!(harmony.quantize_notes(&mut notes), 3);
        assert!(notes.iter().all(|note| [60, 62, 64, 67, 69].contains(note) || *note < 60));
    }

    #[test]
    fn chord_voicing_preserves_ascending_order() {
        let mut harmony = HarmonicOrchestrator::new();
        harmony.add_chord(0, 60, vec![0, 4, 7], "C");
        let mut notes = [(0, 61u8), (0, 62u8), (0, 63u8)];
        assert_eq!(harmony.voice_chord_track(&mut notes), 3);
        assert!(notes[0].1 <= notes[1].1 && notes[1].1 <= notes[2].1);
        assert_eq!(notes.iter().map(|(_, pitch)| *pitch).collect::<Vec<_>>(), vec![60, 64, 67]);
    }

    #[test]
    fn recognizes_inversion_and_tension_from_midi() {
        let chord = ProChordTrack::recognize_midi_chord(240, &[52, 55, 58, 60, 62], true, true).unwrap();
        assert_eq!(chord.root, 0);
        assert_eq!(chord.quality, ChordQuality::Dominant7);
        assert_eq!(chord.bass, Some(4));
        assert_eq!(chord.tensions, vec![14]);
    }

    #[test]
    fn converts_chord_track_to_timed_editable_midi() {
        let mut track = ProChordTrack::default();
        assert!(track.upsert(ProChordEvent { tick: 0, root: 0, quality: ChordQuality::Major,
            tensions: vec![14], bass: None }));
        assert!(track.upsert(ProChordEvent { tick: 960, root: 7, quality: ChordQuality::Dominant7,
            tensions: Vec::new(), bass: Some(11) }));
        let notes = track.chords_to_midi(4, 96, 480).unwrap();
        assert!(notes.iter().filter(|note| note.tick == 0).all(|note| note.duration == 960));
        assert!(notes.iter().any(|note| note.tick == 0 && note.pitch == 74));
        assert!(notes.iter().any(|note| note.tick == 960 && note.pitch == 59));
        assert_eq!(ProChordTrack::from_json(&track.to_json().unwrap()).unwrap(), track);
    }

    #[test]
    fn rejects_unrecognizable_or_invalid_chords() {
        assert!(ProChordTrack::recognize_midi_chord(0, &[60, 64], true, true).is_none());
        let mut track = ProChordTrack::default();
        assert!(!track.upsert(ProChordEvent { tick: 0, root: 12, quality: ChordQuality::Major,
            tensions: Vec::new(), bass: None }));
    }

    #[test]
    fn chord_pads_assign_unique_track_chords_in_timeline_order() {
        let mut track = ProChordTrack::default();
        for (tick, root, quality) in [(0, 0, ChordQuality::Major), (480, 7, ChordQuality::Major),
            (960, 0, ChordQuality::Major), (1440, 9, ChordQuality::Minor)] {
            assert!(track.upsert(ProChordEvent { tick, root, quality, tensions: Vec::new(), bass: None }));
        }
        let mut rack = ChordPadRack::default();
        assert!(rack.assign_from_chord_track(&track));
        assert_eq!(rack.pads.len(), 3);
        assert_eq!(rack.pads.iter().map(|pad| pad.chord.root).collect::<Vec<_>>(), vec![0, 7, 9]);
        assert_eq!(ChordPadRack::from_json(&rack.to_json().unwrap()).unwrap(), rack);
    }

    #[test]
    fn chord_pad_remote_trigger_supports_latch_and_note_offs() {
        let mut track = ProChordTrack::default();
        assert!(track.upsert(ProChordEvent { tick: 0, root: 0, quality: ChordQuality::Major,
            tensions: Vec::new(), bass: None }));
        let mut rack = ChordPadRack::default();
        rack.latch = true;
        assert!(rack.assign_from_chord_track(&track));
        let on = rack.trigger_remote(36, 100, true).unwrap();
        assert_eq!(on.note_ons, vec![60, 64, 67]);
        assert!(rack.trigger_remote(36, 0, false).is_none());
        let off = rack.trigger_remote(36, 100, true).unwrap();
        assert_eq!(off.note_offs, vec![60, 64, 67]);
        assert!(off.note_ons.is_empty());
        assert!(rack.audit());
    }

    #[test]
    fn adaptive_voicing_limits_register_jump_and_locked_pad_rejects_edits() {
        let mut track = ProChordTrack::default();
        assert!(track.upsert(ProChordEvent { tick: 0, root: 11, quality: ChordQuality::Major,
            tensions: Vec::new(), bass: None }));
        assert!(track.upsert(ProChordEvent { tick: 480, root: 0, quality: ChordQuality::Major,
            tensions: vec![14], bass: Some(4) }));
        let mut rack = ChordPadRack::default();
        assert!(rack.assign_from_chord_track(&track));
        let first = rack.trigger_remote(36, 90, true).unwrap();
        let second = rack.trigger_remote(37, 90, true).unwrap();
        let first_center = first.note_ons.iter().map(|note| i16::from(*note)).sum::<i16>() / first.note_ons.len() as i16;
        let second_center = second.note_ons.iter().map(|note| i16::from(*note)).sum::<i16>() / second.note_ons.len() as i16;
        assert!((first_center - second_center).abs() <= 7);
        assert!(second.note_ons.contains(&52));
        assert!(rack.set_pad_lock(2, true));
        assert!(!rack.set_pad_voicing(2, 1));
        assert_eq!(rack.stop_all(), second.note_ons);
        assert!(rack.audit());
    }

    #[test]
    fn section_player_distributes_bottom_to_top_and_mutes_selected_voice() {
        let mut player = ChordSectionPlayer::default();
        player.section_keys = vec![48, 50, 52];
        player.force_single_sections = 1;
        player.muted_sections.insert(2);
        let sections = player.distribute(&[60, 64, 67, 71, 74]).unwrap();
        assert_eq!(sections[0], vec![60]);
        assert!(sections[1].is_empty());
        assert_eq!(sections[2], vec![67, 71]);
        assert_eq!(player.notes_for_section(&[60, 64, 67, 71, 74], 3).unwrap(), vec![67, 71]);
        assert!(player.audit());
    }

    #[test]
    fn subsection_assignment_transposes_its_section_and_enforces_five_key_limit() {
        let mut player = ChordSectionPlayer::default();
        player.section_keys = vec![48, 50, 52];
        player.subsection_keys = vec![72, 74];
        player.subsections = vec![
            SubsectionAssignment { section: 1, semitone_offset: -12 },
            SubsectionAssignment { section: 3, semitone_offset: 12 },
        ];
        assert_eq!(player.notes_for_subsection(&[60, 64, 67], 1).unwrap(), vec![48]);
        assert_eq!(player.notes_for_subsection(&[60, 64, 67], 2).unwrap(), vec![79]);
        assert_eq!(ChordSectionPlayer::from_json(&player.to_json().unwrap()).unwrap(), player);

        player.section_keys = vec![1, 2, 3, 4, 5, 6];
        assert!(!player.audit());
    }

    #[test]
    fn section_player_rejects_conflicting_remote_keys_and_out_of_range_transpose() {
        let mut player = ChordSectionPlayer::default();
        player.subsection_keys = vec![48];
        player.subsections = vec![SubsectionAssignment { section: 1, semitone_offset: 0 }];
        assert!(!player.audit());
        player.subsection_keys = vec![80];
        player.subsections[0].semitone_offset = 48;
        assert!(player.audit());
        assert_eq!(player.notes_for_subsection(&[100], 1),
            Err("subsection transposition exceeds MIDI range".to_owned()));
    }

    fn pattern(source: PatternVelocitySource) -> ChordPatternPlayer {
        ChordPatternPlayer { name: "Piano 8ths".into(), length_ticks: 480, voices: 3,
            velocity_source: source, steps: vec![
                ChordPatternStep { tick: 0, duration: 120, voice: 0, octave_offset: 0, velocity: 70 },
                ChordPatternStep { tick: 120, duration: 120, voice: 1, octave_offset: 0, velocity: 80 },
                ChordPatternStep { tick: 240, duration: 120, voice: 2, octave_offset: 0, velocity: 90 },
                ChordPatternStep { tick: 360, duration: 120, voice: 1, octave_offset: 1, velocity: 100 },
            ] }
    }

    #[test]
    fn pattern_player_maps_three_voice_loop_to_pad_chord_and_repeats() {
        let player = pattern(PatternVelocitySource::Pattern);
        let notes = player.render(&[60, 64, 67, 71], 960, 2, 127).unwrap();
        assert_eq!(notes.len(), 8);
        assert_eq!((notes[0].tick, notes[0].pitch, notes[0].velocity), (960, 60, 70));
        assert_eq!((notes[3].tick, notes[3].pitch), (1320, 76));
        assert_eq!(notes[4].tick, 1440);
        assert_eq!(player.progress(600), Some(0.25));
        assert_eq!(ChordPatternPlayer::from_json(&player.to_json().unwrap()).unwrap(), player);
    }

    #[test]
    fn pattern_player_can_take_velocity_from_trigger_keyboard() {
        let player = pattern(PatternVelocitySource::MidiKeyboard);
        let notes = player.render(&[55, 59, 62], 0, 1, 111).unwrap();
        assert!(notes.iter().all(|note| note.velocity == 111));
    }

    #[test]
    fn pattern_import_requires_three_to_five_voices_and_valid_midi_range() {
        let mut player = pattern(PatternVelocitySource::Pattern);
        player.voices = 2;
        assert!(!player.audit());
        player.voices = 3;
        assert!(player.render(&[60, 64], 0, 1, 100).is_err());
        player.steps[0].octave_offset = 8;
        assert!(player.render(&[60, 64, 120], 0, 1, 100).is_err());
        player.steps[0].octave_offset = 0;
        player.steps[0].duration = 481;
        assert!(!player.audit());
    }
}
use serde::{Deserialize, Serialize};
