#include "../src/core/engine/track.hpp"

#include <array>
#include <cassert>
#include <cmath>
#include <limits>
#include <memory>

int main() {
    using Hirari::Core::AudioBuffer;
    using Hirari::Core::Engine::Track;

    auto audio = std::make_shared<AudioBuffer>(2, 8);
    for (uint32_t i = 0; i < 8; ++i) {
        audio->getWritePointer(0)[i] = 0.25f;
        audio->getWritePointer(1)[i] = 0.25f;
    }
    const std::array<HirariRegionRangeEdit, 3> seed{{
        {0, 100, 0.5f, 999, 999},
        {8, 12, 1.0f, 0, 0},
        {1, 2, std::numeric_limits<float>::quiet_NaN(), 0, 0}}};

    Track track(1, "range-contract", Track::Audio);
    HirariRegionState state{};
    state.len = 8;
    auto* draft = hirari_region_store_draft_new(1, nullptr, 0, nullptr, 0, &state);
    auto* clip = hirari_region_store_draft_clip(draft);
    assert(clip != nullptr);
    assert(hirari_region_clip_edits_replace_range_edits(
        clip, seed.data(), seed.size(), state.len, false));
    assert(hirari_region_store_draft_add(
        draft, &track, new std::shared_ptr<AudioBuffer>(audio)));

    std::array<HirariRegionRangeEdit, 4> read{};
    assert(hirari_region_store_range_edits(&track, 1, read.data(),
                                          read.size()) == 1);
    assert(read[0].end == 8);
    assert(read[0].fade_in == 8);
    assert(read[0].fade_out == 8);

    // Validated re-admission: fades are clamped to the span and a batch whose
    // span leaves the region is refused, leaving the stored list untouched.
    auto* standalone = hirari_region_clip_edits_create();
    const std::array<HirariRegionRangeEdit, 1> fade{{ {0, 4, 1.0f, 99, 99} }};
    assert(hirari_region_clip_edits_replace_range_edits(standalone, fade.data(), fade.size(), 8, true));
    const std::array<HirariRegionRangeEdit, 1> beyond{{ {0, 9, 1.0f, 0, 0} }};
    assert(!hirari_region_clip_edits_replace_range_edits(
        standalone, beyond.data(), beyond.size(), 8, true));
    auto* frozen = hirari_region_clip_edits_freeze(standalone);
    const auto* view = hirari_region_clip_edits_frozen_view(frozen);
    assert(view->rangeEditCount == 1);
    assert(view->rangeEdits->end == 4);
    assert(view->rangeEdits->fade_in == 4 && view->rangeEdits->fade_out == 4);
    hirari_region_clip_edits_frozen_free(frozen);
    hirari_region_clip_edits_destroy(standalone);

    // The store installs what its caller already admitted, so an unbounded fade
    // survives the round trip and a ragged blob is refused.
    const std::array<HirariRegionRangeEdit, 1> replacement{{ {0, 4, 1.0f, 99, 99} }};
    HirariRegionEditPayload payload{};
    payload.trackId = track.getId();
    payload.regionId = 1;
    payload.op = kHirariRegionEditRangeEdits;
    payload.blob = reinterpret_cast<const uint8_t*>(replacement.data());
    payload.blobLen = replacement.size() * sizeof(HirariRegionRangeEdit);
    assert(hirari_region_store_apply_edit(&track, &track, &payload));
    assert(hirari_region_store_range_edits(&track, 1, read.data(),
                                          read.size()) == 1);
    assert(read[0].end == 4 && read[0].fade_in == 99 && read[0].fade_out == 99);

    payload.blobLen = sizeof(HirariRegionRangeEdit) - 1;
    assert(!hirari_region_store_apply_edit(&track, &track, &payload));
    return 0;
}
