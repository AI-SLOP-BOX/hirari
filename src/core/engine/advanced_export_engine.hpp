#include <stdint.h>
#include <vector>
#include <string>
#include <memory>
#include <atomic>
#include <filesystem>
#include <algorithm>
#include <cctype>
#include <utility>
#include <mutex>
#include <set>
#include <limits>
#include "../audio_buffer.hpp"
#include "timeline_system.hpp"
#include "../../io/audio_export_engine.hpp"

namespace Aura::Core::Engine {

/**
 * @struct ExportFormat
 * @brief Configuration for a single export output (e.g., WAV, AIFF, FLAC, MP3).
 */
struct ExportFormat {
    enum class Codec { WAV, AIFF, FLAC, MP3, AAC };
    Codec codec;
    uint32_t bitDepth;
    uint32_t sampleRate;
    bool normalize;
};

/**
 * @class AdvancedExportEngine
 * @brief Industrial multi-format stems and master rendering engine.
 * Provides Ardour-level flexibility for complex project delivery.
 */
class AdvancedExportEngine {
public:
    struct StemJob {
        uint32_t trackId;
        std::string stemName;
        ExportFormat format;
        uint32_t renderTap = 0;
    };

    void queueJob(StemJob job) {
        if (job.trackId == 0 || job.stemName.empty() ||
            job.format.sampleRate < 8000 || job.format.sampleRate > 384000 ||
            (job.format.bitDepth != 16 && job.format.bitDepth != 24 &&
             job.format.bitDepth != 32) || job.renderTap > 2) {
            setError("invalid advanced export job");
            return;
        }
        {
            std::lock_guard<std::mutex> lock(m_jobsMutex);
            m_jobs.push_back(std::move(job));
        }
        setError(std::string{});
    }

    void clearJobs() noexcept {
        {
            std::lock_guard<std::mutex> lock(m_jobsMutex);
            m_jobs.clear();
        }
        setError(std::string{});
        m_completedJobs.store(0, std::memory_order_release);
        m_progress.store(0.0f, std::memory_order_release);
    }

    size_t queuedJobCount() const noexcept {
        std::lock_guard<std::mutex> lock(m_jobsMutex);
        return m_jobs.size();
    }
    size_t completedJobCount() const noexcept {
        return m_completedJobs.load(std::memory_order_acquire);
    }
    float progress() const noexcept {
        return m_progress.load(std::memory_order_acquire);
    }
    bool isCancelled() const noexcept {
        return m_cancelRequested.load(std::memory_order_acquire);
    }
    void cancelExport() noexcept {
        m_cancelRequested.store(true, std::memory_order_release);
    }
    std::string lastError() const {
        std::lock_guard<std::mutex> lock(m_errorMutex);
        return m_lastError;
    }

    /**
     * @brief EXECUTE: Orchestrates the rendering of all registered jobs with industrial precision and export sovereignty.
     */
    void executeExport(const std::string& outputDir) {
        // Preserve the legacy entry point, but make the missing renderer
        // explicit instead of reporting a false success. Call the overload
        // taking TimelineSystem for a real export.
        (void)outputDir;
        setError("timeline renderer is required for advanced export");
    }

    bool executeExport(const std::string& outputDir,
                       TimelineSystem& timeline) {
        std::vector<StemJob> jobs;
        {
            std::lock_guard<std::mutex> lock(m_jobsMutex);
            jobs = m_jobs;
        }
        if (outputDir.empty() || jobs.empty()) {
            setError(outputDir.empty() ? "advanced export output path is empty"
                                       : "no advanced export jobs are queued");
            return false;
        }
        std::error_code error;
        std::filesystem::create_directories(outputDir, error);
        if (error || !std::filesystem::is_directory(outputDir, error) || error) {
            setError("advanced export output directory is unavailable");
            return false;
        }

        const auto outputFor = [&outputDir](const StemJob& job) {
            std::string label = job.stemName;
            for (char& character : label) {
                const bool safe = (character >= 'a' && character <= 'z') ||
                    (character >= 'A' && character <= 'Z') ||
                    (character >= '0' && character <= '9') ||
                    character == '-' || character == '_';
                if (!safe) character = '_';
            }
            return std::filesystem::path(outputDir) /
                (label + "-" + std::to_string(job.trackId) + ".wav");
        };

        // Preflight every job before rendering any output. This prevents a
        // duplicate destination or an already-existing file from leaving a
        // partially published stem set.
        std::set<std::string> destinations;
        for (const auto& job : jobs) {
            if (!timeline.getTrackSnapshot(job.trackId)) {
                setError("advanced export track is missing: " +
                         std::to_string(job.trackId));
                return false;
            }
            if (timeline.getTrackEndSample(job.trackId) == 0) {
                setError("advanced export track has no audio range: " +
                         std::to_string(job.trackId));
                return false;
            }
            const auto destination = outputFor(job);
            if (!destinations.insert(destination.string()).second) {
                setError("advanced export has duplicate output destinations");
                return false;
            }
            if (std::filesystem::exists(destination, error) || error) {
                setError("advanced export output already exists: " +
                         destination.string());
                return false;
            }
        }

        m_cancelRequested.store(false, std::memory_order_release);
        m_progress.store(0.0f, std::memory_order_release);
        m_completedJobs.store(0, std::memory_order_release);
        const float jobCount = static_cast<float>(jobs.size());
        for (size_t jobIndex = 0; jobIndex < jobs.size(); ++jobIndex) {
            const auto& job = jobs[jobIndex];
            if (m_cancelRequested.load(std::memory_order_acquire)) {
                setError("advanced export cancelled");
                return false;
            }
            if (!timeline.getTrackSnapshot(job.trackId)) {
                setError("advanced export track is missing: " +
                         std::to_string(job.trackId));
                return false;
            }
            const uint64_t contentEndSample = timeline.getTrackEndSample(job.trackId);
            uint64_t endSample = contentEndSample;
            const uint64_t tailSamples = timeline.getTrackTailSamples(job.trackId);
            // Keep offline delivery bounded while preserving the complete
            // processor tail (up to the same 30-second safety cap used by
            // the Rust quick-export planner).
            const uint64_t maxTail = static_cast<uint64_t>(job.format.sampleRate) * 30u;
            const uint64_t boundedTail = std::min(tailSamples, maxTail);
            if (endSample > std::numeric_limits<uint64_t>::max() - boundedTail) {
                setError("advanced export tail range overflow");
                return false;
            }
            endSample += boundedTail;
            if (endSample == 0) {
                setError("advanced export track has no audio range: " +
                         std::to_string(job.trackId));
                return false;
            }
            const auto destination = outputFor(job);
            const std::string label = destination.stem().string();
            ::Aura::IO::AudioExportEngine::ExportOptions options;
            options.filename = destination.string();
            options.sampleRate = job.format.sampleRate;
            options.bitDepth = job.format.bitDepth;
            options.endSample = endSample;
            options.channels = 2;
            options.normalize = job.format.normalize;
            options.stemTrackId = job.trackId;
            options.renderTap = job.renderTap;
            options.cancellation = &m_cancelRequested;
            const bool rendered = job.format.codec == ExportFormat::Codec::WAV &&
                ::Aura::IO::AudioExportEngine::bounce(
                    timeline, options, [this, jobIndex, jobCount](float localProgress) {
                        const float bounded = std::clamp(localProgress, 0.0f, 1.0f);
                        const float aggregate =
                            (static_cast<float>(jobIndex) + bounded) / jobCount;
                        m_progress.store(aggregate, std::memory_order_release);
                    });
            if (!rendered) {
                if (m_cancelRequested.load(std::memory_order_acquire)) {
                    setError("advanced export cancelled");
                } else {
                    setError("advanced export failed: " + label);
                }
                return false;
            }
            if (m_cancelRequested.load(std::memory_order_acquire)) {
                setError("advanced export cancelled");
                return false;
            }
            m_completedJobs.fetch_add(1, std::memory_order_release);
        }
        m_progress.store(1.0f, std::memory_order_release);
        setError(std::string{});
        return true;
    }

private:
    std::vector<StemJob> m_jobs;
    mutable std::mutex m_jobsMutex;
    std::atomic<size_t> m_completedJobs{0};
    std::atomic<float> m_progress{0.0f};
    std::atomic<bool> m_cancelRequested{false};
    mutable std::mutex m_errorMutex;
    std::string m_lastError;

    void setError(std::string error) {
        std::lock_guard<std::mutex> lock(m_errorMutex);
        m_lastError = std::move(error);
    }

};

} // namespace Aura::Core::Engine
