#pragma once

#include "../iprocessor.hpp"
#include "../../core/rust_ffi.hpp"

namespace Hirari::DSP::Effects {

/** C++ plugin-host adapter for the Rust FIR cabinet model. */
class CabinetSimulator final : public IProcessor {
public:
    enum class Model : uint32_t { Generic = 0, Stack4x12 = 1, Combo1x12 = 2 };

    CabinetSimulator() : m_state(hirari_cabinet_simulator_create()) {}
    ~CabinetSimulator() override { hirari_cabinet_simulator_destroy(m_state); }

    CabinetSimulator(const CabinetSimulator&) = delete;
    CabinetSimulator& operator=(const CabinetSimulator&) = delete;

    std::string getName() const override { return "Cabinet Simulator"; }
    uint32_t getLatencySamples() const noexcept override { return 0; }
    uint32_t getTailSamples() const noexcept override { return 127; }

    void prepareToPlay(double, uint32_t) noexcept override { reset(); }

    void process(Core::AudioBuffer& buffer, Core::MidiBuffer&,
                 const ProcessContext&) noexcept override {
        if (m_bypassed || buffer.getNumChannels() == 0 || buffer.getNumSamples() == 0) return;
        auto* left = buffer.getWritePointer(0);
        auto* right = buffer.getNumChannels() > 1 ? buffer.getWritePointer(1) : nullptr;
        if (left) {
            hirari_cabinet_simulator_process(m_state, left, right, buffer.getNumSamples());
        }
    }

    void reset() noexcept override { hirari_cabinet_simulator_reset(m_state); }

    void setModel(Model model) noexcept {
        hirari_cabinet_simulator_set_model(m_state, static_cast<uint32_t>(model));
    }

private:
    void* m_state = nullptr;
};

} // namespace Hirari::DSP::Effects
