#include "src/core/engine/sidechain_manager.hpp"

#include <cassert>
#include <atomic>
#include <cmath>
#include <limits>
#include <thread>

int main() {
    using Aura::Core::Engine::SidechainLink;
    using Aura::Core::Engine::SidechainManager;

    SidechainManager manager;
    float leftA[4] = {1.0f, 2.0f, 3.0f, 4.0f};
    float rightA[4] = {-1.0f, -2.0f, -3.0f, -4.0f};
    float leftB[4] = {5.0f, 6.0f, 7.0f, 8.0f};
    float rightB[4] = {-5.0f, -6.0f, -7.0f, -8.0f};

    assert(!manager.registerLink(10, 0, SidechainLink{}));
    assert(!manager.resolveSidechainLinks());
    assert(manager.takeLegacyApiMisuse());
    assert(!manager.registerLink(10, 0, SidechainLink{
        leftA, rightA, 3, 0.5f,
        Aura::Core::Engine::SidechainTapPoint::PostFX, 4, 0, 1}));
    assert(!manager.registerLink(10, 0, SidechainLink{
        leftA, rightA, 3, 0.5f,
        Aura::Core::Engine::SidechainTapPoint::PostFX, 4, 48000, 0}));
    assert(manager.registerLink(10, 0, SidechainLink{
        leftA, rightA, 3, 0.5f,
        Aura::Core::Engine::SidechainTapPoint::PostFX, 4, 48000, 1}));

    SidechainManager::LinkSnapshot snapshot{};
    float snapshotLeft[4]{};
    float snapshotRight[4]{};
    assert(manager.copySidechainBlock(10, 0, snapshotLeft, snapshotRight, 4, snapshot));
    assert(snapshot.left == snapshotLeft && snapshot.right == snapshotRight);
    assert(snapshot.left[0] == leftA[0] && snapshot.right[3] == rightA[3]);
    assert(snapshot.sourceTrackId == 3);
    assert(std::abs(snapshot.level - 0.5f) < 1.0e-6f);
    assert(snapshot.frames == 4);
    const auto firstGeneration = snapshot.generation;

    // Refresh publishes both channels under one versioned slot update.
    assert(manager.controlRefreshSourceBuffer(3, leftB, rightB, 4, 48000, 1));
    assert(manager.copySidechainBlock(10, 0, snapshotLeft, snapshotRight, 4, snapshot));
    assert(snapshot.left[0] == leftB[0] && snapshot.right[3] == rightB[3]);
    assert(snapshot.frames == 4);
    assert(snapshot.generation > firstGeneration);
    float copiedLeft[4]{};
    float copiedRight[4]{};
    assert(manager.copySidechainBlock(10, 0, copiedLeft, copiedRight, 4, snapshot));
    assert(copiedLeft[0] == leftB[0] && copiedRight[0] == rightB[0]);

    assert(manager.registerLink(11, 1, SidechainLink{
        leftA, rightA, 3, -4.0f,
        Aura::Core::Engine::SidechainTapPoint::PostFX, 4, 48000, 1}));
    SidechainManager::LinkSnapshot clamped{};
    float clampedLeft[4]{};
    float clampedRight[4]{};
    assert(manager.copySidechainBlock(11, 1, clampedLeft, clampedRight, 4, clamped));
    assert(clamped.level == 0.0f);
    assert(!manager.registerLink(12, 1, SidechainLink{
        leftA, rightA, 3, std::numeric_limits<float>::quiet_NaN(),
        Aura::Core::Engine::SidechainTapPoint::PostFX, 4}));

    assert(manager.hasLink(10, 0, 3));
    assert(manager.removeLink(10, 0));
    assert(!manager.copySidechainBlock(10, 0, snapshotLeft, snapshotRight, 4, snapshot));
    assert(!manager.hasLink(10, 0, 3));

    // Owned publication path: the manager copies the block into fixed
    // double-buffered storage, so the published pointers do not depend on
    // the caller's temporary arrays remaining allocated or writable.
    SidechainManager ownedManager;
    float ownedLeft[4] = {9.0f, 8.0f, 7.0f, 6.0f};
    float ownedRight[4] = {-9.0f, -8.0f, -7.0f, -6.0f};
    assert(ownedManager.registerLink(20, 0,
        SidechainLink{ownedLeft, ownedRight, 4, 1.0f,
                      Aura::Core::Engine::SidechainTapPoint::PostFX, 4, 48000, 1}));
    SidechainManager::LinkSnapshot ownedSnapshot{};
    float ownedCopyLeft[4]{};
    float ownedCopyRight[4]{};
    assert(ownedManager.copySidechainBlock(20, 0, ownedCopyLeft, ownedCopyRight, 4, ownedSnapshot));
    assert(ownedSnapshot.left[0] == 9.0f && ownedSnapshot.right[3] == -6.0f);

    float nextLeft[4] = {1.0f, 1.0f, 1.0f, 1.0f};
    float nextRight[4] = {-1.0f, -1.0f, -1.0f, -1.0f};
    assert(ownedManager.controlRefreshSourceBuffer(4, nextLeft, nextRight, 4, 48000, 1));
    assert(ownedManager.copySidechainBlock(20, 0, ownedCopyLeft, ownedCopyRight, 4, ownedSnapshot));
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
            SidechainManager::LinkSnapshot snapshot{};
            if (!ownedManager.copySidechainBlock(30, 0, left, right, 16, snapshot)) continue;
            const float expected = snapshot.frames == 8 ? 10.0f : 20.0f;
            for (uint32_t frame = 0; frame < snapshot.frames; ++frame) {
                if (left[frame] != expected || right[frame] != -expected) {
                    coherent.store(false, std::memory_order_release);
                    return;
                }
            }
        }
    });
    std::thread writerA([&] {
        for (int i = 0; i < 100; ++i) {
            assert(ownedManager.registerLink(30, 0,
                SidechainLink{writerAL, writerAR, 30, 1.0f,
                              Aura::Core::Engine::SidechainTapPoint::PostFX, 8, 48000, 1}));
        }
    });
    std::thread writerB([&] {
        for (int i = 0; i < 100; ++i) {
            assert(ownedManager.registerLink(30, 0,
                SidechainLink{writerBL, writerBR, 31, 0.75f,
                              Aura::Core::Engine::SidechainTapPoint::PostFX, 16, 48000, 1}));
        }
    });
    writerA.join();
    writerB.join();
    reader.join();
    assert(coherent.load(std::memory_order_acquire));
    SidechainManager::LinkSnapshot raced{};
    float racedLeft[16]{};
    float racedRight[16]{};
    assert(ownedManager.copySidechainBlock(30, 0, racedLeft, racedRight, 16, raced));
    assert(raced.frames == 8 || raced.frames == 16);
    assert(raced.generation > 0);
    return 0;
}
