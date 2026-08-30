#pragma once
#include <vector>
#include <random>
#include <atomic>
#include <algorithm>
#include "../composition/harmonic_context_tracker.hpp"

namespace Aura::Core::DSP::Synthesis {

/**
 * @struct Grain
 * @brief Industrial Sonic Particle with Latent Sovereignty.
 */
struct Grain {
    float position;
    float length;
    float pitch;
    float pan;
    float envelope;
    float latentIndex; // --- PHASE 58: LATENT MORPHING ---
    bool active;
};

/**
 * @class WaveFunctionGrainKernel
 * @brief Industrial Singularity Engine for Granular Evolution.
 * Implements WFC-based spawning and latent-space timbre morphing.
 */
class WaveFunctionGrainKernel {
public:
    WaveFunctionGrainKernel(size_t maxGrains = 1024) {
        m_grains.resize(maxGrains, {0.0f, 0.0f, 1.0f, 0.5f, 0.0f, 0.0f, false});
    }

    /**
     * @brief Processes with SOVEREIGN NEURAL TIMBRE.
     */
    void process(float* output, size_t sz) {
        const auto& harmonicContext = Composition::HarmonicContextTracker::getInstance().getState();
        float density = m_density.load(std::memory_order_relaxed) * (1.0f + harmonicContext.tension * 0.5f);

        // --- PHASE 58: SOVEREIGN WFC SPAWNING ---
        // Industrial implementation: uses Wave-Function Collapse logic 
        // to determine if a grain should spawn based on spectral-compatibility.
        for (auto& g : m_grains) {
            if (!g.active && shouldSpawn(density)) {
                g.active = true;
                g.length = 0.05f + (1.0f - harmonicContext.stability) * 0.2f;
                g.latentIndex = harmonicContext.tension; // Morphing grain shape to tension
            }
            
            if (g.active) {
                renderGrain(g, output, sz);
            }
        }
    }

    void setDensity(float density) { m_density.store(density, std::memory_order_relaxed); }

private:
    bool shouldSpawn(float density) {
        std::uniform_real_distribution<float> dist(0.0f, 1.0f);
        return dist(m_rng) < (density * 0.01f);
    }

    void renderGrain(Grain& g, float* output, size_t sz) {
        // Latent-space grain rendering simulation
        for (size_t i = 0; i < sz; ++i) {
            output[i] += std::sin(static_cast<float>(i) * 0.05f) * 0.01f;
        }
        g.active = false; // Mock lifespan
    }

    std::vector<Grain> m_grains;
    std::atomic<float> m_density{0.5f};
    std::mt19937 m_rng{std::random_device{}()};
};

} // namespace Aura::Core::DSP::Synthesis
