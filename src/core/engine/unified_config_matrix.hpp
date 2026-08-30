#pragma once

#include <string>
#include <map>
#include <vector>
#include <variant>
#include <fstream>
#include "../../external/nlohmann/json.hpp"

namespace Aura::Core::Engine {

using json = nlohmann::json;

/**
 * @class UnifiedConfigMatrix
 * @brief Industrial-Grade Engine Configuration Orchestrator.
 */
class UnifiedConfigMatrix {
public:
    using ConfigValue = std::variant<int, float, bool, std::string>;

    static UnifiedConfigMatrix& getInstance() { static UnifiedConfigMatrix i; return i; }

    UnifiedConfigMatrix() {
        // --- 1. SET HARDCODED SAFE DEFAULTS ---
        set("audio.buffer_size", 256);
        set("audio.sample_rate", 44100.0f);
        set("audio.rt_priority", 99);
        set("audio.pdc_enabled", true);
        set("spectral.fft_size", 2048);
        set("gui.refresh_rate", 120.0f);

        // --- 2. DYNAMIC OVERRIDE (INDUSTRIAL SOVEREIGNTY) ---
        loadFromFile("aura_config.json");
    }

    void set(const std::string& key, ConfigValue val) {
        if (!key.empty()) m_configs[key] = std::move(val);
    }

    ConfigValue get(const std::string& key) const {
        const auto it = m_configs.find(key);
        return it == m_configs.end() ? ConfigValue{0} : it->second;
    }

    void loadFromFile(const std::string& path) {
        std::ifstream input(path);
        if (!input) return;
        json values;
        try { input >> values; } catch (...) { return; }
        if (!values.is_object()) return;
        for (const auto& [key, value] : values.items()) {
            if (value.is_boolean()) set(key, value.get<bool>());
            else if (value.is_number_integer()) set(key, value.get<int>());
            else if (value.is_number_float()) set(key, value.get<float>());
            else if (value.is_string()) set(key, value.get<std::string>());
        }
    }


private:
    std::map<std::string, ConfigValue> m_configs;
};

} // namespace Aura::Core::Engine
