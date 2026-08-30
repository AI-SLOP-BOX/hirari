#include <cassert>
#include <memory>
#include <string>

#include "../src/core/engine/track.hpp"
#include "../src/core/notation/notation_engine.hpp"
#include "../src/scae/notation_intelligence.hpp"

int main() {
    auto track = std::make_shared<Aura::Core::Engine::Track>(1, "Audio", Aura::Core::Engine::Track::Audio);
    Aura::Core::Engine::Region region{};
    region.id = 3;
    region.start = 44100;
    region.len = 22050;
    region.name = "Verse";
    track->addRegion(region);

    auto manifest = Aura::SCAE::Intelligence::NotationIntelligence::generateScoreManifest(*track);
    assert(manifest.size() == 1);
    assert(manifest.front().symbolType == "audio-region");
    assert(manifest.front().beat > 1.9f && manifest.front().beat < 2.1f);

    auto midi = std::make_shared<Aura::Core::MidiRegion>(7, "Lead", 4.0, 4.0);
    midi->addNote(Aura::Core::MIDINote{60, 100, 0.0, 1.0});
    midi->addNote(Aura::Core::MIDINote{64, 110, 1.0, 0.5});
    track->addMidiRegion(midi);
    auto midiManifest = Aura::SCAE::Intelligence::NotationIntelligence::generateScoreManifest(*track);
    assert(midiManifest.size() == 3);
    assert(midiManifest[1].symbolType == "midi-note");
    assert(midiManifest[1].beat == 4.0f);
    assert(midiManifest[1].staffOffset == 60.0f);
    assert(midiManifest[1].duration == 4);
    assert(midiManifest[2].duration == 8);

    auto audit = Aura::SCAE::Intelligence::NotationIntelligence::conductHarmonicAudit({track});
    assert(audit.find("1 tracks") != std::string::npos);
    assert(audit.find("1 active timeline regions") != std::string::npos);
    assert(audit.find("2 MIDI notes") != std::string::npos);

    auto& notation = Aura::Core::Notation::NotationEngine::getInstance();
    const auto score = notation.renderTrackToScore(*track);
    assert(score.size() == 3);
    assert(notation.tryUpdateGlyph(0, 120.0f, 4.0f));
    assert(notation.lastGlyphs().front().x == 120.0f);
    assert(notation.lastGlyphs().front().pitch == 4);
    assert(notation.tryUpdateGlyph(1, 250.0f, 62.0f));
    std::vector<Aura::Core::MIDINote> editedNotes;
    midi->copyProcessedNotes(editedNotes);
    assert(editedNotes.front().pitch == 62);
    assert(editedNotes.front().startBeat == 1.0);
    const auto standalone = notation.renderMidiToScore({midi});
    assert(standalone.size() == 2);
    assert(!notation.tryUpdateGlyph(0, 100.0f, 48.0f));
    assert(!notation.tryUpdateGlyph(9, 1.0f, 1.0f));
    return 0;
}
