#include <cassert>
#include <cmath>
#include <string>

#include "../src/core/plugins/plugin_host.hpp"
#include "../src/core/plugins/vst3_host_processor.hpp"
#include "../src/core/plugins/clap_host_processor.hpp"
#include "../src/core/plugins/plugin_host_infrastructure.hpp"

#if defined(__APPLE__)
#include "../src/core/plugins/au_host_processor.hpp"
#endif

int main() {
    using namespace Aura::Core::Plugins;

    Aura::Core::AudioBuffer buffer(2, 16);
    Aura::Core::MidiBuffer midi;
    Aura::DSP::ProcessContext context{};

    ExternalPluginProcessor external({"test", "test", PluginFormat::Internal, "internal"});
    assert(external.load());
    assert(external.hasProcessFunction());
    assert(external.isOperational());
    assert(external.loadState() == ExternalPluginProcessor::LoadState::Operational);
    external.process(buffer, midi, context);
    assert(!external.processFailed());

    std::string factoryError;
    auto gain = PluginFactory::create({"Gain", "Aura", PluginFormat::Internal, "builtin://gain"},
                                      &factoryError);
    assert(gain && factoryError.empty());
    gain->prepareToPlay(48000.0, 16);
    gain->setParameter(0, 2.0f);
    buffer.getWritePointer(0)[0] = 0.25f;
    gain->process(buffer, midi, context);
    assert(std::abs(buffer.getReadPointer(0)[0] - 0.5f) < 1.0e-6f);
    assert(!PluginFactory::create({"Missing", "Aura", PluginFormat::Internal, "builtin://missing"},
                                  &factoryError));
    auto& infrastructure = PluginHostInfrastructure::getInstance();
    assert(infrastructure.registerInternal("Gain", "test-builtin-gain"));
    auto managed = infrastructure.createInternalProcessor("test-builtin-gain", &factoryError);
    assert(managed && factoryError.empty());
    const auto runtime = infrastructure.runtimeSnapshot();
    assert(runtime.internal >= 1 && runtime.external == 0);
    assert(!infrastructure.createSandboxProcessor("test-builtin-gain", &factoryError));

    VST3HostProcessor vst3;
    assert(!vst3.hasProcessFunction());
    assert(!vst3.isOperational());
    assert(vst3.loadState() != VST3HostProcessor::LoadState::Operational);
    assert(std::string(vst3.processDiagnostic()) == VST3HostProcessor::kNoProcessFunctionDiagnostic);
    vst3.process(buffer, midi, context);
    assert(vst3.processFailed());
    assert(vst3.lastError() == VST3HostProcessor::kNoProcessFunctionDiagnostic);
    assert(!vst3.loadVst3("") && vst3.loadState() == VST3HostProcessor::LoadState::Failed);

    CLAPHostProcessor clap;
    assert(!clap.hasProcessFunction());
    assert(!clap.isOperational());
    assert(clap.loadState() != CLAPHostProcessor::LoadState::Operational);
    assert(std::string(clap.processDiagnostic()) == CLAPHostProcessor::kNoProcessFunctionDiagnostic);
    clap.process(buffer, midi, context);
    assert(clap.processFailed());
    assert(clap.lastError() == CLAPHostProcessor::kNoProcessFunctionDiagnostic);
    assert(!clap.loadClap("", 0) && clap.loadState() == CLAPHostProcessor::LoadState::Failed);

#if defined(__APPLE__)
    AUHostProcessor au;
    assert(!au.hasProcessFunction());
    assert(!au.isOperational());
    assert(au.loadState() != AUHostProcessor::LoadState::Operational);
    assert(std::string(au.processDiagnostic()) == AUHostProcessor::kNoProcessFunctionDiagnostic);
#endif

    return 0;
}
