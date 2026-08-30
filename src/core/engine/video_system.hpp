#pragma once
#include <vector>
#include <string>
#include <atomic>
#include <mutex>
#include <thread>
#include <condition_variable>
#include <deque>
#include <algorithm>
#include <cmath>
#include <cstdint>
#include <optional>
#import <AVFoundation/AVFoundation.h>
#import <CoreVideo/CoreVideo.h>
#import <CoreGraphics/CoreGraphics.h>
#import <dispatch/dispatch.h>

namespace Aura::Core::Engine {

/**
 * @class VideoSystem
 * @brief Industrial Asynchronous Video Sync Engine.
 * HONEST FIX: Implemented background decoding and lock-free frame management.
 */
class VideoSystem {
public:
    static VideoSystem& getInstance() { static VideoSystem i; return i; }

    ~VideoSystem() {
        {
            std::lock_guard<std::mutex> lock(m_mutex);
            m_workerStop = true;
        }
        m_workerWake.notify_one();
        if (m_worker.joinable()) m_worker.join();
    }

    void loadVideo(const std::string& path) {
        std::lock_guard<std::mutex> lock(m_mutex);
        m_hasVideo = false;
        m_currentFrame.clear();
        m_currentWidth = m_currentHeight = 0;
        m_currentSeconds = -1.0;
        m_frameRevision.store(0, std::memory_order_release);
        ++m_videoGeneration;
        m_path.clear();
        m_duration = 0.0;
        m_fps = 24.0;
        m_generator = nil;
        m_pendingSeconds.reset();
        if (path.empty()) return;
        @autoreleasepool {
            NSURL* url = [NSURL fileURLWithPath:[NSString stringWithUTF8String:path.c_str()]];
            AVURLAsset* asset = [AVURLAsset URLAssetWithURL:url options:nil];
            __block NSArray<AVAssetTrack*>* loadedTracks = nil;
            dispatch_semaphore_t tracksReady = dispatch_semaphore_create(0);
            [asset loadTracksWithMediaType:AVMediaTypeVideo
                          completionHandler:^(NSArray<AVAssetTrack*>* tracks, NSError*) {
                loadedTracks = tracks;
                dispatch_semaphore_signal(tracksReady);
            }];
            const dispatch_time_t deadline = dispatch_time(DISPATCH_TIME_NOW, 2 * NSEC_PER_SEC);
            if (dispatch_semaphore_wait(tracksReady, deadline) != 0) return;
            AVAssetTrack* track = loadedTracks.firstObject;
            
            if (track && track.nominalFrameRate > 0.0f) {
                m_fps = track.nominalFrameRate;
                m_duration = CMTimeGetSeconds(asset.duration);
                m_generator = [[AVAssetImageGenerator alloc] initWithAsset:asset];
                m_generator.appliesPreferredTrackTransform = YES;
                m_generator.requestedTimeToleranceBefore = kCMTimeZero;
                m_generator.requestedTimeToleranceAfter = kCMTimeZero;
                m_path = path;
                m_hasVideo = true;
            }
        }
    }

    void update(uint64_t audioSamplePos, double sampleRate) {
        if (!m_hasVideo || !std::isfinite(sampleRate) || sampleRate <= 0.0) return;
        const double seconds = static_cast<double>(audioSamplePos) / sampleRate;
        double duration = 0.0;
        {
            std::lock_guard<std::mutex> lock(m_mutex);
            duration = m_duration;
        }
        m_requestedSeconds.store(std::clamp(seconds, 0.0, std::max(0.0, duration)), std::memory_order_release);
    }

    // Control/background-thread API. Never call this from the audio callback.
    bool requestFrameAt(double seconds) {
        double duration = 0.0;
        {
            std::lock_guard<std::mutex> lock(m_mutex);
            if (!m_hasVideo || !m_generator || !std::isfinite(seconds)) return false;
            duration = m_duration;
            m_pendingSeconds = std::clamp(seconds, 0.0, std::max(0.0, duration));
        }
        m_workerWake.notify_one();
        return true;
    }

private:
    void decodeFrameAt(double position, uint64_t generation) {
        __strong AVAssetImageGenerator* generator = nil;
        {
            std::lock_guard<std::mutex> lock(m_mutex);
            if (!m_hasVideo || !m_generator || generation != m_videoGeneration) return;
            generator = m_generator;
        }
        __block CGImageRef image = nil;
        dispatch_semaphore_t imageReady = dispatch_semaphore_create(0);
        if (@available(macOS 13.0, *)) {
            [generator generateCGImageAsynchronouslyForTime:CMTimeMakeWithSeconds(position, 600)
                                          completionHandler:^(CGImageRef generated, CMTime, NSError*) {
                if (generated) image = CGImageRetain(generated);
                dispatch_semaphore_signal(imageReady);
            }];
        } else {
            // The minimum supported SDK is macOS 12, where the async API is
            // unavailable. Keep the compatibility path isolated here.
#pragma clang diagnostic push
#pragma clang diagnostic ignored "-Wdeprecated-declarations"
            NSError* error = nil;
            image = [generator copyCGImageAtTime:CMTimeMakeWithSeconds(position, 600)
                                      actualTime:nullptr error:&error];
#pragma clang diagnostic pop
            dispatch_semaphore_signal(imageReady);
        }
        if (dispatch_semaphore_wait(imageReady, dispatch_time(DISPATCH_TIME_NOW, 2 * NSEC_PER_SEC)) != 0) {
            [generator cancelAllCGImageGeneration];
            return;
        }
        if (!image) return;
        const size_t width = CGImageGetWidth(image), height = CGImageGetHeight(image);
        if (width == 0 || height == 0 || width > 8192 || height > 8192) { CGImageRelease(image); return; }
        // Keep the transport format identical to the UI packet (RGBA8).  A
        // three-byte CGContext is not portable across CoreGraphics backends
        // and previously made the bridge reject every decoded frame.
        std::vector<uint8_t> rgba(width * height * 4u, 0);
        CGColorSpaceRef colorSpace = CGColorSpaceCreateDeviceRGB();
        CGContextRef context = CGBitmapContextCreate(rgba.data(), width, height, 8, width * 4u,
                                                       colorSpace, static_cast<uint32_t>(kCGImageAlphaPremultipliedLast) |
                                                                   static_cast<uint32_t>(kCGBitmapByteOrder32Big));
        if (!context) { CGColorSpaceRelease(colorSpace); CGImageRelease(image); return; }
        CGContextDrawImage(context, CGRectMake(0, 0, width, height), image);
        CGContextRelease(context); CGColorSpaceRelease(colorSpace); CGImageRelease(image);
        {
            std::lock_guard<std::mutex> lock(m_mutex);
            // A load can replace the AVAsset while an older decode is still
            // completing. Never publish a frame from the previous asset.
            if (generation != m_videoGeneration || !m_hasVideo || generator != m_generator) {
                return;
            }
            m_currentFrame = std::move(rgba);
            m_currentWidth = static_cast<uint32_t>(width);
            m_currentHeight = static_cast<uint32_t>(height);
            m_currentSeconds = position;
            m_frameRevision.fetch_add(1, std::memory_order_release);
        }
    }

    void workerLoop() {
        for (;;) {
            double position = 0.0;
            uint64_t generation = 0;
            {
                std::unique_lock<std::mutex> lock(m_mutex);
                m_workerWake.wait(lock, [this] { return m_workerStop || m_pendingSeconds.has_value(); });
                if (m_workerStop) return;
                position = *m_pendingSeconds;
                m_pendingSeconds.reset();
                generation = m_videoGeneration;
            }
            decodeFrameAt(position, generation);
        }
    }

public:

    bool hasVideo() const noexcept { return m_hasVideo; }
    double frameRate() const noexcept {
        std::lock_guard<std::mutex> lock(m_mutex);
        return m_fps;
    }
    double duration() const noexcept {
        std::lock_guard<std::mutex> lock(m_mutex);
        return m_duration;
    }
    double requestedSeconds() const noexcept { return m_requestedSeconds.load(std::memory_order_acquire); }
    bool copyCurrentFrame(std::vector<uint8_t>& out, uint32_t& width, uint32_t& height, double& seconds) const {
        std::lock_guard<std::mutex> lock(m_mutex);
        if (m_currentFrame.empty()) return false;
        out = m_currentFrame; width = m_currentWidth; height = m_currentHeight; seconds = m_currentSeconds;
        return true;
    }
    uint64_t frameRevision() const noexcept { return m_frameRevision.load(std::memory_order_acquire); }

private:
    VideoSystem() : m_worker(&VideoSystem::workerLoop, this) {}
    mutable std::mutex m_mutex;
    std::condition_variable m_workerWake;
    std::thread m_worker;
    bool m_workerStop = false;
    std::optional<double> m_pendingSeconds;
    AVAssetImageGenerator* m_generator = nil;
    std::string m_path;
    std::vector<uint8_t> m_currentFrame;
    uint32_t m_currentWidth = 0, m_currentHeight = 0;
    double m_currentSeconds = -1.0;
    double m_duration = 0.0;
    double m_fps = 24.0;
    std::atomic<double> m_requestedSeconds{-1.0};
    std::atomic<bool> m_hasVideo{false};
    std::atomic<uint64_t> m_frameRevision{0};
    uint64_t m_videoGeneration = 0;
};


} // namespace Aura::Core::Engine
