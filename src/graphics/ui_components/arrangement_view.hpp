#pragma once
#include <vector>
#include <string>
#include <memory>
#include <cmath>
#include <algorithm>
#include "../graphics_kernel.hpp"
#include "track_header_renderer.hpp"
#include "../../core/engine/track.hpp"
#include "../../ui/main/view_transformer.hpp"
#include "../../rendering/waveform_overview.hpp"

namespace Aura::Graphics::UI {

/**
 * @class ArrangementView
 * @brief INTERACTIVE Logic Pro 11 Workspace.
 * FIXED: Real Waveforms, Selection Highlighting, and Region Dragging.
 */
class ArrangementView {
public:
    void render(::Aura::Graphics::Platform::IGraphicsKernel& kernel, float x, float y, float w, float h, 
                const std::vector<std::shared_ptr<Core::Engine::Track>>& tracks, float scrollX) {
        
        m_x = x; m_y = y; m_w = w; m_h = h;
        float headerW = 240.0f, timelineX = x + headerW, timelineW = w - headerW, trackH = 80.0f, rulerH = 32.0f;
        auto& vt = ::Aura::UI::Main::ViewTransformer::getInstance();

        // --- 1. RULER ---
        kernel.drawGradientRect(x, y, w, rulerH, 0xFF1C1C1E, 0xFF0D0D0F);
        kernel.drawLine(x, y + rulerH - 1.0f, x + w, y + rulerH - 1.0f, 1.2f, 0xFF000000);
        
        // --- 2. TIMELINE GRID ---
        kernel.drawGradientRect(timelineX, y + rulerH, timelineW, h - rulerH, 0xFF08080A, 0xFF050505);
        float beatW = vt.beatsToPixels(1.0);
        for (float gx = timelineX - std::fmod(scrollX, beatW); gx < timelineX + timelineW; gx += beatW) {
            uint32_t col = (std::fmod(gx - timelineX + scrollX, beatW*4) < 1.0f) ? 0x44FFFFFF : 0x11FFFFFF;
            kernel.drawLine(gx, y, gx, y + h, 1.2f, col);
        }

        // --- 3. TRACKS & HEADERS ---
        float currentTrackY = y + rulerH;
        for (const auto& trackPtr : tracks) {
            if (currentTrackY > y + h) break;
            auto& track = *trackPtr;
            
            m_headerRenderer.render(kernel, x, currentTrackY, headerW, trackH, {
                track.getId(), track.getName(), track.getType(), track.isMuted(), track.isSoloed(), track.isArmed(),
                track.getVolume(), 0, track.getPeakL(), track.getPeakR(), track.getColor(), (m_selectedTrackId == track.getId())
            });
            
            // --- REGIONS (Real Waveforms) ---
            for (const auto& region : track.getAudioRegions()) {
                auto& meta = region->getMeta();
                float rx = timelineX + vt.samplesToPixels(meta.samplePosition) - scrollX;
                float rw = vt.samplesToPixels(meta.sampleLength);
                if (rx + rw < timelineX || rx > timelineX + timelineW) continue;
                
                kernel.drawGlassRect(rx, currentTrackY + 4, rw, trackH - 24, 5, track.getColor());
                if (m_selectedRegionId == meta.id) kernel.drawNeonRect(rx - 1, currentTrackY + 3, rw + 2, trackH - 22, 5, 4, 0xFFFFFFFF);
                
                auto wave = region->getWaveOverview();
                if (wave) {
                    ::Aura::Rendering::WaveformOverview::LOD lod;
                    const uint32_t targetPixels = static_cast<uint32_t>(
                        std::max(1.0f, std::min(rw, 4096.0f)));
                    if (wave->copyBestLOD(targetPixels, lod) &&
                        !lod.minData.empty() &&
                        lod.minData.size() == lod.maxData.size()) {
                        kernel.drawWaveformPath(lod.minData.data(), lod.maxData.data(),
                                                lod.minData.size(), rx,
                                                currentTrackY + trackH / 2.0f,
                                                rw, trackH * 0.4f, 0xAABBBBCB);
                    }
                }
                kernel.drawText(meta.name, rx + 10, currentTrackY + 20, 10, 0xFFFFFFFF);
            }
            kernel.drawLine(timelineX, currentTrackY + trackH, x + w, currentTrackY + trackH, 1.0f, 0xFF1C1C1E);
            currentTrackY += trackH;
        }

        // --- 4. PLAYHEAD ---
        float phX = timelineX + vt.samplesToPixels(::Aura::AuraEngine::getInstance().getCurrentSamplePos()) - scrollX;
        if (phX >= timelineX && phX <= timelineX + timelineW) {
            kernel.drawLine(phX, y, phX, y + h, 1.5f, 0xFFFBBF24);
        }
    }

    bool handleMouseDown(float x, float y, const std::vector<std::shared_ptr<Core::Engine::Track>>& tracks, float scrollX) {
        float headerW = 240.0f, timelineX = m_x + headerW, trackH = 80.0f, rulerH = 32.0f;
        auto& vt = ::Aura::UI::Main::ViewTransformer::getInstance();
        if (y < m_y + rulerH) {
             ::Aura::AuraEngine::getInstance().seekToPos(vt.pixelsToSamples(x - timelineX + scrollX)); return true;
        }

        float currentY = m_y + rulerH;
        for (auto& trackPtr : tracks) {
            if (y >= currentY && y < currentY + trackH) {
                m_selectedTrackId = trackPtr->getId();
                if (x < timelineX) {
                     float hitX = x - m_x;
                     if (hitX >= 120 && hitX < 142) { trackPtr->setMute(!trackPtr->isMuted()); return true; }
                     if (hitX >= 148 && hitX < 170) { trackPtr->setSolo(!trackPtr->isSoloed()); return true; }
                } else {
                    for (auto& region : trackPtr->getAudioRegions()) {
                        float rx = timelineX + vt.samplesToPixels(region->getMeta().samplePosition) - scrollX;
                        float rw = vt.samplesToPixels(region->getMeta().sampleLength);
                        if (x >= rx && x < rx + rw) {
                            m_selectedRegionId = region->getMeta().id;
                            m_draggedRegion = region.get();
                            m_dragStartSamples = region->getMeta().samplePosition;
                            m_currentDragSamples = m_dragStartSamples;
                            m_clickX = x; return true;
                        }
                    }
                    m_selectedRegionId = 0xFFFFFFFF;
                }
            }
            currentY += trackH;
        }
        return false;
    }

    void handleMouseDrag(float x, float y, float dx, float dy, const std::vector<std::shared_ptr<Core::Engine::Track>>& tracks, float scrollX) {
        (void)x; (void)y; (void)dy; (void)tracks; (void)scrollX;
        if (!m_draggedRegion) return;
        auto& vt = ::Aura::UI::Main::ViewTransformer::getInstance();
        // dx is the event delta since the previous callback. Applying the
        // absolute x-click distance on every event compounds the movement.
        int64_t diffSamples = vt.pixelsToSamples(dx);
        m_currentDragSamples = static_cast<uint64_t>(std::max<int64_t>(
            0, static_cast<int64_t>(m_currentDragSamples) + diffSamples));
        m_draggedRegion->setSamplePosition(m_currentDragSamples);
    }

    void handleMouseUp() { m_draggedRegion = nullptr; }

private:
    TrackHeaderRenderer m_headerRenderer;
    uint32_t m_selectedTrackId = 0xFFFFFFFF, m_selectedRegionId = 0xFFFFFFFF;
    ::Aura::Core::AudioRegion* m_draggedRegion = nullptr;
    uint64_t m_dragStartSamples = 0, m_currentDragSamples = 0; float m_clickX = 0;
    float m_x, m_y, m_w, m_h;
};

} // namespace Aura::Graphics::UI
