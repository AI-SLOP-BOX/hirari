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
