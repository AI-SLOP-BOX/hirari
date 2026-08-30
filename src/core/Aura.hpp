/* Aura DAW Ultimate - High-Performance Math & Types - (c) 2026 Aura DAW Project */
#pragma once
#include <cmath>
#include <algorithm>

namespace Aura {
    template<typename T> __attribute__((always_inline)) inline T db2l(T db) { return std::pow(10, db*0.05); }
    template<typename T> __attribute__((always_inline)) inline T l2db(T l) { return 20*std::log10(std::max(1e-9, static_cast<double>(l))); }

    // Constants
    static constexpr float kPI = 3.14159265358979323846f;
    static constexpr float kTwoPI = 6.28318530717958647692f;
}
