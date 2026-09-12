
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

