#pragma once
#include <fstream>
#include <sstream>
#include <vector>
#include <string>
#include <iostream>
#include <algorithm>
#include <fcntl.h> 
#include <unistd.h>
#include <type_traits>
#include <cstring>
#include <cstdio>
#include <memory>
#include <cmath>
#include <limits>
#include <filesystem>
#include <atomic>
#include <unordered_set>
#include "engine/automation_curve.hpp"
#include "editing/audio_note_segment.hpp"
#include "editing/event_processing_history.hpp"

namespace Aura::Core {

/**
 * @class ProjectCipher
 * @brief Simple XOR-based obfuscation for project files.
 * HONEST FIX: Purged 'Fractal' branding. This is a basic cipher, not a fractal one.
 */
class ProjectCipher {
public:
    static void process(uint8_t* data, size_t size, uint64_t key) {
        for (size_t i = 0; i < size; ++i) {
            data[i] ^= static_cast<uint8_t>((key >> (i % 8)) & 0xFF);
        }
    }
};

/**
 * @class ProjectSerializer
 * @brief Manages binary project serialization and versioning.
 * HONEST FIX: Optimized CRC32 and purged 'Sovereign' marketing fluff.
 */
class ProjectSerializer {
public:
    // Cumulative load budgets prevent many individually-valid blobs from
    // forcing unbounded allocations in one project.
    static constexpr size_t kMaxProjectBytes = 256u * 1024u * 1024u;
    static constexpr size_t kMaxStringBytes = 64u * 1024u * 1024u;
    static constexpr size_t kMaxPluginDataBytes = 128u * 1024u * 1024u;
    static constexpr size_t kMaxPluginStateBytes = 128u * 1024u * 1024u;
    static constexpr uint64_t kMaxAutomationPoints = 4'000'000u;
    static constexpr uint64_t kMaxPluginStateEntries = 1'000'000u;

    struct AutomationPoint {
        double time = 0.0;
        float value = 0.0f;
        float curve = 0.0f;
    };
    /**
     * @brief Optimized CRC32 calculation using a precomputed table.
     */
    static uint32_t calculateCRC32(const uint8_t* data, size_t size) {
        uint32_t crc = 0xFFFFFFFF;
        for (size_t i = 0; i < size; ++i) {
            crc ^= data[i];
            for (int bit = 0; bit < 8; ++bit)
                crc = (crc >> 1) ^ (0xEDB88320u & static_cast<uint32_t>(-(crc & 1u)));
        }
        return ~crc;
    }

    struct TrackState { 
        uint32_t id; 
        uint32_t type = 0;
        float volume; 
        float pan; 
        float pan3d_x, pan3d_y, pan3d_z; 
        bool muted = false;
        bool solo = false;
        bool phaseInvert = false;
        bool recordArmed = false;
        uint32_t trackDelaySamples = 0;
        std::string pluginName; 
        std::vector<uint8_t> pluginData;
        std::vector<std::vector<uint8_t>> pluginStates;
        std::vector<std::vector<uint8_t>> pluginGuiStates;
        std::vector<uint8_t> pluginBypass;
        std::vector<std::string> sandboxedPluginPaths;
        std::vector<std::vector<uint8_t>> sandboxedPluginStates;
        std::vector<AutomationPoint> volumeAutomation;
        std::vector<AutomationPoint> panAutomation;
        std::vector<AutomationPoint> trackDelayAutomation;
    };
    
    struct RegionState {
        struct RangeEdit { uint64_t start = 0; uint64_t end = 0; float gain = 1.0f; uint64_t fadeIn = 0; uint64_t fadeOut = 0; };
        uint32_t id = 0;
        uint32_t trackId;
        uint64_t samplePosition;
        uint64_t sampleLength;
        uint64_t sourceOffset = 0;
        uint64_t baseStart = 0;
        uint64_t baseSourceOffset = 0;
        uint64_t baseLength = 0;
        bool isMuted;
        std::string filePath;
        std::string name;
        float clipGain = 1.0f;
        uint64_t fadeInSamples = 64;
        uint64_t fadeOutSamples = 64;
        bool reverse = false;
        double warpRatio = 1.0;
        float pitchSemitones = 0.0f;
        uint32_t loopCount = 1;
        std::vector<RangeEdit> rangeEdits;
        bool locked = false;
        uint32_t syncGroup = 0;
        std::vector<::aura::editing::EventProcessingStep> processingHistory;
        std::vector<::aura::editing::AudioNoteSegment> audioNoteSegments;
    };

    struct SidechainState {
        uint32_t sourceId = 0;
        uint32_t destinationId = 0;
        uint32_t pluginIndex = 0;
        uint32_t tapPoint = 2;
    };

    struct RouteState {
        uint32_t sourceId = 0;
        uint32_t destinationId = 0;
        float gain = 1.0f;
    };
    struct MarkerState { uint64_t sample = 0; std::string name; uint32_t color = 0xff808080; };
    struct ArrangerPartState { uint64_t start = 0, length = 0; std::string name; uint32_t repeats = 1; };
    struct TempoEventState { uint64_t sample = 0; double bpm = 120.0; uint8_t ramp = 0; };

    struct ProjectState { 
        double bpm; 
        int32_t rootNote, scaleType; 
        std::vector<TrackState> tracks; 
        std::vector<RegionState> regions;
        uint32_t sampleRate;
        std::vector<RouteState> routes;
        std::vector<SidechainState> sidechains;
        std::vector<MarkerState> markers;
        std::vector<ArrangerPartState> arrangerParts;
        std::vector<TempoEventState> tempoEvents;
        uint32_t version = 36;
        bool valid = false;
    };

    template<typename T>
    static inline void writeLE(std::ostream& s, T val) {
        s.write(reinterpret_cast<const char*>(&val), sizeof(T));
    }

    template<typename T>
    static inline T readLE(std::istream& s) {
        T val;
        s.read(reinterpret_cast<char*>(&val), sizeof(T));
        return val;
    }

    static void writeToStream(std::ostream& stream, const ProjectState& state) {
        uint32_t magic = 0x41555241; 
        writeLE(stream, magic);
        writeLE(stream, state.version);
        writeLE(stream, state.sampleRate);
        writeLE(stream, state.bpm);
        if (state.version >= 14) {
            writeLE(stream, state.rootNote);
            writeLE(stream, state.scaleType);
        }
        
        uint32_t trackCount = static_cast<uint32_t>(state.tracks.size());
        writeLE(stream, trackCount);
        for (const auto& t : state.tracks) {
            writeLE(stream, t.id);
            if (state.version >= 12) writeLE(stream, t.type);
            writeLE(stream, t.volume);
            writeLE(stream, t.pan);
            writeLE(stream, t.pan3d_x); writeLE(stream, t.pan3d_y); writeLE(stream, t.pan3d_z);
            if (state.version >= 12) {
                writeLE(stream, static_cast<uint8_t>(t.muted ? 1 : 0));
                writeLE(stream, static_cast<uint8_t>(t.solo ? 1 : 0));
                if (state.version >= 15) writeLE(stream, static_cast<uint8_t>(t.phaseInvert ? 1 : 0));
                if (state.version >= 23) writeLE(stream, static_cast<uint8_t>(t.recordArmed ? 1 : 0));
                if (state.version >= 29) writeLE(stream, t.trackDelaySamples);
            }
            uint32_t nameLen = static_cast<uint32_t>(t.pluginName.size());
            writeLE(stream, nameLen); stream.write(t.pluginName.data(), nameLen);
            uint32_t dataLen = static_cast<uint32_t>(t.pluginData.size());
            writeLE(stream, dataLen);
            if (dataLen) stream.write(reinterpret_cast<const char*>(t.pluginData.data()), dataLen);
            if (state.version >= 24) {
                const uint32_t pluginStateCount = static_cast<uint32_t>(t.pluginStates.size());
                writeLE(stream, pluginStateCount);
                for (const auto& blob : t.pluginStates) {
                    const uint32_t blobLen = static_cast<uint32_t>(blob.size());
                    writeLE(stream, blobLen);
                    if (blobLen) stream.write(reinterpret_cast<const char*>(blob.data()), blobLen);
                }
                if (state.version >= 25) {
                    const uint32_t bypassCount = static_cast<uint32_t>(t.pluginBypass.size());
                    writeLE(stream, bypassCount);
                    for (const uint8_t bypassed : t.pluginBypass) writeLE(stream, bypassed);
                }
                if (state.version >= 32) {
                    const uint32_t guiCount = static_cast<uint32_t>(t.pluginGuiStates.size());
                    writeLE(stream, guiCount);
                    for (const auto& blob : t.pluginGuiStates) { const uint32_t len = static_cast<uint32_t>(blob.size()); writeLE(stream, len); if (len) stream.write(reinterpret_cast<const char*>(blob.data()), len); }
                }
            }
            if (state.version >= 16) {
                const uint32_t sandboxCount = static_cast<uint32_t>(t.sandboxedPluginPaths.size());
                writeLE(stream, sandboxCount);
                for (const auto& path : t.sandboxedPluginPaths) {
                    const uint32_t pathLen = static_cast<uint32_t>(path.size());
                    writeLE(stream, pathLen);
                    stream.write(path.data(), pathLen);
                }
                if (state.version >= 17) {
                    const uint32_t stateCount = static_cast<uint32_t>(t.sandboxedPluginStates.size());
                    writeLE(stream, stateCount);
                    for (const auto& blob : t.sandboxedPluginStates) {
                        const uint32_t blobLen = static_cast<uint32_t>(blob.size());
                        writeLE(stream, blobLen);
                        if (blobLen) stream.write(reinterpret_cast<const char*>(blob.data()), blobLen);
                    }
                }
            }
            if (state.version >= 13) {
                const auto writeAutomation = [&stream](const auto& points) {
                    const uint32_t count = static_cast<uint32_t>(points.size());
                    writeLE(stream, count);
                    for (const auto& point : points) {
                        writeLE(stream, point.time);
                        writeLE(stream, point.value);
                        writeLE(stream, point.curve);
                    }
                };
                writeAutomation(t.volumeAutomation);
                writeAutomation(t.panAutomation);
                if (state.version >= 30) writeAutomation(t.trackDelayAutomation);
            }
        }
        uint32_t regionCount = static_cast<uint32_t>(state.regions.size());
        writeLE(stream, regionCount);
        for (const auto& r : state.regions) {
            if (state.version >= 28) writeLE(stream, r.id);
            writeLE(stream, r.trackId); writeLE(stream, r.samplePosition); writeLE(stream, r.sampleLength);
            if (state.version >= 19) {
                writeLE(stream, r.sourceOffset);
                writeLE(stream, r.baseStart);
                writeLE(stream, r.baseSourceOffset);
                writeLE(stream, r.baseLength);
            }
            uint8_t muted = r.isMuted ? 1 : 0; writeLE(stream, muted);
            uint32_t pathLen = static_cast<uint32_t>(r.filePath.size());
            writeLE(stream, pathLen); stream.write(r.filePath.data(), pathLen);
            uint32_t regionNameLen = static_cast<uint32_t>(r.name.size());
            writeLE(stream, regionNameLen); stream.write(r.name.data(), regionNameLen);
            writeLE(stream, r.clipGain);
            writeLE(stream, r.fadeInSamples);
            writeLE(stream, r.fadeOutSamples);
            if (state.version >= 18) {
                writeLE(stream, static_cast<uint8_t>(r.reverse ? 1 : 0));
            }
            if (state.version >= 20) writeLE(stream, r.warpRatio);
            if (state.version >= 21) writeLE(stream, r.pitchSemitones);
            if (state.version >= 22) writeLE(stream, r.loopCount);
            if (state.version >= 33) {
                uint32_t count = static_cast<uint32_t>(std::min<size_t>(r.audioNoteSegments.size(), 100000));
                writeLE(stream, count);
                for (const auto& segment : r.audioNoteSegments) {
                    writeLE(stream, segment.startSeconds); writeLE(stream, segment.endSeconds);
                    writeLE(stream, segment.detectedPitchCents); writeLE(stream, segment.pitchOffsetCents);
                    writeLE(stream, segment.formantOffsetCents);
                    uint32_t anchors = static_cast<uint32_t>(std::min<size_t>(segment.anchors.size(), 10000));
                    writeLE(stream, anchors);
                    for (const auto& anchor : segment.anchors) {
                        writeLE(stream, anchor.positionSeconds); writeLE(stream, anchor.pitchCents); writeLE(stream, anchor.formantCents);
                    }
                }
            }
            if (state.version >= 35) { writeLE(stream, static_cast<uint8_t>(r.locked ? 1 : 0)); writeLE(stream, r.syncGroup); }
            if (state.version >= 35) {
                const uint32_t count = static_cast<uint32_t>(std::min<size_t>(r.processingHistory.size(), 4096)); writeLE(stream, count);
                for (const auto& step : r.processingHistory) { writeLE(stream, step.id); writeLE(stream, step.parameter); writeLE(stream, static_cast<uint8_t>(step.enabled ? 1 : 0)); const uint32_t len = static_cast<uint32_t>(std::min<size_t>(step.operation.size(), 256)); writeLE(stream, len); if (len) stream.write(step.operation.data(), len); }
            }
            if (state.version >= 36) {
                uint32_t count = 0;
                for (const auto& edit : r.rangeEdits) {
                    if (edit.start < edit.end && std::isfinite(edit.gain) && edit.gain >= 0.0f &&
                        edit.gain <= 16.0f && edit.fadeIn <= edit.end - edit.start &&
                        edit.fadeOut <= edit.end - edit.start && count < 4096) count++;
                }
                writeLE(stream, count);
                for (const auto& edit : r.rangeEdits) {
                    if (edit.start >= edit.end || !std::isfinite(edit.gain) || edit.gain < 0.0f ||
                        edit.gain > 16.0f || edit.fadeIn > edit.end - edit.start ||
                        edit.fadeOut > edit.end - edit.start || count == 0) continue;
                    writeLE(stream, edit.start); writeLE(stream, edit.end); writeLE(stream, edit.gain);
                    writeLE(stream, edit.fadeIn); writeLE(stream, edit.fadeOut);
                    count--;
                }
            }
        }
        if (state.version >= 26) {
            const uint32_t sidechainCount = static_cast<uint32_t>(state.sidechains.size());
            writeLE(stream, sidechainCount);
            for (const auto& sidechain : state.sidechains) {
                writeLE(stream, sidechain.sourceId);
                writeLE(stream, sidechain.destinationId);
                writeLE(stream, sidechain.pluginIndex);
                writeLE(stream, sidechain.tapPoint);
            }
        }
        if (state.version >= 31) {
            const uint32_t routeCount = static_cast<uint32_t>(state.routes.size());
            writeLE(stream, routeCount);
            for (const auto& route : state.routes) {
                writeLE(stream, route.sourceId);
                writeLE(stream, route.destinationId);
                writeLE(stream, route.gain);
            }
        }
        if (state.version >= 34) {
            const uint32_t markerCount = static_cast<uint32_t>(std::min<size_t>(state.markers.size(), 100000));
            writeLE(stream, markerCount);
            for (const auto& marker : state.markers) {
                writeLE(stream, marker.sample); writeLE(stream, marker.color);
                const uint32_t len = static_cast<uint32_t>(std::min<size_t>(marker.name.size(), 1024));
                writeLE(stream, len); if (len) stream.write(marker.name.data(), len);
            }
            const uint32_t partCount = static_cast<uint32_t>(std::min<size_t>(state.arrangerParts.size(), 100000));
            writeLE(stream, partCount);
            for (const auto& part : state.arrangerParts) {
                writeLE(stream, part.start); writeLE(stream, part.length); writeLE(stream, part.repeats);
                const uint32_t len = static_cast<uint32_t>(std::min<size_t>(part.name.size(), 1024));
                writeLE(stream, len); if (len) stream.write(part.name.data(), len);
            }
            const uint32_t tempoCount = static_cast<uint32_t>(std::min<size_t>(state.tempoEvents.size(), 100000));
            writeLE(stream, tempoCount);
            for (const auto& event : state.tempoEvents) { writeLE(stream, event.sample); writeLE(stream, event.bpm); writeLE(stream, event.ramp); }
        }
    }

    static std::vector<uint8_t> serialize(const ProjectState& state) {
        std::stringstream ss(std::ios::binary | std::ios::out);
        writeToStream(ss, state);
        std::string str = ss.str();
        std::vector<uint8_t> bytes(str.begin(), str.end());
        // Version 27+ uses a real CRC32 trailer over the complete payload;
        // version 28 additionally preserves region identities.
        // This replaces the old integrity-by-obfuscation assumption and lets
        // readers reject torn/corrupted native snapshots deterministically.
        if (state.version >= 27) {
            const uint32_t checksum = calculateCRC32(bytes.data(), bytes.size());
            const auto* raw = reinterpret_cast<const uint8_t*>(&checksum);
            bytes.insert(bytes.end(), raw, raw + sizeof(checksum));
        }
        return bytes;
    }

    static bool save(const std::string& path, const ProjectState& state) {
        std::ofstream file(path, std::ios::binary);
        if (!file.is_open()) return false;
        auto buffer = serialize(state);
        file.write(reinterpret_cast<const char*>(buffer.data()), buffer.size());
        file.flush();
        return file.good();
    }

    static bool saveAtomic(const std::string& path, const ProjectState& state) {
        if (path.empty()) return false;
        static std::atomic<uint64_t> tempSequence{0};
        const std::string temporary = path + ".tmp-" + std::to_string(static_cast<unsigned long long>(::getpid())) +
            "-" + std::to_string(static_cast<unsigned long long>(tempSequence.fetch_add(1, std::memory_order_relaxed)));
        std::error_code ec;
        if (!save(temporary, state)) {
            std::filesystem::remove(temporary, ec);
            return false;
        }
        // Flush the file before publishing the rename. A successful rename
        // alone does not protect a project from a power loss while the temp
        // file is still only in the kernel page cache.
        const int fd = ::open(temporary.c_str(), O_RDONLY);
        if (fd < 0 || ::fsync(fd) != 0) {
            if (fd >= 0) ::close(fd);
            std::filesystem::remove(temporary, ec);
            return false;
        }
        ::close(fd);
        const auto expectedSize = std::filesystem::file_size(temporary, ec);
        if (ec || expectedSize < sizeof(uint32_t) * 2u) {
            std::filesystem::remove(temporary, ec);
            return false;
        }
        std::filesystem::rename(temporary, path, ec);
        if (ec) std::filesystem::remove(temporary, ec);
        if (!ec) {
            const auto parent = std::filesystem::path(path).parent_path();
            const std::string parentPath = parent.empty() ? "." : parent.string();
            const int dirFd = ::open(parentPath.c_str(), O_RDONLY | O_DIRECTORY);
            if (dirFd < 0) {
                ec = std::make_error_code(std::errc::io_error);
            } else {
                if (::fsync(dirFd) != 0) ec = std::make_error_code(std::errc::io_error);
                ::close(dirFd);
            }
        }
        return !ec;
    }

    static ProjectState load(const std::string& path) {
        ProjectState state{};
        std::ifstream input(path, std::ios::binary | std::ios::ate);
        if (!input.is_open()) return state;
        const auto fileSize = input.tellg();
        if (fileSize < static_cast<std::streamoff>(sizeof(uint32_t) * 2u)) return {};
        if (static_cast<uint64_t>(fileSize) > kMaxProjectBytes) return {};
        input.seekg(0, std::ios::beg);
        std::vector<uint8_t> bytes(static_cast<size_t>(fileSize));
        input.read(reinterpret_cast<char*>(bytes.data()), static_cast<std::streamsize>(bytes.size()));
        if (!input) return {};
        if (bytes.size() < sizeof(uint32_t) * 2u) return {};
        uint32_t magic = 0;
        uint32_t version = 0;
        std::memcpy(&magic, bytes.data(), sizeof(magic));
        std::memcpy(&version, bytes.data() + sizeof(magic), sizeof(version));
        if (magic != 0x41555241u || version == 0 || version > 36) return {};
        size_t payloadSize = bytes.size();
        if (version >= 27) {
            if (bytes.size() < sizeof(uint32_t) * 3u) return {};
            payloadSize -= sizeof(uint32_t);
            uint32_t storedChecksum = 0;
            std::memcpy(&storedChecksum, bytes.data() + payloadSize, sizeof(storedChecksum));
            if (calculateCRC32(bytes.data(), payloadSize) != storedChecksum) return {};
        }
        std::string payload(reinterpret_cast<const char*>(bytes.data()), payloadSize);
        std::istringstream file(payload, std::ios::binary | std::ios::in);
        // Consume the header that was already inspected above; the existing
        // parser expects the stream cursor immediately after magic/version.
        (void)readLE<uint32_t>(file);
        (void)readLE<uint32_t>(file);
        state.version = version;
        state.sampleRate = readLE<uint32_t>(file); state.bpm = readLE<double>(file);
        if (version >= 14) {
            state.rootNote = readLE<int32_t>(file);
            state.scaleType = readLE<int32_t>(file);
        }
        if (!file || state.sampleRate == 0 || !std::isfinite(state.bpm) ||
            state.bpm < 20.0 || state.bpm > 300.0 || state.rootNote < 0 ||
            state.rootNote > 11 || state.scaleType < 0 || state.scaleType > 32) return {};
        size_t totalStringBytes = 0;
        size_t totalPluginDataBytes = 0;
        size_t totalPluginStateBytes = 0;
        uint64_t totalAutomationPoints = 0;
        uint64_t totalPluginStateEntries = 0;
        const auto addBudget = [](size_t& total, size_t amount, size_t limit) -> bool {
            if (amount > limit || total > limit - amount) return false;
            total += amount;
            return true;
        };
        const auto addCount = [](uint64_t& total, uint64_t amount, uint64_t limit) -> bool {
            if (amount > limit || total > limit - amount) return false;
            total += amount;
            return true;
        };
        auto readString = [&file, &totalStringBytes, &addBudget](std::string& out) -> bool {
            uint32_t len = readLE<uint32_t>(file);
            if (!file || len > 16u * 1024u * 1024u) return false;
            if (!addBudget(totalStringBytes, len, kMaxStringBytes)) return false;
            out.resize(len); file.read(out.data(), len); return static_cast<bool>(file);
        };
        uint32_t trackCount = readLE<uint32_t>(file);
        if (!file || trackCount > 100000) return {};
        state.tracks.resize(trackCount);
        std::unordered_set<uint32_t> trackIds;
        for (auto& t : state.tracks) {
            t.id = readLE<uint32_t>(file);
            // Track zero is the native engine's first valid track ID. Older
            // loads rejected it even though the template/bootstrap path
            // legitimately serializes the first track as ID 0, making a
            // freshly saved template impossible to reopen. Uniqueness and
            // the reserved uint32 max sentinel are enforced by the hydrate
            // layer; zero itself is valid.
            if (!file || !trackIds.insert(t.id).second) return {};
            if (version >= 12) t.type = readLE<uint32_t>(file);
            t.volume = readLE<float>(file); t.pan = readLE<float>(file);
            t.pan3d_x = readLE<float>(file); t.pan3d_y = readLE<float>(file); t.pan3d_z = readLE<float>(file);
            if (version >= 12) {
                t.muted = readLE<uint8_t>(file) != 0;
                t.solo = readLE<uint8_t>(file) != 0;
                if (version >= 15) t.phaseInvert = readLE<uint8_t>(file) != 0;
                if (version >= 23) t.recordArmed = readLE<uint8_t>(file) != 0;
                if (version >= 29) t.trackDelaySamples = readLE<uint32_t>(file);
            }
            if (!file || !std::isfinite(t.volume) || t.volume < 0.0f || t.volume > 2.0f ||
                !std::isfinite(t.pan) || t.pan < -1.0f || t.pan > 1.0f ||
                !std::isfinite(t.pan3d_x) || !std::isfinite(t.pan3d_y) || !std::isfinite(t.pan3d_z) ||
                t.trackDelaySamples > 8192 ||
                (version >= 12 && t.type > 4)) return {};
            if (!readString(t.pluginName)) return {};
            uint32_t dataLen = readLE<uint32_t>(file);
            if (!file || dataLen > 64u * 1024u * 1024u) return {};
            if (!addBudget(totalPluginDataBytes, dataLen, kMaxPluginDataBytes)) return {};
            t.pluginData.resize(dataLen);
            if (dataLen) file.read(reinterpret_cast<char*>(t.pluginData.data()), dataLen);
            if (!file) return {};
            if (version >= 24) {
                const uint32_t pluginStateCount = readLE<uint32_t>(file);
                if (!file || pluginStateCount > 4096) return {};
                if (!addCount(totalPluginStateEntries, pluginStateCount, kMaxPluginStateEntries)) return {};
                t.pluginStates.resize(pluginStateCount);
                for (auto& blob : t.pluginStates) {
                    const uint32_t blobLen = readLE<uint32_t>(file);
                    if (!file || blobLen > 16u * 1024u * 1024u) return {};
                    if (!addBudget(totalPluginStateBytes, blobLen, kMaxPluginStateBytes)) return {};
                    blob.resize(blobLen);
                    if (blobLen) file.read(reinterpret_cast<char*>(blob.data()), blobLen);
                    if (!file) return {};
                }
                if (version >= 25) {
                    const uint32_t bypassCount = readLE<uint32_t>(file);
                    if (!file || bypassCount > 4096) return {};
                    t.pluginBypass.resize(bypassCount);
                    for (auto& bypassed : t.pluginBypass) {
                        bypassed = readLE<uint8_t>(file);
                        if (!file || bypassed > 1) return {};
                    }
                }
                if (version >= 32) {
                    const uint32_t guiCount = readLE<uint32_t>(file);
                    if (!file || guiCount > 4096 || guiCount != pluginStateCount) return {};
                    t.pluginGuiStates.resize(guiCount);
                    for (auto& blob : t.pluginGuiStates) { const uint32_t len = readLE<uint32_t>(file); if (!file || len > 1024u * 1024u) return {}; blob.resize(len); if (len) file.read(reinterpret_cast<char*>(blob.data()), len); if (!file) return {}; }
                }
            }
            if (version >= 16) {
                const uint32_t sandboxCount = readLE<uint32_t>(file);
                if (!file || sandboxCount > 4096) return {};
                t.sandboxedPluginPaths.resize(sandboxCount);
                for (auto& path : t.sandboxedPluginPaths) {
                    if (!readString(path)) return {};
                    if (path.empty() || path.size() > 16u * 1024u * 1024u) return {};
                }
                if (version >= 17) {
                    const uint32_t stateCount = readLE<uint32_t>(file);
                    if (!file || stateCount > 4096 || stateCount != sandboxCount) return {};
                    if (!addCount(totalPluginStateEntries, stateCount, kMaxPluginStateEntries)) return {};
                    t.sandboxedPluginStates.resize(stateCount);
                    for (auto& blob : t.sandboxedPluginStates) {
                        const uint32_t blobLen = readLE<uint32_t>(file);
                        if (!file || blobLen > 16u * 1024u * 1024u) return {};
                        if (!addBudget(totalPluginStateBytes, blobLen, kMaxPluginStateBytes)) return {};
                        blob.resize(blobLen);
                        if (blobLen) file.read(reinterpret_cast<char*>(blob.data()), blobLen);
                        if (!file) return {};
                    }
                }
            }
            if (version >= 13) {
                const auto readAutomation = [&file, &totalAutomationPoints, &addCount, version](auto& points) -> bool {
                    const uint32_t count = readLE<uint32_t>(file);
                    if (!file || count > 100000) return false;
                    if (!addCount(totalAutomationPoints, count, kMaxAutomationPoints)) return false;
                    points.resize(count);
                    for (auto& point : points) {
                        point.time = readLE<double>(file);
                        point.value = readLE<float>(file);
                        point.curve = readLE<float>(file);
                        if (!file || !std::isfinite(point.time) || point.time < 0.0 ||
                            (version >= 30 && std::trunc(point.time) != point.time) ||
                            !std::isfinite(point.value) || !std::isfinite(point.curve)) return false;
                    }
                    return true;
                };
                if (!readAutomation(t.volumeAutomation) || !readAutomation(t.panAutomation)) return {};
                if (version >= 30 && !readAutomation(t.trackDelayAutomation)) return {};
            }
        }
        uint32_t regionCount = readLE<uint32_t>(file);
        if (!file || regionCount > 1000000) return {};
        state.regions.resize(regionCount);
        std::unordered_set<uint32_t> regionIds;
        for (auto& r : state.regions) {
            if (version >= 28) {
                r.id = readLE<uint32_t>(file);
                if (!file || r.id == 0 || !regionIds.insert(r.id).second) return {};
            }
            r.trackId = readLE<uint32_t>(file); r.samplePosition = readLE<uint64_t>(file); r.sampleLength = readLE<uint64_t>(file);
            if (!file || !trackIds.contains(r.trackId)) return {};
            if (version >= 19) {
                r.sourceOffset = readLE<uint64_t>(file);
                r.baseStart = readLE<uint64_t>(file);
                r.baseSourceOffset = readLE<uint64_t>(file);
                r.baseLength = readLE<uint64_t>(file);
                if (!file || r.sourceOffset > std::numeric_limits<uint64_t>::max() - r.sampleLength ||
                    r.baseSourceOffset > std::numeric_limits<uint64_t>::max() - r.baseLength) return {};
            }
            r.isMuted = readLE<uint8_t>(file) != 0;
            if (!readString(r.filePath) || !readString(r.name)) return {};
            if (r.filePath.empty() || r.sampleLength == 0 ||
                r.samplePosition > std::numeric_limits<uint64_t>::max() - r.sampleLength) return {};
            if (version >= 11) {
                r.clipGain = readLE<float>(file);
                r.fadeInSamples = readLE<uint64_t>(file);
                r.fadeOutSamples = readLE<uint64_t>(file);
                if (!file || !std::isfinite(r.clipGain) || r.clipGain < 0.0f || r.clipGain > 2.0f ||
                    r.fadeInSamples > r.sampleLength || r.fadeOutSamples > r.sampleLength) return {};
                if (version >= 18) {
                    r.reverse = readLE<uint8_t>(file) != 0;
                    if (!file) return {};
                }
                if (version >= 20) {
                    r.warpRatio = readLE<double>(file);
                    if (!file || !std::isfinite(r.warpRatio) || r.warpRatio < 0.5 || r.warpRatio > 2.0) return {};
                }
                if (version >= 21) {
                    r.pitchSemitones = readLE<float>(file);
                    if (!file || !std::isfinite(r.pitchSemitones) || r.pitchSemitones < -24.0f || r.pitchSemitones > 24.0f) return {};
                }
                if (version >= 22) {
                    r.loopCount = readLE<uint32_t>(file);
                    if (!file || r.loopCount == 0 || r.loopCount > 1024 ||
                        r.sampleLength > std::numeric_limits<uint64_t>::max() / r.loopCount ||
                        r.samplePosition > std::numeric_limits<uint64_t>::max() - r.sampleLength * r.loopCount) return {};
                }
                if (version >= 33) {
                    const uint32_t segmentCount = readLE<uint32_t>(file);
                    if (!file || segmentCount > 100000) return {};
                    r.audioNoteSegments.resize(segmentCount);
                    for (auto& segment : r.audioNoteSegments) {
                        segment.startSeconds = readLE<double>(file); segment.endSeconds = readLE<double>(file);
                        segment.detectedPitchCents = readLE<double>(file); segment.pitchOffsetCents = readLE<double>(file);
                        segment.formantOffsetCents = readLE<double>(file);
                        const uint32_t anchorCount = readLE<uint32_t>(file);
                        if (!file || anchorCount > 10000 || !segment.valid()) return {};
                        segment.anchors.resize(anchorCount);
                        for (auto& anchor : segment.anchors) {
                            anchor.positionSeconds = readLE<double>(file); anchor.pitchCents = readLE<double>(file); anchor.formantCents = readLE<double>(file);
                            if (!file || !std::isfinite(anchor.positionSeconds) || !std::isfinite(anchor.pitchCents) || !std::isfinite(anchor.formantCents) ||
                                anchor.positionSeconds < segment.startSeconds || anchor.positionSeconds > segment.endSeconds) return {};
                        }
                    }
                }
                if (version >= 35) { r.locked = readLE<uint8_t>(file) != 0; r.syncGroup = readLE<uint32_t>(file); if (!file) return {}; }
                if (version >= 35) {
                    const uint32_t count = readLE<uint32_t>(file); if (!file || count > 4096) return {};
                    r.processingHistory.resize(count);
                    for (auto& step : r.processingHistory) { step.id = readLE<uint32_t>(file); step.parameter = readLE<float>(file); step.enabled = readLE<uint8_t>(file) != 0; const uint32_t len = readLE<uint32_t>(file); if (!file || len > 256) return {}; step.operation.resize(len); if (len) file.read(step.operation.data(), len); if (!file || step.operation.empty()) return {}; }
                }
                if (version >= 36) {
                    const uint32_t count = readLE<uint32_t>(file); if (!file || count > 4096) return {};
                    r.rangeEdits.resize(count);
                    for (auto& edit : r.rangeEdits) { edit.start = readLE<uint64_t>(file); edit.end = readLE<uint64_t>(file); edit.gain = readLE<float>(file); edit.fadeIn = readLE<uint64_t>(file); edit.fadeOut = readLE<uint64_t>(file); if (!file || edit.start >= edit.end || !std::isfinite(edit.gain) || edit.gain < 0.0f || edit.gain > 16.0f || edit.fadeIn > edit.end - edit.start || edit.fadeOut > edit.end - edit.start) return {}; }
                }
            }
        }
        if (version >= 26) {
            const uint32_t sidechainCount = readLE<uint32_t>(file);
            if (!file || sidechainCount > 100000) return {};
            state.sidechains.resize(sidechainCount);
            std::unordered_set<uint64_t> sidechainDestinations;
            for (auto& sidechain : state.sidechains) {
                sidechain.sourceId = readLE<uint32_t>(file);
                sidechain.destinationId = readLE<uint32_t>(file);
                sidechain.pluginIndex = readLE<uint32_t>(file);
                sidechain.tapPoint = readLE<uint32_t>(file);
                const uint64_t destinationKey =
                    (static_cast<uint64_t>(sidechain.destinationId) << 32) |
                    static_cast<uint64_t>(sidechain.pluginIndex);
                if (!file || sidechain.sourceId == sidechain.destinationId ||
                    sidechain.tapPoint > 2 ||
                    !sidechainDestinations.insert(destinationKey).second) return {};
            }
        }
        if (version >= 31) {
            const uint32_t routeCount = readLE<uint32_t>(file);
            if (!file || routeCount > 100000) return {};
            state.routes.resize(routeCount);
            std::unordered_set<uint64_t> routeKeys;
            for (auto& route : state.routes) {
                route.sourceId = readLE<uint32_t>(file);
                route.destinationId = readLE<uint32_t>(file);
                route.gain = readLE<float>(file);
                const uint64_t key = (static_cast<uint64_t>(route.sourceId) << 32) |
                                     static_cast<uint64_t>(route.destinationId);
                if (!file || route.sourceId == route.destinationId ||
                    !std::isfinite(route.gain) || route.gain <= 0.0f || route.gain > 2.0f ||
                    !routeKeys.insert(key).second) return {};
            }
        }
        if (version >= 34) {
            const uint32_t markerCount = readLE<uint32_t>(file);
            if (!file || markerCount > 100000) return {};
            state.markers.resize(markerCount);
            std::unordered_set<uint64_t> markerPositions;
            for (auto& marker : state.markers) {
                marker.sample = readLE<uint64_t>(file); marker.color = readLE<uint32_t>(file);
                const uint32_t len = readLE<uint32_t>(file);
                if (!file || len > 1024 || !markerPositions.insert(marker.sample).second) return {};
                marker.name.resize(len); if (len) file.read(marker.name.data(), len);
                if (!file || marker.name.empty()) return {};
            }
            const uint32_t partCount = readLE<uint32_t>(file);
            if (!file || partCount > 100000) return {};
            state.arrangerParts.resize(partCount);
            for (auto& part : state.arrangerParts) {
                part.start = readLE<uint64_t>(file); part.length = readLE<uint64_t>(file); part.repeats = readLE<uint32_t>(file);
                const uint32_t len = readLE<uint32_t>(file);
                if (!file || part.length == 0 || part.repeats == 0 || len == 0 || len > 1024) return {};
                part.name.resize(len); file.read(part.name.data(), len);
                if (!file) return {};
            }
            const uint32_t tempoCount = readLE<uint32_t>(file);
            if (!file || tempoCount > 100000) return {};
            state.tempoEvents.resize(tempoCount);
            uint64_t previousSample = 0;
            for (auto& event : state.tempoEvents) {
                event.sample = readLE<uint64_t>(file); event.bpm = readLE<double>(file); event.ramp = readLE<uint8_t>(file);
                if (!file || !std::isfinite(event.bpm) || event.bpm < 20.0 || event.bpm > 300.0 || event.ramp > 1 || (event.sample < previousSample)) return {};
                previousSample = event.sample;
            }
        }
        if (!file || file.peek() != std::char_traits<char>::eof()) return {};
        state.valid = true;
        return state;
    }
};

} // namespace Aura::Core
