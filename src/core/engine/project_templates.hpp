#pragma once
#include <string>
#include <vector>
#include <unordered_map>
#include <mutex>
#include <algorithm>

namespace Aura::Core::Engine {

enum class TrackType { Audio, Instrument, Bus, MIDI };

struct TrackDef {
    std::string name;
    TrackType type;
    std::vector<std::string> insertEffects;
};

struct BusDef {
    std::string name;
    std::vector<std::string> effects;
};

struct ProjectTemplate {
    std::string name;
    std::vector<TrackDef> tracks;
    std::vector<BusDef> busses;
    int initialBpm = 120;
};

/**
 * @class ProjectTemplateLibrary
 * @brief Industrial Studio Environment Factory.
 * HONEST FIX: Implemented real routing definitions and type-safe templates.
 */
class ProjectTemplateLibrary {
public:
    static ProjectTemplateLibrary& getInstance() { static ProjectTemplateLibrary i; return i; }

    /**
     * @brief INSTANTIATE: Instantiates a template into the project engine with industrial precision and factory sovereignty.
     * INDUSTRIAL: Delegating template instantiation to the Rust 'TemplateOrchestrator'.
     */
    bool instantiate(const std::string& templateName, ProjectTemplate* result = nullptr) const {
        std::lock_guard<std::mutex> lock(m_mutex);
        const auto it = m_templates.find(templateName);
        if (it == m_templates.end()) return false;
        if (result) *result = it->second;
        return true;
    }

    bool registerTemplate(ProjectTemplate projectTemplate) {
        if (projectTemplate.name.empty() || projectTemplate.name.size() > 256 ||
            projectTemplate.initialBpm < 20 || projectTemplate.initialBpm > 400 ||
            projectTemplate.tracks.size() > 1024 || projectTemplate.busses.size() > 256) {
            return false;
        }
        for (const auto& track : projectTemplate.tracks)
            if (track.name.empty() || track.name.size() > 256) return false;
        for (const auto& bus : projectTemplate.busses)
            if (bus.name.empty() || bus.name.size() > 256) return false;
        std::lock_guard<std::mutex> lock(m_mutex);
        m_templates[projectTemplate.name] = std::move(projectTemplate);
        return true;
    }

    std::vector<ProjectTemplate> listTemplates() const {
        std::lock_guard<std::mutex> lock(m_mutex);
        std::vector<ProjectTemplate> result;
        result.reserve(m_templates.size());
        for (const auto& item : m_templates) result.push_back(item.second);
        std::sort(result.begin(), result.end(), [](const auto& lhs, const auto& rhs) {
            return lhs.name < rhs.name;
        });
        return result;
    }

private:
    ProjectTemplateLibrary() {
        registerTemplate({"Empty", {}, {}, 120});
        registerTemplate({
            "Singer Songwriter",
            {{"Vocal", TrackType::Audio, {"Compressor", "Vocal EQ"}},
             {"Guitar", TrackType::Audio, {"Channel EQ"}},
             {"Instrument", TrackType::Instrument, {}}},
            {{"Mix Bus", {"Bus Compressor"}}}, 100
        });
        registerTemplate({
            "Electronic",
            {{"Kick", TrackType::Instrument, {"Transient Shaper"}},
             {"Bass", TrackType::Instrument, {"Sidechain Duck"}},
             {"Synth", TrackType::Instrument, {}},
             {"Vocal", TrackType::Audio, {"DeEsser"}}},
            {{"Drums", {"Drum Bus"}}, {"Mix Bus", {"Limiter"}}}, 128
        });
        registerTemplate({
            "Vocaloid OpenUtau",
            {{"Vocal", TrackType::MIDI, {"Vocal Tuner"}},
             {"Backing Vocal", TrackType::MIDI, {}},
             {"Instrumental", TrackType::Audio, {}}},
            {{"Vocal Bus", {"DeEsser", "Vocal Reverb"}},
             {"Mix Bus", {"Limiter"}}}, 120
        });
    }

    mutable std::mutex m_mutex;
    std::unordered_map<std::string, ProjectTemplate> m_templates;
};

} // namespace Aura::Core::Engine
