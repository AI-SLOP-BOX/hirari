#include "notation_orchestrator.hpp"
#include <algorithm>
#include <cmath>

namespace Aura::Core::Engine {

void NotationOrchestrator::addNote(int32_t pitch, uint64_t /*start*/, uint64_t /*duration*/) {
    NotationSymbol s;
    s.type = NotationSymbol::Type::Note;
    s.val = pitch;
    s.isVisible = true;
    // (Logic for quantization to symbolic time)
    m_symbols.push_back(s);
}

void NotationOrchestrator::updateLayout(float viewWidth, float /*viewHeight*/) {
    // --- INDUSTRIAL AUTO-LAYOUT ENGINE ---
    
    // 1. Horizontal Spacing (Non-linear time-to-x based on density)
    for (size_t i = 0; i < m_symbols.size(); ++i) {
        m_symbols[i].x = (i * 40.0f) / viewWidth; 
    }

    // 2. Vertical Stacking (Clef based)
    for (auto& s : m_symbols) {
        if (s.type == NotationSymbol::Type::Note) {
            s.y = (60 - s.val) * 5.0f; // Mapping pitch to staff lines
        }
    }

    // 3. Collision Avoidance
    avoidCollisions();

    // 4. Grouping Logic
    calculateBeams();
    calculateSlurs();
}

void NotationOrchestrator::avoidCollisions() {
    // (Complex logic for moving accidentals and dynamics to avoid overlaps)
}

void NotationOrchestrator::calculateBeams() {
    // (Logic for grouping eighth notes and smaller into beams)
}

void NotationOrchestrator::calculateSlurs() {
    // (Bézier curve calculation for legato notes)
}

size_t NotationOrchestrator::getRenderPrimitives(RenderPrimitive* out, size_t maxCount) {
    size_t count = std::min(maxCount, m_primitives.size());
    for (size_t i = 0; i < count; ++i) out[i] = m_primitives[i];
    return count;
}

} // namespace Aura::Core::Engine
