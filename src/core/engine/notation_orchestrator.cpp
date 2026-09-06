#include "notation_orchestrator.hpp"
#include <algorithm>
#include <cmath>

namespace Aura::Core::Engine {

void NotationOrchestrator::addNote(int32_t pitch, uint64_t start, uint64_t duration) {
    if (pitch < 0 || pitch > 127 || duration == 0) return;
    NotationSymbol s;
    s.type = NotationSymbol::Type::Note;
    s.val = pitch;
    s.isVisible = true;
    s.start = start;
    s.duration = duration;
    m_symbols.push_back(s);
}

void NotationOrchestrator::updateLayout(float viewWidth, float viewHeight) {
    // --- INDUSTRIAL AUTO-LAYOUT ENGINE ---
    m_primitives.clear();
    if (!std::isfinite(viewWidth) || !std::isfinite(viewHeight) || viewWidth <= 0.0f || viewHeight <= 0.0f) return;
    std::stable_sort(m_symbols.begin(), m_symbols.end(), [](const auto& a, const auto& b) {
        return a.start < b.start;
    });
    uint64_t maxEnd = 1;
    for (const auto& symbol : m_symbols) {
        maxEnd = std::max(maxEnd, symbol.start > UINT64_MAX - symbol.duration
            ? UINT64_MAX : symbol.start + symbol.duration);
    }

    // 1. Horizontal Spacing (Non-linear time-to-x based on density)
    const float usableWidth = std::max(1.0f, viewWidth - 20.0f);
    for (auto& symbol : m_symbols) {
        symbol.x = 10.0f + static_cast<float>(static_cast<long double>(symbol.start)
            / static_cast<long double>(maxEnd) * usableWidth);
    }

    // 2. Vertical Stacking (Clef based)
    for (auto& s : m_symbols) {
        if (s.type == NotationSymbol::Type::Note) {
            s.y = std::clamp(viewHeight * 0.5f - (static_cast<float>(s.val) - 60.0f) * 2.5f,
                             8.0f, std::max(8.0f, viewHeight - 8.0f));
        }
    }

    // 3. Collision Avoidance
    avoidCollisions();

    // 4. Grouping Logic
    calculateBeams();
    calculateSlurs();

    // Generate stable, renderer-agnostic primitives: note head, stem and
    // duration beam.  Consumers can map the type ids to SMuFL glyphs.
    for (const auto& symbol : m_symbols) {
        if (!symbol.isVisible || symbol.type != NotationSymbol::Type::Note) continue;
        m_primitives.push_back({0, symbol.x - 4.0f, symbol.y, symbol.x + 4.0f, symbol.y});
        m_primitives.push_back({1, symbol.x + 3.0f, symbol.y, symbol.x + 3.0f,
                                symbol.y - (symbol.val >= 60 ? 28.0f : -28.0f)});
    }
}

void NotationOrchestrator::avoidCollisions() {
    float lastX = -100000.0f;
    for (auto& symbol : m_symbols) {
        if (!symbol.isVisible) continue;
        if (symbol.x - lastX < 10.0f) symbol.x = lastX + 10.0f;
        lastX = symbol.x;
    }
}

void NotationOrchestrator::calculateBeams() {
    for (size_t i = 1; i < m_symbols.size(); ++i) {
        const auto& previous = m_symbols[i - 1];
        const auto& current = m_symbols[i];
        if (!previous.isVisible || !current.isVisible
            || previous.type != NotationSymbol::Type::Note
            || current.type != NotationSymbol::Type::Note) continue;
        // Short-duration neighbours share a beam in the layout model.
        if (previous.duration <= 240 && current.duration <= 240
            && current.x - previous.x < 100.0f) {
            const float stemY = std::min(previous.y, current.y) - 28.0f;
            m_primitives.push_back({2, previous.x + 3.0f, stemY,
                                    current.x + 3.0f, stemY});
        }
    }
}

void NotationOrchestrator::calculateSlurs() {
    for (size_t i = 1; i < m_symbols.size(); ++i) {
        const auto& a = m_symbols[i - 1];
        const auto& b = m_symbols[i];
        const uint64_t aEnd = a.start > UINT64_MAX - a.duration
            ? UINT64_MAX : a.start + a.duration;
        if (a.type != NotationSymbol::Type::Note || b.type != NotationSymbol::Type::Note
            || !a.isVisible || !b.isVisible || b.start < aEnd) continue;
        const float y = std::min(a.y, b.y) - 36.0f;
        m_primitives.push_back({3, a.x, y, b.x, y});
    }
}

size_t NotationOrchestrator::getRenderPrimitives(RenderPrimitive* out, size_t maxCount) {
    size_t count = std::min(maxCount, m_primitives.size());
    for (size_t i = 0; i < count; ++i) out[i] = m_primitives[i];
    return count;
}

} // namespace Aura::Core::Engine
