#include <cassert>
#include <cmath>
#include <filesystem>
#include <fstream>
#include <limits>
#include <memory>
#include <vector>
#include <thread>
#include <chrono>

#include "../src/core/audio_region.hpp"
#include "../src/io/assets/streaming_source.hpp"
#include "../src/io/streaming_buffer.hpp"

class TestSource final : public Aura::Core::IAudioSource {
public:
    explicit TestSource(std::vector<float> samples) : m_samples(std::move(samples)) {}
    float getSample(uint32_t channel, uint64_t index) const override {
        return channel == 0 && index < m_samples.size() ? m_samples[index] : 0.0f;
    }
    uint64_t getNumSamples() const override { return m_samples.size(); }
    uint32_t getNumChannels() const override { return 1; }
    double getSampleRate() const override { return 48000.0; }
private:
    std::vector<float> m_samples;
};

int main() {
    auto empty = std::make_shared<TestSource>(std::vector<float>{});
    Aura::Core::ResamplingAudioSource resampled(empty, 44100.0);
    assert(resampled.getNumSamples() == 0);
    assert(resampled.getSample(0, 0) == 0.0f);
    assert(resampled.getSample(99, 0) == 0.0f);

    auto source = std::make_shared<TestSource>(std::vector<float>{0.25f, 0.5f, 0.75f});
    Aura::Core::AudioRegion::Meta meta{};
    meta.id = 1;
    meta.sampleLength = 3;
    meta.fadeInSamples = 0;
    meta.fadeOutSamples = 0;
    meta.samplePosition = 0;
    Aura::Core::AudioRegion region(source, meta, 48000.0, 120.0f);
    float left[3] = {}, right[3] = {};
    region.render(left, right, 0, 3);
    assert(std::abs(left[0] - 0.25f) < 1.0e-4f);
    assert(std::isfinite(left[2]));
    region.render(nullptr, right, 0, 3);

    meta.fadeInSamples = 2;
    meta.fadeOutSamples = 2;
    meta.reverse = true;
    Aura::Core::AudioRegion reversed(source, meta, 48000.0, 120.0f);
    reversed.render(left, right, 0, 3);
    for (float sample : left) assert(std::isfinite(sample));
    assert(reversed.setClipGain(2.0f));
    assert(!reversed.setClipGain(std::numeric_limits<float>::quiet_NaN()));
    assert(reversed.setFades(100, 100));
    assert(reversed.getMeta().fadeInSamples == 3);
    auto split = reversed.split(1);
    assert(split && split->getSampleLength() == 2);
    assert(reversed.split(UINT64_MAX) == nullptr);

    const auto path = std::filesystem::temp_directory_path() / "aura-stream-contract.raw";
    {
        std::ofstream file(path, std::ios::binary);
        const float values[] = {0.1f, 0.2f, 0.3f, 0.4f};
        file.write(reinterpret_cast<const char*>(values), sizeof(values));
    }
    Aura::Core::Assets::StreamingSource streaming(path.string());
    assert(streaming.getTotalSamples() == 2);
    assert(!streaming.hasTruncatedTail());
    assert(std::isfinite(streaming.getSample(0, 0)));
    streaming.refill();
    std::filesystem::remove(path);

    Aura::IO::StreamingBuffer buffered(path.string());
    buffered.setLooping(false);
    assert(!buffered.isLooping());
    float output[4]{};
    assert(!buffered.getSamples(output, 4));
    assert(buffered.underrunCount() > 0);
    for (int i = 0; i < 500 && !buffered.sourceFailed(); ++i)
        std::this_thread::yield();
    assert(!buffered.sourceError().empty());
    return 0;
}
