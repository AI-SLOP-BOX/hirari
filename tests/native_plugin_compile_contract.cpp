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

int main() {
    {
        auto& driver = Aura::Core::External::AudioDriverPro::getInstance();
        driver.closeStream();
        bool callbackOpened = false;
        bool callbackClosed = false;
        uint32_t callbackFrames = 0;
        driver.bindExternalBackend(
            Aura::Core::External::AudioDriverPro::Backend::WASAPI,
            [&](double rate, int frames) {
                callbackOpened = rate == 48'000.0 && frames == 256;
                return callbackOpened;
            },
            [&] { callbackClosed = true; },
            [&](float* const* outputs, float* const*, uint32_t frames) {
                callbackFrames = frames;
                if (outputs && outputs[0]) outputs[0][0] = 0.25f;
            });
        if (!driver.openStream(Aura::Core::External::AudioDriverPro::Backend::WASAPI,
                               0, 48'000.0, 256) || !callbackOpened) return 83;
        float output[2][4]{};
        float input[2][4]{};
        float* outputs[] = {output[0], output[1]};
        float* inputs[] = {input[0], input[1]};
        if (!driver.processBlock(outputs, inputs, 4) || callbackFrames != 4 || output[0][0] != 0.25f)
            return 82;
        driver.closeStream();
        if (!callbackClosed) return 80;
        driver.bindExternalBackend(
            Aura::Core::External::AudioDriverPro::Backend::WASAPI,
            [](double, int) { return false; }, [] {});

        if (driver.openStream(Aura::Core::External::AudioDriverPro::Backend::WASAPI,
                              0, 48'000.0, 256)) return 84;
        if (driver.isOpen() || driver.lastError().empty()) return 85;
#if !defined(AURA_ENABLE_ASIO_SDK)
        if (driver.openStream(Aura::Core::External::AudioDriverPro::Backend::ASIO,
                              0, 48'000.0, 256)) return 86;
        if (driver.isOpen() || driver.lastError().find("ASIO SDK") == std::string::npos)
            return 87;
#endif
        auto& jack = Aura::Core::External::JackBridgeDeep::getInstance();
        jack.shutdown();
#if !defined(AURA_ENABLE_JACK)
        if (jack.tryInitialize("aura-contract") || jack.isRunning() ||
            jack.lastError().find("AURA_ENABLE_JACK") == std::string::npos) return 88;
#endif
    }
    const std::string xmlProbe = "C:\\Audio & Mix/<take>\"lead\"'";
    if (Aura::Core::IO::SessionIO::unescapeXmlAttribute(
            Aura::Core::IO::SessionIO::escapeXmlAttribute(xmlProbe)) != xmlProbe) return 81;
    const auto sessionProbe = std::filesystem::temp_directory_path() / "aura-session-atomic-contract.xml";
    Aura::Core::IO::SessionIO::getInstance().saveProject(sessionProbe.string(), {});
    Aura::Core::IO::SessionIO::getInstance().saveProject(
        sessionProbe.string(), {std::shared_ptr<Aura::Core::Engine::Track>{}});
    std::ifstream sessionInput(sessionProbe, std::ios::binary);
    const std::string sessionXml((std::istreambuf_iterator<char>(sessionInput)), {});
    std::error_code sessionCleanup;
    std::filesystem::remove(sessionProbe, sessionCleanup);
    if (sessionXml.find("<AuraProject") == std::string::npos ||
        sessionXml.find("</AuraProject>") == std::string::npos) return 82;
    for (const auto& entry : std::filesystem::directory_iterator(sessionProbe.parent_path())) {
        const auto name = entry.path().filename().string();
        if (name.rfind("aura-session-atomic-contract.xml.tmp-", 0) == 0) return 83;
    }
    {
        SDKContractFactory factory;
        const auto descriptor = factory.getDescriptor();
        if (descriptor.sdkVersion != Aura::SDK::kSDKVersion ||
            descriptor.identifier != "com.aura.sdk.contract" ||
            descriptor.parameters.size() != 1 || !descriptor.supportsSidechain ||
            descriptor.stateSchemaVersion == 0 || descriptor.guiStateSchemaVersion == 0 ||
            !descriptor.isValid()) return 65;
        auto invalid = descriptor;
        invalid.parameters[0].defaultValue = 2.0f;
        if (invalid.isValid()) return 69;
        const auto cache_key = descriptor.stateCacheKey();
        if (cache_key.find("com.aura.sdk.contract@1.0.0") == std::string::npos) return 70;
        if (Aura::Core::Plugins::PluginCacheManager::stateCacheKey(descriptor) != cache_key) return 72;
        invalid = descriptor;
        invalid.stateSchemaVersion++;
        if (invalid.stateCacheKey() == cache_key) return 71;
        invalid = descriptor;
        invalid.parameters.push_back({1, "Duplicate", 0.0f, 1.0f, 0.0f, true});
        if (invalid.isValid()) return 73;
        invalid = descriptor;
        invalid.parameters[0].defaultValue = std::numeric_limits<float>::quiet_NaN();
        if (invalid.isValid()) return 74;
        invalid = descriptor;
        invalid.outputChannels = 33;
        if (invalid.isValid()) return 75;
        invalid = descriptor;
        invalid.supportsSidechain = true;
        invalid.sidechainChannels = 0;
        if (invalid.isValid()) return 76;
        invalid = descriptor;
        invalid.components.push_back({"main", 1, 1, false});
        if (invalid.isValid()) return 79;
        invalid = descriptor;
        invalid.components.push_back({"other", 1, 1, true});
        invalid.supportsSidechain = false;
        if (invalid.isValid()) return 80;
        Aura::SDK::ParameterAutomation automation;
        if (!automation.setPoints({{100, 1.0f}, {0, 0.0f}, {200, 0.5f}}) ||
            automation.valueAt(50, -1.0f) != 0.5f ||
            automation.valueAt(250, -1.0f) != 0.5f) return 77;
        if (automation.setPoints({{0, 0.0f}, {0, 1.0f}}) ||
            automation.setPoints({{0, std::numeric_limits<float>::quiet_NaN()}})) return 78;
        auto processor = factory.create();
        if (!processor || processor->getName() != "SDK Contract") return 66;
        if (!processor->loadState(processor->saveState()) ||
            !processor->loadGuiState(processor->saveGuiState())) return 67;
        factory.destroy(std::move(processor));
    }
    {
        auto& legacySidechain = Aura::Core::DSP::Mixing::SidechainManager::getInstance();
        const float source[4] = {1.0f, 2.0f, 3.0f, 4.0f};
        if (!legacySidechain.writeSource(7, source, 4)) return 63;
        float copy[4] = {};
        const size_t frames = legacySidechain.copySource(7, copy, 4);
        if (frames != 4 || copy[0] != 1.0f || copy[3] != 4.0f) return 64;
    }
    // This translation unit intentionally exercises the production native
    // plugin boundary without loading a third-party binary.
    auto& pool = Aura::Core::Concurrency::ThreadPool::getInstance();
    auto poolResult = pool.enqueue([] { return 42; });
    if (poolResult.get() != 42) return 47;
    const auto peakWav = std::filesystem::temp_directory_path() / "aura-audio-pool-peaks.wav";
    {
        std::vector<uint8_t> wav;
        const auto u16 = [&wav](uint16_t value) {
            wav.push_back(static_cast<uint8_t>(value));
            wav.push_back(static_cast<uint8_t>(value >> 8));
        };
        const auto u32 = [&wav](uint32_t value) {
            for (unsigned shift = 0; shift < 32; shift += 8)
                wav.push_back(static_cast<uint8_t>(value >> shift));
        };
        const uint32_t dataBytes = 4u * sizeof(int16_t);
        wav.insert(wav.end(), {'R','I','F','F'});
        u32(36u + dataBytes);
        wav.insert(wav.end(), {'W','A','V','E','f','m','t',' '});
        u32(16); u16(1); u16(1); u32(48'000); u32(96'000); u16(2); u16(16);
        wav.insert(wav.end(), {'d','a','t','a'}); u32(dataBytes);
        for (int16_t sample : {1000, 2000, 1500, 2500}) u16(static_cast<uint16_t>(sample));
        std::ofstream output(peakWav, std::ios::binary | std::ios::trunc);
        output.write(reinterpret_cast<const char*>(wav.data()), static_cast<std::streamsize>(wav.size()));
    }
    auto& audioPool = Aura::Core::AudioPool::getInstance();
    if (audioPool.addSource((peakWav.string() + ".missing")) != nullptr) return 50;
    const auto peakSource = audioPool.addSource(peakWav.string());
    if (!peakSource) return 48;
    const auto peakDeadline = std::chrono::steady_clock::now() + std::chrono::seconds(2);
    std::shared_ptr<Aura::Core::PeakData> peaks;
    while (std::chrono::steady_clock::now() < peakDeadline) {
        peaks = audioPool.getPeaks(peakWav.string());
        if (peaks && peaks->isReady.load(std::memory_order_acquire)) break;
        std::this_thread::yield();
    }
    if (!peaks || !peaks->isReady.load(std::memory_order_acquire) || peaks->levels.empty() ||
        peaks->levels.front().mins.front() <= 0.0f || peaks->levels.front().maxs.front() <= 0.0f) return 49;
    audioPool.purgeUnused();
    std::error_code peakCleanup;
    std::filesystem::remove(peakWav, peakCleanup);
    // These hosts embed fixed-size realtime mailboxes. Keep them off the
    // test's main stack so the contract remains runnable on macOS's default
    // 8 MiB thread stack even when the sandbox protocol grows.
    auto vst3 = std::make_unique<Aura::Core::Plugins::VST3HostProcessor>();
    auto clap = std::make_unique<Aura::Core::Plugins::CLAPHostProcessor>();
    // The built-in compressor is part of the production plugin contract, not
    // merely a header-only type. Exercise prepare/process/parameter/state so
    // a future refactor cannot leave the browser-visible plugin unlinked.
    {
        Aura::DSP::Effects::DynamicCompressor compressor;
        compressor.prepareToPlay(48'000.0, 128);
        compressor.setParameter(0, 0.7f);
        compressor.setParameter(1, 0.25f);
        Aura::Core::AudioBuffer block(2, 128);
        for (uint32_t i = 0; i < block.getNumSamples(); ++i) {
            block.getWritePointer(0)[i] = 0.9f;
            block.getWritePointer(1)[i] = -0.9f;
        }
        Aura::Core::MidiBuffer midi;
        Aura::DSP::ProcessContext context{};
        context.sampleRate = 48'000.0;
        context.blockSize = 128;
        compressor.process(block, midi, context);
        if (block.sanitizeNonFinite() != 0 || compressor.getNumParameters() != 7) return 67;
        const auto state = compressor.getState();
        Aura::DSP::Effects::DynamicCompressor restored;
        if (!restored.restoreStateChecked(state) ||
            restored.getParameter(0) != compressor.getParameter(0)) return 68;
    }
    // Procedural Foley must be deterministic for offline renders and must
    // fail closed when a UI/FFI caller supplies non-finite event controls.
    {
        Aura::Core::DSP::Effects::ProceduralFoleyKernel first;
        Aura::Core::DSP::Effects::ProceduralFoleyKernel second;
        first.triggerEvent(std::numeric_limits<float>::quiet_NaN(), 0.65f);
        second.triggerEvent(0.5f, 0.65f);
        std::array<float, 512> firstBlock{};
        std::array<float, 512> secondBlock{};
        first.process(firstBlock.data(), static_cast<uint32_t>(firstBlock.size()));
        second.process(secondBlock.data(), static_cast<uint32_t>(secondBlock.size()));
        for (size_t sample = 0; sample < firstBlock.size(); ++sample) {
            if (!std::isfinite(firstBlock[sample]) || !std::isfinite(secondBlock[sample]) ||
                std::abs(firstBlock[sample] - secondBlock[sample]) > 1.0e-6f) return 76;
        }
    }
    {
        auto& assetLibrary = Aura::IO::Assets::AudioAssetLibrary::getInstance();
        assetLibrary.registerPatch("Contract Bass", "Bass", "/tmp/aura-contract-bass.aura");
        assetLibrary.registerPatch("Contract Drums", "Drums", "/tmp/aura-contract-drums.aura");
        assetLibrary.registerPatch("Contract Keys", "Keys", "/tmp/aura-contract-keys.aura");
        const auto categories = assetLibrary.categories();
        if (categories.size() < 3 || !std::is_sorted(categories.begin(), categories.end()) ||
            std::find(categories.begin(), categories.end(), "Bass") == categories.end() ||
            std::find(categories.begin(), categories.end(), "Drums") == categories.end() ||
            std::find(categories.begin(), categories.end(), "Keys") == categories.end()) return 77;
    }
    // The sampler must consume both legacy MIDI and MIDI 2.0 UMP note events,
    // including sustain pedal state, through the same realtime dispatcher.
    {
        static std::array<float, 4096> sample{};
        sample.fill(0.25f);
        auto sampler = std::make_unique<Aura::DSP::Synthesis::SamplerEngine>(48'000.0);
        sampler->addZone({60, 0, 127, 1, 127, {sample.data(), sample.size()}});
        auto block = std::make_unique<Aura::Core::AudioBuffer>(2, 32);
        auto midi = std::make_unique<Aura::Core::MidiBuffer>();
        const uint8_t umpOn[] = {0x40, 0x90, 60, 0, 0x7f, 0, 0, 0,
                                 0, 0, 0, 0, 0, 0, 0, 0};
        midi->addEvent(0, umpOn, sizeof(umpOn));
        Aura::DSP::ProcessContext context{};
        context.sampleRate = 48'000.0;
        context.blockSize = 32;
        sampler->process(*block, *midi, context);
        if (block->sanitizeNonFinite() != 0) return 69;
        const uint8_t sustainDown[] = {0xB0, 64, 127};
        const uint8_t noteOff[] = {0x80, 60, 0};
        midi->clear();
        midi->addEvent(0, sustainDown, sizeof(sustainDown));
        midi->addEvent(1, noteOff, sizeof(noteOff));
        sampler->process(*block, *midi, context);
        if (block->sanitizeNonFinite() != 0) return 70;
        const uint8_t sustainUp[] = {0xB0, 64, 0};
        midi->clear();
        midi->addEvent(0, sustainUp, sizeof(sustainUp));
        sampler->process(*block, *midi, context);
        if (block->sanitizeNonFinite() != 0) return 71;
    }
    // A looped zone must continue producing audio after the source reaches
    // its end, while keeping the output finite.
    {
        static std::array<float, 8> loopSample{};
        for (size_t i = 0; i < loopSample.size(); ++i) loopSample[i] = 0.1f + 0.1f * static_cast<float>(i);
        auto sampler = std::make_unique<Aura::DSP::Synthesis::SamplerEngine>(48'000.0);
        sampler->addZone({60, 60, 60, 1, 127, {loopSample.data(), loopSample.size()}, 2, 6, true});
        auto block = std::make_unique<Aura::Core::AudioBuffer>(1, 32);
        auto midi = std::make_unique<Aura::Core::MidiBuffer>();
        const uint8_t noteOn[] = {0x90, 60, 127};
        midi->addEvent(0, noteOn, sizeof(noteOn));
        Aura::DSP::ProcessContext context{};
        context.sampleRate = 48'000.0;
        context.blockSize = 32;
        sampler->process(*block, *midi, context);
        if (block->sanitizeNonFinite() != 0) return 74;
        bool hasTail = false;
        for (uint32_t i = 8; i < block->getNumSamples(); ++i)
            hasTail = hasTail || std::abs(block->getReadPointer(0)[i]) > 1.0e-5f;
        if (!hasTail) return 75;
    }
    {
        Aura::Synthesis::SamplerMap map;
        map.addZone(-1, 60, 1, 127, "invalid.wav");
        map.addZone(0, 127, 0, 127, "layer-a.wav");
        map.addZone(0, 127, 0, 127, "layer-b.wav");
        if (map.resolveSample(60, 100).empty() || map.resolveSample(60, 100).empty()) return 72;
        Aura::Synthesis::SamplerMap independent;
        independent.addZone(0, 127, 0, 127, "layer-a.wav");
        if (independent.resolveSample(60, 100) != "layer-a.wav") return 73;
    }
    Aura::Core::PluginHost::ClapHostInterface legacyClap("builtin://compile-contract");
    Aura::Core::Engine::MIDIOrchestrator midiOrchestrator;
    // The sandbox host owns bounded shared-memory staging arrays sized for
    // the maximum plugin protocol block. Allocate it on the heap to keep
    // this broad native contract below the platform thread-stack limit.
    auto sandbox = std::make_unique<Aura::Core::Plugins::PluginSandboxHost>("builtin://compile-contract");
    Aura::Core::EffectChain chain;
    Aura::Core::Plugins::ProcessSandboxProcessor::RecoveryMode recovery =
        sandbox->isAlive()
            ? Aura::Core::Plugins::ProcessSandboxProcessor::RecoveryMode::ClearBlock
            : Aura::Core::Plugins::ProcessSandboxProcessor::RecoveryMode::Quarantined;
    static_assert(Aura::Core::Plugins::SandboxProtocol::kStatusHeaderV9 == 0x41555209u);
    static_assert(Aura::Core::Plugins::SandboxProtocol::kStateProtocolVersion == 1u);
    static_assert(static_cast<unsigned>(Aura::Core::Plugins::ProcessSandboxProcessor::RecoveryMode::ClearBlock) == 0u);
    static_assert(static_cast<unsigned>(Aura::Core::Plugins::ProcessSandboxProcessor::RecoveryMode::Quarantined) == 1u);
    if (!Aura::Core::Plugins::SandboxProtocol::isSupportedStateVersion(
            Aura::Core::Plugins::SandboxProtocol::kStateProtocolVersion) ||
        Aura::Core::Plugins::SandboxProtocol::isSupportedStateVersion(0u) ||
        Aura::Core::Plugins::SandboxProtocol::isSupportedStateVersion(
            Aura::Core::Plugins::SandboxProtocol::kStateProtocolVersion + 1u)) return 64;
    (void)vst3;
    (void)clap;
    (void)legacyClap;
    (void)midiOrchestrator;
    (void)sandbox;
    (void)chain;
    (void)recovery;

    // The legacy structural wrapper must remain ABI-compatible with the
    // current processor interface while refusing to masquerade as a loaded
    // third-party plugin. Real processing is covered by the format adapters.
    Aura::Core::PluginHost::ExternalPluginWrapper legacyExternal(
        Aura::Core::PluginHost::ExternalPluginWrapper::Format::VST3,
        "builtin://unsupported-compat-wrapper");
    if (legacyExternal.supportsProcessing()) return 65;

    auto& undo = Aura::Core::Undo::UndoManager::getInstance();
    undo.clear();
    int transactionValue = 0;
    undo.beginTransaction("partial-operation");
    undo.perform(std::make_unique<UndoContractCommand>(transactionValue, 2, "first"));
    undo.perform(std::make_unique<UndoContractCommand>(transactionValue, 3, "second"));
    if (transactionValue != 5 || !undo.transactionActive()) return 16;
    undo.abortTransaction();
    if (transactionValue != 0 || undo.transactionActive() || undo.getUndoCount() != 0) return 17;

    auto& engineUndo = Aura::Core::Engine::UndoTransactionManager::getInstance();
    engineUndo.clear();

    Aura::Core::AudioBuffer finiteContract(1, 3);
    finiteContract.getWritePointer(0)[0] = 1.0f;
    finiteContract.getWritePointer(0)[1] = std::numeric_limits<float>::quiet_NaN();
    finiteContract.getWritePointer(0)[2] = std::numeric_limits<float>::infinity();
    if (finiteContract.sanitizeNonFinite() != 2 ||
        !std::isfinite(finiteContract.getReadPointer(0)[1]) ||
        !std::isfinite(finiteContract.getReadPointer(0)[2])) return 24;

    // The graph must contain third-party faults: non-finite samples are
    // sanitized without allocation and remain finite on subsequent blocks.
    Aura::Core::AudioProcessorGraph graphContract;
    graphContract.addNode(std::make_shared<NonFiniteProcessor>());
    graphContract.prepare(48'000.0, 8);
    Aura::Core::AudioBuffer graphBuffer(1, 8);
    Aura::Core::MidiBuffer graphMidi;
    Aura::DSP::ProcessContext graphContext{};
    graphContext.sampleRate = 48'000.0;
    graphContext.blockSize = 8;
    graphContext.numOutputChannels = 1;
    graphContract.process(graphBuffer, graphMidi, graphContext);
    if (graphContract.getSanitizedSampleCount() != 8 ||
        graphContract.getProcessorFaultCount() != 0 ||
        graphContract.getFaultedNodeCount() != 0) return 31;
    for (uint32_t index = 0; index < graphBuffer.getNumSamples(); ++index) {
        if (!std::isfinite(graphBuffer.getReadPointer(0)[index])) return 32;
    }
    graphContract.process(graphBuffer, graphMidi, graphContext);
    if (graphContract.getSanitizedSampleCount() != 16 ||
        graphContract.getProcessorFaultCount() != 0 ||
        graphContract.getFaultedNodeCount() != 0) return 33;
    int engineTransactionValue = 0;
    engineUndo.beginTransaction("engine-partial-operation");
    engineUndo.performAction("first", [&] { engineTransactionValue -= 2; },
                             [&] { engineTransactionValue += 2; });
    engineUndo.performAction("second", [&] { engineTransactionValue -= 3; },
                             [&] { engineTransactionValue += 3; });
    if (engineTransactionValue != 5 || !engineUndo.transactionActive()) return 18;
    if (!engineUndo.abortTransaction() || engineTransactionValue != 0 ||
        engineUndo.transactionActive() || engineUndo.getUndoCount() != 0) return 19;
    engineUndo.beginTransaction("engine-commit");
    engineUndo.performAction("first", [&] { engineTransactionValue -= 2; },
                             [&] { engineTransactionValue += 2; });
    engineUndo.performAction("second", [&] { engineTransactionValue -= 3; },
                             [&] { engineTransactionValue += 3; });
    if (!engineUndo.endTransaction() || engineUndo.getUndoCount() != 1) return 20;
    engineUndo.undo();
    if (engineTransactionValue != 0 || engineUndo.getRedoCount() != 1) return 21;
    engineUndo.redo();
    if (engineTransactionValue != 5) return 22;
    engineUndo.clear();

    const auto autosavePath = std::filesystem::temp_directory_path() /
        ("aura-autosave-contract-" + std::to_string(
#if !defined(_WIN32)
            static_cast<unsigned long long>(::getpid())
#else
            0ULL
#endif
        ) + ".json");
    auto& autosave = Aura::IO::Persistence::AutoSaveEngine::getInstance();
    autosave.start(autosavePath.string(), 60, [] { return std::string("{\"ok\":true}"); });
    autosave.markModified();
    if (!autosave.flushNow() || autosave.isDirty()) return 23;
    autosave.stop();
    std::error_code autosaveCleanup;
    std::filesystem::remove(autosavePath.string() + ".autosave", autosaveCleanup);

    // A failed DB publication must not remove the last destination. Using a
    // directory as the destination forces rename to fail while making the
    // preservation invariant directly observable.
    auto& projectDb = Aura::Core::Database::ProjectDB::i();
    projectDb.recordForensic("contract", "publication", "preserve-last-good");
    const auto dbDestination = std::filesystem::temp_directory_path() / "aura-project-db-contract-destination";
    std::error_code dbCleanup;
    std::filesystem::remove_all(dbDestination, dbCleanup);
    std::filesystem::create_directory(dbDestination, dbCleanup);
    if (dbCleanup || projectDb.flushToDisk(dbDestination.string()) ||
        !std::filesystem::is_directory(dbDestination)) return 65;
    std::filesystem::remove_all(dbDestination, dbCleanup);

    // A newer asset batch must prevent an older worker from publishing stale
    // results or failure diagnostics into the current project.
    auto& assets = Aura::IO::ParallelAssetManager::getInstance();
    assets.loadAssets({"/definitely/missing/aura-old.wav"});
    if (assets.failedPaths().empty()) return 25;

    // Native project collection must preserve distinct same-basename assets
    // instead of silently reusing the first destination.
    const auto collectorRoot = std::filesystem::temp_directory_path() /
                               "aura-project-collector-contract";
    std::error_code collectorCleanup;
    std::filesystem::remove_all(collectorRoot, collectorCleanup);
    std::filesystem::create_directories(collectorRoot / "one", collectorCleanup);
    std::filesystem::create_directories(collectorRoot / "two", collectorCleanup);
    if (collectorCleanup) return 66;
    const auto collectorFirst = collectorRoot / "one" / "same.wav";
    const auto collectorSecond = collectorRoot / "two" / "same.wav";
    std::ofstream(collectorFirst) << "first";
    std::ofstream(collectorSecond) << "second";
    auto& collector = Aura::IO::Persistence::ProjectCollector::getInstance();
    if (!collector.collect(collectorRoot.string(),
                           {collectorFirst.string(), collectorSecond.string()})) return 67;
    size_t collectedCount = 0;
    for (const auto& entry : std::filesystem::directory_iterator(collectorRoot / "Assets")) {
        if (entry.is_regular_file()) ++collectedCount;
    }
    std::filesystem::remove_all(collectorRoot, collectorCleanup);
    if (collectedCount != 2) return 68;

    auto& telemetryBridge = Aura::Network::SharedMemoryBridge::getInstance();
    if (!telemetryBridge.start()) return 26;
    const std::string telemetryName = telemetryBridge.segmentName();
    if (telemetryName.find("/aura_sovereign_bridge_") != 0) return 27;
    std::array<uint8_t, 300> sharedMidiPayload{};
    sharedMidiPayload.front() = 0xf0;
    sharedMidiPayload.back() = 0xf7;
    if (!telemetryBridge.pushExtendedMidi(24, 3, sharedMidiPayload.data(), sharedMidiPayload.size()) ||
        telemetryBridge.pendingExtendedMidi() != 1) return 59;
    Aura::Core::Plugins::MidiExtendedMessageRing::Message sharedMidiMessage;
    if (!telemetryBridge.popExtendedMidi(sharedMidiMessage) ||
        sharedMidiMessage.size != sharedMidiPayload.size() ||
        sharedMidiMessage.sampleOffset != 24 || sharedMidiMessage.articulationId != 3 ||
        sharedMidiMessage.data.front() != 0xf0 || sharedMidiMessage.data[299] != 0xf7) return 60;
    for (unsigned slot = 0; slot < Aura::Core::Plugins::MidiExtendedMessageRing::kCapacity; ++slot) {
        if (!telemetryBridge.pushExtendedMidi(slot, 0, sharedMidiPayload.data(), sharedMidiPayload.size())) return 61;
    }
    if (telemetryBridge.pushExtendedMidi(99, 0, sharedMidiPayload.data(), sharedMidiPayload.size()) ||
        telemetryBridge.pendingExtendedMidi() != Aura::Core::Plugins::MidiExtendedMessageRing::kCapacity) return 62;
    Aura::Core::Plugins::MidiExtendedMessageRing::Message drainedMessage;
    while (telemetryBridge.popExtendedMidi(drainedMessage)) {}
    if (telemetryBridge.pendingExtendedMidi() != 0) return 63;
    telemetryBridge.stop();
    if (!telemetryBridge.segmentName().empty()) return 28;
    if (!telemetryBridge.start()) return 45;
    const std::string secondTelemetryName = telemetryBridge.segmentName();
    if (secondTelemetryName.empty() || secondTelemetryName == telemetryName) return 46;
    telemetryBridge.stop();

    // Payloads beyond the bounded SysEx/MIDI2 envelope must be observable at
    // ingress rather than silently becoming a truncated channel-voice event.
    Aura::Core::MidiBuffer midiIngress;
    const uint8_t oversizedMidi[257] = {};
    midiIngress.addEvent(0, oversizedMidi, sizeof(oversizedMidi));
    if (!midiIngress.overflowed() || midiIngress.takeOversizeEvents() != 1 ||
        midiIngress.takeExtendedEvents() != 1 || midiIngress.size() != 0) return 15;

    const auto temp = std::filesystem::temp_directory_path() / "aura-plugin-cache-contract.bin";
    {
        std::ofstream output(temp, std::ios::binary | std::ios::trunc);
        output << "version-one";
    }
    const auto first = Aura::Core::Plugins::PluginCacheManager::fingerprintForPath(temp);
    if (!first || *first == 0) return 2;
    {
        std::ofstream output(temp, std::ios::binary | std::ios::trunc);
        output << "version-two-with-a-different-size";
    }
    const auto second = Aura::Core::Plugins::PluginCacheManager::fingerprintForPath(temp);
    std::error_code cleanup;
    std::filesystem::remove(temp, cleanup);
    if (!second || *second == *first) return 3;
    const auto directoryFingerprint = std::filesystem::temp_directory_path() / "aura-plugin-fingerprint-directory";
    std::filesystem::remove_all(directoryFingerprint, cleanup);
    std::filesystem::create_directories(directoryFingerprint / "nested", cleanup);
    {
        std::ofstream(directoryFingerprint / "z.bin", std::ios::binary) << "z";
        std::ofstream(directoryFingerprint / "nested" / "a.bin", std::ios::binary) << "a";
    }
    const auto directoryFirst = Aura::Core::Plugins::PluginCacheManager::fingerprintForPath(directoryFingerprint);
    std::filesystem::remove_all(directoryFingerprint, cleanup);
    std::filesystem::create_directories(directoryFingerprint / "nested", cleanup);
    {
        // Create in the opposite order: the fingerprint must not depend on
        // filesystem iterator ordering.
        std::ofstream(directoryFingerprint / "nested" / "a.bin", std::ios::binary) << "a";
        std::ofstream(directoryFingerprint / "z.bin", std::ios::binary) << "z";
    }
    const auto directorySecond = Aura::Core::Plugins::PluginCacheManager::fingerprintForPath(directoryFingerprint);
    std::filesystem::remove_all(directoryFingerprint, cleanup);
    if (!directoryFirst || !directorySecond || *directoryFirst != *directorySecond) return 4;
    const auto admittedPath = std::filesystem::temp_directory_path() / "aura-admission-contract.clap";
    {
        std::ofstream output(admittedPath, std::ios::binary | std::ios::trunc);
        output << "fixture-v1";
    }
    Aura::Core::Plugins::PluginDescriptor admitted;
    admitted.uuid = "contract-admission-plugin";
    admitted.name = "Admission Contract";
    admitted.format = "CLAP";
    admitted.binaryPath = admittedPath.string();
    auto& infrastructure = Aura::Core::Plugins::PluginHostInfrastructure::getInstance();
    if (!infrastructure.registerPlugin(admitted)) return 28;
    std::string admissionError;
    admissionError = "stale diagnostic";
    if (!infrastructure.validatePlugin(admitted.uuid, &admissionError)) return 29;
    if (!admissionError.empty()) return 33;
    {
        std::ofstream output(admittedPath, std::ios::binary | std::ios::trunc);
        output << "fixture-v2-with-a-new-binary-fingerprint";
    }
    if (infrastructure.validatePlugin(admitted.uuid, &admissionError) ||
        admissionError.find("rescan required") == std::string::npos) return 30;
    if (infrastructure.createPlugin(admitted.uuid, &admissionError)) return 36;
    if (admissionError.find("rescan required") == std::string::npos) return 37;
    const auto collisionPath = std::filesystem::temp_directory_path() /
                               "aura-admission-collision.clap";
    {
        std::ofstream output(collisionPath, std::ios::binary | std::ios::trunc);
        output << "collision";
    }
    Aura::Core::Plugins::PluginDescriptor collision = admitted;
    collision.binaryPath = collisionPath.string();
    collision.binaryFingerprint = Aura::Core::Plugins::PluginCacheManager::fingerprintForPath(
        collisionPath).value_or(0);
    if (infrastructure.registerPlugin(collision)) return 35;
    std::filesystem::remove(collisionPath, cleanup);
    std::filesystem::remove(admittedPath, cleanup);
#if !defined(_WIN32)
    const auto symlinkPath = std::filesystem::temp_directory_path() /
                             "aura-admission-contract-symlink.clap";
    const auto symlinkTarget = std::filesystem::temp_directory_path() /
                               "aura-admission-contract-target.clap";
    {
        std::ofstream output(symlinkTarget, std::ios::binary | std::ios::trunc);
        output << "target";
    }
    std::filesystem::create_symlink(symlinkTarget, symlinkPath, cleanup);
    if (Aura::Core::Plugins::PluginAdmission::isSafeCandidate(symlinkPath, "CLAP")) return 31;
    std::filesystem::remove(symlinkPath, cleanup);
    std::filesystem::remove(symlinkTarget, cleanup);
#endif
    const auto uppercasePath = std::filesystem::temp_directory_path() /
                               "aura-admission-contract-uppercase.VST3";
    {
        std::ofstream output(uppercasePath, std::ios::binary | std::ios::trunc);
        output << "uppercase-extension";
    }
    if (!Aura::Core::Plugins::PluginAdmission::isSafeCandidate(uppercasePath, "vst3")) return 32;
    std::filesystem::remove(uppercasePath, cleanup);
    auto ai = Aura::SCAE::Intelligence::LocalAIModels::startStemSeparation(temp.string());
    const auto aiResult = ai.get();
    if (aiResult.success || aiResult.error.empty()) return 4;

    const auto project = std::filesystem::temp_directory_path() / "aura-region-id-contract.bin";
    Aura::Core::ProjectSerializer::ProjectState state;
    state.version = 28;
    state.sampleRate = 48'000;
    state.bpm = 120.0;
    Aura::Core::ProjectSerializer::TrackState baseTrack{};
    baseTrack.id = 7;
    baseTrack.volume = 1.0f;
    baseTrack.pan = 0.0f;
    state.tracks.push_back(baseTrack);
    Aura::Core::ProjectSerializer::RegionState region;
    region.id = 9001;
    region.trackId = 7;
    region.samplePosition = 0;
    region.sampleLength = 128;
    region.filePath = "contract.wav";
    state.regions.push_back(region);
    if (!Aura::Core::ProjectSerializer::saveAtomic(project.string(), state)) return 5;
    const auto restored = Aura::Core::ProjectSerializer::load(project.string());
    if (!restored.valid || restored.regions.size() != 1 || restored.regions[0].id != 9001) return 6;

    Aura::Core::ProjectSerializer::ProjectState duplicateIds = state;
    Aura::Core::ProjectSerializer::TrackState firstTrack = baseTrack;
    Aura::Core::ProjectSerializer::TrackState duplicateTrack = firstTrack;
    duplicateIds.tracks = {firstTrack, duplicateTrack};
    const auto duplicateTrackProject = std::filesystem::temp_directory_path() /
                                       "aura-duplicate-track-id-contract.bin";
    if (!Aura::Core::ProjectSerializer::saveAtomic(duplicateTrackProject.string(), duplicateIds)) return 61;
    const auto rejectedDuplicateTrack = Aura::Core::ProjectSerializer::load(duplicateTrackProject.string());
    std::filesystem::remove(duplicateTrackProject, cleanup);
    if (rejectedDuplicateTrack.valid) return 62;

    Aura::Core::ProjectSerializer::ProjectState orphanRegion = state;
    orphanRegion.regions[0].trackId = 999;
    const auto orphanRegionProject = std::filesystem::temp_directory_path() /
                                     "aura-orphan-region-contract.bin";
    if (!Aura::Core::ProjectSerializer::saveAtomic(orphanRegionProject.string(), orphanRegion)) return 63;
    const auto rejectedOrphanRegion = Aura::Core::ProjectSerializer::load(orphanRegionProject.string());
    std::filesystem::remove(orphanRegionProject, cleanup);
    if (rejectedOrphanRegion.valid) return 64;

    {
        std::fstream corrupt(project, std::ios::in | std::ios::out | std::ios::binary);
        corrupt.seekg(12, std::ios::beg);
        char byte = 0;
        corrupt.read(&byte, 1);
        corrupt.seekp(12, std::ios::beg);
        byte ^= static_cast<char>(0x5a);
        corrupt.write(&byte, 1);
    }
    const auto rejectedCorruption = Aura::Core::ProjectSerializer::load(project.string());
    std::filesystem::remove(project, cleanup);
    if (rejectedCorruption.valid) return 34;

    // The loader must reject an oversized project before allocating the file
    // buffer.  This is intentionally sparse so the contract stays cheap.
    const auto oversizedProject = std::filesystem::temp_directory_path() /
                                  "aura-oversized-project-contract.bin";
    {
        std::ofstream oversized(oversizedProject, std::ios::binary | std::ios::trunc);
        if (!oversized) return 59;
        oversized.seekp(static_cast<std::streamoff>(Aura::Core::ProjectSerializer::kMaxProjectBytes));
        oversized.put('\0');
    }
    const auto rejectedOversized = Aura::Core::ProjectSerializer::load(oversizedProject.string());
    std::filesystem::remove(oversizedProject, cleanup);
    if (rejectedOversized.valid) return 60;

    const auto legacyProject = std::filesystem::temp_directory_path() / "aura-legacy-serializer-contract.txt";
    Aura::IO::Persistence::ProjectSerializer legacy;
    if (!legacy.saveProject(legacyProject.string(), "{\"tracks\":[1,2]}")) return 7;
    if (legacy.loadProject(legacyProject.string()) != "{\"tracks\":[1,2]}") return 8;
    {
        std::ofstream corrupt(legacyProject, std::ios::trunc);
        corrupt << "{\"tracks\":[1,3]}\n--AURA_CRC:123";
    }
    if (!legacy.loadProject(legacyProject.string()).empty()) return 9;
    std::filesystem::remove(legacyProject, cleanup);

    const auto rf64Project = std::filesystem::temp_directory_path() / "aura-rf64-mmap-contract.wav";
    std::vector<uint8_t> rf64;
    const auto append = [&rf64](const char* bytes, size_t size) {
        rf64.insert(rf64.end(), bytes, bytes + size);
    };
    const auto u16 = [&rf64](uint16_t value) {
        rf64.push_back(static_cast<uint8_t>(value));
        rf64.push_back(static_cast<uint8_t>(value >> 8));
    };
    const auto u32 = [&rf64](uint32_t value) {
        for (unsigned shift = 0; shift < 32; shift += 8) rf64.push_back(static_cast<uint8_t>(value >> shift));
    };
    const auto u64 = [&rf64](uint64_t value) {
        for (unsigned shift = 0; shift < 64; shift += 8) rf64.push_back(static_cast<uint8_t>(value >> shift));
    };
    append("RF64", 4); u32(0xffff'ffffu); append("WAVE", 4);
    append("ds64", 4); u32(28); u64(0); u64(2); u64(1); u32(0);
    append("fmt ", 4); u32(16); u16(1); u16(1); u32(44'100); u32(88'200); u16(2); u16(16);
    append("data", 4); u32(0xffff'ffffu); u16(0);
    {
        std::ofstream output(rf64Project, std::ios::binary | std::ios::trunc);
        output.write(reinterpret_cast<const char*>(rf64.data()), static_cast<std::streamsize>(rf64.size()));
    }
    try {
        Aura::IO::MMapAudioFile mapped(rf64Project.string());
        if (!mapped.isValid() || mapped.getNumChannels() != 1 || mapped.getSampleRate() != 44'100 ||
            mapped.getNumSamples() != 1 || mapped.getSample(0, 0) != 0.0f) return 10;
    } catch (...) {
        return 11;
    }
    std::filesystem::remove(rf64Project, cleanup);

    Aura::Core::Engine::BusRouter router;
    router.addDependency(10, 20);
    Aura::Core::Engine::LatencyManager::getInstance().registerLatency(10, 3);
    Aura::Core::Engine::LatencyManager::getInstance().registerLatency(20, 9);
    Aura::Core::Engine::LatencyManager::getInstance().calculatePDC(router);
    if (Aura::Core::Engine::LatencyManager::getInstance().getCompensationFor(10) != 0 ||
        Aura::Core::Engine::LatencyManager::getInstance().getCompensationFor(20) != 3)
        return 12;

    // Exercise the production scheduler rather than only compiling its
    // headers. In particular, stolen tasks must execute exactly once and the
    // scheduler must be safely destructible after worker shutdown.
    Aura::Core::Concurrency::AudioTaskStealingScheduler scheduler;
    scheduler.start(2);
    auto completed = std::make_shared<std::atomic<uint32_t>>(0);
    for (uint32_t i = 0; i < 64; ++i) {
        if (!scheduler.postTaskAsync(i, [completed]() {
                completed->fetch_add(1, std::memory_order_acq_rel);
            })) return 13;
    }
    const auto deadline = std::chrono::steady_clock::now() + std::chrono::seconds(2);
    while (completed->load(std::memory_order_acquire) != 64 &&
           std::chrono::steady_clock::now() < deadline) {
        std::this_thread::yield();
    }
    scheduler.stop();
    if (completed->load(std::memory_order_acquire) != 64) return 14;

    // OfflineRenderer must execute the supplied render callback and emit a
    // valid BWF payload instead of silently writing the old all-zero stub.
    const auto offlinePath = std::filesystem::temp_directory_path() /
                             "aura-offline-render-contract.wav";
    auto offline = std::make_unique<Aura::Core::OfflineRenderer>(48'000.0,
        [](Aura::Core::AudioBuffer& buffer, uint32_t count) {
            for (uint32_t c = 0; c < buffer.getNumChannels(); ++c) {
                float* samples = buffer.getWritePointer(c);
                for (uint32_t i = 0; i < count; ++i) {
                    samples[i] = (i % 32u == 0u) ? (c == 0u ? 0.5f : -0.25f) : 0.0f;
                }
            }
        });
    offline->renderToFile(offlinePath.string(), 0.01);
    std::ifstream offlineInput(offlinePath, std::ios::binary);
    std::vector<uint8_t> offlineBytes((std::istreambuf_iterator<char>(offlineInput)), {});
    std::error_code offlineCleanup;
    std::filesystem::remove(offlinePath, offlineCleanup);
    if (offlineBytes.size() < 700 ||
        std::string(offlineBytes.begin(), offlineBytes.begin() + 4) != "RIFF" ||
        std::string(offlineBytes.begin() + 8, offlineBytes.begin() + 12) != "WAVE" ||
        std::string(offlineBytes.begin() + 12, offlineBytes.begin() + 16) != "bext" ||
        offlineBytes[16] != 0x5a || offlineBytes[17] != 0x02 ||
        std::string(offlineBytes.begin() + 622, offlineBytes.begin() + 626) != "fmt " ||
        std::string(offlineBytes.begin() + 646, offlineBytes.begin() + 650) != "data") return 18;
    bool hasAudioPayload = false;
    for (size_t i = 654; i < offlineBytes.size(); ++i) {
        if (offlineBytes[i] != 0u) { hasAudioPayload = true; break; }
    }
    if (!hasAudioPayload) return 19;

    const auto saverPath = std::filesystem::temp_directory_path() / "aura-wav-saver-contract.wav";
    const std::vector<std::vector<float>> saverAudio = {
        {0.25f, -0.25f, 0.0f, 0.5f},
        {-0.5f, 0.125f, 0.0f, -0.25f}
    };
    if (!Aura::IO::WavSaver::save(saverPath.string(), saverAudio, 48'000)) return 22;
    Aura::IO::WavLoader::WavInfo saverInfo{};
    try {
        const auto decoded = Aura::IO::WavLoader::load(saverPath.string(), saverInfo);
        if (decoded.size() != 2 || decoded[0].size() != 4 || saverInfo.sampleRate != 48'000 ||
            std::abs(decoded[0][0] - 0.25f) > 0.01f || std::abs(decoded[1][1] - 0.125f) > 0.01f) return 23;
        Aura::IO::WavLoader::WavInfo legacyInfo{};
        const auto legacyDecoded = Aura::IO::WavLoader::loadLegacy(saverPath.string(), legacyInfo);
        if (legacyDecoded.size() != 2 || legacyDecoded[0].size() != 4 || legacyInfo.bitDepth != 24 ||
            std::abs(legacyDecoded[0][0] - 0.25f) > 0.01f) return 27;
    } catch (...) {
        return 24;
    }
    std::filesystem::remove(saverPath, offlineCleanup);

    const auto multichannelSaverPath = std::filesystem::temp_directory_path() / "aura-wav-saver-multichannel-contract.wav";
    const std::vector<std::vector<float>> multichannelAudio = {
        {0.1f, 0.2f, 0.3f}, {-0.1f, -0.2f, -0.3f}, {0.0f, 0.4f, -0.4f}
    };
    if (!Aura::IO::WavSaver::save(multichannelSaverPath.string(), multichannelAudio, 48'000)) return 28;
    try {
        Aura::IO::WavLoader::WavInfo multichannelInfo{};
        const auto decoded = Aura::IO::WavLoader::load(multichannelSaverPath.string(), multichannelInfo);
        if (decoded.size() != 3 || decoded[0].size() != 3 || multichannelInfo.bitDepth != 24 ||
            std::abs(decoded[2][1] - 0.4f) > 0.01f) return 29;
    } catch (...) {
        return 30;
    }
    std::filesystem::remove(multichannelSaverPath, offlineCleanup);

    // Exercise the canonical decoder across its bounded read chunk boundary;
    // this guards against regressions that only pass on tiny fixture files.
    const auto chunkedSaverPath = std::filesystem::temp_directory_path() / "aura-wav-saver-chunked-contract.wav";
    std::vector<std::vector<float>> chunkedAudio(2, std::vector<float>(20'000));
    for (size_t frame = 0; frame < chunkedAudio[0].size(); ++frame) {
        chunkedAudio[0][frame] = (frame % 97 == 0) ? 0.75f : -0.125f;
        chunkedAudio[1][frame] = (frame % 113 == 0) ? -0.5f : 0.25f;
    }
    if (!Aura::IO::WavSaver::save(chunkedSaverPath.string(), chunkedAudio, 48'000)) return 31;
    try {
        Aura::Core::IO::WavDecoder chunkedDecoder;
        if (!chunkedDecoder.open(chunkedSaverPath.string())) return 32;
        Aura::Core::AudioBuffer chunkedBuffer;
        chunkedDecoder.decodeFull(chunkedBuffer);
        if (chunkedBuffer.getNumChannels() != 2 || chunkedBuffer.getNumSamples() != 20'000 ||
            std::abs(chunkedBuffer.getReadPointer(0)[16'384] + 0.125f) > 0.01f ||
            std::abs(chunkedBuffer.getReadPointer(1)[19'999] - 0.25f) > 0.01f) return 33;
    } catch (...) {
        return 34;
    }
    std::filesystem::remove(chunkedSaverPath, offlineCleanup);

    const auto oversizedExpansionPath = std::filesystem::temp_directory_path() /
                                        "aura-wav-expansion-limit-contract.wav";
    {
        constexpr uint32_t expansionDataBytes = 300u * 1024u * 1024u;
        std::ofstream oversized(oversizedExpansionPath, std::ios::binary | std::ios::trunc);
        const uint32_t riffSize = 36u + expansionDataBytes;
        const uint32_t fmtSize = 16;
        const uint16_t pcm = 1, channels = 2, align = 4, bits = 16;
        const uint32_t sampleRate = 48'000, byteRate = sampleRate * align;
        const uint32_t dataId = 0x61746164u;
        oversized.write("RIFF", 4);
        oversized.write(reinterpret_cast<const char*>(&riffSize), sizeof(riffSize));
        oversized.write("WAVEfmt ", 8);
        oversized.write(reinterpret_cast<const char*>(&fmtSize), sizeof(fmtSize));
        oversized.write(reinterpret_cast<const char*>(&pcm), sizeof(pcm));
        oversized.write(reinterpret_cast<const char*>(&channels), sizeof(channels));
        oversized.write(reinterpret_cast<const char*>(&sampleRate), sizeof(sampleRate));
        oversized.write(reinterpret_cast<const char*>(&byteRate), sizeof(byteRate));
        oversized.write(reinterpret_cast<const char*>(&align), sizeof(align));
        oversized.write(reinterpret_cast<const char*>(&bits), sizeof(bits));
        oversized.write(reinterpret_cast<const char*>(&dataId), sizeof(dataId));
        oversized.write(reinterpret_cast<const char*>(&expansionDataBytes), sizeof(expansionDataBytes));
        oversized.seekp(static_cast<std::streamoff>(44ull + expansionDataBytes - 1ull));
        oversized.put('\0');
    }
    Aura::Core::IO::WavDecoder oversizedDecoder;
    Aura::Core::AudioBuffer oversizedBuffer;
    const bool openedOversized = oversizedDecoder.open(oversizedExpansionPath.string());
    if (!openedOversized) return 35;
    oversizedDecoder.decodeFull(oversizedBuffer);
    std::filesystem::remove(oversizedExpansionPath, offlineCleanup);
    if (oversizedBuffer.getNumChannels() != 0 || oversizedBuffer.getNumSamples() != 0) return 36;

    auto& waveformCache = Aura::UI::WaveformCache::getInstance();
    waveformCache.clearRegion(77);
    Aura::UI::WaveformLevel waveform;
    waveform.minPeaks = {-0.5f, -0.25f};
    waveform.maxPeaks = {0.5f, 0.25f};
    if (!waveformCache.requestWaveform(77, 2, std::vector<float>(2'000'000, 0.1f))) return 29;
    waveformCache.putWaveform(77, 2, waveform);
    waveformCache.waitForPending();
    const auto* waveformSnapshot = waveformCache.getWaveform(77, 2);
    if (waveformSnapshot == nullptr || waveformSnapshot->minPeaks[0] != -0.5f) return 25;
    waveformCache.clearRegion(77);
    if (!waveformCache.requestWaveform(77, 2, std::vector<float>(2'000'000, 0.2f))) return 30;
    waveformCache.clearRegion(77);
    waveformCache.waitForPending();
    if (waveformCache.getWaveform(77, 2) != nullptr) return 31;

    const auto cachePath = std::filesystem::temp_directory_path() / "aura-cache-contract.raw";
    auto& cache = Aura::IO::Assets::CachingSubsystem::getInstance();
    cache.registerCacheSlot(501, cachePath.string());
    const auto cachedPath = cache.getCachedPath(501);
    if (!cachedPath || *cachedPath != cachePath.string()) return 28;
    cache.writeAsync(501, {0.125f, -0.25f, 0.5f});
    cache.writeAsync(501, {0.75f, -0.5f, 0.25f});
    std::vector<float> cached(3, 0.0f);
    const auto cacheDeadline = std::chrono::steady_clock::now() + std::chrono::seconds(2);
    bool cachePublished = false;
    while (std::chrono::steady_clock::now() < cacheDeadline) {
        std::ifstream cacheInput(cachePath, std::ios::binary);
        if (cacheInput) {
            cacheInput.read(reinterpret_cast<char*>(cached.data()),
                            static_cast<std::streamsize>(cached.size() * sizeof(float)));
            cachePublished = cacheInput && cached[0] == 0.75f &&
                cached[1] == -0.5f && cached[2] == 0.25f;
            if (cachePublished) break;
        }
        std::this_thread::yield();
    }
    std::filesystem::remove(cachePath, offlineCleanup);
    if (!cachePublished) return 26;

    const auto pluginFingerprintPath = std::filesystem::temp_directory_path() /
                                       "aura-plugin-fingerprint-contract.clap";
    {
        std::ofstream pluginFile(pluginFingerprintPath, std::ios::binary | std::ios::trunc);
        pluginFile << "plugin-v1";
    }
    const auto fingerprintV1 = Aura::Core::Plugins::PluginCacheManager::fingerprintForPath(
        pluginFingerprintPath);
    if (!fingerprintV1) return 32;
    {
        std::ofstream pluginFile(pluginFingerprintPath, std::ios::binary | std::ios::trunc);
        pluginFile << "plugin-v2-with-new-content";
    }
    const auto fingerprintV2 = Aura::Core::Plugins::PluginCacheManager::fingerprintForPath(
        pluginFingerprintPath);
    auto& pluginCache = Aura::Core::Plugins::PluginCacheManager::getInstance();
    pluginCache.recordScanFailure(pluginFingerprintPath.string(), 17);
    if (!pluginCache.isBlacklisted(pluginFingerprintPath.string())) return 43;
    const auto aliasPath = pluginFingerprintPath.parent_path() / "." /
                           pluginFingerprintPath.filename();
    if (!pluginCache.isBlacklisted(aliasPath.string())) return 47;
    {
        std::ofstream pluginFile(pluginFingerprintPath, std::ios::binary | std::ios::trunc);
        pluginFile << "plugin-v3-replaced";
    }
    if (pluginCache.isBlacklisted(pluginFingerprintPath.string())) return 44;
    pluginCache.clearScanFailure(aliasPath.string());
    std::filesystem::remove(pluginFingerprintPath, offlineCleanup);
    if (!fingerprintV2 || *fingerprintV1 == *fingerprintV2) return 33;

    const auto assetRoot = std::filesystem::temp_directory_path() /
                           ("aura-asset-collector-" +
#if defined(_WIN32)
                            std::to_string(0)
#else
                            std::to_string(::getpid())
#endif
                           );
    const auto assetSource = assetRoot / "source" / "kick.wav";
    const auto assetProject = assetRoot / "project";
    std::filesystem::create_directories(assetSource.parent_path());
    {
        std::ofstream output(assetSource, std::ios::binary | std::ios::trunc);
        output << "asset-v1";
    }
    const bool failedCollection = Aura::IO::Persistence::ProjectCollector::getInstance().collect(
        assetProject.string(), {assetSource.string(), (assetRoot / "missing.wav").string()});
    if (failedCollection || std::filesystem::exists(assetProject / "Assets" / "kick.wav")) return 20;
    if (!Aura::IO::Persistence::ProjectCollector::getInstance().collect(
            assetProject.string(), {assetSource.string()})) return 16;
    if (!std::filesystem::exists(assetProject / "Assets" / "kick.wav")) return 21;
    const auto collidingSource = assetRoot / "alternate" / "kick.wav";
    std::filesystem::create_directories(collidingSource.parent_path());
    {
        std::ofstream output(collidingSource, std::ios::binary | std::ios::trunc);
        output << "asset-v2";
    }
    if (!Aura::IO::Persistence::ProjectCollector::getInstance().collect(
            assetProject.string(), {collidingSource.string()})) return 48;
    size_t collectedKickFiles = 0;
    for (const auto& entry : std::filesystem::directory_iterator(assetProject / "Assets")) {
        if (entry.path().extension() == ".wav") ++collectedKickFiles;
    }
    if (collectedKickFiles != 2) return 49;
    std::error_code assetCleanup;
#if !defined(_WIN32)
    const auto assetLink = assetRoot / "source-link.wav";
    std::filesystem::create_symlink(assetSource, assetLink, assetCleanup);
    if (Aura::IO::Persistence::ProjectCollector::getInstance().collect(
            assetProject.string(), {assetLink.string()})) return 50;
    std::filesystem::remove(assetLink, assetCleanup);
#endif
    std::filesystem::remove_all(assetRoot, assetCleanup);
    static uint8_t integrityRegion[] = {0x01, 0x02, 0x03, 0x04};
    auto& security = Aura::Core::Security::SecurityManager::getInstance();
    security.enableMonitoring();
    security.registerRegion(integrityRegion, sizeof(integrityRegion));
    if (!security.verifyIntegrity()) return 35;
    integrityRegion[2] ^= 0xffu;
    if (security.verifyIntegrity()) return 36;

    auto& vault = Aura::Core::Security::LicenseVault::getInstance();
    vault.registerAsset("contract-asset");
    const std::vector<uint8_t> protectedPayload{0x01, 0x02, 0x03};
    if (!vault.isAuthorized("contract-asset")) return 40;
    if (!vault.decryptAsset(protectedPayload, "contract-asset").empty()) return 41;

    Aura::Core::Engine::TimelineSystem encodedTimeline;
    auto encodedTrack = std::make_shared<Aura::Core::Engine::Track>(
        77, "VCA Contract", Aura::Core::Engine::Track::Audio);
    auto midiRegion = std::make_shared<Aura::Core::MidiRegion>(88, "MIDI Contract", 0.0, 4.0);
    midiRegion->addNote(Aura::Core::MIDINote{60, 100, 0.0, 1.0});
    encodedTrack->addMidiRegion(midiRegion);
    encodedTimeline.addTrack(encodedTrack);
    {
        // Freeze playback is a real native path: publish happens off the
        // audio thread, while Track::process only copies from the immutable
        // bounded snapshot and bypasses the live region/effect graph.
        auto frozen = std::make_shared<Aura::Core::AudioBuffer>(2, 8);
        for (uint32_t index = 0; index < 8; ++index) {
            frozen->getWritePointer(0)[index] = static_cast<float>(index + 1);
            frozen->getWritePointer(1)[index] = static_cast<float>(100 + index);
        }
        auto freezeTrack = std::make_shared<Aura::Core::Engine::Track>(79, "Frozen", Aura::Core::Engine::Track::Audio);
        freezeTrack->prepareToPlay(48'000.0, 8);
        if (!freezeTrack->publishFrozenAudio(frozen, 8, 48'000.0, 11, 13) ||
            !freezeTrack->isFrozen() || freezeTrack->frozenProjectGeneration() != 11 ||
            freezeTrack->frozenAudioGeneration() != 13) return 65;
        Aura::Core::AudioBuffer rendered(2, 8);
        rendered.clear();
        freezeTrack->process(rendered, 8, 0);
        if (rendered.getReadPointer(0)[0] != 1.0f ||
            rendered.getReadPointer(0)[7] != 8.0f ||
            rendered.getReadPointer(1)[0] != 100.0f ||
            rendered.getReadPointer(1)[7] != 107.0f) return 66;
        freezeTrack->clearFrozenAudio();
        if (freezeTrack->isFrozen()) return 67;
        auto renderedTrack = std::make_shared<Aura::Core::Engine::Track>(80, "Rendered Freeze", Aura::Core::Engine::Track::Audio);
        Aura::Core::Engine::Region sourceRegion{};
        sourceRegion.id = 1;
        sourceRegion.start = 0;
        sourceRegion.len = 8;
        sourceRegion.muted = false;
        sourceRegion.name = "freeze-source";
        sourceRegion.audio = frozen;
        renderedTrack->addRegion(sourceRegion);
        if (!Aura::Core::Engine::TrackFreezeManager::freezeTrack(renderedTrack, 8, 48'000) ||
            !renderedTrack->isFrozen()) return 68;
        Aura::Core::Engine::TrackFreezeManager::unfreezeTrack(renderedTrack);
        renderedTrack->collectRetiredFrozenAudio();
        if (renderedTrack->isFrozen()) return 69;
    }
    auto& vca = Aura::Core::Engine::VCAControlSystem::getInstance();
    vca.createGroup("Contract VCA", {77});
    const auto encodedProject = Aura::IO::Persistence::ProjectEncoder::encode(encodedTimeline);
    if (encodedProject.find("\"vca_groups\"") == std::string::npos ||
        encodedProject.find("Contract VCA") == std::string::npos ||
        encodedProject.find("77") == std::string::npos ||
        encodedProject.find("MIDI Contract") == std::string::npos ||
        encodedProject.find("\"pitch\": 60") == std::string::npos) return 42;

    Aura::DSP::Mixing::NeuralDynamicsModel neural;
    Aura::DSP::Mixing::NeuralDynamicsModel::Weights malformedWeights;
    malformedWeights.inputWeights = {std::numeric_limits<float>::quiet_NaN()};
    malformedWeights.recurrentWeights = {0.25f};
    const float neuralOutput = neural.process(std::numeric_limits<float>::infinity(), malformedWeights);
    if (!std::isfinite(neuralOutput)) return 37;
    Aura::DSP::Analysis::SpectralProcessor spectral;
    Aura::Core::AudioBuffer invalidSpectralBuffer(1, 128);
    spectral.applyMask(invalidSpectralBuffer, 48'000.0,
                       Aura::DSP::Analysis::SpectralProcessor::Rect{
                           std::numeric_limits<float>::quiet_NaN(), 0.0f, 1.0f, 100.0f},
                       0.0f);
    if (spectral.canUndo()) return 80;
    std::string spectralFactoryError;
    auto spectralPlugin = Aura::Core::Plugins::PluginFactory::create(
        Aura::Core::Plugins::PluginDescription{"Spectral Restoration", "Aura", Aura::Core::Plugins::PluginFormat::Internal, ""},
        &spectralFactoryError);
    if (!spectralPlugin || spectralPlugin->getName() != "Spectral Restoration" ||
        !spectralFactoryError.empty()) return 75;
    if (spectralPlugin->getTailSamples() <= spectralPlugin->getLatencySamples()) return 92;
    spectralPlugin->setBypassed(true);
    spectralPlugin->setSidechainBus(7);
    const auto spectralState = spectralPlugin->getState();
    if (spectralState.size() != 24u) return 76;
    auto restoredSpectral = Aura::Core::Plugins::PluginFactory::create(
        Aura::Core::Plugins::PluginDescription{"Spectral Restoration", "Aura", Aura::Core::Plugins::PluginFormat::Internal, ""});
    if (!restoredSpectral || !restoredSpectral->setState(spectralState) ||
        !restoredSpectral->isBypassed() || restoredSpectral->getSidechainBus() != 7u) return 77;
    Aura::Core::AudioBuffer spectralBuffer(1, 2048);
    spectralBuffer.getWritePointer(0)[0] = 1.0f;
    spectral.applyMask(spectralBuffer, 48'000.0,
                       Aura::DSP::Analysis::SpectralProcessor::Rect{0.0f, 0.0f, 1.0f, 24'000.0f},
                       0.5f);
    if (!std::isfinite(spectralBuffer.getReadPointer(0)[0])) return 38;
    Aura::Core::AudioBuffer multiRegion(1, 4096);
    for (uint32_t i = 0; i < multiRegion.getNumSamples(); ++i)
        multiRegion.getWritePointer(0)[i] = 0.25f;
    double energyBefore = 0.0;
    for (uint32_t i = 0; i < multiRegion.getNumSamples(); ++i)
        energyBefore += std::fabs(multiRegion.getReadPointer(0)[i]);
    const std::vector<Aura::DSP::Analysis::SpectralProcessor::Rect> regions{
        {0.0f, 0.0f, 0.04f, 900.0f},
        {0.04f, 1'100.0f, 0.08f, 2'000.0f}};
    spectral.applySpectralGain(multiRegion, 48'000.0, regions, 0.0f);
    double energyAfter = 0.0;
    for (uint32_t i = 0; i < multiRegion.getNumSamples(); ++i)
        energyAfter += std::fabs(multiRegion.getReadPointer(0)[i]);
    if (!spectral.canUndo(multiRegion) || spectral.undoDepth(multiRegion) != 1u ||
        !(energyAfter < energyBefore)) return 88;
    if (!spectral.undo(multiRegion) || spectral.canUndo(multiRegion)) return 89;
    Aura::Core::AudioBuffer brushRegion(1, 4096);
    for (uint32_t i = 0; i < brushRegion.getNumSamples(); ++i)
        brushRegion.getWritePointer(0)[i] = 0.25f;
    const std::vector<Aura::DSP::Analysis::SpectralProcessor::RegionGain> brushMask{
        {{0.0f, 0.0f, 0.04f, 900.0f}, 0.0f},
        {{0.04f, 1'100.0f, 0.08f, 2'000.0f}, 2.0f}};
    spectral.applySpectralMask(brushRegion, 48'000.0, brushMask);
    if (!spectral.canUndo(brushRegion) || spectral.undoLabel(brushRegion) != "Spectral Brush" ||
        !spectral.undo(brushRegion)) return 93;
    Aura::Core::AudioBuffer selectedClicks(1, 2048);
    selectedClicks.getWritePointer(0)[300] = 1.0f;
    selectedClicks.getWritePointer(0)[1500] = 1.0f;
    if (spectral.removeClicks(selectedClicks, 48'000.0,
                              Aura::DSP::Analysis::SpectralProcessor::Rect{0.005f, 0.0f, 0.010f, 20'000.0f},
                              0.1f, 2) == 0 ||
        std::fabs(selectedClicks.getReadPointer(0)[1500] - 1.0f) > 1.0e-6f ||
        std::fabs(selectedClicks.getReadPointer(0)[300]) > 0.5f) return 90;
    Aura::Core::AudioBuffer selectedClips(1, 2048);
    selectedClips.getWritePointer(0)[499] = 0.1f;
    selectedClips.getWritePointer(0)[500] = 1.0f;
    selectedClips.getWritePointer(0)[501] = 1.0f;
    selectedClips.getWritePointer(0)[502] = 0.1f;
    selectedClips.getWritePointer(0)[1500] = 1.0f;
    if (spectral.repairClipped(selectedClips, 48'000.0,
                               Aura::DSP::Analysis::SpectralProcessor::Rect{0.009f, 0.0f, 0.011f, 20'000.0f},
                               0.98f) == 0 ||
        std::fabs(selectedClips.getReadPointer(0)[1500] - 1.0f) > 1.0e-6f ||
        std::fabs(selectedClips.getReadPointer(0)[500] - 1.0f) < 1.0e-3f) return 91;
    spectralBuffer.getWritePointer(0)[100] = 1.0f;
    spectralBuffer.getWritePointer(0)[101] = -1.0f;
    if (spectral.removeClicks(spectralBuffer, 0.25f, 2) == 0) return 68;
    spectralBuffer.getWritePointer(0)[200] = 1.0f;
    spectralBuffer.getWritePointer(0)[201] = 1.0f;
    if (spectral.repairClipped(spectralBuffer, 0.98f) == 0) return 67;
    spectral.reduceNoise(spectralBuffer, 48'000.0, 0.75f, 0.05f);
    spectral.interpolateRegion(
        spectralBuffer, 48'000.0,
        Aura::DSP::Analysis::SpectralProcessor::Rect{0.0f, 200.0f, 0.02f, 2'000.0f},
        0.8f);
    for (uint32_t i = 0; i < spectralBuffer.getNumSamples(); ++i)
        if (!std::isfinite(spectralBuffer.getReadPointer(0)[i])) return 70;
    if (!spectral.canUndo() || !spectral.undo(spectralBuffer) || !spectral.canRedo() ||
        !spectral.redo(spectralBuffer)) return 74;
    if (spectral.undoDepth() == 0 || spectral.redoDepth() != 0) return 82;
    Aura::DSP::Analysis::SpectralProcessor ownershipHistory;
    Aura::Core::AudioBuffer ownerA(1, 128), ownerB(1, 128);
    ownershipHistory.applyMask(ownerA, 48'000.0,
                               Aura::DSP::Analysis::SpectralProcessor::Rect{0.0f, 0.0f, 1.0f, 2'000.0f},
                               0.5f);
    if (ownershipHistory.undo(ownerB) || !ownershipHistory.canUndo()) return 85;
    ownershipHistory.applyMask(ownerB, 48'000.0,
                               Aura::DSP::Analysis::SpectralProcessor::Rect{0.0f, 0.0f, 1.0f, 2'000.0f},
                               0.5f);
    if (!ownershipHistory.canUndo(ownerA) || !ownershipHistory.undo(ownerA) ||
        !ownershipHistory.canUndo(ownerB)) return 86;
    ownershipHistory.clearHistory(ownerA);
    if (ownershipHistory.canUndo(ownerA) || !ownershipHistory.canUndo(ownerB)) return 87;
    Aura::DSP::Effects::SpectralRestorationProcessor restoration(48'000.0);
    restoration.applySpectralGain(
        spectralBuffer,
        Aura::DSP::Analysis::SpectralProcessor::Rect{0.0f, 100.0f, 0.02f, 8'000.0f},
        0.25f);
    restoration.healRegion(
        spectralBuffer,
        Aura::DSP::Analysis::SpectralProcessor::Rect{0.0f, 100.0f, 0.02f, 8'000.0f},
        0.5f);
    restoration.reduceNoise(
        spectralBuffer,
        Aura::DSP::Analysis::SpectralProcessor::Rect{0.0f, 0.0f, 0.02f, 0.0f},
        0.5f, 0.05f);
    Aura::Core::AudioBuffer localNoise(1, 4096);
    for (uint32_t i = 0; i < localNoise.getNumSamples(); ++i)
        localNoise.getWritePointer(0)[i] = 0.1f;
    restoration.reduceNoise(
        localNoise,
        Aura::DSP::Analysis::SpectralProcessor::Rect{0.0f, 0.0f, 0.01f, 0.0f},
        0.5f, 0.05f);
    if (std::fabs(localNoise.getReadPointer(0)[3500] - 0.1f) > 1.0e-6f) return 79;
    restoration.eraseHarmonics(
        spectralBuffer, 60.0f,
        Aura::DSP::Analysis::SpectralProcessor::Rect{0.0f, 0.0f, 0.02f, 4'000.0f},
        4, 4.0f);
    if (restoration.undoLabelOffline() != "Remove Hum") return 84;
    if (!restoration.canUndoOffline() || !restoration.undoOffline(spectralBuffer) ||
        !restoration.canRedoOffline() || restoration.redoLabelOffline() != "Remove Hum" ||
        !restoration.redoOffline(spectralBuffer)) return 78;
    if (restoration.undoDepthOffline() == 0 || restoration.redoDepthOffline() != 0) return 83;
    restoration.reset();
    if (restoration.canUndoOffline() || restoration.canRedoOffline()) return 81;
    Aura::DSP::Effects::DivineConsoleStrip console;
    console.prepareToPlay(48'000.0, 64);
    if (!console.setBand(0, 120.0f, 6.0f, 0.8f) || console.getNumParameters() != 7) return 71;
    Aura::Core::AudioBuffer consoleBuffer(2, 64);
    for (uint32_t i = 0; i < 64; ++i) {
        consoleBuffer.getWritePointer(0)[i] = 0.1f;
        consoleBuffer.getWritePointer(1)[i] = 0.1f;
    }
    Aura::Core::MidiBuffer consoleMidi;
    Aura::DSP::ProcessContext consoleContext{};
    consoleContext.sampleRate = 48'000.0; consoleContext.blockSize = 64;
    console.process(consoleBuffer, consoleMidi, consoleContext);
    for (uint32_t i = 0; i < 64; ++i)
        if (!std::isfinite(consoleBuffer.getReadPointer(0)[i])) return 72;
    const auto consoleState = console.getState();
    Aura::DSP::Effects::DivineConsoleStrip restoredConsole;
    if (!restoredConsole.setState(consoleState) || restoredConsole.getNumParameters() != 7) return 73;
    Aura::DSP::Effects::SidechainSpectralDucker ducker;
    ducker.prepareToPlay(48'000.0, 128);
    Aura::Core::AudioBuffer duckBuffer(2, 1024), sidechainBuffer(2, 1024);
    for (uint32_t i = 0; i < 1024; ++i) {
        duckBuffer.getWritePointer(0)[i] = 0.1f;
        duckBuffer.getWritePointer(1)[i] = -0.1f;
        sidechainBuffer.getWritePointer(0)[i] = (i % 32 == 0) ? 1.0f : 0.0f;
        sidechainBuffer.getWritePointer(1)[i] = sidechainBuffer.getReadPointer(0)[i];
    }
    Aura::DSP::ProcessContext duckContext{};
    duckContext.sampleRate = 48'000.0; duckContext.blockSize = 1024;
    duckContext.sidechainBuffer = &sidechainBuffer;
    Aura::Core::MidiBuffer duckMidi;
    ducker.process(duckBuffer, duckMidi, duckContext);
    for (uint32_t i = 0; i < 1024; ++i)
        if (!std::isfinite(duckBuffer.getReadPointer(0)[i]) || !std::isfinite(duckBuffer.getReadPointer(1)[i])) return 74;
    Aura::DSP::Effects::Arpeggiator arp;
    arp.prepareToPlay(48'000.0, 256);
    Aura::Core::AudioBuffer arpAudio(2, 256);
    Aura::Core::MidiBuffer arpMidi;
    arpMidi.addNoteOn(2, 60, 100, 0);
    arpMidi.addNoteOn(2, 64, 100, 1);
    Aura::DSP::ProcessContext arpContext{};
    arpContext.sampleRate = 48'000.0; arpContext.bpm = 120.0; arpContext.blockStart = 0; arpContext.blockSize = 256;
    arp.process(arpAudio, arpMidi, arpContext);
    Aura::Core::MidiBuffer releaseMidi;
    releaseMidi.addNoteOff(2, 64, 2);
    arp.process(arpAudio, releaseMidi, arpContext);
    if (arp.getNumParameters() != 1) return 75;
    Aura::DSP::Effects::StereoChorus chorus;
    chorus.prepareToPlay(48'000.0, 64);
    chorus.setParameter(0, 0.4f); chorus.setParameter(1, 0.65f);
    Aura::Core::AudioBuffer chorusBuffer(2, 64);
    for (uint32_t i = 0; i < 64; ++i) { chorusBuffer.getWritePointer(0)[i] = 0.2f; chorusBuffer.getWritePointer(1)[i] = -0.2f; }
    Aura::Core::MidiBuffer chorusMidi;
    chorus.process(chorusBuffer, chorusMidi, duckContext);
    for (uint32_t i = 0; i < 64; ++i)
        if (!std::isfinite(chorusBuffer.getReadPointer(0)[i]) || !std::isfinite(chorusBuffer.getReadPointer(1)[i])) return 76;
    Aura::DSP::Effects::StereoChorus restoredChorus;
    if (!restoredChorus.setState(chorus.getState()) || restoredChorus.getNumParameters() != 2) return 77;
    auto legacyDriver = Aura::IO::HardwareFactory::createDefault();
    if (!legacyDriver || !legacyDriver->initialize(48'000.0, 256) ||
        legacyDriver->getDeviceName().empty()) return 39;
    {
        using Transport = Aura::Core::Plugins::MidiFragmentReassembler;
        Transport transport;
        Transport::Fragment first{};
        first.messageId = 77; first.index = 0; first.total = 2; first.sampleOffset = 12;
        first.size = 2; first.payload[0] = 0xf0; first.payload[1] = 0x01;
        Transport::Fragment second{};
        second.messageId = 77; second.index = 1; second.total = 2;
        second.size = 2; second.payload[0] = 0x02; second.payload[1] = 0xf7;
        if (transport.push(first) != Transport::Result::Accepted ||
            transport.push(second) != Transport::Result::Complete ||
            !transport.complete() || transport.size() != 4 ||
            transport.data()[0] != 0xf0 || transport.data()[3] != 0xf7) return 54;
        Aura::Core::MidiBuffer mailbox;
        Transport mailboxTransport;
        if (mailbox.addFragment(mailboxTransport, first) != Transport::Result::Accepted ||
            mailbox.addFragment(mailboxTransport, second) != Transport::Result::Complete ||
            mailbox.size() != 1 || mailbox.getEvents()[0].size != 4 ||
            mailbox.getEvents()[0].data[3] != 0xf7) return 56;
        Transport::Fragment largeFirst{};
        largeFirst.messageId = 91; largeFirst.index = 0; largeFirst.total = 2;
        largeFirst.size = Transport::kFragmentPayloadBytes;
        Transport::Fragment largeSecond{};
        largeSecond.messageId = 91; largeSecond.index = 1; largeSecond.total = 2;
        largeSecond.size = Transport::kFragmentPayloadBytes;
        std::fill(largeFirst.payload.begin(), largeFirst.payload.end(), 0x55);
        std::fill(largeSecond.payload.begin(), largeSecond.payload.end(), 0xaa);
        Transport largeTransport;
        auto extended = std::make_unique<Aura::Core::Plugins::MidiExtendedMessageRing>();
        if (mailbox.addFragment(largeTransport, largeFirst, extended.get()) != Transport::Result::Accepted ||
            mailbox.addFragment(largeTransport, largeSecond, extended.get()) != Transport::Result::Complete ||
            mailbox.size() != 1 || extended->size() != 1) return 57;
        Aura::Core::Plugins::MidiExtendedMessageRing::Message largeMessage;
        if (!extended->pop(largeMessage) || largeMessage.size != 2 * Transport::kFragmentPayloadBytes ||
            largeMessage.data[0] != 0x55 || largeMessage.data[largeMessage.size - 1] != 0xaa) return 58;
        Transport::Fragment wrongOrder = first;
        wrongOrder.messageId = 88;
        if (transport.push(wrongOrder) != Transport::Result::Accepted) return 55;
    }
    const auto wave64Path = std::filesystem::temp_directory_path() / "aura-native-wave64-contract.w64";
    const float waveLeft[] = {0.0f, 0.25f};
    const float waveRight[] = {-0.25f, 0.5f};
    if (!Aura::IO::Persistence::WavWriter::writeWave64(wave64Path.string(), waveLeft, waveRight, 2, 48'000)) return 43;
    {
        std::ifstream wave64(wave64Path, std::ios::binary);
        std::array<uint8_t, 24> header{};
        wave64.read(reinterpret_cast<char*>(header.data()), static_cast<std::streamsize>(header.size()));
        if (!wave64 || header[0] != 0x52 || header[1] != 0x49 || header[2] != 0x46 || header[3] != 0x46) return 44;
    }
    Aura::IO::WavLoader::WavInfo waveInfo{};
    const auto waveChannels = Aura::IO::WavLoader::loadWave64(wave64Path.string(), waveInfo);
    if (waveInfo.sampleRate != 48'000 || waveInfo.numChannels != 2 || waveInfo.bitDepth != 32 ||
        waveChannels.size() != 2 || waveChannels[0].size() != 2 ||
        std::abs(waveChannels[1][1] - 0.5f) > 0.0001f) return 45;
    const auto wave64MultiPath = std::filesystem::temp_directory_path() / "aura-native-wave64-multichannel-contract.w64";
    const std::vector<std::vector<float>> wave64Multi{{0.0f, 0.25f}, {-0.25f, 0.5f}, {0.75f, -0.5f}};
    if (!Aura::IO::WavSaver::saveWave64(wave64MultiPath.string(), wave64Multi, 48'000)) return 52;
    Aura::IO::WavLoader::WavInfo wave64MultiInfo{};
    const auto wave64MultiDecoded = Aura::IO::WavLoader::loadWave64(
        wave64MultiPath.string(), wave64MultiInfo);
    if (wave64MultiInfo.numChannels != 3 || wave64MultiDecoded.size() != 3 ||
        wave64MultiDecoded[2].size() != 2 ||
        std::abs(wave64MultiDecoded[2][0] - 0.75f) > 0.0001f) return 53;
    const auto oversizedWave64Path = std::filesystem::temp_directory_path() /
                                     "aura-native-wave64-oversized-contract.w64";
    {
        std::ofstream oversized(oversizedWave64Path, std::ios::binary | std::ios::trunc);
        oversized.seekp(static_cast<std::streamoff>(Aura::IO::WavLoader::kMaximumDecodedBytes + 2ull * 1024ull * 1024ull - 1ull));
        oversized.put('\0');
    }
    bool oversizedRejected = false;
    try {
        Aura::IO::WavLoader::WavInfo oversizedInfo{};
        (void)Aura::IO::WavLoader::loadWave64(oversizedWave64Path.string(), oversizedInfo);
    } catch (const std::runtime_error&) {
        oversizedRejected = true;
    }
    std::filesystem::remove(oversizedWave64Path, offlineCleanup);
    if (!oversizedRejected) return 61;
    const auto reservedTempPath = std::filesystem::temp_directory_path() /
                                  "aura-native-wav-reservation-contract.tmp";
    std::filesystem::remove(reservedTempPath, offlineCleanup);
    if (!Aura::IO::Persistence::WavWriter::reserveTemporary(reservedTempPath) ||
        Aura::IO::Persistence::WavWriter::reserveTemporary(reservedTempPath)) return 62;
    std::filesystem::remove(reservedTempPath, offlineCleanup);
    const auto pcm16Path = std::filesystem::temp_directory_path() / "aura-native-pcm16-contract.wav";
    if (!Aura::Core::Utils::WavWriter::write(pcm16Path.string(), waveLeft, waveRight, 2, 48'000)) return 48;
    const auto pcm16MultiPath = std::filesystem::temp_directory_path() / "aura-native-pcm16-multichannel-contract.wav";
    const std::vector<std::vector<float>> pcm16Multi{{0.0f, 0.25f}, {-0.25f, 0.5f}, {0.75f, -0.5f}};
    if (!Aura::IO::Persistence::WavWriter::writePcm16Interleaved(
            pcm16MultiPath.string(), pcm16Multi, 48'000)) return 60;
    Aura::IO::WavLoader::WavInfo pcm16Info{};
    const auto pcm16Decoded = Aura::IO::WavLoader::load(pcm16MultiPath.string(), pcm16Info);
    if (pcm16Info.numChannels != 3 || pcm16Info.bitDepth != 16 || pcm16Decoded.size() != 3 ||
        pcm16Decoded[2].size() != 2 || std::abs(pcm16Decoded[2][0] - 0.75f) > 0.001f ||
        std::abs(pcm16Decoded[1][1] - 0.5f) > 0.001f) return 61;
    std::filesystem::remove(pcm16MultiPath, offlineCleanup);
    {
        std::ifstream pcm16(pcm16Path, std::ios::binary);
        std::array<uint8_t, 36> header{};
        pcm16.read(reinterpret_cast<char*>(header.data()), static_cast<std::streamsize>(header.size()));
        if (!pcm16 || std::string(reinterpret_cast<const char*>(header.data()), 4) != "RIFF" ||
            header[34] != 16 || header[35] != 0) return 49;
    }
    const auto pcm24Path = std::filesystem::temp_directory_path() / "aura-native-pcm24-contract.wav";
    if (!Aura::Core::Utils::WavWriter::writePcm24(pcm24Path.string(), waveLeft, waveRight, 2, 48'000)) return 50;
    {
        std::ifstream pcm24(pcm24Path, std::ios::binary);
        std::array<uint8_t, 36> header{};
        pcm24.read(reinterpret_cast<char*>(header.data()), static_cast<std::streamsize>(header.size()));
        if (!pcm24 || std::string(reinterpret_cast<const char*>(header.data()), 4) != "RIFF" ||
            header[34] != 24 || header[35] != 0) return 51;
    }
    const auto legacyBouncePath = std::filesystem::temp_directory_path() /
                                  "aura-native-legacy-bounce-contract.wav";
    Aura::Core::Engine::LegacyBounceEngine::ExportProgress legacyProgress;
    Aura::Core::Engine::LegacyBounceEngine::renderToFile(
        legacyBouncePath.string(), 0.001, 48'000.0, legacyProgress,
        [](float* left, float* right, uint32_t count) {
            for (uint32_t i = 0; i < count; ++i) { left[i] = 0.25f; right[i] = -0.25f; }
        });
    if (!legacyProgress.isDone.load() || legacyProgress.cancelled.load() ||
        !std::filesystem::is_regular_file(legacyBouncePath)) return 61;
    std::ifstream legacyBounce(legacyBouncePath, std::ios::binary);
    std::array<uint8_t, 36> legacyHeader{};
    legacyBounce.read(reinterpret_cast<char*>(legacyHeader.data()),
                      static_cast<std::streamsize>(legacyHeader.size()));
    if (!legacyBounce || std::string(legacyHeader.begin(), legacyHeader.begin() + 4) != "RIFF" ||
        legacyHeader[34] != 16 || legacyHeader[35] != 0) return 62;
    const auto streamPath = std::filesystem::temp_directory_path() / "aura-native-pcm24-stream-contract.wav";
    {
        Aura::IO::Persistence::WavWriter::Pcm24StreamWriter stream(
            streamPath.string(), 4, 48'000);
        if (!stream.isOpen() || !stream.writeFrames(waveLeft, waveRight, 2) ||
            !stream.writeFrames(waveLeft, waveRight, 2) || !stream.finish()) return 59;
        std::ifstream streamInput(streamPath, std::ios::binary);
        std::array<uint8_t, 44> streamHeader{};
        streamInput.read(reinterpret_cast<char*>(streamHeader.data()),
                         static_cast<std::streamsize>(streamHeader.size()));
        if (!streamInput || std::string(streamHeader.begin(), streamHeader.begin() + 4) != "RIFF" ||
            std::string(streamHeader.begin() + 8, streamHeader.begin() + 12) != "WAVE" ||
            streamHeader[34] != 24 || streamHeader[35] != 0 ||
            std::filesystem::file_size(streamPath) != 68) return 60;
    }
    const auto pcm16StreamPath = std::filesystem::temp_directory_path() /
                                 "aura-native-pcm16-stream-contract.wav";
    {
        Aura::IO::Persistence::WavWriter::Pcm16StreamWriter stream(
            pcm16StreamPath.string(), 4, 48'000);
        if (!stream.isOpen() || !stream.writeFrames(waveLeft, waveRight, 1) ||
            !stream.writeFrames(waveLeft + 1, waveRight + 1, 3) || !stream.finish()) return 63;
        std::ifstream streamInput(pcm16StreamPath, std::ios::binary);
        std::array<uint8_t, 44> streamHeader{};
        streamInput.read(reinterpret_cast<char*>(streamHeader.data()),
                         static_cast<std::streamsize>(streamHeader.size()));
        if (!streamInput || std::string(streamHeader.begin(), streamHeader.begin() + 4) != "RIFF" ||
            std::string(streamHeader.begin() + 8, streamHeader.begin() + 12) != "WAVE" ||
            streamHeader[34] != 16 || streamHeader[35] != 0 ||
            std::filesystem::file_size(pcm16StreamPath) != 60) return 64;
    }
    const auto wave64StreamPath = std::filesystem::temp_directory_path() /
                                  "aura-native-wave64-stream-contract.w64";
    {
        Aura::IO::Persistence::WavWriter::Wave64FloatStreamWriter stream(
            wave64StreamPath.string(), 4, 48'000, 2);
        const float streamLeft[4] = {0.0f, 0.25f, 0.5f, 0.75f};
        const float streamRight[4] = {-0.25f, 0.5f, -0.75f, 1.0f};
        const float* channels[2] = {streamLeft, streamRight};
        if (!stream.isOpen() || !stream.writeFrames(channels, 2) ||
            !stream.writeFrames(channels, 2) || stream.framesWritten() != 4 ||
            !stream.finish()) return 76;
        Aura::IO::WavLoader::WavInfo info{};
        const auto decoded = Aura::IO::WavLoader::loadWave64(wave64StreamPath.string(), info);
        if (info.sampleRate != 48'000 || info.numChannels != 2 || info.bitDepth != 32 ||
            decoded.size() != 2 || decoded[0].size() != 4 ||
            std::abs(decoded[1][1] - 0.5f) > 0.0001f ||
            std::filesystem::file_size(wave64StreamPath) != 104 + 4 * 2 * sizeof(float)) return 77;
    }
    const auto monoPcm16StreamPath = std::filesystem::temp_directory_path() /
                                     "aura-native-pcm16-mono-stream-contract.wav";
    {
        Aura::IO::Persistence::WavWriter::Pcm16StreamWriter stream(
            monoPcm16StreamPath.string(), 4, 48'000, 1);
        if (!stream.isOpen() || !stream.writeFrames(waveLeft, nullptr, 4) || !stream.finish()) return 65;
        std::ifstream streamInput(monoPcm16StreamPath, std::ios::binary);
        std::array<uint8_t, 44> streamHeader{};
        streamInput.read(reinterpret_cast<char*>(streamHeader.data()),
                         static_cast<std::streamsize>(streamHeader.size()));
        if (!streamInput || streamHeader[22] != 1 || streamHeader[34] != 16 ||
            std::filesystem::file_size(monoPcm16StreamPath) != 52) return 66;
    }
    const auto monoPcm24StreamPath = std::filesystem::temp_directory_path() /
                                     "aura-native-pcm24-mono-stream-contract.wav";
    {
        Aura::IO::Persistence::WavWriter::Pcm24StreamWriter stream(
            monoPcm24StreamPath.string(), 4, 48'000, false, 1);
        if (!stream.isOpen() || !stream.writeFrames(waveLeft, nullptr, 4) || !stream.finish()) return 67;
        std::ifstream streamInput(monoPcm24StreamPath, std::ios::binary);
        std::array<uint8_t, 44> streamHeader{};
        streamInput.read(reinterpret_cast<char*>(streamHeader.data()),
                         static_cast<std::streamsize>(streamHeader.size()));
        if (!streamInput || streamHeader[22] != 1 || streamHeader[34] != 24 ||
            std::filesystem::file_size(monoPcm24StreamPath) != 56) return 68;
    }
    const auto floatStreamPath = std::filesystem::temp_directory_path() /
                                 "aura-native-float32-stream-contract.wav";
    {
        const float floatLeft[2] = {0.25f, std::numeric_limits<float>::quiet_NaN()};
        const float floatRight[2] = {-0.5f, 0.75f};
        const float* floatChannels[2] = {floatLeft, floatRight};
        Aura::IO::Persistence::WavWriter::Float32StreamWriter stream(
            floatStreamPath.string(), 48'000, 2);
        if (!stream.isOpen() || !stream.writeFrames(floatChannels, 2) ||
            stream.framesWritten() != 2 || !stream.finish()) return 69;
        std::ifstream streamInput(floatStreamPath, std::ios::binary);
        std::array<uint8_t, 80> streamHeader{};
        streamInput.read(reinterpret_cast<char*>(streamHeader.data()),
                         static_cast<std::streamsize>(streamHeader.size()));
        if (!streamInput || std::string(streamHeader.begin(), streamHeader.begin() + 4) != "RIFF" ||
            std::string(streamHeader.begin() + 8, streamHeader.begin() + 12) != "WAVE" ||
            streamHeader[56] != 3 || streamHeader[57] != 0 ||
            streamHeader[58] != 2 || streamHeader[59] != 0 ||
            std::filesystem::file_size(floatStreamPath) != 80 + 2 * 2 * sizeof(float)) return 70;
        float decoded[4] = {};
        streamInput.read(reinterpret_cast<char*>(decoded), sizeof(decoded));
        if (!streamInput || decoded[0] != 0.25f || decoded[1] != -0.5f ||
            decoded[2] != 0.0f || decoded[3] != 0.75f) return 71;
    }
    const auto recordingPath = std::filesystem::temp_directory_path() /
                               "aura-native-recording-engine-contract.wav";
    {
        Aura::Core::RecordingEngine recorder;
        const float left[4] = {0.1f, 0.2f, 0.3f, 0.4f};
        const float right[4] = {-0.1f, -0.2f, -0.3f, -0.4f};
        if (!recorder.start(recordingPath.string(), 48'000.0) ||
            !recorder.write(left, right, 4)) return 72;
        recorder.stop();
        if (recorder.hasWriteError() || !std::filesystem::is_regular_file(recordingPath) ||
            std::filesystem::file_size(recordingPath) != 80 + 4 * 2 * sizeof(float)) return 73;
        std::ifstream recordingInput(recordingPath, std::ios::binary);
        std::array<uint8_t, 80> recordingHeader{};
        recordingInput.read(reinterpret_cast<char*>(recordingHeader.data()),
                            static_cast<std::streamsize>(recordingHeader.size()));
        if (!recordingInput || std::string(recordingHeader.begin(), recordingHeader.begin() + 4) != "RIFF" ||
            recordingHeader[56] != 3 || recordingHeader[57] != 0 ||
            recordingHeader[58] != 2 || recordingHeader[59] != 0) return 74;
    }
    const auto malformedWave64Path = std::filesystem::temp_directory_path() / "aura-native-wave64-malformed.w64";
    {
        std::ifstream input(wave64Path, std::ios::binary);
        std::vector<uint8_t> bytes((std::istreambuf_iterator<char>(input)), std::istreambuf_iterator<char>());
        if (bytes.size() < 64) return 46;
        for (size_t i = 56; i < 64; ++i) bytes[i] = 0; // fmt chunk size must be >= 40
        std::ofstream output(malformedWave64Path, std::ios::binary | std::ios::trunc);
        output.write(reinterpret_cast<const char*>(bytes.data()), static_cast<std::streamsize>(bytes.size()));
    }
    bool malformedRejected = false;
    try {
        Aura::IO::WavLoader::WavInfo malformedInfo{};
        (void)Aura::IO::WavLoader::loadWave64(malformedWave64Path.string(), malformedInfo);
    } catch (const std::exception&) {
        malformedRejected = true;
    }
    if (!malformedRejected) return 47;
    const auto validWaveDiagnostic = Aura::IO::WavLoader::loadDiagnosticJson(
        wave64Path.string(), true);
    if (validWaveDiagnostic.find("\"ok\":true") == std::string::npos ||
        validWaveDiagnostic.find("\"format\":\"WAVE64\"") == std::string::npos ||
        validWaveDiagnostic.find("\"channels\":2") == std::string::npos) return 56;
    const auto malformedWaveDiagnostic = Aura::IO::WavLoader::loadDiagnosticJson(
        malformedWave64Path.string(), true);
    if (malformedWaveDiagnostic.find("\"ok\":false") == std::string::npos ||
        malformedWaveDiagnostic.find("\"error\":\"") == std::string::npos) return 57;
    const auto missingWaveDiagnostic = Aura::IO::WavLoader::loadDiagnosticJson(
        (wave64Path.parent_path() / "aura-wav-does-not-exist.w64").string(), true);
    if (missingWaveDiagnostic.find("\"ok\":false") == std::string::npos ||
        missingWaveDiagnostic.find("WAVE64 file not found") == std::string::npos) return 58;
    std::error_code wave64Cleanup;
    std::filesystem::remove(wave64Path, wave64Cleanup);
    std::filesystem::remove(wave64MultiPath, wave64Cleanup);
    std::filesystem::remove(malformedWave64Path, wave64Cleanup);
    std::filesystem::remove(pcm16Path, wave64Cleanup);
    std::filesystem::remove(pcm24Path, wave64Cleanup);
    std::filesystem::remove(streamPath, wave64Cleanup);
    std::filesystem::remove(pcm16StreamPath, wave64Cleanup);
    std::filesystem::remove(wave64StreamPath, wave64Cleanup);
    std::filesystem::remove(monoPcm16StreamPath, wave64Cleanup);
    std::filesystem::remove(monoPcm24StreamPath, wave64Cleanup);
    std::filesystem::remove(legacyBouncePath, wave64Cleanup);
    return 0;
}
