#import <AudioToolbox/AudioToolbox.h>
#import <AudioUnit/AudioUnit.h>
#include <iostream>
#include "../AuraUltimate.hpp"

namespace Aura::Driver {

/**
 * @class macOSDriver
 * @brief THE HEARTBEAT: Low-latency Core Audio (AUHAL) direct hardware interface.
 * SOLVES: "Empty Engine" and "Lack of Core Audio Driver" from the audit.
 */
class macOSDriver {
public:
    static macOSDriver& getInstance() { static macOSDriver i; return i; }

    bool initialize(double sr, uint32_t bs) {
        m_sampleRate = sr;
        m_blockSize = bs;

        AudioComponentDescription desc;
        desc.componentType = kAudioUnitType_Output;
        desc.componentSubType = kAudioUnitSubType_DefaultOutput;
        desc.componentManufacturer = kAudioUnitManufacturer_Apple;
        desc.componentFlags = 0;
        desc.componentFlagsMask = 0;

        AudioComponent comp = AudioComponentFindNext(NULL, &desc);
        if (!comp) {
            std::cerr << "AURA | DRIVER | ERROR: Could not find Default Output Component." << std::endl;
            return false;
        }

        OSStatus err = AudioComponentInstanceNew(comp, &m_audioUnit);
        if (err != noErr) {
            std::cerr << "AURA | DRIVER | ERROR: AudioComponentInstanceNew failed (" << err << ")" << std::endl;
            return false;
        }

        err = AudioUnitInitialize(m_audioUnit);
        if (err != noErr) {
            std::cerr << "AURA | DRIVER | ERROR: AudioUnitInitialize failed (" << err << ")" << std::endl;
            return false;
        }

        // --- SET RENDER CALLBACK ---
        AURenderCallbackStruct input;
        input.inputProc = RenderCallback;
        input.inputProcRefCon = this;
        err = AudioUnitSetProperty(m_audioUnit, kAudioUnitProperty_SetRenderCallback, kAudioUnitScope_Input, 0, &input, sizeof(input));
        if (err != noErr) {
            std::cerr << "AURA | DRIVER | ERROR: Could not set render callback (" << err << ")" << std::endl;
            return false;
        }

        // --- Stream Format (Float32, Stereo) ---
        AudioStreamBasicDescription format;
        format.mSampleRate = sr;
        format.mFormatID = kAudioFormatLinearPCM;
        format.mFormatFlags = kAudioFormatFlagIsFloat | kAudioFormatFlagIsPacked | kAudioFormatFlagIsNonInterleaved;
        format.mBytesPerPacket = 4;
        format.mFramesPerPacket = 1;
        format.mBytesPerFrame = 4;
        format.mChannelsPerFrame = 2;
        format.mBitsPerChannel = 32;
        err = AudioUnitSetProperty(m_audioUnit, kAudioUnitProperty_StreamFormat, kAudioUnitScope_Input, 0, &format, sizeof(format));
        if (err != noErr) {
            std::cerr << "AURA | DRIVER | ERROR: Could not set stream format (" << err << ")" << std::endl;
            return false;
        }

        err = AudioOutputUnitStart(m_audioUnit);
        if (err != noErr) {
            std::cerr << "AURA | DRIVER | ERROR: AudioOutputUnitStart failed (" << err << ")" << std::endl;
            return false;
        }

        std::cout << "AURA | DRIVER | SUCCESS: Core Audio Started (" << sr << "Hz / " << bs << " samples)" << std::endl;
        return true;
    }

private:
    static OSStatus RenderCallback(void *inRefCon, AudioUnitRenderActionFlags *ioActionFlags, 
                                   const AudioTimeStamp *inTimeStamp, UInt32 inBusNumber, 
                                   UInt32 inNumberFrames, AudioBufferList *ioData) {
        auto* driver = static_cast<macOSDriver*>(inRefCon);
        float* outL = static_cast<float*>(ioData->mBuffers[0].mData);
        float* outR = static_cast<float*>(ioData->mBuffers[1].mData);

        // --- REAL-TIME THREAD: ZERO-ALLOCATION PATH ---
        ::Aura::AuraEngine::getInstance().process(outL, outR, inNumberFrames);
        return noErr;
    }

    AudioUnit m_audioUnit;
    double m_sampleRate;
    uint32_t m_blockSize;
};

} // namespace Aura::Driver
