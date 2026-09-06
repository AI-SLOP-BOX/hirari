#pragma once
#include <stdint.h>
#include <vector>
#include <string>
#include <memory>

namespace Aura::Core::Engine {

/**
 * @struct NotationSymbol
 * @brief Representation of a musical symbol (Note, Rest, Clef, Accidental).
 */
struct NotationSymbol {
    enum class Type { Note, Rest, Clef, Accidental, Slur, Dynamic };
    Type type;
    uint32_t val; // Pitch or Type-specific value
    float x, y;   // Layout coordinates
    bool isVisible;
    uint64_t start = 0;
    uint64_t duration = 1;
};

struct RenderPrimitive {
    int32_t type;
    float x1, y1, x2, y2;
};

/**
 * @class NotationOrchestrator
 * @brief Industrial notation and symbolic layout engine.
 * Orchestrates beam grouping, slur calculation, and auto-layout for scores.
 */
class NotationOrchestrator {
public:
    NotationOrchestrator() {}

    void clear() { m_symbols.clear(); m_primitives.clear(); }
    void addNote(int32_t pitch, uint64_t start, uint64_t duration);
    void updateLayout(float viewWidth, float viewHeight);
    size_t getRenderPrimitives(RenderPrimitive* out, size_t maxCount);

private:
    void avoidCollisions();
    void calculateBeams();
    void calculateSlurs();

    std::vector<NotationSymbol> m_symbols;
    std::vector<RenderPrimitive> m_primitives;
};

} // namespace Aura::Core::Engine
