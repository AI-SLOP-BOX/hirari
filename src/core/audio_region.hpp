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
        l = getSample(0, static_cast<uint64_t>(pos));
        r = (getNumChannels() > 1) ? getSample(1, static_cast<uint64_t>(pos)) : l;
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
    }

    void render(float* outL, float* outR, uint64_t timelineStart, uint32_t len) const {
        if (!outL || !outR || !m_source || len == 0 || m_meta.isMuted ||
            !std::isfinite(m_meta.clipGain) || m_meta.clipGain < 0.0001f ||
            m_meta.sampleLength == 0) return;
        
        uint64_t rStart = (timelineStart >= m_meta.samplePosition) ? (timelineStart - m_meta.samplePosition) : 0;
        
        for (uint32_t s = 0; s < len; ++s) {
            uint64_t rIdx = rStart + s;
            if (rIdx >= m_meta.sampleLength) break;

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
            const double relativeSeconds = static_cast<double>(rIdx) / getSampleRate();
            for (const auto& segment : m_audioNoteSegments) {
                if (relativeSeconds >= segment.startSeconds && relativeSeconds <= segment.endSeconds) {
                    const double cents = segment.pitchAt(relativeSeconds);
                    if (std::isfinite(cents)) pitchRatio = std::pow(2.0, std::clamp(cents, -4800.0, 4800.0) / 1200.0);
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

            outL[s] += sL * m_meta.clipGain * fade;
            outR[s] += sR * m_meta.clipGain * fade;
        }
    }

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
        auto it = std::find_if(m_audioNoteSegments.begin(), m_audioNoteSegments.end(),
            [&](const ::aura::editing::AudioNoteSegment& current) {
                return std::abs(current.startSeconds - segment.startSeconds) < 1e-9;
            });
        if (it == m_audioNoteSegments.end()) m_audioNoteSegments.push_back(std::move(segment));
        else *it = std::move(segment);
        std::sort(m_audioNoteSegments.begin(), m_audioNoteSegments.end(),
            [](const auto& a, const auto& b) { return a.startSeconds < b.startSeconds; });
        return true;
    }

    const std::vector<::aura::editing::AudioNoteSegment>& getAudioNoteSegments() const noexcept {
        return m_audioNoteSegments;
    }

    // Run analysis off the audio callback and replace only the analysis layer;
    // source samples and ordinary clip edits remain untouched.
    bool analyzeAudioNotes(const float* monoSamples, std::size_t sampleCount,
                           double sampleRate,
                           ::aura::editing::AudioPitchAnalyzer::Config config) {
        auto detected = ::aura::editing::AudioPitchAnalyzer::analyze(
            monoSamples, sampleCount, sampleRate, config);
        if (sampleCount > 0 && monoSamples == nullptr) return false;
        m_audioNoteSegments = std::move(detected);
        return true;
    }

    bool analyzeAudioNotes(const float* monoSamples, std::size_t sampleCount,
                           double sampleRate) {
        return analyzeAudioNotes(monoSamples, sampleCount, sampleRate,
            ::aura::editing::AudioPitchAnalyzer::Config{});
    }

private:
    std::shared_ptr<IAudioSource> m_source;
    Meta m_meta;
    double m_warpRatio = 1.0;
    mutable ::Aura::DSP::Analysis::SovereignTimeStretcher m_stretcher;
    std::shared_ptr<Rendering::WaveformOverview> m_waveOverview;
    std::vector<::aura::editing::AudioNoteSegment> m_audioNoteSegments;
};

} // namespace Aura::Core
