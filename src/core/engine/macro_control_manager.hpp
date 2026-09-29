#pragma once

#include <cstddef>
#include <cstdint>
#include "../rust_ffi.hpp"

namespace Hirari::Core::Engine {

struct MacroMapping {
    uint32_t targetParamId;
    float min;
    float max;
    bool invert;
};

/** C++ API adapter for the Rust-owned macro values, smoothing and mappings. */
class MacroControlManager {
public:
    static constexpr size_t kMaxMacros = 128;

    MacroControlManager() : m_state(hirari_macro_mapping_create()) {}
    ~MacroControlManager() { hirari_macro_mapping_destroy(m_state); }
    MacroControlManager(const MacroControlManager&) = delete;
    MacroControlManager& operator=(const MacroControlManager&) = delete;

    static MacroControlManager& getInstance() {
        static MacroControlManager instance;
        return instance;
    }

    void setMacroValue(uint32_t macroIdx, float value) {
        hirari_macro_control_set_value(m_state, macroIdx, value);
    }

    float getMacroValue(uint32_t macroIdx) const noexcept {
        return hirari_macro_control_get_value(m_state, macroIdx);
    }

    // MIDI Learn targets this opaque Rust state and a macro index directly.
    const void* midiLearnState() const noexcept { return m_state; }

    void addMapping(uint32_t macroIdx, const MacroMapping& mapping) {
        hirari_macro_mapping_add(m_state, macroIdx, mapping.targetParamId,
                                 mapping.min, mapping.max, mapping.invert);
    }

    void clearMappings(uint32_t macroIdx) {
        hirari_macro_mapping_clear(m_state, macroIdx);
    }

    float getMappedValue(uint32_t macroIdx, uint32_t targetId) const {
        return hirari_macro_mapping_evaluate(m_state, macroIdx, targetId);
    }

    void updateSmoothers(float sampleRate) {
        hirari_macro_control_update_smoothers(m_state, sampleRate);
    }

private:
    void* m_state = nullptr;
};

} // namespace Hirari::Core::Engine
