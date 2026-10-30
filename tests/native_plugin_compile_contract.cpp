// The ARUA project codec is Rust-owned; this generated header exposes the cxx
// bridge types the contract uses to round-trip a project container.
#include "hirari-core-bridge/src/lib.rs.h"
#include "src/core/plugins/vst3_host_processor.hpp"
#include "src/core/plugins/plugin_sandbox_host.hpp"
#include "src/core/plugins/process_sandbox_processor.hpp"
#include "src/core/plugins/au_sandbox_adapter.hpp"
#include "src/core/plugins/plugin_cache_manager.hpp"
#include "src/core/effect_chain.hpp"
#include "src/core/project_automation.hpp"
#include "src/io/wav_loader_utils.hpp"
#include "src/core/engine/track_freeze_manager.hpp"
#include "src/core/engine/undo_transaction_manager.hpp"
#include "src/core/midi_buffer.hpp"
#include "src/core/plugins/midi_fragment_transport.hpp"
#include "src/core/concurrency/audio_task_manager.hpp"
#include "src/core/concurrency/thread_pool.hpp"
#include "src/core/plugin_host.hpp"
#include "src/core/database/project_db.hpp"
#include "src/core/external/jack_bridge_deep.hpp"
#include "src/core/HirariPluginSDK.hpp"
#include <atomic>
#include <array>
#include <chrono>
#include <filesystem>
#include <fstream>
#include <limits>
#include <string>
#include <thread>
#include <utility>

namespace {
class SDKContractProcessor final : public Hirari::SDK::IProcessor {
public:
    std::string getName() const override { return "SDK Contract"; }
    void prepareToPlay(double, uint32_t) override {}
    void process(::Hirari::Core::AudioBuffer&, ::Hirari::Core::MidiBuffer&, const Hirari::SDK::ProcessContext&) override {}
    void reset() override {}
};

class SDKContractFactory final : public Hirari::SDK::IPluginFactory {
public:
    Hirari::SDK::PluginDescriptor getDescriptor() const override {
        Hirari::SDK::PluginDescriptor descriptor;
        descriptor.identifier = "com.hirari.sdk.contract";
        descriptor.name = "SDK Contract";
        descriptor.vendor = "Hirari";
        descriptor.version = "1.0.0";
        descriptor.parameters.push_back({1, "Mix", 0.0f, 1.0f, 1.0f, true});
        descriptor.supportsSidechain = true;
        descriptor.sidechainChannels = 2;
        descriptor.components.push_back({"main", 2, 2, false});
        descriptor.components.push_back({"sidechain", 2, 2, true});
        return descriptor;
    }
    std::unique_ptr<Hirari::SDK::IProcessor> create() const override {
        return std::make_unique<SDKContractProcessor>();
    }
};

// The UndoContractCommand scaffolding that used to live here derived from
// Hirari::Core::Undo::Command, which the Rust migration deleted
// (src/core/undo/undo_manager.hpp was removed in b6c5e68). The transactional
// undo guarantee it covered still exists and is now exercised against the
// Rust-owned manager in native_plugin_compile_contract_part_1.inc, so no
// coverage was dropped.

}

#include "native_plugin_compile_contract_part_1.inc"
#include "native_plugin_compile_contract_part_2.inc"
#include "native_plugin_compile_contract_part_3.inc"
