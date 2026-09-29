/*
 * Hirari DAW Ultimate - High-Performance Digital Audio Workstation
 * Copyright (c) 2024-2026 Hirari DAW Project. All rights reserved.
 * Licensed under the MIT License.
 */

#include "notation_intelligence.hpp"
#include "../core/engine/track.hpp"
#include "../core/midi_region.hpp"
#include "../core/engine/scale_system.hpp"
#include "../core/rust_ffi.hpp"
#include <map>
#include <algorithm>
#include <cmath>
#include <sstream>

namespace Hirari::SCAE::Intelligence {

std::vector<NotationIntelligence::ScoreGlyph> NotationIntelligence::generateScoreManifest(const ::Hirari::Core::Engine::Track& track) {
    std::vector<ScoreGlyph> manifest;
    std::vector<uint64_t> starts, lengths;
    std::vector<uint8_t> muted;
    std::vector<double> sampleRates;
    for (const auto& region : track.getRegions()) {
        starts.push_back(region.start);
        lengths.push_back(region.len);
        muted.push_back(static_cast<uint8_t>(region.muted));
        sampleRates.push_back(track.getSampleRate());
    }
    if (!starts.empty()) {
        std::vector<HirariMidiScoreGlyph> audioGlyphs(starts.size());
        const size_t count = hirari_notation_audio_manifest(
            starts.data(), lengths.data(), muted.data(), sampleRates.data(), starts.size(),
            audioGlyphs.data(), audioGlyphs.size());
        manifest.reserve(count);
        for (size_t index = 0; index < count; ++index) {
            const auto& source = audioGlyphs[index];
            ScoreGlyph glyph;
            glyph.beat = source.beat;
            glyph.staffOffset = source.staff_offset;
            glyph.symbolType = "audio-region";
            glyph.duration = source.duration;
            glyph.voice = 1;
            manifest.push_back(std::move(glyph));
        }
    }

    // MIDI regions are first-class score material.  Keep their timeline
    // position relative to the region, but publish the absolute beat so the
    // notation and piano-roll views share one coordinate system.
    const auto midiManifest = generateMidiManifest(track.getMidiRegions());
    manifest.insert(manifest.end(), midiManifest.begin(), midiManifest.end());

    std::sort(manifest.begin(), manifest.end(), [](const auto& a, const auto& b) {
        return a.beat < b.beat;
    });
    return manifest;
}

std::vector<NotationIntelligence::ScoreGlyph> NotationIntelligence::generateMidiManifest(
    const std::vector<std::shared_ptr<::Hirari::Core::MidiRegion>>& regions) {
    std::vector<ScoreGlyph> manifest;
    std::vector<double> regionStarts, regionLengths, noteStarts, noteLengths;
    std::vector<uint32_t> regionIds, noteIndices;
    std::vector<uint8_t> pitches, velocities;
    for (const auto& midiRegion : regions) {
        if (!midiRegion) continue;

        std::vector<::Hirari::Core::MIDINote> notes;
        midiRegion->copyProcessedNotes(notes);
        for (size_t noteIndex = 0; noteIndex < notes.size(); ++noteIndex) {
            const auto& note = notes[noteIndex];
            regionStarts.push_back(midiRegion->getStartBeat());
            regionLengths.push_back(midiRegion->getLengthBeats());
            regionIds.push_back(midiRegion->getId());
            noteStarts.push_back(note.startBeat);
            noteLengths.push_back(note.lengthBeats);
            pitches.push_back(note.pitch);
            velocities.push_back(note.velocity);
            noteIndices.push_back(static_cast<uint32_t>(noteIndex));
        }
    }

    if (noteStarts.empty()) return manifest;
    std::vector<HirariMidiScoreGlyph> rustGlyphs(noteStarts.size());
    const size_t count = hirari_notation_midi_manifest(
        regionStarts.data(), regionLengths.data(), regionIds.data(),
        noteStarts.data(), noteLengths.data(), pitches.data(), velocities.data(),
        noteIndices.data(), noteStarts.size(), rustGlyphs.data(), rustGlyphs.size());
    manifest.reserve(count);
    for (size_t index = 0; index < count; ++index) {
        const auto& source = rustGlyphs[index];
        ScoreGlyph glyph;
        glyph.beat = source.beat;
        glyph.staffOffset = source.staff_offset;
        glyph.symbolType = "midi-note";
        glyph.duration = source.duration;
        glyph.voice = 1;
        glyph.sourceKind = 1;
        glyph.sourceRegionId = source.source_region_id;
        glyph.sourceNoteIndex = source.source_note_index;
        manifest.push_back(std::move(glyph));
    }
    return manifest;
}
 
std::string NotationIntelligence::conductHarmonicAudit(const std::vector<std::shared_ptr<::Hirari::Core::Engine::Track>>& tracks) {
    size_t activeTracks = 0;
    size_t regions = 0;
    for (const auto& track : tracks) {
        if (!track) continue;
        ++activeTracks;
        for (const auto& region : track->getRegions()) {
            if (!region.muted && region.len > 0) ++regions;
        }
    }
    std::ostringstream result;
    result << "HARMONIC AUDIT: " << activeTracks
           << " tracks, " << regions
           << " active timeline regions, ";
    size_t notes = 0;
    for (const auto& track : tracks) {
        if (!track) continue;
        for (const auto& region : track->getMidiRegions()) {
            if (region) {
                std::vector<::Hirari::Core::MIDINote> snapshot;
                region->copyProcessedNotes(snapshot);
                notes += snapshot.size();
            }
        }
    }
    result << notes << " MIDI notes.";
    return result.str();
}

} // namespace Hirari::SCAE::Intelligence
