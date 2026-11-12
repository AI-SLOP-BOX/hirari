//! Bounded inspection of the native `ARUA` project container.
//!
//! The command-line protocol also accepts JSON project documents, but the
//! desktop application currently persists its native binary container.  Keep
//! this module read-only and deliberately small: hydration still belongs to
//! the native engine, while `project inspect` must not reject a real `.hirari`
//! file with a JSON parser error.

use crate::ffi::{NativeArrangerPartState, NativeMarkerState, NativeProjectState, NativeRouteState, NativeSidechainState, NativeTrackState, };
use anyhow::{bail, Context, Result};
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::path::{Component, Path, PathBuf};

pub(crate) const MAGIC: u32 = 0x4155_5241;
const MAX_PROJECT_BYTES: usize = 256 * 1024 * 1024;
const MIN_PROJECT_SAMPLE_RATE: u32 = 8_000;
const MAX_PROJECT_SAMPLE_RATE: u32 = 384_000;
const MAX_PROJECT_VERSION: u32 = 42;

/// Header fields shared by the reader and the writer.
#[derive(Debug, Clone, Copy)]
pub(crate) struct NativeProjectHeader {
    pub version: u32,
    pub sample_rate: u32,
    pub header_size: usize,
    pub track_count: u32,
    pub bpm: f64,
    pub root_note: i32,
    pub scale_type: i32,
}

/// Reads an `ARUA` payload, validating the envelope and removing the checksum
/// trailer. The returned bytes are the versioned payload only.
pub(crate) fn read_payload(path: &str) -> Option<(Vec<u8>, NativeProjectHeader)> {
    let metadata = std::fs::metadata(path).ok()?;
    // Reject oversized inputs from their metadata, before the whole file is
    // pulled into memory. The old native loader did the same, and a sparse
    // multi-hundred-megabyte file must not cost an allocation to reject.
    if !metadata.is_file()
        || metadata.len() < 8
        || metadata.len() > MAX_PROJECT_BYTES as u64
    {
        return None;
    }
    let mut bytes = std::fs::read(path).ok()?;
    if bytes.len() < 8 || bytes.len() > MAX_PROJECT_BYTES {
        return None;
    }
    let magic = u32::from_ne_bytes(bytes[..4].try_into().ok()?);
    let version = u32::from_ne_bytes(bytes[4..8].try_into().ok()?);
    if magic != MAGIC || !(1..=MAX_PROJECT_VERSION).contains(&version) {
        return None;
    }
    let header_size = if version >= 14 { 28 } else { 20 };
    if bytes.len() < header_size {
        return None;
    }
    let sample_rate = u32::from_ne_bytes(bytes[8..12].try_into().ok()?);
    let bpm = f64::from_ne_bytes(bytes[12..20].try_into().ok()?);
    let (root_note, scale_type) = if version >= 14 {
        (
            i32::from_ne_bytes(bytes[20..24].try_into().ok()?),
            i32::from_ne_bytes(bytes[24..28].try_into().ok()?),
        )
    } else {
        (0, 0)
    };
    if sample_rate == 0
        || !bpm.is_finite()
        || !(20.0..=300.0).contains(&bpm)
        || !(0..=11).contains(&root_note)
        || !(0..=32).contains(&scale_type)
    {
        return None;
    }
    let track_count =
        u32::from_ne_bytes(bytes.get(header_size..header_size + 4)?.try_into().ok()?);
    if track_count > 100_000 {
        return None;
    }
    if version >= 27 {
        if bytes.len() < 12 {
            return None;
        }
        let payload_size = bytes.len() - 4;
        let stored = u32::from_ne_bytes(bytes[payload_size..].try_into().ok()?);
        if crc32(&bytes[..payload_size]) != stored {
            return None;
        }
        bytes.truncate(payload_size);
    }
    Some((
        bytes,
        NativeProjectHeader {
            version,
            sample_rate,
            header_size,
            track_count,
            bpm,
            root_note,
            scale_type,
        },
    ))
}

/// Checks the plugin-array shape a track will be hydrated from: one
/// little-endian plugin id per slot, positional state/bypass arrays that may
/// not exceed it, and a sandbox path for every sandbox marker.
fn plugin_data_is_consistent(track: &NativeTrackState) -> bool {
    if track.pluginData.len() % 4 != 0 {
        return false;
    }
    let slots = track.pluginTypes.len();
    if track.pluginData.len() / 4 != slots {
        return false;
    }
    if track.pluginStates.len() > slots || track.pluginBypass.len() > slots {
        return false;
    }
    let sandbox_markers = track
        .pluginTypes
        .iter()
        .filter(|kind| **kind == u32::MAX)
        .count();
    sandbox_markers <= track.sandboxedPluginPaths.len()
}

/// Plugin slots a hydrated track will expose: the serialized chain plus any
/// legacy trailing sandbox paths that predate the marker.
fn effective_plugin_count(track: &NativeTrackState) -> usize {
    let sandbox_markers = track
        .pluginTypes
        .iter()
        .filter(|kind| **kind == u32::MAX)
        .count();
    track.pluginTypes.len() + track.sandboxedPluginPaths.len().saturating_sub(sandbox_markers)
}

/// Resolves a stored media path against the project directory and removes
/// `.`/`..` segments without touching the filesystem, mirroring the loader's
/// former `std::filesystem::path` normalization.
fn resolve_region_path(project_dir: &Path, stored: &str) -> String {
    let stored_path = Path::new(stored);
    let joined = if stored_path.is_absolute() {
        stored_path.to_path_buf()
    } else {
        project_dir.join(stored_path)
    };
    let mut normalized = PathBuf::new();
    for component in joined.components() {
        match component {
            Component::Prefix(_) | Component::RootDir => normalized.push(component.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            Component::Normal(part) => normalized.push(part),
        }
    }
    normalized.to_string_lossy().into_owned()
}

/// Applies the defaults the engine used to fill in while copying a decoded
/// region, so the loader's hydration is a direct field copy.
fn normalize_region(
    region: &mut crate::ffi::NativeRegionState,
    project_dir: &Path,
    project_rate: u32,
) {
    region.filePath = resolve_region_path(project_dir, &region.filePath);
    if region.name.is_empty() {
        region.name = region.filePath.clone();
    }
    if region.sourceSampleRate == 0 {
        region.sourceSampleRate = project_rate;
    }
    if region.baseStart == 0 {
        region.baseStart = region.samplePosition;
    }
    if region.baseLength == 0 {
        region.baseLength = region.sampleLength;
    }
    region.clipGain = region.clipGain.clamp(0.0, 2.0);
    region.fadeInSamples = region.fadeInSamples.min(region.sampleLength);
    region.fadeOutSamples = region.fadeOutSamples.min(region.sampleLength);
}

/// Cross-document invariants that the per-section decoders cannot see:
/// references between tracks, sidechains, and routes. The engine hydrates a
/// decoded state without repeating these checks.
fn validate_decoded(
    tracks: &[NativeTrackState],
    sidechains: &[NativeSidechainState],
    routes: &[NativeRouteState],
) -> bool {
    const ROUTING_NODE_LIMIT: u32 = crate::routing_gains::MAX_ROUTING_NODES;
    const BUS_TRACK_TYPE: u32 = 3;

    let mut track_ids = HashSet::with_capacity(tracks.len());
    let mut bus_ids = HashSet::new();
    let mut plugin_counts = HashMap::with_capacity(tracks.len());
    for track in tracks {
        if !plugin_data_is_consistent(track) {
            return false;
        }
        let plugin_count = effective_plugin_count(track);
        // Automation targets a plugin slot in the chain the loader rebuilds.
        if track
            .pluginAutomation
            .iter()
            .any(|lane| lane.pluginIndex as usize >= plugin_count)
        {
            return false;
        }
        plugin_counts.insert(track.id, plugin_count);
        track_ids.insert(track.id);
        if track.trackType == BUS_TRACK_TYPE {
            bus_ids.insert(track.id);
        }
    }

    for sidechain in sidechains {
        if !track_ids.contains(&sidechain.sourceId)
            || !track_ids.contains(&sidechain.destinationId)
        {
            return false;
        }
        // Sandbox admission can drop a stored path at hydration, so this index
        // is checked against the chain the engine will actually expose.
        if plugin_counts
            .get(&sidechain.destinationId)
            .map_or(true, |count| sidechain.pluginIndex as usize >= *count)
        {
            return false;
        }
    }
    // A detector link may not form a cycle: the destination must never reach
    // the source through further links.
    for candidate in sidechains {
        let mut pending = vec![candidate.destinationId];
        let mut visited = HashSet::new();
        while let Some(node) = pending.pop() {
            if node == candidate.sourceId {
                return false;
            }
            if !visited.insert(node) {
                continue;
            }
            for link in sidechains {
                if link.sourceId == node {
                    pending.push(link.destinationId);
                }
            }
        }
    }

    for route in routes {
        if route.sourceId >= ROUTING_NODE_LIMIT
            || route.destinationId >= ROUTING_NODE_LIMIT
            || !track_ids.contains(&route.sourceId)
            || !track_ids.contains(&route.destinationId)
            || (route.send && !bus_ids.contains(&route.destinationId))
        {
            return false;
        }
    }
    true
}

/// Decodes an `ARUA` container into the engine-facing state record.
pub(crate) fn load_state(path: &str) -> NativeProjectState {
    let mut state = NativeProjectState::default();
    let Some((bytes, header)) = read_payload(path) else {
        return state;
    };
    let Some((tracks, track_offset, total_strings)) = crate::project_track_decoder::decode_tracks(
        &bytes,
        header.header_size as u32,
        header.version,
        header.track_count,
    ) else {
        return NativeProjectState::default();
    };
    let track_ids: Vec<u32> = tracks.iter().map(|track| track.id).collect();
    let Some((regions, region_offset)) = crate::project_region_decoder::decode_regions(
        &bytes,
        track_offset,
        header.version,
        header.sample_rate,
        &track_ids,
        total_strings,
    ) else {
        return NativeProjectState::default();
    };
    let Some((sidechains, routes, markers, arranger_parts, tempo_events, tail_offset)) =
        crate::project_tail_decoder::decode_tail(&bytes, region_offset, header.version)
    else {
        return NativeProjectState::default();
    };
    if tail_offset != bytes.len() {
        return NativeProjectState::default();
    }
    if !validate_decoded(&tracks, &sidechains, &routes) {
        return NativeProjectState::default();
    }
    state.bpm = header.bpm;
    state.rootNote = header.root_note;
    state.scaleType = header.scale_type;
    state.sampleRate = header.sample_rate;
    state.version = header.version;
    state.tracks = tracks;
    state.regions = regions;
    state.sidechains = sidechains;
    state.routes = routes;
    state.markers = markers;
    state.arrangerParts = arranger_parts;
    state.tempoEvents = tempo_events;
    let project_dir = Path::new(path)
        .parent()
        .filter(|dir| !dir.as_os_str().is_empty())
        .map(Path::to_path_buf)
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
    for region in &mut state.regions {
        normalize_region(region, &project_dir, header.sample_rate);
    }
    state.valid = true;
    state
}

/// Encodes an engine state record as an `ARUA` container and stores it.
pub(crate) fn save_state(path: &str, state: &NativeProjectState) -> bool {
    if path.is_empty() || !(1..=MAX_PROJECT_VERSION).contains(&state.version) {
        return false;
    }
    let version = state.version;
    let Some(tracks) = crate::project_track_decoder::encode_tracks(version, &state.tracks) else {
        return false;
    };
    let Some(regions) =
        crate::project_region_decoder::encode_regions(version, state.sampleRate, &state.regions)
    else {
        return false;
    };
    let Some(tail) = crate::project_tail_decoder::encode_tail(
        version,
        &state.sidechains,
        &state.routes,
        &state.markers,
        &state.arrangerParts,
        &state.tempoEvents,
    ) else {
        return false;
    };
    let Some(mut payload) = crate::project_tail_decoder::assemble_payload(
        version,
        state.sampleRate,
        state.bpm,
        state.rootNote,
        state.scaleType,
        &tracks,
        &regions,
        &tail,
    ) else {
        return false;
    };
    if payload.len() > MAX_PROJECT_BYTES - 4 {
        return false;
    }
    if version >= 27 {
        let checksum = crc32(&payload);
        payload.extend_from_slice(&checksum.to_ne_bytes());
    }
    crate::project_asset_orchestrator::ProjectOrchestrator::new()
        .save_project_checked(path, &payload)
        .is_ok()
}

#[derive(Debug, Clone, Serialize)]
pub struct NativeProjectInspection {
    pub format: &'static str,
    pub version: u32,
    pub sample_rate: u32,
    pub bpm: f64,
    pub root_note: Option<i32>,
    pub scale_type: Option<i32>,
    pub track_count: Option<u32>,
    pub region_count: Option<u32>,
    pub checksum_valid: bool,
    pub bytes: usize,
}

pub fn is_native_project(path: impl AsRef<Path>) -> Result<bool> {
    let path = path.as_ref();
    let mut file = std::fs::File::open(path)
        .with_context(|| format!("failed to read project {}", path.display()))?;
    let mut magic = [0u8; 4];
    let read = file
        .read(&mut magic)
        .context("failed to read project header")?;
    Ok(read == magic.len() && u32::from_le_bytes(magic) == MAGIC)
}

pub fn inspect(path: impl AsRef<Path>) -> Result<NativeProjectInspection> {
    let path = path.as_ref();
    let size = std::fs::metadata(path)
        .with_context(|| format!("failed to stat project {}", path.display()))?
        .len();
    if size > MAX_PROJECT_BYTES as u64 {
        bail!(
            "native project exceeds {} byte inspection limit",
            MAX_PROJECT_BYTES
        );
    }
    let bytes = std::fs::read(path)
        .with_context(|| format!("failed to read project {}", path.display()))?;
    if bytes.len() < 12 || read_u32(&bytes, 0) != Some(MAGIC) {
        bail!("not a native ARUA project container");
    }

    let version = read_u32(&bytes, 4).context("native project header is truncated")?;
    if !(1..=28).contains(&version) {
        bail!("unsupported native project version {version}");
    }
    let sample_rate = read_u32(&bytes, 8).context("native project sample rate is truncated")?;
    let bpm = read_f64(&bytes, 12).context("native project tempo is truncated")?;
    if !(MIN_PROJECT_SAMPLE_RATE..=MAX_PROJECT_SAMPLE_RATE).contains(&sample_rate)
        || !bpm.is_finite()
        || !(20.0..=300.0).contains(&bpm)
    {
        bail!("native project header contains invalid audio or tempo metadata");
    }

    let (root_note, scale_type, count_offset) = if version >= 14 {
        (
            Some(read_i32(&bytes, 20).context("native project root note is truncated")?),
            Some(read_i32(&bytes, 24).context("native project scale is truncated")?),
            28,
        )
    } else {
        (None, None, 20)
    };
    if root_note.is_some_and(|value| !(0..=11).contains(&value)) {
        bail!("native project root note is outside the supported range");
    }

    let (payload, checksum_valid) = if version >= 27 {
        if bytes.len() < 4 + 4 + 4 {
            bail!("native project checksum trailer is truncated");
        }
        let payload_len = bytes.len() - 4;
        let stored =
            read_u32(&bytes, payload_len).context("native project checksum is truncated")?;
        (
            &bytes[..payload_len],
            crc32(&bytes[..payload_len]) == stored,
        )
    } else {
        (&bytes[..], true)
    };
    if !checksum_valid {
        bail!("native project checksum mismatch");
    }

    // Counts are safe to expose from the fixed header without pretending that
    // this lightweight inspector has hydrated the full native graph.
    let (track_count, region_count) = parse_counts(payload, version, count_offset)
        .map_or((read_u32(payload, count_offset), None), |counts| {
            (Some(counts.0), Some(counts.1))
        });

    Ok(NativeProjectInspection {
        format: "hirari-native-binary",
        version,
        sample_rate,
        bpm,
        root_note,
        scale_type,
        track_count,
        region_count,
        checksum_valid,
        bytes: bytes.len(),
    })
}

fn read_u32(bytes: &[u8], offset: usize) -> Option<u32> {
    let end = offset.checked_add(4)?;
    let raw: [u8; 4] = bytes.get(offset..end)?.try_into().ok()?;
    Some(u32::from_le_bytes(raw))
}

fn read_i32(bytes: &[u8], offset: usize) -> Option<i32> {
    read_u32(bytes, offset).map(|value| i32::from_le_bytes(value.to_le_bytes()))
}

fn read_f64(bytes: &[u8], offset: usize) -> Option<f64> {
    let end = offset.checked_add(8)?;
    let raw: [u8; 8] = bytes.get(offset..end)?.try_into().ok()?;
    Some(f64::from_le_bytes(raw))
}

fn parse_counts(bytes: &[u8], version: u32, track_offset: usize) -> Option<(u32, u32)> {
    let mut cursor = Cursor {
        bytes,
        offset: track_offset,
    };
    let track_count = cursor.u32()?;
    if track_count > 512 {
        return None;
    }
    for _ in 0..track_count {
        cursor.skip(4)?; // track id
        if version >= 12 {
            cursor.skip(4)?; // track type
        }
        cursor.skip(5 * 4)?; // volume, pan, and 3D position
        if version >= 12 {
            cursor.skip(2)?; // mute, solo
            if version >= 15 {
                cursor.skip(1)?; // phase invert
            }
            if version >= 23 {
                cursor.skip(1)?; // record armed
            }
        }
        cursor.skip_blob()?; // plugin name
        cursor.skip_blob()?; // plugin data
        if version >= 24 {
            let state_count = cursor.u32()?;
            if state_count > 1_000_000 {
                return None;
            }
            for _ in 0..state_count {
                cursor.skip_blob()?;
            }
            if version >= 25 {
                let bypass_count = cursor.u32()?;
                cursor.skip(bypass_count as usize)?;
            }
        }
        if version >= 16 {
            let sandbox_count = cursor.u32()?;
            if sandbox_count > 65_535 {
                return None;
            }
            for _ in 0..sandbox_count {
                cursor.skip_blob()?;
            }
            if version >= 17 {
                let state_count = cursor.u32()?;
                if state_count > 1_000_000 {
                    return None;
                }
                for _ in 0..state_count {
                    cursor.skip_blob()?;
                }
            }
        }
        if version >= 13 {
            skip_automation(&mut cursor)?;
            skip_automation(&mut cursor)?;
        }
    }
    let region_count = cursor.u32()?;
    (region_count <= 4_000_000).then_some((track_count, region_count))
}

fn skip_automation(cursor: &mut Cursor<'_>) -> Option<()> {
    let count = cursor.u32()?;
    if count > 4_000_000 {
        return None;
    }
    cursor.skip((count as usize).checked_mul(16)?)
}

struct Cursor<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl Cursor<'_> {
    fn skip(&mut self, amount: usize) -> Option<()> {
        self.offset = self.offset.checked_add(amount)?;
        (self.offset <= self.bytes.len()).then_some(())
    }

    fn u32(&mut self) -> Option<u32> {
        let value = read_u32(self.bytes, self.offset)?;
        self.offset = self.offset.checked_add(4)?;
        Some(value)
    }

    fn skip_blob(&mut self) -> Option<()> {
        let size = self.u32()? as usize;
        self.skip(size)
    }
}

pub(crate) fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for byte in bytes {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xEDB8_8320u32 & 0u32.wrapping_sub(crc & 1));
        }
    }
    !crc
}

/// Bridge entrypoint: decode an `ARUA` container into the engine state record.
pub fn native_project_load(path: &str) -> NativeProjectState {
    load_state(path)
}

/// Bridge entrypoint: encode and store an engine state record as `ARUA`.
pub fn native_project_save(path: &str, state: &NativeProjectState) -> bool {
    save_state(path, state)
}

/// Bridge entrypoint: encode and store an engine state record as `ARUA`,
/// taking the timeline markers and arranger parts from the Rust arrangement
/// document so the engine does not have to mirror them.
pub fn native_project_save_with_arrangement(
    path: &str,
    state: &NativeProjectState,
    arrangement_document: usize,
) -> bool {
    let Some((markers, parts)) =
        crate::arrangement_document::snapshot(arrangement_document as *mut std::ffi::c_void)
    else {
        return false;
    };
    let mut state = state.clone();
    state.markers = markers
        .into_iter()
        .map(|(sample, name, color)| NativeMarkerState {
            sample,
            name: String::from_utf8_lossy(&name).into_owned(),
            color,
        })
        .collect();
    state.arrangerParts = parts
        .into_iter()
        .map(|(start, length, name, repeats)| NativeArrangerPartState {
            start,
            length,
            name: String::from_utf8_lossy(&name).into_owned(),
            repeats,
        })
        .collect();
    save_state(path, &state)
}

#[cfg(test)]
#[path = "native_project_tests.rs"]
mod tests;

