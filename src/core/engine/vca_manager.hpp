#pragma once

#include "../rust_ffi.hpp"

#include <cstdint>
#include <string>
#include <vector>

namespace Hirari::Core::Engine {

/** Thin C++ compatibility layer over the Rust runtime VCA manager. */
class VCAManager {
public:
    static constexpr size_t kMaxTracks = 2048;
    static VCAManager& getInstance() {
        static VCAManager instance;
        return instance;
    }

    float getCumulativeGain(uint32_t trackId) const {
        return hirari_vca_get_cumulative_gain(trackId);
    }

    std::string snapshotGroupsJson() const {
        const size_t required = hirari_vca_snapshot_json(nullptr, 0);
        if (required == 0) return "[]";
        std::string json(required - 1, '\0');
        hirari_vca_snapshot_json(reinterpret_cast<uint8_t*>(json.data()), required);
        return json;
    }

    void clear() { hirari_vca_clear(); }
    void resolveHierarchy() { hirari_vca_resolve_hierarchy(); }
    void addGroup(uint32_t id, float gain) { hirari_vca_add_group(id, gain); }
    bool assignTrack(uint32_t trackId, uint32_t groupId) {
        return hirari_vca_assign_track(trackId, groupId);
    }

private:
    VCAManager() = default;
};

} // namespace Hirari::Core::Engine
