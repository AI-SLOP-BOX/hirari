#pragma once

#include <string>
#include <vector>
#include <memory>
#include <unordered_set>
#include <cmath>
#include <limits>
#include "../../external/nlohmann/json.hpp"
#include "../../core/engine/timeline_system.hpp"

namespace Aura::IO::Persistence {

/**
 * @class ProjectDecoder
 * @brief Validated JSON project decoder.
 * Parsing is deliberately separate from publication: malformed input never
 * clears or partially hydrates the live timeline.
 */
class ProjectDecoder {
public:
    static bool decode(const std::string& jsonData, Core::Engine::TimelineSystem& timeline) {
        if (jsonData.empty()) return false;

        using Json = nlohmann::json;
        const Json document = Json::parse(jsonData, nullptr, false);
        if (document.is_discarded() || !document.is_object() ||
            !document.contains("tracks") || !document.at("tracks").is_array()) return false;

        std::vector<std::shared_ptr<Core::Engine::Track>> decodedTracks;
        std::unordered_set<uint32_t> trackIds;
        std::unordered_set<uint32_t> regionIds;
        constexpr size_t kMaxTracks = 256;
        constexpr size_t kMaxRegions = 1'000'000;

        try {
            const auto& tracks = document.at("tracks");
            if (tracks.size() == 0 || tracks.size() > kMaxTracks) return false;
            for (const auto& value : tracks) {
                if (!value.is_object() || !value.contains("id") ||
                    !value.at("id").is_number_unsigned()) return false;
                const uint64_t rawId = value.at("id").get<uint64_t>();
                if (rawId == 0 || rawId > std::numeric_limits<uint32_t>::max() ||
                    !trackIds.insert(static_cast<uint32_t>(rawId)).second) return false;

                uint32_t typeValue = value.value("type", 0u);
                if (typeValue > static_cast<uint32_t>(Core::Engine::Track::Type::Vocal)) return false;
                std::string name = value.value("name", std::string("Restored Track"));
                if (name.empty() || name.size() > 4096) name = "Restored Track";
                auto track = std::make_shared<Core::Engine::Track>(
                    static_cast<uint32_t>(rawId), name,
                    static_cast<Core::Engine::Track::Type>(typeValue));

                if (value.contains("volume")) track->setVolume(requireFinite<float>(value.at("volume")));
                if (value.contains("pan")) track->setPan(requireFinite<float>(value.at("pan")));
                if (value.contains("muted")) track->setMuted(value.at("muted").get<bool>());
                if (value.contains("solo")) track->setSolo(value.at("solo").get<bool>());
                if (value.contains("phase_invert")) track->setPhaseInverted(value.at("phase_invert").get<bool>());
                if (value.contains("record_armed")) track->setRecordArmed(value.at("record_armed").get<bool>());

                const auto* regions = findArray(value, "audio_regions");
                if (value.contains("audio_regions") && regions == nullptr) return false;
                if (regions == nullptr) {
                    regions = findArray(value, "regions");
                    if (value.contains("regions") && regions == nullptr) return false;
                }
                if (regions != nullptr) {
                    if (regionIds.size() + regions->size() > kMaxRegions) return false;
                    for (const auto& regionValue : *regions) {
                        if (!regionValue.is_object() || !regionValue.contains("id") ||
                            !regionValue.contains("start") || !regionValue.contains("length")) return false;
                        const uint64_t regionId = requireUnsigned(regionValue.at("id"));
                        const uint64_t start = requireUnsigned(regionValue.at("start"));
                        const uint64_t length = requireUnsigned(regionValue.at("length"));
                        if (regionId == 0 || regionId > std::numeric_limits<uint32_t>::max() ||
                            length == 0 || start > UINT64_MAX - length ||
                            !regionIds.insert(static_cast<uint32_t>(regionId)).second) return false;
                        Core::Engine::Region region{};
                        region.id = static_cast<uint32_t>(regionId);
                        region.start = start;
                        region.len = length;
                        region.path = regionValue.value("path", std::string{});
                        region.name = regionValue.value("name", region.path);
                        if (region.path.size() > 32768 || region.name.size() > 4096) return false;
                        region.muted = regionValue.value("muted", false);
                        region.clipGain = valueOrFinite(regionValue, "clip_gain", 1.0f);
                        region.warpRatio = valueOrFinite(regionValue, "warp_ratio", 1.0);
                        region.pitchSemitones = valueOrFinite(regionValue, "pitch_semitones", 0.0f);
                        region.loopCount = regionValue.value("loop_count", 1u);
                        if (region.loopCount == 0 || region.loopCount > 1'000'000u) return false;
                        track->addRegion(region);
                    }
                }
                decodedTracks.push_back(std::move(track));
            }
        } catch (const nlohmann::json::exception&) {
            return false;
        } catch (const std::exception&) {
            return false;
        }

        if (decodedTracks.empty()) return false;
        timeline.replaceTracks(std::move(decodedTracks));
        return true;
    }

private:
    using Json = nlohmann::json;

    static const Json* findArray(const Json& object, const char* key) {
        return object.contains(key) && object.at(key).is_array() ? &object.at(key) : nullptr;
    }

    static uint64_t requireUnsigned(const Json& value) {
        if (!value.is_number_unsigned()) throw std::invalid_argument("expected unsigned integer");
        return value.get<uint64_t>();
    }

    template <typename T>
    static T requireFinite(const Json& value) {
        if (!value.is_number()) throw std::invalid_argument("expected number");
        const T result = value.get<T>();
        if (!std::isfinite(result)) throw std::invalid_argument("non-finite number");
        return result;
    }

    template <typename T>
    static T valueOrFinite(const Json& object, const char* key, T fallback) {
        if (!object.contains(key)) return fallback;
        // Present-but-invalid values must fail the decode. Falling back here
        // would silently turn a corrupted project into a different one.
        return requireFinite<T>(object.at(key));
    }
};

} // namespace Aura::IO::Persistence
