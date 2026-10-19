#include <cassert>
#include <string>

#include "../src/core/plugins/vst3_host_processor.hpp"

int main() {
    using namespace Hirari::Core::Plugins;

    Hirari::Core::AudioBuffer buffer(2, 16);
    Hirari::Core::MidiBuffer midi;
    Hirari::DSP::ProcessContext context{};

    VST3HostProcessor vst3;
    assert(!vst3.hasProcessFunction());
    assert(!vst3.isOperational());
    assert(vst3.loadState() != VST3HostProcessor::LoadState::Operational);
    assert(std::string(vst3.processDiagnostic()) == VST3HostProcessor::kNoProcessFunctionDiagnostic);
    vst3.process(buffer, midi, context);
    assert(vst3.processFailed());
    assert(vst3.lastError() == VST3HostProcessor::kNoProcessFunctionDiagnostic);
    assert(!vst3.loadVst3("") && vst3.loadState() == VST3HostProcessor::LoadState::Failed);

    return 0;
}
