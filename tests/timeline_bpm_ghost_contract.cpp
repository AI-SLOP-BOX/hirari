// Characterization contract for TimelineSystem's independent BPM field.
//
// TimelineSystem owns its own `std::atomic<double> m_bpm` (default 120) while
// the project's authoritative tempo lives in the Rust-owned NativeTempoMap
// behind HirariUnifiedEngine.  Before removing anything, this contract must
// answer one question empirically: with a NON-120 project tempo set through
// the real production path, does the duplicate TimelineSystem value produce an
// observable wrong result?
//
// Everything is measured as produced WAV frame counts, never by comparing two
// stored fields against each other.
#include <cassert>
#include <cmath>
#include <cstdint>
#include <cstdio>
#include <cstdlib>
#include <filesystem>
#include <memory>
#include <string>
#include <vector>

#include "src/core/hirari_unified_engine.hpp"
#include "src/core/engine/timeline_system.hpp"
#include "src/core/io/bounce_system.hpp"
#include "src/io/wav_loader_utils.hpp"

namespace {

struct BounceMeasurement {
    bool rendered = false;
    uint64_t frames = 0;
};

// Runs the production bounce path and reads back the observable result: the
// frame count of the file the writer actually published.
BounceMeasurement measureBounce(const Hirari::Core::Engine::TimelineSystem& timeline,
                                 uint32_t sampleRate, const std::string& tag) {
    const auto path =
        std::filesystem::temp_directory_path() / ("hirari-bpm-ghost-" + tag + ".wav");
    std::error_code ec;
    std::filesystem::remove(path, ec);

    Hirari::Core::IO::BounceEngine engine;
    Hirari::Core::IO::BounceEngine::Options options;
    options.path = path.string();
    options.sampleRate = sampleRate;
    options.bitDepth = 24;
    options.normalize = false;
    options.dither = false;
    engine.render(options, timeline);

    BounceMeasurement measurement;
    if (!std::filesystem::is_regular_file(path)) {
        std::printf("    [diagnostic] bounce published no file at %s\n", path.c_str());
        return measurement;
    }
    Hirari::IO::WavLoader::WavInfo info{};
    try {
        const auto decoded = Hirari::IO::WavLoader::load(path.string(), info);
        if (!decoded.empty()) {
            measurement.rendered = true;
            measurement.frames = info.numSamples;
        }
    } catch (const std::exception& error) {
        std::printf("    [diagnostic] decode failed (%llu bytes): %s\n",
                    static_cast<unsigned long long>(std::filesystem::file_size(path)),
                    error.what());
    }
    if (!measurement.rendered) {
        std::printf("    [diagnostic] file size = %llu bytes\n",
                    static_cast<unsigned long long>(std::filesystem::file_size(path)));
    }
    std::filesystem::remove(path, ec);
    return measurement;
}

std::shared_ptr<Hirari::Core::Engine::Track> makeTrack(uint32_t id, const char* name,
                                                       uint32_t length) {
    using Hirari::Core::AudioBuffer;
    using Hirari::Core::Engine::Region;
    using Hirari::Core::Engine::Track;
    auto track = std::make_shared<Track>(id, name, Track::Audio);
    auto source = std::make_shared<AudioBuffer>(2, length);
    for (uint32_t i = 0; i < length; ++i) {
        source->getWritePointer(0)[i] = 0.25f;
        source->getWritePointer(1)[i] = -0.25f;
    }
    Region region{};
    region.id = 1;
    region.path = "bpm-ghost.wav";
    region.start = 0;
    region.len = length;
    region.muted = false;
    region.name = "BPM Ghost";
    region.audio = source;
    track->addRegion(region);
    return track;
}

} // namespace
int main() {
    using Hirari::Core::Engine::HirariUnifiedEngine;
    using Hirari::Core::Engine::TimelineSystem;

    // The engine singleton starts a maintenance thread and a forensic journal
    // under a relative path; a contract must not leave either in the repo.
    ::setenv("HIRARI_DISABLE_RECOVERY_THREAD", "1", 1);

    // ---- 1. Authoritative project tempo through the REAL production path.
    // set_tempo() writes an event into the Rust-owned TempoMap and updates the
    // transport BPM that the rest of the product reads.
    auto& engine = HirariUnifiedEngine::getInstance();
    assert(engine.set_tempo(137.0f));
    const float authoritativeTempo = engine.get_tempo();
    assert(std::abs(authoritativeTempo - 137.0f) < 1.0e-3f);

    // ---- 2. A timeline whose extent is known in the sample domain.
    TimelineSystem timeline;
    timeline.addTrack(makeTrack(1, "BPM Ghost", 16));
    // The offline bounce mixes in 1024-frame blocks, so the timeline must be
    // prepared for at least that many frames per block.
    timeline.prepare(48000.0, 1024);
    assert(timeline.getSelectionLength() > 0.0);
    std::printf("selection length = %.9f beats, track end = %llu samples\n",
                timeline.getSelectionLength(),
                static_cast<unsigned long long>(timeline.getTrackEndSample(1)));

    // The extent is a sample-domain length, so rendering it at the matching
    // rate must always publish 16 frames, whatever scalar any duplicate BPM
    // field happens to hold.
    const uint32_t kRate = 48000;
    const uint64_t expectedFrames = 16;
    const struct {
        double bpm;
        const char* tag;
    } cases[] = {
        {120.0, "default"},  // what production would leave behind
        {60.0, "60"},
        {90.0, "90"},
        {137.0, "137"},
        {180.0, "180"},
    };

    bool allMatchExpectation = true;
    std::printf("authoritative project tempo = %.3f BPM (Rust-owned)\n",
                authoritativeTempo);
    std::printf("timeline extent = %llu frames at %u Hz\n",
                static_cast<unsigned long long>(expectedFrames), kRate);
    for (const auto& testCase : cases) {
        timeline.setBPM(testCase.bpm);
        assert(std::abs(timeline.getBPM() - testCase.bpm) < 1.0e-9);
        const auto measurement = measureBounce(timeline, kRate, testCase.tag);
        assert(measurement.rendered);
        std::printf("  duplicate TimelineSystem bpm=%5.1f -> %llu frames%s\n",
                    testCase.bpm, static_cast<unsigned long long>(measurement.frames),
                    measurement.frames == expectedFrames ? " (expected)"
                                                         : " (UNEXPECTED)");
        if (measurement.frames != expectedFrames) allMatchExpectation = false;
    }
    assert(allMatchExpectation);

    // ---- 3. Where the duplicate value IS observable: the empty timeline.
    // getSelectionLength() is 0 there, so the bounce falls back to a fixed
    // 128 BEATS.  That fallback is beat-denominated and follows the duplicate.
    // Note: as of this writing NO production code calls this render(); the
    // exhaustive reference map shows only this contract exercising it, so the
    // line below pins current behavior rather than endorsing the fallback.
    TimelineSystem empty;
    empty.prepare(48000.0, 1024);
    assert(empty.getSelectionLength() <= 0.0);
    empty.setBPM(120.0);
    assert(std::abs(empty.getBPM() - 120.0) < 1.0e-9);
    const auto emptyAt120 = measureBounce(empty, kRate, "empty-120");
    assert(emptyAt120.rendered);
    empty.setBPM(60.0);
    assert(std::abs(empty.getBPM() - 60.0) < 1.0e-9);
    const auto emptyAt60 = measureBounce(empty, kRate, "empty-60");
    assert(emptyAt60.rendered);
    std::printf("empty timeline fallback: 120 BPM -> %llu frames, 60 BPM -> %llu frames\n",
                static_cast<unsigned long long>(emptyAt120.frames),
                static_cast<unsigned long long>(emptyAt60.frames));
    assert(emptyAt120.frames > 0 && emptyAt60.frames > 0);
    // 60 BPM holds 128 beats exactly twice as long as 120 BPM does.
    assert(emptyAt60.frames == emptyAt120.frames * 2);

    std::printf("timeline-bpm-ghost-contract PASSED\n");
    return 0;
}



