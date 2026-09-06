#pragma once

#include <vector>
#include <map>
#include <array>
#include <cstdint>
#include <string>
#include "../core/engine/param_tree.hpp"

namespace Aura::IO {

/**
 * @brief ControlSurface: Professional Hardware Interaction.
 * Standard protocols for MCU (Mackie Control Universal) and HUI.
 */
class ControlSurfaceManager {
public:
    enum class Protocol { MCU, HUI };

    static ControlSurfaceManager& getInstance() { static ControlSurfaceManager i; return i; }

    /**
     * @brief BANKING: Shifts the 8-fader window of the hardware console.
     */
    void bankShift(int delta) {
        m_currentBankOffset = std::max(0, m_currentBankOffset + (delta * 8));
        updateHardware();
    }

    /**
     * @brief HARDWARE -> DAW: Handles incoming MIDI from physical faders.
     */
    void processMidiIn(uint8_t status, uint8_t data1, uint8_t data2) {
        // Logic for Mackie Protocol (Pitch Bend = Fader, V-Pot = CC)
        if ((status & 0xF0) == 0xE0 && data1 < 128 && data2 < 128) { // Pitch Bend (Faders 1-8)
            int faderIdx = status & 0x0F;
            float value = ((data2 << 7) | data1) / 16383.0f;
            uint32_t trackId = m_currentBankOffset + faderIdx;
            
            // Interaction with ParamTree (Volume Param usually offset 0)
            Core::Engine::ParamTree::getInstance().setParam(trackId * 10, value);
            updateHardware();
        }
    }

    /**
     * @brief DAW -> HARDWARE: Sends feedback to motorized faders.
     */
    void updateHardware() {
        // Build the canonical 14-bit MCU feedback frame.  The platform MIDI
        // backend consumes this frame; keeping it here also makes headless
        // clients able to inspect exactly what would be sent to hardware.
        for (uint32_t i = 0; i < 8; ++i) {
            const float value = Core::Engine::ParamTree::getInstance().getParam(
                static_cast<uint32_t>(m_currentBankOffset + static_cast<int>(i)) * 10u, 0.0f);
            const uint16_t bend = static_cast<uint16_t>(std::clamp(value, 0.0f, 1.0f) * 16383.0f + 0.5f);
            m_feedback[i] = bend;
        }
    }

    std::array<uint16_t, 8> feedbackFrame() const noexcept { return m_feedback; }

private:
    ControlSurfaceManager() : m_currentBankOffset(0) {}
    int m_currentBankOffset;
    std::array<uint16_t, 8> m_feedback{};
};

} // namespace Aura::IO
