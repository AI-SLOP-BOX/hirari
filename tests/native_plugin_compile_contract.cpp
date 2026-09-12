#include "src/core/audio_pool.hpp"
#include "src/core/plugins/vst3_host_processor.hpp"
#include "src/core/plugins/clap_host_processor.hpp"
#include "src/core/plugins/plugin_sandbox_host.hpp"
#include "src/core/plugins/process_sandbox_processor.hpp"
#include "src/core/plugin_host/clap_host_interface.hpp"
#include "src/core/plugins/au_sandbox_adapter.hpp"
#include "src/core/plugins/au_host_processor.hpp"
#include "src/core/plugins/plugin_cache_manager.hpp"
#include "src/core/plugins/plugin_host_infrastructure.hpp"
#include "src/core/plugin_host/plugin_wrapper_lv2_vst3.hpp"
#include "src/core/effect_chain.hpp"
#include "src/core/audio_processor_graph.hpp"
#include "src/io/persistence/project_collector.hpp"
#include "src/core/project_serializer.hpp"
#include "src/io/persistence/project_serializer.hpp"
#include "src/io/mmap_audio_file.hpp"
#include "src/io/wav_loader_utils.hpp"
#include "src/io/audio_export_engine.hpp"
#include "src/io/assets/caching_subsystem.hpp"
#include "src/io/assets/audio_asset_library.hpp"
#include "src/core/io/bounce_system.hpp"
#include "src/core/io/ffmpeg_engine.hpp"
#include "src/core/offline_renderer.hpp"
#include "src/core/io/session_io.hpp"
#include "src/core/engine/latency_manager.hpp"
#include "src/core/engine/midi_orchestrator.hpp"
#include "src/core/engine/track_freeze_manager.hpp"
#include "src/core/undo/undo_manager.hpp"
#include "src/core/engine/undo_transaction_manager.hpp"
#include "src/core/midi_buffer.hpp"
#include "src/core/plugins/midi_fragment_transport.hpp"
#include "src/dsp/synthesis/sampler_engine.hpp"
#include "src/synthesis/sampler_map.hpp"
#include "src/core/recording_engine.hpp"
#include "src/io/persistence/auto_save_engine.hpp"
#include "src/io/persistence/export_manager.hpp"
#include "src/io/parallel_asset_manager.hpp"
#include "src/network/shared_memory_bridge.hpp"
#include "src/core/concurrency/audio_task_manager.hpp"
#include "src/core/concurrency/thread_pool.hpp"
#include "src/rendering/bounce/bounce_engine.hpp"
#include "src/rendering/bounce/bouncing_engine.hpp"
#include "src/core/engine/bounce_engine.hpp"
#include "src/core/io/bounce_system.hpp"
#include "src/ui/waveform_cache.hpp"
#include "src/ai/local_ai_models.hpp"
#include "src/core/security/security_manager.hpp"
#include "src/core/security/license_vault.hpp"
#include "src/dsp/mixing/neural_dynamics_model.hpp"
#include "src/dsp/mixing/sidechain_manager.hpp"
#include "src/dsp/analysis/spectral_processor.hpp"
#include "src/dsp/effects/console_model.hpp"
#include "src/core/dsp/effects/procedural_foley_kernel.hpp"
#include "src/dsp/effects/spectral_ducker.hpp"
#include "src/dsp/effects/arpeggiator.hpp"
#include "src/dsp/effects/chorus.hpp"
#include "src/dsp/effects/compressor.hpp"
#include "src/io/persistence/project_encoder.hpp"
#include "src/core/database/project_db.hpp"
#include "src/io/persistence/wav_writer.hpp"
#include "src/core/utils/wav_writer.hpp"
#include "src/io/audio_interface.hpp"
#include "src/core/external/audio_driver_pro.hpp"
#include "src/core/external/jack_bridge_deep.hpp"
#include "src/core/AuraPluginSDK.hpp"
#include <atomic>
#include <array>
#include <chrono>
#include <filesystem>
#include <fstream>
#include <limits>
#include <string>
#include <thread>
#if !defined(_WIN32)
#include <unistd.h>
#endif

namespace {
class SDKContractProcessor final : public Aura::SDK::IProcessor {
public:
    std::string getName() const override { return "SDK Contract"; }
    void prepareToPlay(double, uint32_t) override {}
    void process(::Aura::Core::AudioBuffer&, ::Aura::Core::MidiBuffer&, const Aura::SDK::ProcessContext&) override {}
    void reset() override {}
};

class SDKContractFactory final : public Aura::SDK::IPluginFactory {
public:
    Aura::SDK::PluginDescriptor getDescriptor() const override {
        Aura::SDK::PluginDescriptor descriptor;
        descriptor.identifier = "com.aura.sdk.contract";
        descriptor.name = "SDK Contract";
        descriptor.vendor = "Aura";
        descriptor.version = "1.0.0";
        descriptor.parameters.push_back({1, "Mix", 0.0f, 1.0f, 1.0f, true});
        descriptor.supportsSidechain = true;
        descriptor.sidechainChannels = 2;
        descriptor.components.push_back({"main", 2, 2, false});
        descriptor.components.push_back({"sidechain", 2, 2, true});
        return descriptor;
    }
    std::unique_ptr<Aura::SDK::IProcessor> create() const override {
        return std::make_unique<SDKContractProcessor>();
    }
};

class UndoContractCommand final : public Aura::Core::Undo::Command {
public:
    UndoContractCommand(int& value, int delta, std::string name)
        : m_value(value), m_delta(delta), m_name(std::move(name)) {}
    void execute() override { m_value += m_delta; }
    void undo() override { m_value -= m_delta; }
    std::string getName() const override { return m_name; }
private:
    int& m_value;
    int m_delta;
    std::string m_name;
};

class NonFiniteProcessor final : public Aura::DSP::IProcessor {
public:
    void prepareToPlay(double, uint32_t) noexcept override {}
    void process(Aura::Core::AudioBuffer& buffer, Aura::Core::MidiBuffer&, const Aura::DSP::ProcessContext&) noexcept override {
        for (uint32_t channel = 0; channel < buffer.getNumChannels(); ++channel) {
            float* samples = buffer.getWritePointer(channel);
            for (uint32_t index = 0; index < buffer.getNumSamples(); ++index) {
                samples[index] = (index % 2u == 0u)
                    ? std::numeric_limits<float>::quiet_NaN()
                    : std::numeric_limits<float>::infinity();
            }
        }
    }
    void reset() noexcept override {}
    std::string getName() const override { return "NonFiniteContract"; }
};

}

#include "native_plugin_compile_contract_part_1.inc"
#include "native_plugin_compile_contract_part_2.inc"
#include "native_plugin_compile_contract_part_3.inc"
