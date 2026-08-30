#pragma once

#include <vector>
#include <string>
#include <map>
#include <array>
#include <filesystem>
#include <regex>
#include "sampler_engine.hpp"
#include "../../io/persistence/wav_reader.hpp"
#include "../../core/io/sample_pool.hpp"

namespace Aura::DSP::Synthesis {

/**
 * @class Multisampler
 * @brief High-performance Multi-zone Sampler Instrument.
 * HONEST FIX: Automatically scans sample directories (like VSCO2) 
 * and maps them to MIDI note ranges based on filename parsing.
 */
class Multisampler {
public:
    struct Zone {
        uint8_t lowNote, highNote;
        uint8_t lowVel, highVel;
        uint8_t rootNote;
        std::shared_ptr<Core::AudioBuffer> sampleData;
    };

    void loadDirectory(const std::string& path) {
        namespace fs = std::filesystem;
        if (!fs::exists(path)) return;

        for (const auto& entry : fs::recursive_directory_iterator(path)) {
            if (entry.path().extension() == ".wav") {
                parseSample(entry.path().string());
            }
        }
        
        std::sort(m_zones.begin(), m_zones.end(), [](const Zone& a, const Zone& b) { 
            if (a.lowNote != b.lowNote) return a.lowNote < b.lowNote;
            return a.lowVel < b.lowVel;
        });
        m_isLoaded = !m_zones.empty();
    }


    void noteOn(uint8_t note, uint8_t velocity) {
        if (!m_isLoaded) return;

        // Select one velocity layer first, then rotate only among zones that
        // describe that exact layer.  Rotating across layers would make the
        // same MIDI velocity change timbre from hit to hit.
        const Zone* selectedLayer = nullptr;
        for (const auto& zone : m_zones) {
            if (note >= zone.lowNote && note <= zone.highNote &&
                velocity >= zone.lowVel && velocity <= zone.highVel) {
                selectedLayer = &zone;
                break;
            }
        }
        if (!selectedLayer) return;

        std::array<const Zone*, 256> roundRobin{};
        size_t roundRobinCount = 0;
        for (const auto& zone : m_zones) {
            if (note >= zone.lowNote && note <= zone.highNote &&
                velocity >= zone.lowVel && velocity <= zone.highVel &&
                zone.lowVel == selectedLayer->lowVel &&
                zone.highVel == selectedLayer->highVel) {
                if (roundRobinCount < roundRobin.size()) {
                    roundRobin[roundRobinCount++] = &zone;
                }
            }
        }
        if (roundRobinCount != 0) {
            const uint32_t ordinal = m_roundRobinCounters[note]++;
            const Zone& zone = *roundRobin[ordinal % roundRobinCount];
            m_engine.noteOn(note, velocity, zone.sampleData.get(), zone.rootNote);
        }
    }

    void noteOff(uint8_t note) { m_engine.NoteOff(note); }
    void process(Core::AudioBuffer& buffer) { m_engine.process(buffer); }

private:
    void parseSample(const std::string& path) {
        std::string name = std::filesystem::path(path).filename().string();
        // Catch VSCO/Decent-style naming: ArcoVib_A4_v1.wav 
        std::regex re("_([A-G]#?)([0-9])_v([0-9])");
        std::smatch match;
        if (std::regex_search(name, match, re)) {
            std::string noteName = match[1];
            int octave = std::stoi(match[2]);
            int velLayer = std::stoi(match[3]); 
            uint8_t midinot = nameToMidi(noteName, octave);
            
            // Map v1..v4 to MIDI velocity ranges
            uint8_t lowV = (velLayer - 1) * 32;
            uint8_t highV = (velLayer == 4) ? 127 : (velLayer * 32 - 1);

            auto data = Core::IO::GlobalSamplePool::getInstance().acquireSample(path);
            if (data) {
                m_zones.push_back({ (uint8_t)(midinot - 3), (uint8_t)(midinot + 3), lowV, highV, midinot, data });
            }
        }
    }


    uint8_t nameToMidi(const std::string& name, int octave) {
        static const std::map<std::string, int> map = {
            {"C",0},{"C#",1},{"Db",1},{"D",2},{"D#",3},{"Eb",3},{"E",4},{"F",5},{"F#",6},{"Gb",6},{"G",7},{"G#",8},{"Ab",8},{"A",9},{"A#",10},{"Bb",10},{"B",11}
        };
        auto it = map.find(name);
        int offset = (it != map.end()) ? it->second : 0;
        return static_cast<uint8_t>((octave + 1) * 12 + offset);
    }

    SamplerEngine m_engine;
    std::vector<Zone> m_zones;
    std::array<uint32_t, 128> m_roundRobinCounters{};
    bool m_isLoaded = false;
};

} // namespace Aura::DSP::Synthesis
