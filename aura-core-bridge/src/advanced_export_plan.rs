#[derive(Clone, Copy, Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub enum ExportChannelKind { Audio, Instrument, Group, Effect, Output }

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct AvailableExportChannel {
    pub id: u32,
    pub name: String,
    pub kind: ExportChannelKind,
    pub channels: u16,
    pub selected: bool,
    pub requires_realtime: bool,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct NamedExportRange { pub id: u32, pub name: String, pub start_sample: u64, pub end_sample: u64 }

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub enum ExportRangeSelection {
    Locators { start_sample: u64, end_sample: u64 },
    CycleMarkers(Vec<NamedExportRange>),
    ArrangerChains(Vec<NamedExportRange>),
}

#[derive(Clone, Copy, Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub enum ExportEffectsMode { InsertsAndStrip, Dry, GroupsAndSends, MasterGroupsAndSends }

#[derive(Clone, Copy, Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub enum ExportChannelMode { Interleaved, SplitChannels, MonoDownmix, LeftRightFromSurround }
#[derive(Clone, Copy, Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub enum ExistingFilePolicy { Error, IncrementName, Overwrite }

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub enum NamingPart { Project, Channel, Range, Format, Counter { width: u8 }, Literal(String) }

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct ExportNamingScheme { pub parts: Vec<NamingPart>, pub separator: String }

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct ExportRequestPro {
    pub project_name: String,
    pub channel_ids: Vec<u32>,
    pub range: ExportRangeSelection,
    pub codec: CodecRust,
    pub sample_rate: u32,
    pub bit_depth: u16,
    pub effects: ExportEffectsMode,
    pub channel_mode: ExportChannelMode,
    pub naming: ExportNamingScheme,
    pub realtime: bool,
    pub deactivate_external_midi: bool,
    pub existing_file_policy: ExistingFilePolicy,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct PlannedExportFile {
    pub channel_id: u32,
    pub range_id: u32,
    pub start_sample: u64,
    pub end_sample: u64,
    pub output_channels: u16,
    pub filename: String,
    pub realtime: bool,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct ExportPlanPro {
    pub files: Vec<PlannedExportFile>,
    pub total_source_samples: u128,
    pub realtime: bool,
}

pub fn plan_export(request: &ExportRequestPro, available: &[AvailableExportChannel], existing_names: &[String])
    -> Result<ExportPlanPro, String> {
    request.validate()?;
    if available.len() > 65_536 || !available.iter().all(AvailableExportChannel::validate)
        || available.iter().enumerate().any(|(index, channel)| available[..index].iter().any(|previous| previous.id == channel.id)) {
        return Err("available export channels are invalid".into());
    }
    let selected: Vec<_> = request.channel_ids.iter().map(|id| available.iter().find(|channel| channel.id == *id)
        .ok_or_else(|| format!("export channel {id} is unavailable"))).collect::<Result<_, _>>()?;
    let ranges = request.ranges()?;
    let extension = codec_extension(request.codec);
    let mut occupied: std::collections::BTreeSet<String> = existing_names.iter().map(|name| name.to_ascii_lowercase()).collect();
    let mut planned = std::collections::BTreeSet::new();
    let mut files = Vec::new();
    let mut counter = 1u64;
    let realtime = request.realtime || selected.iter().any(|channel| channel.requires_realtime);
    for range in ranges {
        for channel in &selected {
            let output_channels = output_channel_count(channel.channels, request.channel_mode)?;
            let split_count = if request.channel_mode == ExportChannelMode::SplitChannels { channel.channels } else { 1 };
            for split_index in 0..split_count {
                let mut stem = request.naming.render(&request.project_name, &channel.name, &range.name, extension, counter)?;
                if split_count > 1 { stem.push_str(&format!("_ch{:02}", split_index + 1)); }
                let proposed = format!("{stem}.{extension}");
                let filename = resolve_collision(
                    &proposed,
                    request.existing_file_policy,
                    &mut occupied,
                    &mut planned,
                )?;
                files.push(PlannedExportFile { channel_id: channel.id, range_id: range.id,
                    start_sample: range.start_sample, end_sample: range.end_sample,
                    output_channels, filename, realtime });
                counter = counter.checked_add(1).ok_or_else(|| "export counter overflow".to_owned())?;
            }
        }
    }
    if files.is_empty() || files.len() > 1_000_000 { return Err("export plan file count is invalid".into()); }
    let total_source_samples = files.iter().map(|file| u128::from(file.end_sample - file.start_sample)).sum();
    Ok(ExportPlanPro { files, total_source_samples, realtime })
}

impl ExportPlanPro {
    pub fn validate(&self) -> bool {
        if self.files.is_empty() || self.files.len() > 1_000_000 { return false; }
        let mut names = std::collections::BTreeSet::new();
        let mut total = 0u128;
        for file in &self.files {
            if file.channel_id == 0 || file.range_id == 0 || file.start_sample >= file.end_sample
                || !(1..=32).contains(&file.output_channels) || file.filename.len() > 1024
                || unsafe_name(&file.filename) || !file.filename.contains('.')
                || !names.insert(file.filename.to_ascii_lowercase()) { return false; }
            let Some(next) = total.checked_add(u128::from(file.end_sample - file.start_sample)) else { return false; };
            total = next;
        }
        total == self.total_source_samples
            && self.realtime == self.files.iter().any(|file| file.realtime)
            && self.files.iter().all(|file| file.realtime == self.realtime)
    }
}

impl ExportRequestPro {
    fn validate(&self) -> Result<(), String> {
        if self.project_name.trim().is_empty() || self.project_name.len() > 256 || unsafe_name(&self.project_name) {
            return Err("export project name is invalid".into());
        }
        if self.channel_ids.is_empty() || self.channel_ids.len() > 65_536 || self.channel_ids.contains(&0)
            || self.channel_ids.iter().enumerate().any(|(index, id)| self.channel_ids[..index].contains(id)) {
            return Err("export channel selection is invalid".into());
        }
        if !(8_000..=384_000).contains(&self.sample_rate) || !valid_bit_depth(self.codec, self.bit_depth) {
            return Err("export format is invalid".into());
        }
        self.naming.validate()?;
        self.ranges().map(|_| ())
    }

    fn ranges(&self) -> Result<Vec<NamedExportRange>, String> {
        let ranges = match &self.range {
            ExportRangeSelection::Locators { start_sample, end_sample } => vec![NamedExportRange {
                id: 1, name: "Locators".into(), start_sample: *start_sample, end_sample: *end_sample }],
            ExportRangeSelection::CycleMarkers(ranges) | ExportRangeSelection::ArrangerChains(ranges) => ranges.clone(),
        };
        if ranges.is_empty() || ranges.len() > 65_536 || !ranges.iter().all(NamedExportRange::validate)
            || ranges.iter().enumerate().any(|(index, range)| ranges[..index].iter().any(|previous| previous.id == range.id)) {
            return Err("export range selection is invalid".into());
        }
        Ok(ranges)
    }
}

impl AvailableExportChannel {
    fn validate(&self) -> bool {
        self.id != 0 && !self.name.trim().is_empty() && self.name.len() <= 256 && !unsafe_name(&self.name)
            && (1..=32).contains(&self.channels)
    }
}

impl NamedExportRange {
    fn validate(&self) -> bool {
        self.id != 0 && !self.name.trim().is_empty() && self.name.len() <= 256 && !unsafe_name(&self.name)
            && self.start_sample < self.end_sample
    }
}

impl ExportNamingScheme {
    fn validate(&self) -> Result<(), String> {
        if self.parts.is_empty() || self.parts.len() > 32 || self.separator.len() > 8 || unsafe_name(&self.separator) {
            return Err("export naming scheme is invalid".into());
        }
        for part in &self.parts {
            match part {
                NamingPart::Counter { width } if !(1..=12).contains(width) => return Err("export counter width is invalid".into()),
                NamingPart::Literal(value) if value.len() > 128 || unsafe_name(value) => return Err("export naming literal is invalid".into()),
                _ => {}
            }
        }
        Ok(())
    }

    fn render(&self, project: &str, channel: &str, range: &str, format: &str, counter: u64) -> Result<String, String> {
        self.validate()?;
        let values: Vec<String> = self.parts.iter().map(|part| match part {
            NamingPart::Project => sanitize_export_name(project), NamingPart::Channel => sanitize_export_name(channel),
            NamingPart::Range => sanitize_export_name(range), NamingPart::Format => format.to_owned(),
            NamingPart::Counter { width } => format!("{counter:0width$}", width = usize::from(*width)),
            NamingPart::Literal(value) => sanitize_export_name(value),
        }).collect();
        let result = values.join(&self.separator);
        if result.is_empty() || result.len() > 512 { Err("rendered export filename is invalid".into()) } else { Ok(result) }
    }
}

fn output_channel_count(source: u16, mode: ExportChannelMode) -> Result<u16, String> {
    match mode {
        ExportChannelMode::Interleaved => Ok(source),
        ExportChannelMode::SplitChannels | ExportChannelMode::MonoDownmix => Ok(1),
        ExportChannelMode::LeftRightFromSurround if source >= 2 => Ok(2),
        ExportChannelMode::LeftRightFromSurround => Err("L/R export requires at least two source channels".into()),
    }
}

fn valid_bit_depth(codec: CodecRust, depth: u16) -> bool {
    match codec { CodecRust::Wav | CodecRust::Aiff => matches!(depth, 16 | 24 | 32),
        CodecRust::Flac => matches!(depth, 16 | 24), CodecRust::Mp3 | CodecRust::Aac => depth == 16 }
}

fn codec_extension(codec: CodecRust) -> &'static str {
    match codec { CodecRust::Wav => "wav", CodecRust::Aiff => "aiff", CodecRust::Flac => "flac",
        CodecRust::Mp3 => "mp3", CodecRust::Aac => "m4a" }
}

fn resolve_collision(
    proposed: &str,
    policy: ExistingFilePolicy,
    occupied: &mut std::collections::BTreeSet<String>,
    planned: &mut std::collections::BTreeSet<String>,
)
    -> Result<String, String> {
    let normalized = proposed.to_ascii_lowercase();
    if !occupied.contains(&normalized) && planned.insert(normalized.clone()) {
        occupied.insert(normalized); return Ok(proposed.to_owned());
    }
    match policy {
        ExistingFilePolicy::Error => Err(format!("export filename already exists: {proposed}")),
        ExistingFilePolicy::Overwrite if !planned.contains(&normalized) => {
            planned.insert(normalized);
            Ok(proposed.to_owned())
        }
        ExistingFilePolicy::Overwrite => Err(format!("export plan contains a duplicate filename: {proposed}")),
        ExistingFilePolicy::IncrementName => {
            let (stem, extension) = proposed.rsplit_once('.').unwrap_or((proposed, ""));
            for suffix in 2..=1_000_000u32 {
                let candidate = if extension.is_empty() { format!("{stem}_{suffix}") }
                    else { format!("{stem}_{suffix}.{extension}") };
                let normalized = candidate.to_ascii_lowercase();
                if occupied.insert(normalized.clone()) && planned.insert(normalized) { return Ok(candidate); }
            }
            Err("could not allocate unique export filename".into())
        }
    }
}

fn unsafe_name(value: &str) -> bool {
    value.contains('\0') || value.contains('/') || value.contains('\\') || value.contains("..")
        || value.chars().any(char::is_control)
}

fn sanitize_export_name(value: &str) -> String {
    value.trim().trim_matches('.').chars().filter(|character| !matches!(character, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|'))
        .filter(|character| !character.is_control()).collect::<String>().trim().to_owned()
}
