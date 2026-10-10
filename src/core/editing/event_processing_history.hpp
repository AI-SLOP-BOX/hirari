#pragma once
#include <cstdint>
#include <cstddef>
#include <string>
#include <vector>
#include <memory>
#include "../rust_ffi.hpp"

namespace hirari::editing {
struct EventProcessingStep {
    uint32_t id = 0;
    std::string operation;
    float parameter = 0.0f;
    bool enabled = true;
};

// Rust-owned processing-step history for one region.
//
// `EventProcessingStep` above stays the C++ transfer type used by the engine
// API and the project serializer. The authoritative table lives in Rust
// (hirari-core-bridge/src/region_processing_history.rs); C++ hands steps in and
// materializes them back out. The render path never reads it, so unlike the
// note-edit table there is no frozen projection to publish.
class RegionProcessingHistory {
public:
    RegionProcessingHistory()
        : store_(hirari_region_processing_history_new(),
                 [](HirariRegionProcessingHistoryStore* s) {
                     hirari_region_processing_history_clear(s);
                 }) {}

    RegionProcessingHistory(const RegionProcessingHistory&) = default;
    RegionProcessingHistory& operator=(const RegionProcessingHistory&) = default;
    RegionProcessingHistory(RegionProcessingHistory&&) noexcept = default;
    RegionProcessingHistory& operator=(RegionProcessingHistory&&) noexcept = default;

    bool valid() const noexcept { return static_cast<bool>(store_); }

    /// Appends one step. The admissibility policy (id, operation size,
    /// parameter finiteness, 4096-step cap) is owned by the Rust store.
    bool append(const EventProcessingStep& step) const {
        if (!store_ || step.id == 0 || step.operation.empty()) return false;
        HirariEventStep ffi{};
        ffi.id = step.id;
        ffi.operation = reinterpret_cast<const uint8_t*>(step.operation.data());
        ffi.operation_len = step.operation.size();
        ffi.parameter = step.parameter;
        ffi.enabled = step.enabled;
        return hirari_region_processing_history_append(store_.get(), ffi);
    }

    void clear() const {
        if (store_) hirari_region_processing_history_reset(store_.get());
    }

    bool empty() const {
        return hirari_region_processing_history_count(store_.get()) == 0;
    }

    size_t size() const {
        return store_ ? hirari_region_processing_history_count(store_.get()) : 0;
    }

    /// Materializes the table into the C++ transfer type.
    bool copy(std::vector<EventProcessingStep>& out) const {
        out.clear();
        if (!store_) return false;
        size_t count = 0;
        size_t byteTotal = 0;
        if (!hirari_region_processing_history_sizes(store_.get(), &count, &byteTotal)) {
            return false;
        }
        if (count == 0) return true;
        std::vector<HirariEventStepView> views(count);
        std::vector<uint8_t> bytes(byteTotal);
        size_t writtenCount = 0;
        size_t writtenBytes = 0;
        if (!hirari_region_processing_history_materialize(
                store_.get(), views.data(), views.size(), bytes.data(), bytes.size(),
                &writtenCount, &writtenBytes)) {
            return false;
        }
        out.reserve(writtenCount);
        for (size_t index = 0; index < writtenCount; ++index) {
            const auto& view = views[index];
            EventProcessingStep step{};
            step.id = view.id;
            step.parameter = view.parameter;
            step.enabled = view.enabled;
            step.operation.assign(
                reinterpret_cast<const char*>(bytes.data()) + view.operation_offset,
                view.operation_len);
            out.push_back(std::move(step));
        }
        return true;
    }

private:
    std::shared_ptr<HirariRegionProcessingHistoryStore> store_;
};
}
