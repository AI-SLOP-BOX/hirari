#pragma once
#include <vector>
#include <array>
#include <memory>
#include <algorithm>
#include <atomic>
#include <cstdint>
#include <mutex>
#include <string>
#include <string_view>
#include <cmath>
#include <fstream>
#include <filesystem>
#include <chrono>
#include "../../graphics/graphics_kernel.hpp"
#include "../../core/aura_unified_engine.hpp"
#include "../../core/engine/track.hpp"
#include "view_transformer.hpp"
#include "../../external/nlohmann/json.hpp"

namespace Aura::UI::Main {

enum class LayoutMode {
    Single,
    SplitHorizontal,
    SplitVertical,
    Floating
};

// Experience density is deliberately independent from window layout.  A
// producer can keep a split layout while switching between a guided surface
// and a full engineering surface, and extensions can add their own panel
// without inventing another global UI mode.
enum class ExperienceMode : uint8_t {
    Beginner,
    Pro,
    Custom
};

/**
 * @class WorkspaceManager
 * @brief Manages window Z-order and layout orchestration.
 * HONEST FIX: Purged 'UI DNA' and 'Infinite Synthesis' hallucinations.
 */
class WorkspaceManager {
public:
    enum Panel : uint32_t {
        PanelTimeline = 1u << 0,
        PanelDiagnostics = 1u << 1,
        PanelMixer = 1u << 2,
        PanelPlugin = 1u << 3,
        PanelBrowser = 1u << 4,
        PanelAutomation = 1u << 5
    };

    struct LayoutMetrics {
        static constexpr float controlBar = 54.0f;
        static constexpr float ruler = 32.0f;
        static constexpr float trackHeader = 240.0f;
        static constexpr float trackHeight = 80.0f;
        static constexpr float minContentWidth = 320.0f;

    };
    struct FrameSnapshot {
        float width = 0.0f;
        float height = 0.0f;
        LayoutMode layoutMode = LayoutMode::Single;
        ExperienceMode experienceMode = ExperienceMode::Pro;
        uint32_t visiblePanels = PanelTimeline | PanelDiagnostics | PanelMixer;
        uint64_t generation = 0;
    };

    struct PanelPlacement {
        float x = 0.0f;
        float y = 0.0f;
        float width = 0.0f;
        float height = 0.0f;
        bool floating = false;
    };

    struct ExtensionPanel {
        std::string id;
        std::string title;
        bool visible = true;
        PanelPlacement placement;
    };

    static WorkspaceManager& getInstance() {
        static WorkspaceManager instance;
        return instance;
    }

    WorkspaceManager(::Aura::Core::Engine::AuraUnifiedEngine& engine,
                     ViewTransformer& transformer)
        : m_engine(&engine), m_transformer(&transformer), m_layoutMode(LayoutMode::Single) {}

    void initialize(float w, float h) {
        publish(w, h, m_layoutMode);
    }

    /**
     * @brief Renders all UI components according to Z-order and Layout.
     */
    void render(::Aura::Graphics::Platform::IGraphicsKernel& kernel) {
        // Copy the immutable frame description once.  Rendering never reads
        // dimensions or layout state while another thread is resizing it.
        const FrameSnapshot frame = snapshot();
        const float width = frame.width;
        const float height = frame.height;
        const LayoutMode layoutMode = frame.layoutMode;
        (void)layoutMode;
        const float timelineX = LayoutMetrics::trackHeader;
        kernel.drawRect(0, 0, width, height, 0xFF0F1012);
        kernel.drawGradientRect(0, 0, width, LayoutMetrics::controlBar, 0xFF26282C, 0xFF1A1B1E);
        kernel.drawGradientRect(0, LayoutMetrics::controlBar, width, LayoutMetrics::ruler, 0xFF303238, 0xFF24262A);
        if (frame.experienceMode == ExperienceMode::Beginner) {
            renderBeginnerGuide(kernel);
        } else if (frame.experienceMode == ExperienceMode::Custom) {
            kernel.drawText("CUSTOM WORKSPACE", 18.0f, 20.0f, 10, 0xFF9CA3AF);
        }
        if ((frame.visiblePanels & PanelDiagnostics) != 0u) {
            renderDiagnostics(kernel, width, frame);
        }
        if ((frame.visiblePanels & PanelTimeline) == 0u) {
            kernel.drawText("Timeline hidden  |  enable Timeline in workspace panels", 18.0f,
                            LayoutMetrics::controlBar + LayoutMetrics::ruler + 28.0f, 12,
                            0xFF9CA3AF);
            renderExtensionPanels(kernel, width, height);
            return;
        }
        kernel.drawGradientRect(0, LayoutMetrics::controlBar + LayoutMetrics::ruler, width,
                                std::max(0.0f, height - LayoutMetrics::controlBar - LayoutMetrics::ruler),
                                0xFF17191C, 0xFF101113);
        kernel.drawRect(timelineX, LayoutMetrics::controlBar + LayoutMetrics::ruler, 1.0f,
                        std::max(0.0f, height - LayoutMetrics::controlBar - LayoutMetrics::ruler),
                        0xFF3A3D43);

        auto& engine = *m_engine;
        auto& vt = *m_transformer;
        const float beatWidth = static_cast<float>(vt.getPixelsPerSecond() * 0.5);
        if (beatWidth > 0.0f) {
            for (float x = timelineX; x < width; x += beatWidth) {
                kernel.drawLine(x, LayoutMetrics::controlBar, x, height, 1.0f, 0x182F3338);
            }
        }

        float trackY = LayoutMetrics::controlBar + LayoutMetrics::ruler;
        const auto tracks = engine.get_tracks_snapshot();
        for (const auto& trackPtr : tracks) {
            if (!trackPtr || trackY >= height) break;
            const auto& track = *trackPtr;
            const uint32_t row = (static_cast<uint32_t>(trackY / LayoutMetrics::trackHeight) & 1u)
                ? 0xFF1B1D21 : 0xFF202226;
            kernel.drawRect(0, trackY, width, LayoutMetrics::trackHeight, row);
            kernel.drawText(track.getName(), 16, trackY + 22, 11, 0xFFE0E2E5);
            kernel.drawText(track.isMuted() ? "M" : "m", 150, trackY + 22, 10,
                            track.isMuted() ? 0xFF30B0C7 : 0xFF8A8D93);
            kernel.drawText(track.isSolo() ? "S" : "s", 176, trackY + 22, 10,
                            track.isSolo() ? 0xFFFFC857 : 0xFF8A8D93);
            const auto* regions = track.getRegionSnapshot();
            if (regions == nullptr) {
                trackY += LayoutMetrics::trackHeight;
                continue;
            }
            for (const auto& region : *regions) {
                const float rx = timelineX + vt.sampleToX(region.start) - m_scrollX;
                const uint64_t safeEnd = region.len > UINT64_MAX - region.start
                    ? UINT64_MAX : region.start + region.len;
                const float rw = std::max(8.0f, vt.sampleToX(safeEnd) - vt.sampleToX(region.start));
                if (rx + rw < timelineX || rx > width) continue;
                kernel.drawGlassRect(rx, trackY + 5, rw, LayoutMetrics::trackHeight - 10, 4,
                                     region.id == m_selectedRegionId ? 0xFF3B82B5 : 0xFF2A5575);
                kernel.drawText(region.name.empty() ? "Audio Region" : region.name,
                                rx + 8, trackY + 22, 10, 0xFFE8EBEE);
            }
            trackY += LayoutMetrics::trackHeight;
        }
        // Extension panels are rendered last so a registered tool can float
        // above the timeline without being painted over by the arrangement
        // background.
        renderExtensionPanels(kernel, width, height);
        const float playheadX = timelineX + vt.sampleToX(engine.get_playhead()) - m_scrollX;
        if (playheadX >= timelineX && playheadX <= width) {
            kernel.drawLine(playheadX, LayoutMetrics::controlBar, playheadX, height, 1.5f, 0xFFC5E138);
        }
    }

    // Compact always-on health readout.  It gives a performer immediate
    // feedback about the conditions that otherwise only become visible after
    // an audible dropout: DSP load, engine latency, callback block size and
    // saturated sample ranges.  The values are atomic/native snapshots, so
    // the UI never reaches into the audio graph's mutable containers.
    void renderDiagnostics(::Aura::Graphics::Platform::IGraphicsKernel& kernel,
                           float width, const FrameSnapshot& frame) const {
        if (m_engine == nullptr || width < 420.0f) return;
        (void)frame;
        const float load = m_engine->get_dsp_load();
        const float latency = m_engine->get_latency_ms();
        const uint32_t block = m_engine->get_block_size();
        const bool overflow = m_engine->audio_range_overflowed();
        const std::string text = "DSP " + std::to_string(static_cast<int>(load * 100.0f))
            + "%  LAT " + std::to_string(static_cast<int>(latency * 100.0f) / 100.0f)
            + "ms  B " + std::to_string(block) + (overflow ? "  RANGE!" : "");
        const PanelPlacement placement = panelPlacement(PanelDiagnostics);
        const float x = placement.floating ? placement.x : std::max(260.0f, width - 300.0f);
        const float y = placement.floating ? placement.y : 20.0f;
        kernel.drawText(text, x, y, 10,
                        overflow ? 0xFFFF6B6B : (load > 0.85f ? 0xFFFFC857 : 0xFF9CA3AF));
    }

    void renderBeginnerGuide(::Aura::Graphics::Platform::IGraphicsKernel& kernel) const {
        if (m_engine == nullptr) return;
        const auto tracks = m_engine->get_tracks_snapshot();
        bool hasAudio = false;
        for (const auto& track : tracks) {
            if (track && track->getRegionSnapshot() != nullptr &&
                !track->getRegions().empty()) {
                hasAudio = true;
                break;
            }
        }
        bool hasPlugin = false;
        for (const auto& track : tracks) {
            if (track && track->getPluginCount() > 0) {
                hasPlugin = true;
                break;
            }
        }
        const uint32_t completed = static_cast<uint32_t>(!tracks.empty()) +
            static_cast<uint32_t>(hasAudio) +
            static_cast<uint32_t>(m_engine->is_playing()) +
            static_cast<uint32_t>(hasPlugin);
        const char* title = nullptr;
        const char* instruction = nullptr;
        const char* detail = nullptr;
        if (tracks.empty()) {
            title = "STEP 1 OF 4  |  CREATE YOUR FIRST TRACK";
            instruction = "Add an audio or instrument track";
            detail = "Use the + button, or choose a template to start quickly.";
        } else if (!hasAudio) {
            title = "STEP 2 OF 4  |  PUT SOUND ON THE TIMELINE";
            instruction = "Drag a WAV onto the timeline or press Record";
            detail = "You can undo anything. Your original audio stays untouched.";
        } else if (!m_engine->is_playing()) {
            title = "STEP 3 OF 4  |  LISTEN TO YOUR IDEA";
            instruction = "Press Space to play and stop";
            detail = "Click the ruler to choose where playback begins.";
        } else if (!hasPlugin) {
            title = "STEP 4 OF 4  |  SHAPE THE SOUND";
            instruction = "Open the plugin browser and add one effect";
            detail = "Start with a preset; advanced controls stay hidden in Beginner mode.";
        } else {
            title = "YOU ARE READY  |  KEEP BUILDING";
            instruction = "Add another track, edit a region, or save your project";
            detail = "Switch to Pro mode any time to reveal the full workspace.";
        }
        kernel.drawGlassRect(14.0f, 10.0f, 420.0f, 58.0f, 6, 0xD92A3430);
        kernel.drawText(title, 26.0f, 27.0f, 10, 0xFFB9E6A5);
        kernel.drawText(instruction, 26.0f, 44.0f, 12, 0xFFF3F4F6);
        kernel.drawText(detail, 26.0f, 60.0f, 9, 0xFFB5BCC5);
        const float progress = static_cast<float>(completed) / 4.0f;
        kernel.drawRect(26.0f, 67.0f, 392.0f, 3.0f, 0xFF333A40);
        kernel.drawRect(26.0f, 67.0f, 392.0f * std::clamp(progress, 0.0f, 1.0f),
                        3.0f, 0xFFB9E6A5);
    }

    bool handleMouseDown(float x, float y) {
        if (y < LayoutMetrics::controlBar) return false;
        // Floating extension panels own their hit rectangle before the
        // arrangement does. This makes custom tools draggable without
        // accidentally moving a region underneath them.
        for (const auto& panel : extensionPanels()) {
            if (!panel.visible || !panel.placement.floating) continue;
            const auto placement = sanitizePlacement(panel.placement);
            if (x >= placement.x && x <= placement.x + placement.width &&
                y >= placement.y && y <= placement.y + placement.height) {
                m_draggedExtensionPanel = panel.id;
                m_extensionDragOffsetX = x - placement.x;
                m_extensionDragOffsetY = y - placement.y;
                m_lastMouseY = y;
                return true;
            }
        }
        if (y < LayoutMetrics::controlBar + LayoutMetrics::ruler) {
            auto& vt = *m_transformer;
            m_engine->set_playhead(
                vt.xToSample(x - LayoutMetrics::trackHeader + m_scrollX));
            return true;
        }
        const size_t index = static_cast<size_t>((y - LayoutMetrics::controlBar - LayoutMetrics::ruler)
                                                  / LayoutMetrics::trackHeight);
        auto& engine = *m_engine;
        const auto tracks = engine.get_tracks_snapshot();
        if (index >= tracks.size() || !tracks[index]) return false;
        auto& track = *tracks[index];
        if (x < LayoutMetrics::trackHeader) {
            if (x >= 140.0f && x < 166.0f) track.setMuted(!track.isMuted());
            else if (x >= 168.0f && x < 194.0f) track.setSolo(!track.isSolo());
            return true;
        }
        const auto& vt = *m_transformer;
        const uint64_t sample = vt.xToSample(x - LayoutMetrics::trackHeader + m_scrollX);
        const auto* regions = track.getRegionSnapshot();
        if (regions == nullptr) return false;
        for (const auto& region : *regions) {
            const uint64_t regionEnd = region.len > UINT64_MAX - region.start
                ? UINT64_MAX : region.start + region.len;
            if (sample >= region.start && sample < regionEnd) {
                m_draggedTrackId = track.getId();
                m_draggedRegionId = region.id;
                m_selectedRegionId = region.id;
                m_currentDragSamples = region.start;
                m_lastMouseX = x;
                return true;
            }
        }
        return false;
    }
    void handleMouseDrag(float x, float y) {
        if (!m_draggedExtensionPanel.empty()) {
            auto placement = panelPlacementForExtension(m_draggedExtensionPanel);
            placement.x = x - m_extensionDragOffsetX;
            placement.y = y - m_extensionDragOffsetY;
            setExtensionPanelPlacement(m_draggedExtensionPanel, placement);
            m_lastMouseY = y;
            return;
        }
        if (m_draggedTrackId == kInvalidId || m_draggedRegionId == kInvalidId) return;
        const auto& vt = *m_transformer;
        const int64_t now = static_cast<int64_t>(vt.xToSample(
            x - LayoutMetrics::trackHeader + m_scrollX));
        const int64_t previous = static_cast<int64_t>(vt.xToSample(
            m_lastMouseX - LayoutMetrics::trackHeader + m_scrollX));
        m_currentDragSamples = static_cast<uint64_t>(std::max<int64_t>(
            0, static_cast<int64_t>(m_currentDragSamples) + (now - previous)));
        auto& engine = *m_engine;
        engine.move_region(m_draggedTrackId, m_draggedRegionId,
                           engine.samples_to_beats(m_currentDragSamples));
        m_lastMouseX = x;
    }
    void handleMouseUp(float, float) {
        m_draggedTrackId = kInvalidId;
        m_draggedRegionId = kInvalidId;
        m_draggedExtensionPanel.clear();
    }
    bool handleKeyDown(int keyCode) {
        if (keyCode == 49) {
            m_engine->set_playing(!m_engine->is_playing());
            return true;
        }
        return false;
    }

    void setLayoutMode(LayoutMode mode) {
        const FrameSnapshot current = snapshot();
        // Layout changes must not silently reset the user's experience mode
        // or custom panel visibility.  These are orthogonal workspace
        // dimensions and are commonly changed independently by extensions.
        publish(current.width, current.height, mode, current.experienceMode,
                current.visiblePanels);
    }

    ExperienceMode experienceMode() const noexcept { return snapshot().experienceMode; }

    void setExperienceMode(ExperienceMode mode) {
        const FrameSnapshot current = snapshot();
        uint32_t panels = current.visiblePanels;
        if (mode == ExperienceMode::Beginner) {
            // Beginner mode keeps the timeline and mixer essentials visible,
            // while removing forensic/engineering surfaces that obscure the
            // next action.  This is a deterministic policy, not a collection
            // of ad-hoc widget visibility checks.
            panels = PanelTimeline | PanelMixer;
        } else if (mode == ExperienceMode::Pro) {
            panels = PanelTimeline | PanelDiagnostics | PanelMixer |
                     PanelPlugin | PanelBrowser | PanelAutomation;
        }
        publish(current.width, current.height, current.layoutMode, mode, panels);
    }

    void setVisiblePanels(uint32_t panels) {
        const FrameSnapshot current = snapshot();
        publish(current.width, current.height, current.layoutMode,
                ExperienceMode::Custom, panels & kKnownPanels);
    }

    uint32_t visiblePanels() const noexcept { return snapshot().visiblePanels; }

    bool isPanelVisible(Panel panel) const noexcept {
        return (visiblePanels() & static_cast<uint32_t>(panel)) != 0u;
    }

    void setPanelPlacement(Panel panel, float x, float y, float width, float height,
                           bool floating = true) {
        const uint32_t bit = static_cast<uint32_t>(panel);
        if (bit == 0u || (bit & kKnownPanels) == 0u) return;
        const auto clampCoordinate = [](float value) {
            return std::isfinite(value) ? std::clamp(value, -8192.0f, 8192.0f) : 0.0f;
        };
        const auto clampSize = [](float value) {
            return std::isfinite(value) ? std::clamp(value, 16.0f, 8192.0f) : 16.0f;
        };
        std::lock_guard<std::mutex> lock(m_panelPlacementMutex);
        auto& placement = m_panelPlacements[panelIndex(panel)];
        placement = {clampCoordinate(x), clampCoordinate(y), clampSize(width),
                     clampSize(height), floating};
    }

    PanelPlacement panelPlacement(Panel panel) const noexcept {
        const uint32_t bit = static_cast<uint32_t>(panel);
        if (bit == 0u || (bit & kKnownPanels) == 0u) return {};
        std::lock_guard<std::mutex> lock(m_panelPlacementMutex);
        return m_panelPlacements[panelIndex(panel)];
    }

    void resetPanelPlacements() noexcept {
        std::lock_guard<std::mutex> lock(m_panelPlacementMutex);
        for (auto& placement : m_panelPlacements) placement = {};
    }

    bool registerExtensionPanel(std::string_view id, std::string_view title) {
        if (id.empty() || title.empty() || id.size() > 128 || title.size() > 256 ||
            id.find('\0') != std::string_view::npos || title.find('\0') != std::string_view::npos) {
            return false;
        }
        std::lock_guard<std::mutex> lock(m_extensionMutex);
        if (m_extensionPanels.size() >= 64 || std::any_of(
                m_extensionPanels.begin(), m_extensionPanels.end(),
                [id](const ExtensionPanel& panel) { return panel.id == id; })) {
            return false;
        }
        m_extensionPanels.push_back({std::string(id), std::string(title), true,
                                     {48.0f, 96.0f, 320.0f, 220.0f, true}});
        return true;
    }

    bool unregisterExtensionPanel(std::string_view id) {
        std::lock_guard<std::mutex> lock(m_extensionMutex);
        const auto it = std::find_if(m_extensionPanels.begin(), m_extensionPanels.end(),
                                     [id](const ExtensionPanel& panel) { return panel.id == id; });
        if (it == m_extensionPanels.end()) return false;
        m_extensionPanels.erase(it);
        return true;
    }

    bool setExtensionPanelVisible(std::string_view id, bool visible) {
        std::lock_guard<std::mutex> lock(m_extensionMutex);
        for (auto& panel : m_extensionPanels) {
            if (panel.id == id) {
                panel.visible = visible;
                return true;
            }
        }
        return false;
    }

    bool setExtensionPanelPlacement(std::string_view id, PanelPlacement placement) {
        std::lock_guard<std::mutex> lock(m_extensionMutex);
        for (auto& panel : m_extensionPanels) {
            if (panel.id == id) {
                panel.placement = sanitizePlacement(placement);
                return true;
            }
        }
        return false;
    }

    std::vector<ExtensionPanel> extensionPanels() const {
        std::lock_guard<std::mutex> lock(m_extensionMutex);
        return m_extensionPanels;
    }

    // Workspace state is data only. Restoring it never loads or executes an
    // extension; a panel must already be registered by the trusted manifest
    // boundary before its visibility or placement can be changed.
    std::string serializeWorkspaceState() const {
        const FrameSnapshot frame = snapshot();
        nlohmann::json state = {
            {"schema", 1},
            {"layout_mode", static_cast<uint32_t>(frame.layoutMode)},
            {"experience_mode", static_cast<uint32_t>(frame.experienceMode)},
            {"visible_panels", frame.visiblePanels},
            {"panels", nlohmann::json::array()},
            {"extensions", nlohmann::json::array()}
        };
        for (const auto panel : {PanelTimeline, PanelDiagnostics, PanelMixer,
                                 PanelPlugin, PanelBrowser, PanelAutomation}) {
            const auto placement = panelPlacement(panel);
            state["panels"].push_back({
                {"id", static_cast<uint32_t>(panel)}, {"x", placement.x},
                {"y", placement.y}, {"width", placement.width},
                {"height", placement.height}, {"floating", placement.floating}
            });
        }
        for (const auto& panel : extensionPanels()) {
            state["extensions"].push_back({
                {"id", panel.id}, {"visible", panel.visible},
                {"x", panel.placement.x}, {"y", panel.placement.y},
                {"width", panel.placement.width}, {"height", panel.placement.height},
                {"floating", panel.placement.floating}
            });
        }
        return state.dump();
    }

    bool restoreWorkspaceState(std::string_view encoded) {
        if (encoded.empty() || encoded.size() > 1024u * 1024u) return false;
        try {
            const auto state = nlohmann::json::parse(encoded.begin(), encoded.end());
            if (!state.is_object() || state.value("schema", 0u) != 1u) return false;
            const auto layoutValue = state.value("layout_mode", 0u);
            const auto experienceValue = state.value("experience_mode", 1u);
            if (layoutValue > static_cast<uint32_t>(LayoutMode::Floating) ||
                experienceValue > static_cast<uint32_t>(ExperienceMode::Custom)) return false;
            const auto current = snapshot();
            publish(current.width, current.height, static_cast<LayoutMode>(layoutValue),
                    static_cast<ExperienceMode>(experienceValue),
                    state.value("visible_panels", current.visiblePanels) & kKnownPanels);
            const auto panelsIt = state.find("panels");
            if (panelsIt != state.end() && panelsIt->is_array()) {
                for (const auto& entry : *panelsIt) {
                    if (!entry.is_object()) continue;
                    const auto id = entry.value("id", 0u);
                    if (id == 0u || (id & kKnownPanels) == 0u || (id & (id - 1u)) != 0u) continue;
                    setPanelPlacement(static_cast<Panel>(id), entry.value("x", 0.0f),
                                      entry.value("y", 0.0f), entry.value("width", 320.0f),
                                      entry.value("height", 220.0f), entry.value("floating", false));
                }
            }
            const auto extensionsIt = state.find("extensions");
            if (extensionsIt != state.end() && extensionsIt->is_array()) {
                std::lock_guard<std::mutex> lock(m_extensionMutex);
                for (const auto& entry : *extensionsIt) {
                    if (!entry.is_object()) continue;
                    const auto id = entry.value("id", std::string{});
                    for (auto& panel : m_extensionPanels) {
                        if (panel.id != id) continue;
                        panel.visible = entry.value("visible", panel.visible);
                        panel.placement = sanitizePlacement({
                            entry.value("x", panel.placement.x), entry.value("y", panel.placement.y),
                            entry.value("width", panel.placement.width),
                            entry.value("height", panel.placement.height),
                            entry.value("floating", panel.placement.floating)});
                    }
                }
            }
            return true;
        } catch (...) {
            return false;
        }
    }

    bool saveWorkspaceStateToFile(const std::filesystem::path& path) const {
        if (path.empty() || path.has_parent_path() && path.parent_path().empty()) return false;
        const auto parent = path.parent_path().empty()
            ? std::filesystem::path{"."} : path.parent_path();
        std::error_code ec;
        if (!std::filesystem::is_directory(parent, ec) || ec) return false;
        static std::atomic<uint64_t> saveSequence{0};
        const auto nonce = saveSequence.fetch_add(1, std::memory_order_relaxed) + 1;
        const auto temporary = path.string() + ".tmp-workspace-" + std::to_string(nonce);
        const std::string encoded = serializeWorkspaceState();
        {
            std::ofstream output(temporary, std::ios::binary | std::ios::trunc);
            if (!output.is_open()) return false;
            output.write(encoded.data(), static_cast<std::streamsize>(encoded.size()));
            output.flush();
            if (!output.good()) {
                output.close();
                std::filesystem::remove(temporary, ec);
                return false;
            }
        }
        std::filesystem::rename(temporary, path, ec);
        if (ec) {
            std::filesystem::remove(temporary, ec);
            return false;
        }
        return true;
    }

    bool restoreWorkspaceStateFromFile(const std::filesystem::path& path) {
        if (path.empty()) return false;
        std::error_code ec;
        if (!std::filesystem::is_regular_file(path, ec) || ec) return false;
        const auto size = std::filesystem::file_size(path, ec);
        if (ec || size == 0 || size > 1024u * 1024u) return false;
        std::ifstream input(path, std::ios::binary);
        if (!input.is_open()) return false;
        std::string encoded(static_cast<size_t>(size), '\0');
        input.read(encoded.data(), static_cast<std::streamsize>(encoded.size()));
        if (!input.good() && !input.eof()) return false;
        return restoreWorkspaceState(encoded);
    }

    PanelPlacement panelPlacementForExtension(std::string_view id) const noexcept {
        std::lock_guard<std::mutex> lock(m_extensionMutex);
        for (const auto& panel : m_extensionPanels) {
            if (panel.id == id) return panel.placement;
        }
        return {};
    }

    FrameSnapshot snapshot() const noexcept {
        std::lock_guard<std::mutex> lock(m_snapshotMutex);
        const uint32_t index = m_activeSnapshot.load(std::memory_order_acquire);
        return m_snapshots[index];
    }

    /**
     * @brief Brings a window/view to the front.
     */
    void bringToFront(uint32_t viewId) {
        auto it = std::find(m_zOrder.begin(), m_zOrder.end(), viewId);
        if (it != m_zOrder.end()) {
            m_zOrder.erase(it);
        }
        m_zOrder.push_back(viewId);
    }

private:
    WorkspaceManager()
        : WorkspaceManager(::Aura::Core::Engine::AuraUnifiedEngine::getInstance(),
                           ViewTransformer::getInstance()) {}

    void publish(float width, float height, LayoutMode mode,
                 ExperienceMode experience = ExperienceMode::Pro,
                 uint32_t panels = PanelTimeline | PanelDiagnostics | PanelMixer) noexcept {
        std::lock_guard<std::mutex> lock(m_snapshotMutex);
        const uint32_t current = m_activeSnapshot.load(std::memory_order_relaxed);
        const uint32_t next = current ^ 1u;
        m_snapshots[next] = {std::max(0.0f, width), std::max(0.0f, height), mode,
                             experience, panels & kKnownPanels,
                             m_snapshots[current].generation + 1};
        m_activeSnapshot.store(next, std::memory_order_release);
        m_width = m_snapshots[next].width;
        m_height = m_snapshots[next].height;
        m_layoutMode = mode;
    }

    static PanelPlacement sanitizePlacement(PanelPlacement placement) noexcept {
        const auto coordinate = [](float value) {
            return std::isfinite(value) ? std::clamp(value, -8192.0f, 8192.0f) : 0.0f;
        };
        const auto size = [](float value) {
            return std::isfinite(value) ? std::clamp(value, 16.0f, 8192.0f) : 16.0f;
        };
        placement.x = coordinate(placement.x);
        placement.y = coordinate(placement.y);
        placement.width = size(placement.width);
        placement.height = size(placement.height);
        return placement;
    }

    void renderExtensionPanels(::Aura::Graphics::Platform::IGraphicsKernel& kernel,
                               float width, float height) const {
        std::vector<ExtensionPanel> panels;
        {
            std::lock_guard<std::mutex> lock(m_extensionMutex);
            panels = m_extensionPanels;
        }
        for (const auto& panel : panels) {
            if (!panel.visible) continue;
            const auto placement = sanitizePlacement(panel.placement);
            const float x = placement.floating ? placement.x : std::max(12.0f, width - 260.0f);
            const float y = placement.floating ? placement.y : LayoutMetrics::controlBar + 8.0f;
            const float panelWidth = std::min(placement.width, std::max(16.0f, width - x));
            const float panelHeight = std::min(placement.height, std::max(16.0f, height - y));
            kernel.drawGlassRect(x, y, panelWidth, panelHeight, 5, 0xD92B3038);
            kernel.drawText(panel.title, x + 8.0f, y + 18.0f, 10, 0xFFE0E2E5);
        }
    }

    float m_width = 1920, m_height = 1080;
    ::Aura::Core::Engine::AuraUnifiedEngine* m_engine;
    ViewTransformer* m_transformer;
    LayoutMode m_layoutMode;
    std::vector<uint32_t> m_zOrder;
    static constexpr uint32_t kInvalidId = UINT32_MAX;
    uint32_t m_draggedTrackId = kInvalidId;
    uint32_t m_draggedRegionId = kInvalidId;
    uint32_t m_selectedRegionId = 0;
    uint64_t m_currentDragSamples = 0;
    float m_lastMouseX = 0.0f;
    float m_lastMouseY = 0.0f;
    float m_scrollX = 0.0f;
    std::string m_draggedExtensionPanel;
    float m_extensionDragOffsetX = 0.0f;
    float m_extensionDragOffsetY = 0.0f;
    static constexpr uint32_t kKnownPanels = PanelTimeline | PanelDiagnostics | PanelMixer |
                                              PanelPlugin | PanelBrowser | PanelAutomation;
    static constexpr size_t panelIndex(Panel panel) noexcept {
        switch (panel) {
            case PanelTimeline: return 0;
            case PanelDiagnostics: return 1;
            case PanelMixer: return 2;
            case PanelPlugin: return 3;
            case PanelBrowser: return 4;
            case PanelAutomation: return 5;
        }
        return 0;
    }
    std::array<PanelPlacement, 6> m_panelPlacements{};
    mutable std::mutex m_panelPlacementMutex;
    std::vector<ExtensionPanel> m_extensionPanels;
    mutable std::mutex m_extensionMutex;
    FrameSnapshot m_snapshots[2]{{1920.0f, 1080.0f, LayoutMode::Single,
                                  ExperienceMode::Pro,
                                  PanelTimeline | PanelDiagnostics | PanelMixer, 0},
                                 {1920.0f, 1080.0f, LayoutMode::Single,
                                  ExperienceMode::Pro,
                                  PanelTimeline | PanelDiagnostics | PanelMixer, 0}};
    std::atomic<uint32_t> m_activeSnapshot{0};
    mutable std::mutex m_snapshotMutex;
};

// Compatibility Typedef
using AuraWorkspace = WorkspaceManager;

} // namespace Aura::UI::Main
