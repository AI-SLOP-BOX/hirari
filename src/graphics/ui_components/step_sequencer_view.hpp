#pragma once
#include <vector>
#include <string>
#include <cmath>
#include "../graphics_kernel.hpp"
#include "../../core/engine/step_sequencer.hpp"
#include "../../AuraUltimate.hpp"

namespace Aura::Graphics::UI {

/**
 * @class StepSequencerView
 * @brief High-Fidelity Step Sequencer Grid for Beat Making.
 */
class StepSequencerView {
public:
    void render(::Aura::Graphics::Platform::IGraphicsKernel& kernel, float x, float y, float w, float h) {
        
        // --- 1. EBONY GRID ENCLOSURE ---
        kernel.drawGradientRect(x, y, w, h, 0xFF141416, 0xFF0D0D0F);
        
        float trackH = 40.0f, padW = 28.0f, padH = 24.0f, padGap = 4.0f;
        int numTracks = 8, numSteps = 16;
        
        // Header
        kernel.drawText("STEP SEQUENCER: DRUMS", x + 16, y + 24, 11, 0xFFF1F5F9);
        
        // --- 2. THE GRID ---
        float gridY = y + 48.0f;
        double bpm = ::Aura::AuraEngine::getInstance().getBPM();
        double sr = ::Aura::AuraEngine::getInstance().getSampleRate();
        double stepSamples = (60.0 / (bpm > 0.0 ? bpm : 120.0) / 4.0) * (sr > 0.0 ? sr : 44100.0);
        uint32_t currentStep = 0;
        if (stepSamples > 0.0) {
            currentStep = static_cast<uint32_t>(::Aura::AuraEngine::getInstance().getCurrentSamplePos() / stepSamples) % 16;
        }

        for (int t = 0; t < numTracks; ++t) {
            float ty = gridY + t * trackH;
            kernel.drawText("DRUM " + std::to_string(t+1), x + 10, ty + 18, 9, 0xFF94A3B8);
            
            for (int s = 0; s < numSteps; ++s) {
                float px = x + 80 + s * (padW + padGap);
                float py = ty + 4;
                
                // Read the actual active step state from core engine!
                bool active = false;
                auto* seq = ::Aura::AuraEngine::getInstance().getStepSequencer();
                if (seq) {
                    active = seq->getStep(t, s);
                }
                
                bool isCurrent = (s == currentStep);
                
                uint32_t padCol = active ? (t < 4 ? 0xFF30B0FF : 0xFFFBBF24) : 0xFF1C1C1E;
                kernel.drawRoundedRect(px, py, padW, padH, 3.0f, padCol);
                
                if (isCurrent) {
                    kernel.drawNeonRect(px - 1, py - 1, padW + 2, padH + 2, 3.0f, 4.0f, 0xFFFFFFFF);
                }
            }
        }
    }
    
    /**
     * @brief Handle user clicks on pads to toggle step values dynamically.
     */
    bool handleMouseDown(float mouseX, float mouseY, float startX, float startY) {
        float gridY = startY + 48.0f;
        float trackH = 40.0f, padW = 28.0f, padH = 24.0f, padGap = 4.0f;
        int numTracks = 8, numSteps = 16;
        
        for (int t = 0; t < numTracks; ++t) {
            float ty = gridY + t * trackH;
            for (int s = 0; s < numSteps; ++s) {
                float px = startX + 80 + s * (padW + padGap);
                float py = ty + 4;
                
                if (mouseX >= px && mouseX <= px + padW && mouseY >= py && mouseY <= py + padH) {
                    auto* seq = ::Aura::AuraEngine::getInstance().getStepSequencer();
                    if (seq) {
                        bool current = seq->getStep(t, s);
                        seq->setStep(t, s, !current);
                        return true; 
                    }
                }
            }
        }
        return false;
    }
};

} // namespace Aura::Graphics::UI
