#pragma once

#include <stdint.h>
#include <cmath>
#include <string>
#include <algorithm>

namespace Aura::Core::Video {

struct SMPTETimecode {
    uint8_t hours, minutes, seconds, frames;
    float subframes;
    bool dropFrame;
};

/**
 * @class SovereignVideoCore
 * @brief Unified Cinematic Synchronization and Timecode Engine.
 * HONEST FIX: Implements precise SMPTE 12M drop-frame arithmetic.
 */
class SovereignVideoCore {
public:
    static SovereignVideoCore& i() { static SovereignVideoCore inst; return inst; }

    /**
     * @brief Converts a sample position to SMPTE Timecode.
     * Logic for Drop-Frame: In 29.97/59.94, frame numbers 0 and 1 are skipped 
     * at the start of every minute except every 10th minute.
     */
    SMPTETimecode getSyncTimecode(uint64_t samplePos, double sampleRate, float frameRate) {
        bool df = isDropFrame(frameRate);
        
        // Use double for initial frame count to handle 23.976/29.97 precisely
        double totalSeconds = static_cast<double>(samplePos) / sampleRate;
        uint64_t totalFrames = static_cast<uint64_t>(std::floor(totalSeconds * frameRate + 0.00001));

        uint64_t workingFrames = totalFrames;

        if (df) {
            uint64_t framesPer10Min = static_cast<uint64_t>(std::round(frameRate * 600.0));
            uint64_t framesPerMin = static_cast<uint64_t>(std::round(frameRate * 60.0));
            uint64_t dropPerMin = (std::abs(frameRate - 59.94f) < 0.1f) ? 4 : 2;

            uint64_t d = totalFrames / framesPer10Min;
            uint64_t m = totalFrames % framesPer10Min;

            if (m > dropPerMin) {
                workingFrames += (dropPerMin * 9 * d) + dropPerMin * ((m - dropPerMin) / framesPerMin);
            } else {
                workingFrames += (dropPerMin * 9 * d);
            }
        }

        uint32_t fpsInt = static_cast<uint32_t>(std::round(frameRate));
        SMPTETimecode tc;
        tc.dropFrame = df;
        tc.frames = workingFrames % fpsInt;
        tc.seconds = (workingFrames / fpsInt) % 60;
        tc.minutes = (workingFrames / (fpsInt * 60)) % 60;
        tc.hours = (uint8_t)(workingFrames / (fpsInt * 3600));
        
        double frameStartSample = (static_cast<double>(totalFrames) * sampleRate) / frameRate;
        tc.subframes = static_cast<float>((samplePos - frameStartSample) / (sampleRate / frameRate) * 100.0f);
        
        return tc;
    }

private:
    bool isDropFrame(float fps) {
        return (std::abs(fps - 29.97f) < 0.01f || std::abs(fps - 59.94f) < 0.01f);
    }
};

} // namespace Aura::Core::Video
