#pragma once

#include <algorithm>
#include <cmath>
#include <cstdint>
#include <string>
#include <vector>

namespace Aura::Core::Composition {

enum class SnapMode : uint8_t { Off, Grid, RelativeGrid, ZeroCrossing };

struct ArrangementMarker { uint64_t sample = 0; std::string name; uint32_t color = 0xff808080; };
struct ArrangerPart { uint64_t start = 0, length = 0; std::string name; uint32_t repeats = 1; };

class ArrangementEditModel {
public:
    void setSnap(SnapMode mode, uint64_t gridSamples = 1) noexcept {
        m_snap = mode; m_grid = std::max<uint64_t>(1, gridSamples);
    }
    SnapMode snapMode() const noexcept { return m_snap; }
    uint64_t snap(uint64_t sample, uint64_t relativeTo = 0) const noexcept {
        if (m_snap == SnapMode::Off) return sample;
        const uint64_t origin = m_snap == SnapMode::RelativeGrid ? relativeTo : 0;
        const uint64_t offset = sample >= origin ? sample - origin : 0;
        const uint64_t rounded = ((offset + m_grid / 2) / m_grid) * m_grid;
        return origin + rounded;
    }
    bool upsertMarker(ArrangementMarker marker) {
        if (marker.name.empty()) return false;
        auto it = std::lower_bound(m_markers.begin(), m_markers.end(), marker.sample,
            [](const auto& a, uint64_t s){ return a.sample < s; });
        if (it != m_markers.end() && it->sample == marker.sample) *it = std::move(marker);
        else m_markers.insert(it, std::move(marker));
        return true;
    }
    bool removeMarker(uint64_t sample) {
        auto it = std::find_if(m_markers.begin(), m_markers.end(), [sample](const auto& m){ return m.sample == sample; });
        if (it == m_markers.end()) return false; m_markers.erase(it); return true;
    }
    void clearMarkers() noexcept { m_markers.clear(); }
    const std::vector<ArrangementMarker>& markers() const noexcept { return m_markers; }
    bool setArrangerParts(std::vector<ArrangerPart> parts) {
        for (const auto& p : parts) if (p.length == 0 || p.repeats == 0 || p.name.empty()) return false;
        std::sort(parts.begin(), parts.end(), [](const auto& a, const auto& b){ return a.start < b.start; });
        m_parts = std::move(parts); return true;
    }
    const std::vector<ArrangerPart>& arrangerParts() const noexcept { return m_parts; }
private:
    SnapMode m_snap = SnapMode::Grid; uint64_t m_grid = 1;
    std::vector<ArrangementMarker> m_markers; std::vector<ArrangerPart> m_parts;
};
}
