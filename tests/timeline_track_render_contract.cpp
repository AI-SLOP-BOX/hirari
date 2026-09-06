#include <cassert>
#include <cmath>
#include <filesystem>
#include <fstream>
#include <iostream>
#include <memory>
#include <string>
#include <vector>

#include "../src/core/engine/timeline_system.hpp"
#include "../src/io/wav_loader_utils.hpp"
#include "../src/core/io/bounce_system.hpp"
#include "../src/io/audio_export_engine.hpp"

int main() {
    using Aura::Core::AudioBuffer;
    using Aura::Core::Engine::Region;
    using Aura::Core::Engine::TimelineSystem;
    using Aura::Core::Engine::Track;

    TimelineSystem timeline;
    auto track = std::make_shared<Track>(77, "Bounce Contract", Track::Audio);
    auto source = std::make_shared<AudioBuffer>(2, 8);
    for (uint32_t i = 0; i < 8; ++i) {
        source->getWritePointer(0)[i] = static_cast<float>(i + 1) * 0.1f;
        source->getWritePointer(1)[i] = -static_cast<float>(i + 1) * 0.1f;
    }
    Region baseRegion{};
    baseRegion.id = 1;
    baseRegion.path = "contract.wav";
    baseRegion.start = 0;
    baseRegion.len = 8;
    baseRegion.muted = false;
    baseRegion.name = "Contract";
    baseRegion.audio = source;
    track->addRegion(baseRegion);
    timeline.addTrack(track);
    timeline.prepare(48000.0, 4);

    AudioBuffer output(2, 4);
    assert(timeline.renderTrackInto(77, output, 4, 2));
    assert(std::isfinite(output.getReadPointer(0)[0]));
    assert(std::isfinite(output.getReadPointer(1)[3]));

    // A malformed host block must be rejected before Track::process can
    // index past the destination buffer.  Existing audio must be cleared,
    // not partially overwritten.
    output.getWritePointer(0)[0] = 1.0f;
    output.getWritePointer(1)[0] = -1.0f;
    track->process(output, 8, 0);
    assert(output.getReadPointer(0)[0] == 0.0f);
    assert(output.getReadPointer(1)[0] == 0.0f);

    // Invalid track IDs and undersized destinations must fail without
    // touching the output buffer.
    AudioBuffer undersized(2, 2);
    assert(!timeline.renderTrackInto(999, output, 4, 0));
    assert(!timeline.renderTrackInto(77, undersized, 4, 0));
    assert(timeline.getTrackSnapshot(77) == track);

    // The arrangement path must apply loop, warp, and pitch during actual
    // rendering, rather than merely persisting those values in the project.
    auto transformedTrack = std::make_shared<Track>(78, "Flex Contract", Track::Audio);
    Region transformed = baseRegion;
    transformed.id = 2;
    transformed.path = "flex.wav";
    transformed.name = "Flex";
    transformed.loopCount = 2;
    transformed.warpRatio = 0.5;
    transformed.pitchSemitones = 12.0f;
    transformed.fadeInSamples = 0;
    transformed.fadeOutSamples = 0;
    transformedTrack->addRegion(transformed);
    transformedTrack->setVolume(1.0f);
    transformedTrack->setPan(0.0f);
    timeline.addTrack(transformedTrack);
    timeline.prepare(48000.0, 16);

    AudioBuffer transformedOutput(2, 16);
    assert(timeline.renderTrackInto(78, transformedOutput, 16, 0));
    for (uint32_t i = 0; i < 8; ++i) {
        assert(std::isfinite(transformedOutput.getReadPointer(0)[i]));
        assert(std::abs(transformedOutput.getReadPointer(0)[i] -
                        transformedOutput.getReadPointer(0)[i + 8]) < 1.0e-5f);
    }

    auto crossfadeTrack = std::make_shared<Track>(79, "Crossfade Contract", Track::Audio);
    auto replacement = std::make_shared<AudioBuffer>(2, 8);
    for (uint32_t i = 0; i < 8; ++i) {
        replacement->getWritePointer(0)[i] = -1.0f;
        replacement->getWritePointer(1)[i] = -1.0f;
    }
    Region outgoing = baseRegion;
    outgoing.id = 3;
    outgoing.audio = source;
    outgoing.fadeInSamples = 0;
    outgoing.fadeOutSamples = 0;
    Region incoming = baseRegion;
    incoming.id = 4;
    incoming.start = 4;
    incoming.audio = replacement;
    incoming.fadeInSamples = 0;
    incoming.fadeOutSamples = 0;
    crossfadeTrack->addRegion(outgoing);
    crossfadeTrack->addRegion(incoming);
    crossfadeTrack->setVolume(1.0f);
    crossfadeTrack->setPan(0.0f);
    timeline.addTrack(crossfadeTrack);
    timeline.prepare(48000.0, 12);
    AudioBuffer crossfadeOutput(2, 12);
    assert(timeline.renderTrackInto(79, crossfadeOutput, 12, 0));
    assert(crossfadeOutput.getReadPointer(0)[4] > crossfadeOutput.getReadPointer(0)[5]);
    assert(crossfadeOutput.getReadPointer(0)[5] > crossfadeOutput.getReadPointer(0)[6]);
    assert(crossfadeOutput.getReadPointer(0)[6] > crossfadeOutput.getReadPointer(0)[7]);
    assert(crossfadeOutput.getReadPointer(0)[8] < -0.5f);

    const auto destination = std::filesystem::temp_directory_path() /
                             "aura-bounce-contract.wav";
    std::vector<std::vector<float>> rendered{
        {0.0f, 0.25f, -0.25f, 0.5f},
        {0.0f, -0.25f, 0.25f, -0.5f}
    };
    assert(Aura::IO::WavSaver::save(destination.string(), rendered, 48000));
    std::ifstream file(destination, std::ios::binary);
    assert(file.good());
    char riff[4] = {};
    file.read(riff, sizeof(riff));
    assert(std::string(riff, sizeof(riff)) == "RIFF");

    std::error_code ec;
    std::filesystem::remove(destination, ec);

    // The product BounceEngine must publish through the canonical streaming
    // writer, not its former private RIFF/RF64 implementation.
    const auto streamedBounce = std::filesystem::temp_directory_path() /
                                "aura-bounce-streaming-contract.wav";
    Aura::Core::IO::BounceEngine bounce;
    Aura::Core::IO::BounceEngine::Options bounceOptions;
    bounceOptions.path = streamedBounce.string();
    bounceOptions.sampleRate = 48'000;
    bounceOptions.bitDepth = 24;
    bounce.render(bounceOptions, timeline);
    assert(std::filesystem::is_regular_file(streamedBounce));
    assert(std::filesystem::file_size(streamedBounce) > 44);
    std::ifstream streamedInput(streamedBounce, std::ios::binary);
    char streamedHeader[12] = {};
    streamedInput.read(streamedHeader, sizeof(streamedHeader));
    assert(streamedInput && std::string(streamedHeader, 4) == "RIFF" &&
           std::string(streamedHeader + 8, 4) == "WAVE");
    std::filesystem::remove(streamedBounce, ec);

    const auto pcm16Bounce = std::filesystem::temp_directory_path() /
                             "aura-bounce-pcm16-contract.wav";
    Aura::Core::IO::BounceEngine::Options pcm16Options;
    pcm16Options.path = pcm16Bounce.string();
    pcm16Options.sampleRate = 48'000;
    pcm16Options.bitDepth = 16;
    pcm16Options.normalize = true;
    pcm16Options.dither = false;
    bounce.render(pcm16Options, timeline);
    assert(std::filesystem::is_regular_file(pcm16Bounce));
    std::ifstream pcm16Input(pcm16Bounce, std::ios::binary);
    std::array<uint8_t, 36> pcm16Header{};
    pcm16Input.read(reinterpret_cast<char*>(pcm16Header.data()),
                    static_cast<std::streamsize>(pcm16Header.size()));
    assert(pcm16Input && std::string(pcm16Header.begin(), pcm16Header.begin() + 4) == "RIFF" &&
           pcm16Header[34] == 16 && pcm16Header[35] == 0);
    std::filesystem::remove(pcm16Bounce, ec);

    // AudioExportEngine must use the same bounded-memory streaming writer for
    // both channel layouts.  The normalized path is intentionally exercised
    // because it performs a two-pass render rather than buffering the song.
    const auto exportPcm16 = std::filesystem::temp_directory_path() /
                             "aura-audio-export-pcm16-contract.wav";
    Aura::IO::AudioExportEngine::ExportOptions exportOptions;
    exportOptions.filename = exportPcm16.string();
    exportOptions.sampleRate = 48'000.0;
    exportOptions.bitDepth = 16;
    exportOptions.startSample = 0;
    exportOptions.endSample = 8;
    exportOptions.channels = 2;
    exportOptions.normalize = true;
    const bool exportOk = Aura::IO::AudioExportEngine::bounce(timeline, exportOptions, {});
    if (!exportOk) {
        std::cerr << "AudioExportEngine PCM16 normalized contract failed\n";
        return 100;
    }
    std::ifstream exportInput(exportPcm16, std::ios::binary);
    std::array<uint8_t, 36> exportHeader{};
    exportInput.read(reinterpret_cast<char*>(exportHeader.data()),
                     static_cast<std::streamsize>(exportHeader.size()));
    assert(exportInput && exportHeader[34] == 16 && exportHeader[35] == 0);
    std::filesystem::remove(exportPcm16, ec);

    const auto exportMono = std::filesystem::temp_directory_path() /
                            "aura-audio-export-mono-contract.wav";
    exportOptions.filename = exportMono.string();
    exportOptions.bitDepth = 24;
    exportOptions.channels = 1;
    exportOptions.normalize = false;
    const bool monoExportOk = Aura::IO::AudioExportEngine::bounce(timeline, exportOptions, {});
    if (!monoExportOk) {
        std::cerr << "AudioExportEngine mono PCM24 contract failed\n";
        return 101;
    }
    std::ifstream monoExportInput(exportMono, std::ios::binary);
    std::array<uint8_t, 36> monoExportHeader{};
    monoExportInput.read(reinterpret_cast<char*>(monoExportHeader.data()),
                         static_cast<std::streamsize>(monoExportHeader.size()));
    assert(monoExportInput && monoExportHeader[22] == 1 &&
           monoExportHeader[34] == 24 && monoExportHeader[35] == 0);
    std::filesystem::remove(exportMono, ec);

    // A mono 24-bit file has an odd-sized data chunk.  The writer must add
    // the RIFF padding byte while the loader must still report one sample.
    const auto oddPath = std::filesystem::temp_directory_path() /
                         "aura-bounce-odd-contract.wav";
    assert(Aura::IO::WavSaver::save(oddPath.string(), {{0.25f}}, 48000));
    Aura::IO::WavLoader::WavInfo oddInfo{};
    const auto oddDecoded = Aura::IO::WavLoader::load(oddPath.string(), oddInfo);
    assert(oddInfo.numChannels == 1 && oddInfo.numSamples == 1);
    assert(oddDecoded.size() == 1 && std::isfinite(oddDecoded[0][0]));
    std::filesystem::remove(oddPath, ec);

    assert(timeline.removeTrack(77));
    assert(!timeline.getTrackSnapshot(77));
    assert(!timeline.removeTrack(77));
    assert(!timeline.renderTrackInto(77, output, 4, 0));
    return 0;
}
