#pragma once

namespace Aura::Core::Composition {

struct HarmonicContextState {
    float tension = 0.5f;
    float valence = 0.5f;
    float stability = 0.5f;
};

class HarmonicContextTracker {
public:
    static HarmonicContextTracker& getInstance() {
        static HarmonicContextTracker instance;
        return instance;
    }

    HarmonicContextState getState() const {
        return m_state;
    }

    void setState(const HarmonicContextState& state) {
        m_state = state;
    }

private:
    HarmonicContextTracker() = default;
    HarmonicContextState m_state;
};

} // namespace Aura::Core::Composition
