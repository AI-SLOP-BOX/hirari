/*
 * Aura DAW Ultimate - High-Performance Digital Audio Workstation
 * Copyright (c) 2024-2026 Aura DAW Project. All rights reserved.
 * Licensed under the MIT License.
 */

#pragma once

#if defined(_WIN32)
#include <process.h>
#else
#include <unistd.h>
#endif

#include "core/engine/timeline_system.hpp"
#include "core/engine/tempo_map.hpp"
#include "core/engine/track.hpp"
#include "core/engine/step_sequencer.hpp"
#include "core/engine/undo_transaction_manager.hpp"
#include "core/engine/plugin_sandbox.hpp"
#include "core/engine/bounce_engine.hpp"
#include "core/recording_engine.hpp"
#include "core/asset_manager.hpp"
#include "core/version_control/evolution_manager.hpp"
#include "core/aura_unified_engine.hpp"
#include "core/project_serializer.hpp"
#include "external/nlohmann/json.hpp"
#include "core/id_generator.hpp"
#include "io/assets/streaming_source.hpp"
#include "io/assets/streaming_engine.hpp"
#include "dsp/synthesis/virtuoso_drum_synth.hpp"
#include "dsp/synthesis/virtuoso_stradivari.hpp"
#include "dsp/effects/virtuoso_pitch.hpp"
#include "dsp/effects/virtuoso_vocal.hpp"
#include "dsp/effects/virtuoso_space.hpp"
#include "core/engine/snapshot_manager.hpp"
#include "core/engine/pdc_manager.hpp"
#include "dsp/mixing/master_suite.hpp"
#include "rendering/waveform_overview.hpp"
#include <iostream>
#include <atomic>
#include <cmath>
#include <cstring>
#include <filesystem>
#include <fstream>
#include <limits>
#include <thread>

namespace Aura {

class AuraEngine {
public:
    static AuraEngine& getInstance() { static AuraEngine i; return i; }

    Core::Engine::TimelineSystem& getTimeline() { return *m_timeline; }
    
    AuraEngine() 
        : m_timeline(std::make_unique<Core::Engine::TimelineSystem>()),
          m_masterSuite(std::make_unique<DSP::Mixing::MasterSuite>()),
          m_stepSequencer(std::make_unique<Core::Engine::StepSequencer>()),
          m_drumSynth(std::make_unique<DSP::Synthesis::VirtuosoDrumSynth>()),
          m_stringSynth(std::make_unique<DSP::Synthesis::VirtuosoStradivari>()),
          m_pitchCorrector(std::make_unique<DSP::Effects::VirtuosoPitch>()),
          m_vocalTransformer(std::make_unique<DSP::Effects::VirtuosoVocal>()),
          m_reverb(std::make_unique<DSP::Effects::VirtuosoSpace>())
    {
        m_masterBuf.resize(2, Core::Engine::TimelineSystem::kMaxBlockSize);
        m_masterSuite->prepareToPlay(m_sampleRate, m_blockSize);
    }

    DSP::Synthesis::VirtuosoDrumSynth* getDrumSynth() { return m_drumSynth.get(); }
    DSP::Synthesis::VirtuosoStradivari* getStringSynth() { return m_stringSynth.get(); }
    DSP::Effects::VirtuosoPitch* getPitchCorrector() { return m_pitchCorrector.get(); }
    DSP::Effects::VirtuosoVocal* getVocalTransformer() { return m_vocalTransformer.get(); }
    DSP::Effects::VirtuosoSpace* getReverb() { return m_reverb.get(); }
    Core::Engine::StepSequencer* getStepSequencer() { return m_stepSequencer.get(); }

    void startRecording(std::string_view path, uint32_t trackId) {
        if (path.empty()) {
            ::Aura::Core::Diagnostics::LogBuffer::post(2, 0, "SYSTEM | ERROR | RECORDING_PATH_EMPTY");
            return;
        }
        if (m_isRecording.load(std::memory_order_acquire)) {
            stopRecording();
        }
        m_recordingTrackId = trackId;
        m_recordingFault.store(false, std::memory_order_release);
        if (m_recorder.start(std::string(path), m_sampleRate)) {
            m_isRecording.store(true, std::memory_order_release);
        } else {
            ::Aura::Core::Diagnostics::LogBuffer::post(2, 0, "SYSTEM | ERROR | RECORDING_START_FAILED");
        }
    }

    void stopRecording() {
        m_isRecording.store(false, std::memory_order_release);
        m_recorder.stop();
    }

    bool saveProject(const std::string& path) {
        if (path.empty()) return false;
        const bool ok = Core::Engine::AuraUnifiedEngine::getInstance().saveProject(path);
        if (!ok) std::cerr << "[Aura] Project save failed: " << path << std::endl;
        return ok;
    }

    bool loadProject(const std::string& path) {
        if (path.empty()) return false;
        const bool ok = Core::Engine::AuraUnifiedEngine::getInstance().loadProject(path);
        if (!ok) std::cerr << "[Aura] Project load failed: " << path << std::endl;
        return ok;
    }

    std::string serializeState() {
        const std::string tempPath = temporaryStatePath();
        if (Core::Engine::AuraUnifiedEngine::getInstance().saveProject(tempPath)) {
            std::ifstream file(tempPath);
            if (file.is_open()) {
                std::string content((std::istreambuf_iterator<char>(file)),
                                    std::istreambuf_iterator<char>());
                file.close();
                std::error_code ec;
                std::filesystem::remove(tempPath, ec);
                return content;
            }
        }
        std::error_code ec;
        std::filesystem::remove(tempPath, ec);
        return "";
    }

    bool restoreState(const std::string& state) {
        if (state.empty()) return false;
        const std::string tempPath = temporaryStatePath();
        bool restored = false;
        std::ofstream file(tempPath);
        if (file.is_open()) {
            file << state;
            file.flush();
            if (file) {
                file.close();
                restored = Core::Engine::AuraUnifiedEngine::getInstance().loadProject(tempPath);
            } else {
                file.close();
            }
        }
        std::error_code ec;
        std::filesystem::remove(tempPath, ec);
        return restored;
    }

    void prepareToPlay(double sr, uint32_t bs) {
        if (!std::isfinite(sr) || sr <= 0.0 || bs == 0 ||
            bs > Core::Engine::TimelineSystem::kMaxBlockSize) return;
        m_sampleRate = sr;
        m_blockSize = bs;
        m_timeline->prepare(sr, bs);
        // Allocate on the control thread. The audio callback must only reuse
        // this storage and never resize it.
        m_masterBuf.resize(2, Core::Engine::TimelineSystem::kMaxBlockSize);
        m_masterSuite->prepareToPlay(sr, bs);
    }

    /**
     * @brief Process audio block and update timeline playhead.
     */
    void process(float* outL, float* outR, uint32_t numSamples) {
        if (outL == nullptr || outR == nullptr || numSamples == 0 ||
            numSamples > m_blockSize ||
            numSamples > m_masterBuf.getNumSamples()) {
            if (outL && outR && numSamples > 0) {
                std::fill_n(outL, numSamples, 0.0f);
                std::fill_n(outR, numSamples, 0.0f);
            }
            return;
        }
        ::Aura::DSP::ProcessContext ctx{};
        ctx.playhead = m_timeline->getCurrentPos();
        if (ctx.playhead > std::numeric_limits<uint64_t>::max() - numSamples) {
            std::fill_n(outL, numSamples, 0.0f);
            std::fill_n(outR, numSamples, 0.0f);
            ::Aura::Core::Diagnostics::LogBuffer::post(
                2, 0, "AUDIO | ERROR | PLAYHEAD_RANGE_EXHAUSTED");
            return;
        }
        ctx.blockStart = ctx.playhead;
        ctx.blockEnd = ctx.playhead + numSamples;
        ctx.sampleRate = m_sampleRate;
        ctx.blockSize = numSamples;
        ctx.isPlaying = m_timeline->isPlaying();

        Core::Engine::AuraUnifiedEngine::getInstance().processBlock(m_masterBuf, 0, numSamples);

        // AuraUnifiedEngine owns the graph; MasterSuite is the final output
        // stage.  It must run after graph rendering and before recording/copy
        // out, otherwise the advertised mastering controls have no effect on
        // the actual product output.
        m_dummyMidi.clear();
        m_masterSuite->process(m_masterBuf, m_dummyMidi, ctx);

        if (m_isRecording.load(std::memory_order_acquire)) {
            const bool blockAccepted = m_recorder.write(
                m_masterBuf.getReadPointer(0),
                m_masterBuf.getReadPointer(1),
                numSamples
            );
            if (!blockAccepted || m_recorder.hasWriteError()) {
                m_isRecording.store(false, std::memory_order_release);
                m_recordingFault.store(true, std::memory_order_release);
                ::Aura::Core::Diagnostics::LogBuffer::post(
                    2, 0,
                    m_recorder.hasWriteError()
                        ? "SYSTEM | ERROR | RECORDING_DISK_WRITE_FAILED"
                        : "SYSTEM | ERROR | RECORDING_BUFFER_OVERFLOW");
            }
        }

        std::memcpy(outL, m_masterBuf.getReadPointer(0), numSamples * sizeof(float));
        std::memcpy(outR, m_masterBuf.getReadPointer(1), numSamples * sizeof(float));

        if (ctx.isPlaying) m_timeline->setPlayhead(ctx.playhead + numSamples);
    }

    float getBPM() const { return Core::Engine::AuraUnifiedEngine::getInstance().get_tempo(); }
    double getSampleRate() const { return m_sampleRate; }
    uint64_t getCurrentSamplePos() const { return m_timeline->getCurrentPos(); }
    void seekToPos(uint64_t pos) { m_timeline->setPlayhead(pos); }
    void togglePlayback() { m_timeline->setPlaying(!m_timeline->isPlaying()); }
    bool isPlaying() const { return m_timeline->isPlaying(); }

private:
    static std::string temporaryStatePath() {
        std::error_code ec;
        const auto base = std::filesystem::temp_directory_path(ec);
        static std::atomic<uint64_t> sequence{0};
        const auto token = std::hash<std::thread::id>{}(std::this_thread::get_id());
#if defined(_WIN32)
        const auto processId = static_cast<unsigned long long>(::_getpid());
#else
        const auto processId = static_cast<unsigned long long>(::getpid());
#endif
        const auto name = "aura_undo_temp-" + std::to_string(processId) +
            "-" + std::to_string(token) + "-" +
            std::to_string(sequence.fetch_add(1, std::memory_order_relaxed)) + ".json";
        if (ec) return name;
        return (base / name).string();
    }

    double m_sampleRate{44100.0};
    uint32_t m_blockSize{512};
    Core::MidiBuffer m_dummyMidi;
    
    std::unique_ptr<Core::Engine::TimelineSystem> m_timeline;
    std::unique_ptr<DSP::Mixing::MasterSuite> m_masterSuite;
    std::unique_ptr<Core::Engine::StepSequencer> m_stepSequencer;
    std::unique_ptr<DSP::Synthesis::VirtuosoDrumSynth> m_drumSynth;
    std::unique_ptr<DSP::Synthesis::VirtuosoStradivari> m_stringSynth;
    std::unique_ptr<DSP::Effects::VirtuosoPitch> m_pitchCorrector;
    std::unique_ptr<DSP::Effects::VirtuosoVocal> m_vocalTransformer;
    std::unique_ptr<DSP::Effects::VirtuosoSpace> m_reverb;
    Core::RecordingEngine m_recorder;
    std::atomic<bool> m_isRecording{false};
    std::atomic<bool> m_recordingFault{false};
    std::string m_lastRecordingPath;
    uint32_t m_recordingTrackId{0};
    Core::AudioBuffer m_masterBuf;
};

} // namespace Aura
