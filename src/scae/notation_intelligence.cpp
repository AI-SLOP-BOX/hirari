/*
 * Aura DAW Ultimate - High-Performance Digital Audio Workstation
 * Copyright (c) 2024-2026 Aura DAW Project. All rights reserved.
 * Licensed under the MIT License.
 */

#include "notation_intelligence.hpp"
#include "../core/engine/track.hpp"
#include "../core/midi_region.hpp"
#include "../core/engine/scale_system.hpp"
#include <map>
#include <algorithm>
#include <cmath>
#include <sstream>

namespace Aura::SCAE::Intelligence {

std::vector<NotationIntelligence::ScoreGlyph> NotationIntelligence::generateScoreManifest(const ::Aura::Core::Engine::Track& track) {
    std::vector<ScoreGlyph> manifest;
    
    // Audio regions remain useful score timeline anchors. MIDI regions are
    // appended below into the same stable representation, so mixed audio/MIDI
    // tracks can be rendered without a second notation coordinate system.
    for (const auto& region : track.getRegions()) {
        if (region.len == 0 || region.muted) continue;
        const double beat = std::isfinite(region.start * 1.0)
            ? static_cast<double>(region.start) / track.getSampleRate() * 2.0
            : 0.0;
        const double durationBeats =
            std::max(1.0 / 16.0, static_cast<double>(region.len) /
                                      track.getSampleRate() * 2.0);
        if (!std::isfinite(beat) || !std::isfinite(durationBeats)) continue;
        ScoreGlyph glyph;
        glyph.beat = static_cast<float>(std::max(0.0, beat));
        glyph.staffOffset = 0.0f;
        glyph.symbolType = "audio-region";
        glyph.duration = std::max(1, static_cast<int>(std::lround(1.0 / durationBeats)));
        glyph.voice = 1;
        manifest.push_back(std::move(glyph));
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
    const std::vector<std::shared_ptr<::Aura::Core::MidiRegion>>& regions) {
    std::vector<ScoreGlyph> manifest;
    for (const auto& midiRegion : regions) {
        if (!midiRegion || !std::isfinite(midiRegion->getStartBeat()) ||
            !std::isfinite(midiRegion->getLengthBeats()) ||
            midiRegion->getLengthBeats() <= 0.0) {
            continue;
        }
        const double regionStart = midiRegion->getStartBeat();
        const double regionEnd = regionStart + midiRegion->getLengthBeats();
        if (!std::isfinite(regionEnd) || regionEnd < regionStart) continue;

        std::vector<::Aura::Core::MIDINote> notes;
        midiRegion->copyProcessedNotes(notes);
        for (size_t noteIndex = 0; noteIndex < notes.size(); ++noteIndex) {
            const auto& note = notes[noteIndex];
            if (note.velocity == 0 || note.pitch > 127 ||
                !std::isfinite(note.startBeat) || !std::isfinite(note.lengthBeats) ||
                note.startBeat < 0.0 || note.lengthBeats <= 0.0) {
                continue;
            }
            const double beat = regionStart + note.startBeat;
            const double end = beat + note.lengthBeats;
            if (!std::isfinite(beat) || !std::isfinite(end) || end <= beat ||
                beat >= regionEnd) continue;

            // MIDI pitch is intentionally retained as the staff coordinate;
            // the notation renderer can apply clef/key-aware placement later.
            ScoreGlyph glyph;
            glyph.beat = static_cast<float>(std::max(0.0, beat));
            glyph.staffOffset = static_cast<float>(note.pitch);
            glyph.symbolType = "midi-note";
            const double duration = std::min(note.lengthBeats, regionEnd - beat);
            // Glyph duration uses the notation engine's denominator convention:
            // 1=whole, 2=half, 4=quarter, 8=eighth, etc.
            glyph.duration = std::max(1, static_cast<int>(std::lround(4.0 / duration)));
            glyph.voice = 1;
            glyph.sourceKind = 1;
            glyph.sourceRegionId = midiRegion->getId();
            glyph.sourceNoteIndex = static_cast<uint32_t>(noteIndex);
            manifest.push_back(std::move(glyph));
        }
    }
 
    std::sort(manifest.begin(), manifest.end(), [](const auto& a, const auto& b) {
        return a.beat < b.beat;
    });
 
    return manifest;
}
 
std::string NotationIntelligence::conductHarmonicAudit(const std::vector<std::shared_ptr<::Aura::Core::Engine::Track>>& tracks) {
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
                std::vector<::Aura::Core::MIDINote> snapshot;
                region->copyProcessedNotes(snapshot);
                notes += snapshot.size();
            }
        }
    }
    result << notes << " MIDI notes.";
    return result.str();
}

} // namespace Aura::SCAE::Intelligence
