#include "src/core/rust_ffi.hpp"

#include <atomic>
#include <bit>
#include <cassert>
#include <cmath>
#include <cstdint>
#include <limits>
#include <thread>

namespace {

constexpr uint8_t kTapPostFx = 1;

struct Snapshot {
    const float* left = nullptr;
    const float* right = nullptr;
    uint32_t sourceTrackId = 0;
    float level = 1.0f;
    uint8_t tapPoint = kTapPostFx;
    uint32_t frames = 0;
    uint32_t sampleRate = 0;
    uint64_t sourceGeneration = 0;
    uint64_t generation = 0;
};

bool registerLink(void* engine, uint32_t destination, uint32_t plugin,
                  const float* left, const float* right, uint32_t frames,
                  uint32_t sampleRate, uint64_t generation, float level) {
    return hirari_sidechain_engine_register_link(
        engine, destination, plugin, 3, left, right, frames, sampleRate,
        generation, level, kTapPostFx);
}

bool copyBlock(void* engine, uint32_t destination, uint32_t plugin,
               float* left, float* right, uint32_t capacity, Snapshot& snapshot) {
    uint64_t values[6]{};
    const uint32_t frames = hirari_sidechain_engine_copy_link(
        engine, destination, plugin, left, right, capacity, values);
    if (frames == 0) return false;
    snapshot.left = left;
    snapshot.right = right;
    snapshot.sourceTrackId = static_cast<uint32_t>(values[0]);
    snapshot.level = std::bit_cast<float>(static_cast<uint32_t>(values[1]));
    snapshot.tapPoint = static_cast<uint8_t>(values[2]);
    snapshot.sampleRate = static_cast<uint32_t>(values[3]);
    snapshot.sourceGeneration = values[4];
    snapshot.generation = values[5];
    snapshot.frames = frames;
    return true;
}

}  // namespace

int main() {
    void* engine = hirari_sidechain_engine_create();
    assert(engine != nullptr);
    float leftA[4] = {1.0f, 2.0f, 3.0f, 4.0f};
    float rightA[4] = {-1.0f, -2.0f, -3.0f, -4.0f};
    float leftB[4] = {5.0f, 6.0f, 7.0f, 8.0f};
    float rightB[4] = {-5.0f, -6.0f, -7.0f, -8.0f};

    assert(!registerLink(engine, 10, 0, nullptr, nullptr, 0, 0, 0, 1.0f));
    assert(!hirari_sidechain_engine_resolve_links(engine));
    assert(hirari_sidechain_engine_take_legacy_misuse(engine));
    assert(!hirari_sidechain_engine_take_legacy_misuse(engine));
    assert(!registerLink(engine, 10, 0, leftA, rightA, 4, 48000, 0, 0.5f));
    assert(!registerLink(engine, 10, 0, leftA, rightA, 4, 0, 1, 0.5f));
    assert(registerLink(engine, 10, 0, leftA, rightA, 4, 48000, 1, 0.5f));

    Snapshot snapshot{};
    float snapshotLeft[4]{};
    float snapshotRight[4]{};
    assert(copyBlock(engine, 10, 0, snapshotLeft, snapshotRight, 4, snapshot));
    assert(snapshot.left == snapshotLeft && snapshot.right == snapshotRight);
    assert(snapshot.left[0] == leftA[0] && snapshot.right[3] == rightA[3]);
    assert(snapshot.sourceTrackId == 3);
    assert(std::abs(snapshot.level - 0.5f) < 1.0e-6f);
    assert(snapshot.frames == 4);
    const auto firstGeneration = snapshot.generation;

    // Refresh publishes both channels under one versioned slot update.
    assert(hirari_sidechain_engine_refresh_source(engine, 3, leftB, rightB, 4, 48000, 1));
    assert(copyBlock(engine, 10, 0, snapshotLeft, snapshotRight, 4, snapshot));
    assert(snapshot.left[0] == leftB[0] && snapshot.right[3] == rightB[3]);
    assert(snapshot.frames == 4);
    assert(snapshot.generation > firstGeneration);

    assert(registerLink(engine, 11, 1, leftA, rightA, 4, 48000, 1, -4.0f));
    Snapshot clamped{};
    float clampedLeft[4]{};
    float clampedRight[4]{};
    assert(copyBlock(engine, 11, 1, clampedLeft, clampedRight, 4, clamped));
    assert(clamped.level == 0.0f);
    assert(!registerLink(engine, 12, 1, leftA, rightA, 4, 48000, 1,
                         std::numeric_limits<float>::quiet_NaN()));

    assert(hirari_sidechain_engine_has_link(engine, 10, 0, 3));
    assert(hirari_sidechain_engine_remove_link(engine, 10, 0));
    assert(!copyBlock(engine, 10, 0, snapshotLeft, snapshotRight, 4, snapshot));
    assert(!hirari_sidechain_engine_has_link(engine, 10, 0, 3));

    // Capture/restore: the undone-track path hands the snapshot back as an
    // opaque handle and re-registers every captured link.
    void* captured = hirari_sidechain_engine_capture_links(engine, 3);
    assert(captured != nullptr);
    assert(hirari_sidechain_engine_remove_link(engine, 11, 1));
    assert(!hirari_sidechain_engine_has_link(engine, 11, 1, 3));
    assert(hirari_sidechain_engine_restore_links(engine, captured));
    assert(hirari_sidechain_engine_has_link(engine, 11, 1, 3));
    hirari_sidechain_engine_free_captured_links(captured);

    // Owned publication path: the engine copies the block into fixed
    // double-buffered storage, so the published pointers do not depend on
    // the caller's temporary arrays remaining allocated or writable.
    void* ownedEngine = hirari_sidechain_engine_create();
    assert(ownedEngine != nullptr);
    float ownedLeft[4] = {9.0f, 8.0f, 7.0f, 6.0f};
    float ownedRight[4] = {-9.0f, -8.0f, -7.0f, -6.0f};
    assert(registerLink(ownedEngine, 20, 0, ownedLeft, ownedRight, 4, 48000, 1, 1.0f));
    Snapshot ownedSnapshot{};
    float ownedCopyLeft[4]{};
    float ownedCopyRight[4]{};
    assert(copyBlock(ownedEngine, 20, 0, ownedCopyLeft, ownedCopyRight, 4, ownedSnapshot));
    assert(ownedSnapshot.left[0] == 9.0f && ownedSnapshot.right[3] == -6.0f);

    float nextLeft[4] = {1.0f, 1.0f, 1.0f, 1.0f};
    float nextRight[4] = {-1.0f, -1.0f, -1.0f, -1.0f};
    assert(hirari_sidechain_engine_refresh_source(ownedEngine, 3, nextLeft, nextRight, 4, 48000, 1));
    assert(copyBlock(ownedEngine, 20, 0, ownedCopyLeft, ownedCopyRight, 4, ownedSnapshot));
    assert(ownedSnapshot.left[0] == 1.0f && ownedSnapshot.right[0] == -1.0f);

    // Seqlock publication is single-writer by contract. The control writer
    // mutex makes that contract true even when two control clients race.
    float writerAL[8]{};
    float writerAR[8]{};
    float writerBL[16]{};
    float writerBR[16]{};
    for (float& sample : writerAL) sample = 10.0f;
    for (float& sample : writerAR) sample = -10.0f;
    for (float& sample : writerBL) sample = 20.0f;
    for (float& sample : writerBR) sample = -20.0f;
    std::atomic<bool> coherent{true};
    std::thread reader([&] {
        float left[16]{};
        float right[16]{};
        for (int i = 0; i < 2000; ++i) {
            Snapshot current{};
            if (!copyBlock(ownedEngine, 30, 0, left, right, 16, current)) continue;
            const float expected = current.frames == 8 ? 10.0f : 20.0f;
            for (uint32_t frame = 0; frame < current.frames; ++frame) {
                if (left[frame] != expected || right[frame] != -expected) {
                    coherent.store(false, std::memory_order_release);
                    return;
                }
            }
        }
    });
    std::thread writerA([&] {
        for (int i = 0; i < 100; ++i) {
            assert(registerLink(ownedEngine, 30, 0, writerAL, writerAR, 8, 48000, 1, 1.0f));
        }
    });
    std::thread writerB([&] {
        for (int i = 0; i < 100; ++i) {
            assert(registerLink(ownedEngine, 30, 0, writerBL, writerBR, 16, 48000, 1, 0.75f));
        }
    });
    writerA.join();
    writerB.join();
    reader.join();
    assert(coherent.load(std::memory_order_acquire));
    Snapshot raced{};
    float racedLeft[16]{};
    float racedRight[16]{};
    assert(copyBlock(ownedEngine, 30, 0, racedLeft, racedRight, 16, raced));
    assert(raced.frames == 8 || raced.frames == 16);
    assert(raced.generation > 0);

    hirari_sidechain_engine_destroy(ownedEngine);
    hirari_sidechain_engine_destroy(engine);
    return 0;
}
