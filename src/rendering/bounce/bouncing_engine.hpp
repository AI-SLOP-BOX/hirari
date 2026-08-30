#pragma once
#include <string>
#include <vector>
#include <future>
#include <iostream>
#include <filesystem>
#include <limits>
#include <cmath>
#include <chrono>
#include <thread>
#include <atomic>
#include <memory>
#if !defined(_WIN32)
#include <fcntl.h>
#include <unistd.h>
#endif
#include "../../core/engine/timeline_system.hpp"
#include "../../io/wav_loader_utils.hpp"
#include "../../core/status_queue.hpp"
#include "../../core/concurrency/thread_pool.hpp"

namespace Aura::Core::Engine {

/**
 * @brief BouncingEngine: High-speed offline rendering for "Bounce-In-Place."
 */
class BouncingEngine {
public:
    enum class OutputFormat { WAV, WAVE64 };

    static BouncingEngine& getInstance() {
        static BouncingEngine instance;
        return instance;
    }

    /**
     * @brief Renders a track's audio output into a local file.
     */
    std::future<bool> bounceInPlace(uint32_t trackId, const std::string& destinationPath,
                                    OutputFormat format = OutputFormat::WAV) {
        return Aura::Core::Concurrency::ThreadPool::getInstance().enqueue([this, trackId, destinationPath, format]() {
            return this->performInternalRender(TimelineSystem::getInstance(), trackId, destinationPath, format);
        });
    }

    // Session-owned entry point. New callers must pass their timeline rather
    // than sharing the process compatibility singleton across projects.
    std::future<bool> bounceInPlace(TimelineSystem& timeline, uint32_t trackId,
                                    const std::string& destinationPath,
                                    OutputFormat format = OutputFormat::WAV) {
        return Aura::Core::Concurrency::ThreadPool::getInstance().enqueue([this, &timeline, trackId, destinationPath, format]() {
            return this->performInternalRender(timeline, trackId, destinationPath, format);
        });
    }

private:
    BouncingEngine() = default;

    bool performInternalRender(TimelineSystem& timeline, uint32_t trackId,
                               const std::string& path, OutputFormat format) {
        std::filesystem::path temporary;
        try {
            if (path.empty()) return false;
            const std::filesystem::path destination(path);
            if (destination.filename().empty()) return false;
            static std::atomic<uint64_t> publishSequence{0};
            const auto sequence = publishSequence.fetch_add(1, std::memory_order_relaxed);
#if defined(_WIN32)
            const auto processToken = static_cast<unsigned long long>(0);
#else
            const auto processToken = static_cast<unsigned long long>(::getpid());
#endif
            temporary = destination.string() +
                ".tmp-bounce-" + std::to_string(processToken) +
                "-" + std::to_string(sequence);
            uint32_t numChannels = 2;
            uint32_t blockSize = 1024;
            const double sampleRate = timeline.getSampleRate();
            if (!std::isfinite(sampleRate) || sampleRate <= 0.0 || sampleRate > 384000.0) {
                ::Aura::Core::StatusQueue::getInstance().pushFromAudio(
                    StatusQueue::Severity::Error, "Bounce-In-Place: invalid project sample rate.");
                return false;
            }
            
            // Bounce the actual occupied span of the requested track.
            const uint64_t totalSamples = timeline.getTrackEndSample(trackId);
            if (totalSamples == 0) {
                ::Aura::Core::StatusQueue::getInstance().pushFromAudio(
                    StatusQueue::Severity::Error, "Bounce-In-Place: track has no audio regions.");
                return false;
            }
            if (totalSamples > static_cast<uint64_t>(std::numeric_limits<size_t>::max())) {
                return false;
            }
            // The normal WAV path is streamed directly to the durable writer;
            // only WAVE64 keeps a bounded-by-project float buffer until the
            // dedicated float32 streaming writer is introduced.
            std::unique_ptr<::Aura::IO::Persistence::WavWriter::Pcm16StreamWriter> pcmStream;
            std::vector<std::vector<float>> exportBuffer;
            if (format == OutputFormat::WAV) {
                pcmStream = std::make_unique<
                    ::Aura::IO::Persistence::WavWriter::Pcm16StreamWriter>(
                        destination.string(), totalSamples, static_cast<uint32_t>(sampleRate));
                if (!pcmStream->isOpen()) {
                    ::Aura::Core::StatusQueue::getInstance().pushFromAudio(
                        StatusQueue::Severity::Error,
                        "Bounce-In-Place: unable to open streaming WAV output.");
                    return false;
                }
            } else {
                // WavSaver promotes large exports to RF64/WAVE64; keep only
                // the host allocation guard here instead of rejecting valid
                // large files.
                exportBuffer.assign(
                    numChannels, std::vector<float>(static_cast<size_t>(totalSamples), 0.0f));
            }
            AudioBuffer renderedBlock(numChannels, blockSize);

            ::Aura::Core::StatusQueue::getInstance().pushFromAudio(StatusQueue::Severity::Info, "Starting Bounce-In-Place...");

            for (uint64_t pos = 0; pos < totalSamples; pos += blockSize) {
                uint32_t frameCount = static_cast<uint32_t>(std::min<uint64_t>(blockSize, totalSamples - pos));
                
                if (!timeline.renderTrackInto(trackId, renderedBlock, frameCount, pos)) {
                    ::Aura::Core::StatusQueue::getInstance().pushFromAudio(
                        StatusQueue::Severity::Error,
                        "Bounce-In-Place: track render failed.");
                    return false;
                }

                const float* left = renderedBlock.getReadPointer(0);
                const float* right = renderedBlock.getReadPointer(1);
                // A mono processor is a valid bounce source. Preserve the
                // canonical L/R export layout without dereferencing a
                // missing channel; mono is duplicated to the right channel.
                if (!left) return false;
                if (!right) right = left;
                if (format == OutputFormat::WAV) {
                    if (!pcmStream->writeFrames(left, right, frameCount)) {
                        ::Aura::Core::StatusQueue::getInstance().pushFromAudio(
                            StatusQueue::Severity::Error,
                            "Bounce-In-Place: streaming WAV write failed.");
                        return false;
                    }
                } else {
                    std::copy(left, left + frameCount,
                              exportBuffer[0].begin() + static_cast<size_t>(pos));
                    std::copy(right, right + frameCount,
                              exportBuffer[1].begin() + static_cast<size_t>(pos));
                }
            }

            bool success = format == OutputFormat::WAV
                ? pcmStream->finish()
                : ::Aura::IO::WavSaver::saveWave64(
                    temporary.string(), exportBuffer, static_cast<uint32_t>(sampleRate));
            if (success && format == OutputFormat::WAVE64) {
                std::error_code publishError;
                std::filesystem::rename(temporary, destination, publishError);
                success = !publishError;
#if !defined(_WIN32)
                if (success) {
                    const auto parent = destination.parent_path().empty()
                        ? std::filesystem::path(".") : destination.parent_path();
                    const int directory = ::open(parent.c_str(), O_RDONLY | O_DIRECTORY);
                    if (directory < 0 || ::fsync(directory) != 0) {
                        if (directory >= 0) ::close(directory);
                        success = false;
                    } else {
                        ::close(directory);
                    }
                }
#endif
            }
            if (!success) {
                std::error_code cleanupError;
                std::filesystem::remove(temporary, cleanupError);
            }
            
            if (success) {
                ::Aura::Core::StatusQueue::getInstance().pushFromAudio(StatusQueue::Severity::Info, "Bounce-In-Place Completed: " + path);
            } else {
                ::Aura::Core::StatusQueue::getInstance().pushFromAudio(StatusQueue::Severity::Error, "Export Failed.");
            }

            return success;
        } catch (const std::exception& e) {
            if (!temporary.empty()) {
                std::error_code cleanupError;
                std::filesystem::remove(temporary, cleanupError);
            }
            ::Aura::Core::StatusQueue::getInstance().pushFromAudio(
                StatusQueue::Severity::Error,
                std::string("Bounce Exception: ") + e.what());
            return false;
        } catch (...) {
            if (!temporary.empty()) {
                std::error_code cleanupError;
                std::filesystem::remove(temporary, cleanupError);
            }
            ::Aura::Core::StatusQueue::getInstance().pushFromAudio(
                StatusQueue::Severity::Error, "Bounce Exception: unknown failure");
            return false;
        }
    }
};

} // namespace Aura::Core::Engine
