# AURA STUDIO PRO: CORE DOCTRINE (ROOT)

## 0. ABSOLUTE GOAL
Create the "Singularity DAW" - a cinematic production environment that integrates and surpasses the capabilities of all existing workstations (Ardour, LMMS, Logic Pro, Zrythm, etc.) in a unified, deterministic, high-density architecture.

## 1. AI INTEGRATION (THE LIBRARY RULE)
- **Status**: AI is a utility, not the core DSP.
- **Rule**: AI components and models are to be integrated via established **libraries**. Do not attempt to reinvent core AI/ML inference logic. Focus development effort on the orchestration and the "Soul" of the DAW.

## 2. ANTI-HARIBOTE (REAL LOGIC ONLY)
- **Status**: Zero tolerance for facades.
- **Rule**: Every UI element must be backed by deterministic Rust/C++ logic. Avoid placeholder animations or mock telemetry. If a feature is visible, its underlying math must be industrial-grade.

## 3. ARCHITECTURE
- **UI**: Slint (High-density, declarative, zero-lag).
- **Orchestration**: Rust (Memory safety, thread-level determinism).
- **Core Engine**: C++ (High-performance SIMD/DSP kernels, legacy compatibility).

## 4. METRICS
- **Status**: Currently ~20,000 LOC of custom logic.
- **Target**: Industrial-grade depth comparable to Ardour (800k LOC), achieved through modern code efficiency and library orchestration.
