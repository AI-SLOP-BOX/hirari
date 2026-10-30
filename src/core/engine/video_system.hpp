#pragma once

#include <algorithm>
#include <cmath>
#include <cstdint>
#include <memory>
#include <mutex>
#include <string>
#include <vector>
#include "../rust_ffi.hpp"
#import <AVFoundation/AVFoundation.h>
#import <CoreVideo/CoreVideo.h>
#import <CoreGraphics/CoreGraphics.h>
#import <dispatch/dispatch.h>

namespace Hirari::Core::Engine {

/** AVFoundation adapter for the Rust-owned video request and frame worker. */
class VideoSystem {
public:
    static VideoSystem& getInstance() { static VideoSystem instance; return instance; }

    ~VideoSystem() { hirari_video_runtime_destroy(m_runtime); }

    void loadVideo(const std::string& path) {
        const uint64_t generation = hirari_video_runtime_clear(m_runtime);
        std::lock_guard<std::mutex> lock(m_nativeMutex);
        m_nativeGeneration = generation;
        m_nativeHasVideo = false;
        m_generator = nil;
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
            if (dispatch_semaphore_wait(
                    tracksReady, dispatch_time(DISPATCH_TIME_NOW, 2 * NSEC_PER_SEC)) != 0) return;
            AVAssetTrack* track = loadedTracks.firstObject;
            if (!track || !(track.nominalFrameRate > 0.0f)) return;
            const double fps = track.nominalFrameRate;
            const double duration = CMTimeGetSeconds(asset.duration);
            AVAssetImageGenerator* generator = [[AVAssetImageGenerator alloc] initWithAsset:asset];
            generator.appliesPreferredTrackTransform = YES;
            generator.requestedTimeToleranceBefore = kCMTimeZero;
            generator.requestedTimeToleranceAfter = kCMTimeZero;
            m_generator = generator;
            m_nativeHasVideo = hirari_video_runtime_set_metadata(m_runtime, duration, fps);
        }
    }

    void update(uint64_t audioSamplePos, double sampleRate) {
        hirari_video_runtime_update(m_runtime, audioSamplePos, sampleRate);
    }

    bool requestFrameAt(double seconds) {
        return hirari_video_runtime_request_frame(m_runtime, seconds);
    }

    bool hasVideo() const noexcept { return hirari_video_runtime_has_video(m_runtime); }
    double frameRate() const noexcept { return hirari_video_runtime_fps(m_runtime); }
    double duration() const noexcept { return hirari_video_runtime_duration(m_runtime); }
    uint64_t frameRevision() const noexcept { return hirari_video_runtime_revision(m_runtime); }

    bool copyCurrentFrame(std::vector<uint8_t>& out, uint32_t& width,
                          uint32_t& height, double& seconds) const {
        struct CopyContext { std::vector<uint8_t>* output; } context{&out};
        return hirari_video_runtime_copy_frame(
            m_runtime, &context,
            +[](void* opaque, const uint8_t* bytes, size_t size) noexcept {
                auto* context = static_cast<CopyContext*>(opaque);
                if (!context || !context->output || (!bytes && size != 0)) return;
                try { context->output->assign(bytes, bytes + size); }
                catch (...) { context->output->clear(); }
            }, &width, &height, &seconds);
    }

private:
    struct NativeFrame {
        std::vector<uint8_t> rgba;
        uint32_t width = 0;
        uint32_t height = 0;
        double seconds = 0.0;
    };

    static bool decodeNativeFrame(void* opaque, double position, uint64_t generation,
                                  void** output) noexcept {
        auto* self = static_cast<VideoSystem*>(opaque);
        if (!self || !output) return false;
        __strong AVAssetImageGenerator* generator = nil;
        {
            std::lock_guard<std::mutex> lock(self->m_nativeMutex);
            if (!self->m_nativeHasVideo || generation != self->m_nativeGeneration ||
                !self->m_generator) return false;
            generator = self->m_generator;
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
            return false;
        }
        if (!image) return false;
        const size_t width = CGImageGetWidth(image), height = CGImageGetHeight(image);
        if (width == 0 || height == 0 || width > 8192 || height > 8192) {
            CGImageRelease(image);
            return false;
        }
        auto frame = std::make_unique<NativeFrame>();
        frame->width = static_cast<uint32_t>(width);
        frame->height = static_cast<uint32_t>(height);
        frame->seconds = position;
        frame->rgba.resize(width * height * 4u, 0);
        CGColorSpaceRef colorSpace = CGColorSpaceCreateDeviceRGB();
        CGContextRef context = CGBitmapContextCreate(
            frame->rgba.data(), width, height, 8, width * 4u, colorSpace,
            static_cast<uint32_t>(kCGImageAlphaPremultipliedLast) |
            static_cast<uint32_t>(kCGBitmapByteOrder32Big));
        if (!context) {
            CGColorSpaceRelease(colorSpace);
            CGImageRelease(image);
            return false;
        }
        CGContextDrawImage(context, CGRectMake(0, 0, width, height), image);
        CGContextRelease(context);
        CGColorSpaceRelease(colorSpace);
        CGImageRelease(image);
        {
            std::lock_guard<std::mutex> lock(self->m_nativeMutex);
            if (!self->m_nativeHasVideo || generation != self->m_nativeGeneration ||
                generator != self->m_generator) return false;
        }
        *output = frame.release();
        return true;
    }

    static bool readNativeFrame(void* opaque, uint32_t* width, uint32_t* height,
                                double* seconds, const uint8_t** pixels,
                                size_t* size) noexcept {
        const auto* frame = static_cast<const NativeFrame*>(opaque);
        if (!frame || !width || !height || !seconds || !pixels || !size) return false;
        *width = frame->width;
        *height = frame->height;
        *seconds = frame->seconds;
        *pixels = frame->rgba.data();
        *size = frame->rgba.size();
        return true;
    }

    static void destroyNativeFrame(void* opaque) noexcept {
        delete static_cast<NativeFrame*>(opaque);
    }

    VideoSystem() : m_runtime(hirari_video_runtime_create(
        this, &decodeNativeFrame, &readNativeFrame, &destroyNativeFrame)) {}

    void* m_runtime = nullptr;
    mutable std::mutex m_nativeMutex;
    AVAssetImageGenerator* m_generator = nil;
    bool m_nativeHasVideo = false;
    uint64_t m_nativeGeneration = 0;
};

} // namespace Hirari::Core::Engine
