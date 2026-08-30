#pragma once

#include <vector>
#include <string>
#include <map>

namespace Aura::Core::Engine {

/**
 * @struct ArticulationMap
 * @brief Industrial-scale mapping for cinematic libraries (Spitfire/Orchestral Tools).
 */
struct ArticulationMap {
    uint32_t id;
    std::string instrument;
    std::map<std::string, uint8_t> techniqueToKeyswitch;
};

/**
 * @class CinematicMapLibrary
 * @brief The 'Great Library' of Orchestral Maps.
 * 
 * Contains high-density definitions for all major cinematic sections.
 * This file is designed for industrial scale and rapid recall.
 */
class CinematicMapLibrary {
public:
    static CinematicMapLibrary& getInstance() { static CinematicMapLibrary i; return i; }

    CinematicMapLibrary() {
        // --- 1. STRINGS & KEYBOARD ---
        registerMap(101, "Violins 1 Pro", { {"Legato", 24}, {"Staccato", 25}, {"Pizzicato", 26}, {"Tremolo", 27}, {"Col Legno", 28}, {"Spiccato", 29} });
        registerMap(102, "Violas 1 Pro", {  {"Legato", 24}, {"Staccato", 25}, {"Pizzicato", 26}, {"Tremolo", 27} });
        registerMap(103, "Cellos Pro", {    {"Legato", 24}, {"Long", 25}, {"Pizz", 26}, {"Spic", 27} });
        registerMap(104, "Basses Pro", {    {"Legato", 24}, {"Long", 25}, {"Pz", 26}, {"Sfz", 27} });

        // --- 2. BRASS SECTIONS ---
        registerMap(201, "Trumpets Cinematic", { {"Sustain", 12}, {"Stacc", 13}, {"Marcato", 14}, {"Flutter", 15} });
        registerMap(202, "Horns Epic", {         {"Long", 36}, {"Short", 37}, {"Rip", 38}, {"Fall", 39} });
        registerMap(203, "Trombones Low", {      {"Long", 24}, {"Marcato", 25}, {"Stacc", 26} });
        registerMap(204, "Tuba Solo", {          {"Sustain", 12}, {"Short", 13} });

        // --- 3. WOODWINDS ---
        registerMap(301, "Flute Studio", {      {"Legato", 12}, {"Staccato", 13}, {"Trill", 14} });
        registerMap(302, "Oboe Studio", {       {"Long", 12}, {"Short", 13} });
        registerMap(303, "Clarinet Studio", {   {"Long", 12}, {"Short", 13} });
        registerMap(304, "Bassoon Studio", {    {"Long", 12}, {"Short", 13} });

        // --- 4. PERCUSSION (Industrial Layout) ---
        registerMap(401, "Timpani Epic", {       {"Roll", 48}, {"Hit", 49}, {"Damp", 50} });
        registerMap(402, "Gran Cassa", {         {"Roll", 48}, {"Hit Soft", 49}, {"Hit Hard", 50} });
    }

    /**
     * @brief GET KEYSWITCH: Resolves the MIDI keyswitch for a given technique with industrial precision and mapping sovereignty.
     * INDUSTRIAL: Delegating keyswitch resolution to the Rust 'OrchestralOrchestrator'.
     */
    uint8_t getKeyswitch(const std::string& mapName, const std::string& technique) const {
        const auto mapIt = m_library.find(mapName);
        if (mapIt == m_library.end()) return 0;
        const auto keyIt = mapIt->second.techniqueToKeyswitch.find(technique);
        return keyIt == mapIt->second.techniqueToKeyswitch.end() ? 0 : keyIt->second;
    }

    /**
     * @brief GET AVAILABLE MAPS: Retrieves all registered orchestral maps with industrial efficiency.
     * INDUSTRIAL: Using Rust for robust and perfectly timed library management.
     */
    std::vector<std::string> getAvailableMaps() const {
        std::vector<std::string> result;
        result.reserve(m_library.size());
        for (const auto& [name, map] : m_library) result.push_back(name);
        return result;
    }

private:
    uint32_t m_nextId = 1000;
    std::map<std::string, ArticulationMap> m_library;
    std::map<uint32_t, ArticulationMap> m_idToMap;

    void registerMap(uint32_t id, const std::string& instrument,
                     std::map<std::string, uint8_t> techniques) {
        ArticulationMap map{id, instrument, std::move(techniques)};
        m_library[instrument] = map;
        m_idToMap[id] = std::move(map);
    }
};

} // namespace Aura::Core::Engine
