#pragma once
#include <cstdint>
#include <string>
#include <vector>
#if __has_include("rust/cxx.h")
#include "rust/cxx.h"
#elif __has_include("../target/cxxbridge/rust/cxx.h")
#include "../target/cxxbridge/rust/cxx.h"
#else
namespace rust {
    using String = std::string;
    template <typename T> class Vec : public std::vector<T> {
    public:
        using std::vector<T>::vector;
        void reserve(size_t n) { std::vector<T>::reserve(n); }
    };
    template <typename T> class Slice {
    public:
        Slice(const T* data, size_t len) : m_data(data), m_len(len) {}
        const T* data() const { return m_data; }
        size_t len() const { return m_len; }
    private:
        const T* m_data;
        size_t m_len;
    };
}
#endif

namespace Aura::Core {

enum class CommandType : uint32_t {
    SetVolume = 0,
    SetPan = 1,
    SetMute = 2,
    SetSolo = 3,
    AddPlugin = 4,
    RemovePlugin = 5,
    GenerateDrumFill = 6,
    ExecuteAutoMixing = 7,
    ExecuteAutoArrangement = 8,
    SetSpatialMode = 9,
    SetSpatialPosition = 10,
    SetAscendedMode = 11,
    SetAutomationRecordMode = 12
};

struct MarkerInfo {
    uint64_t sample_position;
    rust::String name;
    uint32_t color;
};

namespace Bridge {
    struct StructureNode {
        rust::String label;
        double timestamp;
    };

    struct PlainClash {
        uint32_t bin;
        float intensity;
        uint32_t track_b;
    };

    struct PlainLoudness {
        float integrated;
        float short_term;
        float true_peak_l;
        float true_peak_r;
        float correlation;
    };
}

struct EngineEvent {
    uint64_t timestamp;
    uint32_t type;
    uint32_t trackId;
    float value;
    char label[128]; // Pre-allocated fixed buffer for RT-safety
};

// Re-export or forward declare BridgeFFI items if needed
namespace BridgeFFI {
    using ::Aura::Core::MarkerInfo;
    using ::Aura::Core::EngineEvent;
}

} // namespace Aura::Core
