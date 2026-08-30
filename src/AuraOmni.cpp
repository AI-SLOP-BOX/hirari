#include "AuraOmni.hpp"
#include "core/aura_unified_engine.hpp"
#include "core/plugins/distributed_plugin_host.hpp"
#include "dsp/mixing/master_suite.hpp"
#include <mutex>

namespace Aura::Omni {

// Global definitions for extern variables declared in AuraOmni.hpp
alignas(64) char S[kSessionBytes] = {0};
std::atomic<int> P{0};
std::atomic<uint64_t> EngineState{0};

enum class OmniEngineState : uint32_t { Initialized, Running, Paused, OverloadProtection };
static std::atomic<OmniEngineState> g_state{OmniEngineState::Initialized};

// --- PHASE 41: PLANETARY RESILIENCE BRIDGE ---
static std::unique_ptr<Core::Plugins::DistributedPluginHost> g_remoteBridge;
static std::once_flag g_bridgeInitOnce;
// The C ABI is used by the packaged audio entry point. Keep the final output
// stage alive for the lifetime of the process so the callback never performs
// lazy allocation or plugin-chain construction.
static DSP::Mixing::MasterSuite g_masterSuite;
static Core::MidiBuffer g_masterMidi;

extern "C" {
    void aura_execute(uint32_t op, void* data) {
        auto& engine = Core::Engine::AuraUnifiedEngine::getInstance();
        if (op == 1) {
            engine.set_playing(true);
            // Prepare for the largest supported callback. Device-specific
            // reconfiguration is handled by the native engine; this default
            // keeps the standalone ABI safe before a device is attached.
            g_masterSuite.prepareToPlay(44100.0, Core::Engine::AuraUnifiedEngine::kMaxAudioBlockSize);
            g_state.store(OmniEngineState::Running, std::memory_order_release);
        } else if (op == 2) {
            engine.set_playing(false);
            g_state.store(OmniEngineState::Paused, std::memory_order_release);
        } else if (op == 3 && data != nullptr) {
            engine.saveProject(static_cast<const char*>(data));
        } else if (op == 4 && data != nullptr) {
            engine.loadProject(static_cast<const char*>(data));
        }
        
        // Thread-safe remote bridge initialization on first run
        std::call_once(g_bridgeInitOnce, []() {
            g_remoteBridge = std::make_unique<Core::Plugins::DistributedPluginHost>("PlanetaryNode_A");
        });
    }

    uint64_t aura_sync() { 
        return Core::Engine::AuraUnifiedEngine::getInstance().get_playhead(); 
    }

    /**
     * @brief Audio processing entry point with PLANETARY RESILIENCE.
     */
    void aura_process(float* l, float* r, int n) {
        OmniEngineState currentState = g_state.load(std::memory_order_acquire);
        if (currentState == OmniEngineState::Initialized) return;
        auto& engine = Core::Engine::AuraUnifiedEngine::getInstance();

        if (l == nullptr || r == nullptr || n <= 0 ||
            static_cast<uint32_t>(n) > Core::Engine::AuraUnifiedEngine::kMaxAudioBlockSize) {
            if (l != nullptr && r != nullptr && n > 0 &&
                static_cast<uint32_t>(n) <= Core::Engine::AuraUnifiedEngine::kMaxAudioBlockSize) {
                std::fill_n(l, static_cast<size_t>(n), 0.0f);
                std::fill_n(r, static_cast<size_t>(n), 0.0f);
            }
            return;
        }

        // The ABI has a hard block-size contract.  Keep the callback buffer at
        // its prepared capacity; resizing here would allocate on the realtime
        // thread when a driver reports an unexpected block size.
        static thread_local Core::AudioBuffer wrap(
            2, Core::Engine::AuraUnifiedEngine::kMaxAudioBlockSize);
        
        float* channels[2] = { l, r };
        wrap.wrapChannels(channels, 2, static_cast<uint32_t>(n));

        // --- PHASE 41: AUTONOMOUS ROUTING ---
        if (currentState == OmniEngineState::OverloadProtection && g_remoteBridge) {
            g_remoteBridge->pushToRemote(wrap);
            // Attempt to pull (Wait-free). If remote stalled, it will hit local fallback inside the host.
            if (g_remoteBridge->pullFromRemote(wrap, 1)) return; 
        }

        // Standard local processing
        engine.processBlock(wrap, 0, static_cast<uint32_t>(n));

        ::Aura::DSP::ProcessContext context{};
        context.playhead = engine.get_playhead();
        context.blockStart = context.playhead;
        context.blockEnd = context.playhead <= UINT64_MAX - static_cast<uint32_t>(n)
            ? context.playhead + static_cast<uint32_t>(n)
            : UINT64_MAX;
        context.sampleRate = engine.get_sample_rate();
        context.blockSize = static_cast<uint32_t>(n);
        context.audioConfigGeneration = engine.get_audio_config_generation();
        context.isPlaying = engine.is_playing();
        g_masterMidi.clear();
        g_masterSuite.process(wrap, g_masterMidi, context);
    }
}

} // namespace Aura::Omni
