#pragma once

#include <vector>
#include "editing/audio_note_segment.hpp"
#include "editing/audio_pitch_analysis.hpp"
#include <string>
#include <memory>
#include <algorithm>
#include <map>
#include <thread>
#include <mutex>
#include <condition_variable>
#include <atomic>
#include <cmath>
#include "../dsp/iprocessor.hpp"
#include "../dsp/utils/dsp_utils.hpp"
#include "../dsp/analysis/time_stretcher.hpp"

#include "../io/mmap_audio_file.hpp"
#include "id_generator.hpp"

namespace Aura::Rendering { class WaveformOverview; }

namespace Aura::Core {

/**
 * @interface IAudioSource
 */
class IAudioSource {
public:
    virtual ~IAudioSource() = default;
    virtual float getSample(uint32_t channel, uint64_t sampleIdx) const = 0;
    
    virtual void getSample(double pos, float& l, float& r) const {
        l = r = 0.0f;
        const uint64_t count = getNumSamples();
        if (count == 0 || !std::isfinite(pos) || pos < 0.0) return;
        const double bounded = std::min(pos, static_cast<double>(count - 1));
        const uint64_t base = static_cast<uint64_t>(bounded);
        const float t = static_cast<float>(bounded - static_cast<double>(base));
        auto sample = [&](uint32_t channel, int64_t index) {
            const uint64_t clamped = static_cast<uint64_t>(std::clamp<int64_t>(
                index, 0, static_cast<int64_t>(count - 1)));
            const float value = getSample(channel, clamped);
            return std::isfinite(value) ? value : 0.0f;
        };
        auto hermite = [&](uint32_t channel) {
            const float y0 = sample(channel, static_cast<int64_t>(base) - 1);
            const float y1 = sample(channel, static_cast<int64_t>(base));
            const float y2 = sample(channel, static_cast<int64_t>(base) + 1);
            const float y3 = sample(channel, static_cast<int64_t>(base) + 2);
            const float c0 = y1;
            const float c1 = 0.5f * (y2 - y0);
            const float c2 = y0 - 2.5f * y1 + 2.0f * y2 - 0.5f * y3;
            const float c3 = 0.5f * (y3 - y0) + 1.5f * (y1 - y2);
            const float value = ((c3 * t + c2) * t + c1) * t + c0;
            return std::isfinite(value) ? value : 0.0f;
        };
        l = hermite(0);
        r = getNumChannels() > 1 ? hermite(1) : l;
    }

    virtual uint64_t getNumSamples() const = 0;
    virtual uint32_t getNumChannels() const = 0;
    virtual double getSampleRate() const { return 44100.0; }
    virtual std::string getFilePath() const { return ""; }
};

/**
 * @class ResamplingAudioSource
 */
class ResamplingAudioSource : public IAudioSource {
public:
    ResamplingAudioSource(std::shared_ptr<IAudioSource> src, double targetSR) 
        : m_source(src), m_targetSR(targetSR) {
        const double sourceRate = src ? src->getSampleRate() : 0.0;
        m_ratio = (src && std::isfinite(sourceRate) && sourceRate > 0.0 &&
                   std::isfinite(targetSR) && targetSR > 0.0)
            ? sourceRate / targetSR : 1.0;
    }
    
    float getSample(uint32_t c, uint64_t s) const override {
        if (!m_source || c >= m_source->getNumChannels() ||
            !std::isfinite(m_ratio) || m_ratio <= 0.0) return 0.0f;
        double sourcePos = s * m_ratio;
        uint64_t p1 = static_cast<uint64_t>(sourcePos);
        float t = static_cast<float>(sourcePos - p1);

        uint64_t n = m_source->getNumSamples();
        if (n == 0) return 0.0f;
        auto getS = [&](int64_t idx) { 
            return m_source->getSample(c, std::clamp<int64_t>(idx, 0, n - 1)); 
        };

        return ::Aura::DSP::Utils::DSPUtils::interpolateHermite(getS(p1-1), getS(p1), getS(p1+1), getS(p1+2), t);
    }
    
    uint64_t getNumSamples() const override {
        if (!m_source || !std::isfinite(m_ratio) || m_ratio <= 0.0) return 0;
        return static_cast<uint64_t>(m_source->getNumSamples() / m_ratio);
    }
    uint32_t getNumChannels() const override { return m_source ? m_source->getNumChannels() : 0; }
    double getSampleRate() const override {
        return std::isfinite(m_targetSR) && m_targetSR > 0.0 ? m_targetSR : 44100.0;
    }
private:
    std::shared_ptr<IAudioSource> m_source;
    double m_targetSR, m_ratio;
};

class AudioRegion {
public:
    struct GainRange { uint64_t start; uint64_t end; float gain; };
    struct FadeRange { uint64_t start; uint64_t end; uint64_t fadeIn; uint64_t fadeOut; };
    struct Meta {
        uint32_t id;
        std::string name;
        uint64_t samplePosition;
        uint64_t sampleOffset;
        uint64_t sampleLength;
        float sourceBPM = 120.0f; 
        float clipGain = 1.0f;
        uint64_t fadeInSamples = 64;
        uint64_t fadeOutSamples = 64;
        bool reverse = false;
        bool followProjectTempo = true;
        bool isMuted = false;
        double timelineStartBeats = 0.0;
        double lengthBeats = 0.0;
    };

    AudioRegion(std::shared_ptr<IAudioSource> source, Meta m, double projectSR = 44100.0, float projectBPM = 120.0f) 
        : m_source(source), m_meta(std::move(m)) {
        if (!std::isfinite(m_meta.clipGain)) m_meta.clipGain = 1.0f;
        m_meta.clipGain = std::clamp(m_meta.clipGain, 0.0f, 4.0f);
        m_meta.fadeInSamples = std::min(m_meta.fadeInSamples, m_meta.sampleLength);
        m_meta.fadeOutSamples = std::min(m_meta.fadeOutSamples, m_meta.sampleLength);
        
        if (source && std::abs(source->getSampleRate() - projectSR) > 0.01) {
            m_source = std::make_shared<ResamplingAudioSource>(source, projectSR);
        }
        
        m_warpRatio = 1.0;
        if (m_meta.followProjectTempo && m_meta.sourceBPM > 10.0f &&
            std::isfinite(projectBPM) && projectBPM > 10.0f) {
            m_warpRatio = (double)m_meta.sourceBPM / projectBPM; 
        }
        publishAudioNoteSnapshot();
    }

    void render(float* outL, float* outR, uint64_t timelineStart, uint32_t len) const {
        if (!outL || !outR || !m_source || len == 0 || m_meta.isMuted ||
            !std::isfinite(m_meta.clipGain) || m_meta.clipGain < 0.0001f ||
            m_meta.sampleLength == 0) return;
        
        uint64_t rStart = (timelineStart >= m_meta.samplePosition) ? (timelineStart - m_meta.samplePosition) : 0;
        
        for (uint32_t s = 0; s < len; ++s) {
            uint64_t rIdx = rStart + s;
            if (rIdx >= m_meta.sampleLength) break;

            const bool rangeMuted = std::any_of(
                m_mutedRanges.begin(), m_mutedRanges.end(),
                [rIdx](const auto& range) { return rIdx >= range.first && rIdx < range.second; });
            if (rangeMuted) continue;
            float rangeGain = 1.0f;
            for (const auto& range : m_gainRanges) {
                if (rIdx >= range.start && rIdx < range.end) rangeGain *= range.gain;
            }
            for (const auto& range : m_fadeRanges) {
                if (rIdx >= range.start && rIdx < range.end) {
                    const auto length = static_cast<float>(range.end - range.start);
                    const auto offset = static_cast<float>(rIdx - range.start);
                    const float inGain = range.fadeIn == 0 ? 1.0f :
                        std::clamp((offset + 1.0f) / static_cast<float>(range.fadeIn), 0.0f, 1.0f);
                    const float outGain = range.fadeOut == 0 ? 1.0f :
                        std::clamp((length - offset) / static_cast<float>(range.fadeOut), 0.0f, 1.0f);
                    rangeGain *= inGain * outGain;
                }
            }

            float fade = 1.0f;
            if (m_meta.fadeInSamples > 0 && rIdx < m_meta.fadeInSamples) {
                fade = (float)rIdx / (float)m_meta.fadeInSamples;
            } else if (m_meta.fadeOutSamples > 0 &&
                       m_meta.sampleLength > m_meta.fadeOutSamples &&
                       rIdx >= (m_meta.sampleLength - m_meta.fadeOutSamples)) {
                fade = (float)(m_meta.sampleLength - rIdx) /
                       (float)m_meta.fadeOutSamples;
            }

            double pitchRatio = 1.0;
            double formantCents = 0.0;
            const double relativeSeconds = static_cast<double>(rIdx) / getSampleRate();
            const auto noteSnapshot = std::atomic_load_explicit(&m_audioNoteSnapshot,
                                                                std::memory_order_acquire);
            for (const auto& segment : noteSnapshot ? *noteSnapshot : m_audioNoteSegments) {
                if (relativeSeconds >= segment.startSeconds && relativeSeconds <= segment.endSeconds) {
                    const double cents = segment.pitchAt(relativeSeconds);
                    const double formant = segment.formantAt(relativeSeconds);
                    if (std::isfinite(cents)) pitchRatio = std::pow(2.0, std::clamp(cents, -4800.0, 4800.0) / 1200.0);
                    if (std::isfinite(formant)) formantCents = std::clamp(formant, -2400.0, 2400.0);
                    break;
                }
            }
            double sourcePos = rIdx * m_warpRatio * pitchRatio;
            if (!std::isfinite(sourcePos) || sourcePos < 0.0) continue;
            double finalSourcePos = m_meta.reverse ? (m_meta.sampleLength - 1.0 - sourcePos) : sourcePos;
            
            float sL = 0, sR = 0;
            if (std::abs(m_warpRatio - 1.0) < 0.0001) {
                 sL = getInterpolatedSample(0, finalSourcePos);
                 sR = getInterpolatedSample(1, finalSourcePos);
            } else {
                 m_stretcher.process(m_source.get(), finalSourcePos, m_warpRatio, sL, sR);
            }

            const float tilt = static_cast<float>(formantCents / 2400.0) * 0.5f;
            const float nextL = getInterpolatedSample(0, finalSourcePos + 1.0);
            const float nextR = getInterpolatedSample(1, finalSourcePos + 1.0);
            sL = std::clamp(sL + (nextL - sL) * tilt, -2.0f, 2.0f);
            sR = std::clamp(sR + (nextR - sR) * tilt, -2.0f, 2.0f);
            outL[s] += sL * m_meta.clipGain * fade * rangeGain;
            outR[s] += sR * m_meta.clipGain * fade * rangeGain;
        }
    }

    /// Add a non-destructive mute range in source samples. Ranges are clipped
    /// to the region and merged on render without touching the source file.
    bool muteRange(uint64_t start, uint64_t end) {
        if (start >= end || start >= m_meta.sampleLength || m_mutedRanges.size() >= 4096) return false;
        end = std::min(end, m_meta.sampleLength);
        m_mutedRanges.emplace_back(start, end);
        std::sort(m_mutedRanges.begin(), m_mutedRanges.end());
        std::vector<std::pair<uint64_t, uint64_t>> merged;
        for (const auto& range : m_mutedRanges) {
            if (!merged.empty() && range.first <= merged.back().second) {
                merged.back().second = std::max(merged.back().second, range.second);
            } else {
                merged.push_back(range);
            }
        }
        m_mutedRanges = std::move(merged);
        return true;
    }

    void clearMutedRanges() { m_mutedRanges.clear(); }

    const std::vector<std::pair<uint64_t, uint64_t>>& getMutedRanges() const noexcept {
        return m_mutedRanges;
    }

    bool applyGainRange(uint64_t start, uint64_t end, float gain) {
        if (start >= end || start >= m_meta.sampleLength || !std::isfinite(gain) ||
            gain < 0.0f || gain > 16.0f) return false;
        end = std::min(end, m_meta.sampleLength);
        for (auto& range : m_gainRanges) {
            if (std::abs(range.gain - gain) < 1.0e-6f &&
                range.end >= start && end >= range.start) {
                range.start = std::min(range.start, start);
                range.end = std::max(range.end, end);
                return true;
            }
        }
        if (m_gainRanges.size() >= 4096) return false;
        m_gainRanges.push_back({start, end, gain});
        std::sort(m_gainRanges.begin(), m_gainRanges.end(),
                  [](const GainRange& a, const GainRange& b) {
                      if (a.start != b.start) return a.start < b.start;
                      return a.end < b.end;
                  });
        return true;
    }

    void clearGainRanges() { m_gainRanges.clear(); }

    const std::vector<GainRange>& getGainRanges() const noexcept { return m_gainRanges; }

    bool applyFadeRange(uint64_t start, uint64_t end, uint64_t fadeIn, uint64_t fadeOut) {
        if (start >= end || start >= m_meta.sampleLength || m_fadeRanges.size() >= 4096) return false;
        m_fadeRanges.push_back({start, std::min(end, m_meta.sampleLength),
                                std::min(fadeIn, end - start), std::min(fadeOut, end - start)});
        std::sort(m_fadeRanges.begin(), m_fadeRanges.end(),
                  [](const FadeRange& a, const FadeRange& b) {
                      if (a.start != b.start) return a.start < b.start;
                      return a.end < b.end;
                  });
        return true;
    }

    void clearFadeRanges() { m_fadeRanges.clear(); }

    const std::vector<FadeRange>& getFadeRanges() const noexcept { return m_fadeRanges; }

    float getInterpolatedSample(uint32_t chan, double pos) const {
        if (!m_source) return 0.0f;
        uint64_t n = m_source->getNumSamples();
        if (n == 0 || chan >= m_source->getNumChannels() || !std::isfinite(pos)) return 0.0f;
        int64_t p = static_cast<int64_t>(std::floor(pos));
        float t = static_cast<float>(pos - p);
        
        auto getS = [&](int64_t idx) {
            const int64_t sourceOffset = static_cast<int64_t>(std::min<uint64_t>(
                m_meta.sampleOffset, static_cast<uint64_t>(INT64_MAX)));
            const int64_t raw = sourceOffset > INT64_MAX - idx ? INT64_MAX : sourceOffset + idx;
            uint64_t safeIdx = static_cast<uint64_t>(std::clamp<int64_t>(raw, 0, static_cast<int64_t>(n - 1)));
            return m_source->getSample(chan, safeIdx);
        };

        return ::Aura::DSP::Utils::DSPUtils::interpolateHermite(getS(p-1), getS(p), getS(p+1), getS(p+2), t);
    }

    uint32_t getId() const { return m_meta.id; }
    uint64_t getSamplePosition() const { return m_meta.samplePosition; }
    void setSamplePosition(uint64_t pos) { m_meta.samplePosition = pos; }
    bool setClipGain(float gain) {
        if (!std::isfinite(gain)) return false;
        m_meta.clipGain = std::clamp(gain, 0.0f, 4.0f);
        return true;
    }
    bool setFades(uint64_t fadeIn, uint64_t fadeOut) {
        m_meta.fadeInSamples = std::min(fadeIn, m_meta.sampleLength);
        m_meta.fadeOutSamples = std::min(fadeOut, m_meta.sampleLength);
        return true;
    }
    uint64_t getSampleLength() const { return m_meta.sampleLength; }
    std::shared_ptr<IAudioSource> getSource() const { return m_source; }
    uint64_t getSourceSampleCount() const { return m_source ? m_source->getNumSamples() : 0; }
    const Meta& getMeta() const { return m_meta; }
    void setTimelineStartBeats(double beats) { m_meta.timelineStartBeats = beats; }

    std::shared_ptr<AudioRegion> split(uint64_t relativeSample) {
        if (relativeSample <= 0 || relativeSample >= m_meta.sampleLength) return nullptr;
        
        Meta newMeta = m_meta;
        if (m_meta.samplePosition > UINT64_MAX - relativeSample ||
            m_meta.sampleOffset > UINT64_MAX - relativeSample) return nullptr;
        newMeta.samplePosition += relativeSample;
        newMeta.sampleOffset += relativeSample;
        newMeta.sampleLength -= relativeSample;
        newMeta.id = Core::IDGenerator::nextRegionID();

        m_meta.sampleLength = relativeSample; 
        return std::make_shared<AudioRegion>(m_source, std::move(newMeta));
    }

    std::shared_ptr<AudioRegion> clone() const {
        return std::make_shared<AudioRegion>(m_source, m_meta, getSampleRate(),
                                             m_meta.sourceBPM);
    }

    double getSampleRate() const noexcept {
        return m_source ? m_source->getSampleRate() : 44100.0;
    }

    void setWaveOverview(std::shared_ptr<Rendering::WaveformOverview> ov) { m_waveOverview = ov; }

    // VariAudio-style non-destructive note edits live with the region so they
    // follow trims, duplication and project saves without rewriting source.
    bool upsertAudioNoteSegment(::aura::editing::AudioNoteSegment segment) {
        if (!segment.valid()) return false;
        for (const auto& current : m_audioNoteSegments) {
            const bool sameStart = std::abs(current.startSeconds - segment.startSeconds) < 1e-9;
            const bool overlaps = segment.startSeconds < current.endSeconds &&
                                  current.startSeconds < segment.endSeconds;
            if (overlaps && !sameStart) return false;
        }
        auto it = std::find_if(m_audioNoteSegments.begin(), m_audioNoteSegments.end(),
            [&](const ::aura::editing::AudioNoteSegment& current) {
                return std::abs(current.startSeconds - segment.startSeconds) < 1e-9;
            });
        if (it == m_audioNoteSegments.end()) m_audioNoteSegments.push_back(std::move(segment));
        else *it = std::move(segment);
        std::sort(m_audioNoteSegments.begin(), m_audioNoteSegments.end(),
            [](const auto& a, const auto& b) { return a.startSeconds < b.startSeconds; });
        publishAudioNoteSnapshot();
        return true;
    }

    void clearAudioNoteSegments() { m_audioNoteSegments.clear(); publishAudioNoteSnapshot(); }

    const std::vector<::aura::editing::AudioNoteSegment>& getAudioNoteSegments() const noexcept {
        return m_audioNoteSegments;
    }
    std::vector<::aura::editing::AudioNoteSegment> getAudioNoteSegmentsSnapshot() const {
        const auto snapshot = std::atomic_load_explicit(&m_audioNoteSnapshot,
                                                         std::memory_order_acquire);
        return snapshot ? *snapshot : std::vector<::aura::editing::AudioNoteSegment>{};
    }

    // Run analysis off the audio callback and replace only the analysis layer;
    // source samples and ordinary clip edits remain untouched.
    bool analyzeAudioNotes(const float* monoSamples, std::size_t sampleCount,
                           double sampleRate,
                           ::aura::editing::AudioPitchAnalyzer::Config config) {
        if ((sampleCount > 0 && monoSamples == nullptr) ||
            sampleCount > 16'000'000 || !std::isfinite(sampleRate) ||
            sampleRate < 8'000.0 || sampleRate > 384'000.0) {
            return false;
        }
        auto detected = ::aura::editing::AudioPitchAnalyzer::analyze(
            monoSamples, sampleCount, sampleRate, config);
        m_audioNoteSegments = std::move(detected);
        publishAudioNoteSnapshot();
        return true;
    }

    bool analyzeAudioNotes(const float* monoSamples, std::size_t sampleCount,
                           double sampleRate) {
        return analyzeAudioNotes(monoSamples, sampleCount, sampleRate,
            ::aura::editing::AudioPitchAnalyzer::Config{});
    }

private:
    void publishAudioNoteSnapshot() const {
        auto snapshot = std::make_shared<const std::vector<::aura::editing::AudioNoteSegment>>(m_audioNoteSegments);
        std::atomic_store_explicit(&m_audioNoteSnapshot, std::move(snapshot), std::memory_order_release);
    }
    std::shared_ptr<IAudioSource> m_source;
    Meta m_meta;
    double m_warpRatio = 1.0;
    mutable ::Aura::DSP::Analysis::SovereignTimeStretcher m_stretcher;
    std::shared_ptr<Rendering::WaveformOverview> m_waveOverview;
    std::vector<::aura::editing::AudioNoteSegment> m_audioNoteSegments;
    mutable std::shared_ptr<const std::vector<::aura::editing::AudioNoteSegment>> m_audioNoteSnapshot;
    std::vector<std::pair<uint64_t, uint64_t>> m_mutedRanges;
    std::vector<GainRange> m_gainRanges;
    std::vector<FadeRange> m_fadeRanges;
};

} // namespace Aura::Core
