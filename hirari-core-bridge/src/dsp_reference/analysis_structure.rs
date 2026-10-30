//! Frozen arrangement-section oracle (bar-block segmentation policy).

#[no_mangle]
pub unsafe extern "C" fn hirari_analysis_structure_reference(
    _region_starts: *const u64,
    _region_count: usize,
    total_length: u64,
    bpm: f64,
    sample_rate: f64,
    tension: f32,
    valence: f32,
    starts: *mut u64,
    ends: *mut u64,
    kinds: *mut u8,
    energies: *mut f32,
    motivic_ids: *mut u32,
    flows: *mut f32,
    capacity: usize,
) -> usize {
    #[derive(Clone, Copy)]
    struct Section {
        start: u64,
        end: u64,
        kind: u8,
        energy: f32,
        motivic: u32,
        flow: f32,
    }

    let samples_per_beat = (60.0 / bpm) * sample_rate;
    let bar_block = (samples_per_beat * 4.0 * 8.0) as u64;
    if bar_block == 0 {
        return 0;
    }
    let mut sections: Vec<Section> = Vec::new();
    let mut t = 0u64;
    while t < total_length {
        let energy = tension;
        let kind = if energy > 0.8 {
            2
        } else if energy < 0.2 {
            1
        } else {
            3
        };
        sections.push(Section {
            start: t,
            end: (t + bar_block).min(total_length),
            kind,
            energy,
            motivic: (energy * 1000.0) as u32,
            flow: tension * valence,
        });
        t += bar_block;
    }
    sections.sort_by(|a, b| a.start.cmp(&b.start));
    for index in 1..sections.len() {
        let previous = sections[index - 1].energy;
        if (sections[index].energy - previous).abs() > 0.5 {
            sections[index].energy = previous + (sections[index].energy - previous) * 0.5;
        }
    }
    if sections.len() > capacity
        || starts.is_null()
        || ends.is_null()
        || kinds.is_null()
        || energies.is_null()
        || motivic_ids.is_null()
        || flows.is_null()
    {
        return 0;
    }
    for (index, section) in sections.iter().enumerate() {
        *starts.add(index) = section.start;
        *ends.add(index) = section.end;
        *kinds.add(index) = section.kind;
        *energies.add(index) = section.energy;
        *motivic_ids.add(index) = section.motivic;
        *flows.add(index) = section.flow;
    }
    sections.len()
}
