#pragma once

#if defined(__APPLE__)

#include <AudioToolbox/AudioToolbox.h>
#include <AudioUnit/AudioUnit.h>
#include <CoreFoundation/CoreFoundation.h>

#include <algorithm>
#include <cmath>
#include <cstdint>
#include <cstring>
#include <limits>
#include <string>
#include <vector>

#include "plugin_sandbox_protocol.hpp"

namespace Aura::Core::Plugins::SandboxAU {

class Runtime final {
public:
    static constexpr size_t kMaxStateBytes = 4u * 1024u * 1024u;

    Runtime() = default;
    Runtime(const Runtime&) = delete;
    Runtime& operator=(const Runtime&) = delete;
    ~Runtime() { shutdown(); }

    bool load(const char* path, double sampleRate, uint32_t maxFrames,
              uint32_t channels = SandboxProtocol::kMaxChannels) noexcept {
        shutdown();
        if (!path || *path == '\0' || !std::isfinite(sampleRate) || sampleRate <= 0.0 || maxFrames == 0 ||
            channels == 0 || channels > SandboxProtocol::kMaxChannels)
            return false;

        struct LoadCleanup final {
            Runtime* runtime;
            bool committed = false;
            ~LoadCleanup() {
                if (!committed) runtime->shutdown();
            }
        } cleanup{this};

        CFURLRef url = CFURLCreateFromFileSystemRepresentation(
            kCFAllocatorDefault, reinterpret_cast<const UInt8*>(path), std::strlen(path), true);
        if (!url) return false;
        m_bundle = CFBundleCreate(kCFAllocatorDefault, url);
        CFRelease(url);
        if (!m_bundle || !CFBundleLoadExecutable(m_bundle)) return false;

        m_sampleRate = sampleRate;
        m_maxFrames = maxFrames;

        std::vector<AudioComponentDescription> descriptions;
        if (!readDescriptions(descriptions)) return false;
        // A bundle may contain effect, instrument, MIDI, mono, and stereo
        // components.  A component that can be instantiated can still reject
        // its stream format or fail initialization, so candidate selection is
        // only successful after the complete setup sequence succeeds.
        for (const auto& description : descriptions) {
            shutdownUnitOnly();
            m_component = AudioComponentFindNext(nullptr, &description);
            if (!m_component || AudioComponentInstanceNew(m_component, &m_unit) != noErr || !m_unit) {
                shutdownUnitOnly();
                continue;
            }
            m_componentType = description.componentType;
            m_componentSubType = description.componentSubType;
            m_componentManufacturer = description.componentManufacturer;
            if (configureAndInitialize(channels)) break;
            shutdownUnitOnly();
        }
        if (!m_component || !m_unit || !m_initialized) return false;

        m_channels = channels;
        m_ready = true;
        cleanup.committed = true;
        return true;
    }

private:
    bool configureAndInitialize(uint32_t channels) noexcept {
        AudioStreamBasicDescription format{};
        format.mSampleRate = m_sampleRate;
        format.mFormatID = kAudioFormatLinearPCM;
        format.mFormatFlags = kAudioFormatFlagIsFloat | kAudioFormatFlagIsPacked |
                              kAudioFormatFlagIsNonInterleaved;
        format.mBytesPerPacket = sizeof(float);
        format.mFramesPerPacket = 1;
        format.mBytesPerFrame = sizeof(float);
        format.mChannelsPerFrame = channels;
        format.mBitsPerChannel = 32;

        // Instruments (aumu) have no audio input bus.  Treating them as an
        // effect and writing the input stream format makes otherwise valid
        // synth AUs reject initialization (Vital is one such component).
        const bool hasAudioInput = m_componentType != kAudioUnitType_MusicDevice &&
                                   m_componentType != kAudioUnitType_MusicEffect;
        if ((hasAudioInput && AudioUnitSetProperty(m_unit, kAudioUnitProperty_StreamFormat,
                                 kAudioUnitScope_Input, 0, &format, sizeof(format)) != noErr) ||
            AudioUnitSetProperty(m_unit, kAudioUnitProperty_StreamFormat,
                                 kAudioUnitScope_Output, 0, &format, sizeof(format)) != noErr ||
            AudioUnitSetProperty(m_unit, kAudioUnitProperty_MaximumFramesPerSlice,
                                 kAudioUnitScope_Global, 0, &m_maxFrames, sizeof(m_maxFrames)) != noErr)
            return false;

        AURenderCallbackStruct callback{&Runtime::inputCallback, this};
        if ((hasAudioInput && AudioUnitSetProperty(m_unit, kAudioUnitProperty_SetRenderCallback,
                                 kAudioUnitScope_Input, 0, &callback, sizeof(callback)) != noErr) ||
            AudioUnitInitialize(m_unit) != noErr)
            return false;
        m_initialized = true;
        return true;
    }

public:

    bool process(SandboxProtocol::SharedAudioBlock& shared,
                 uint32_t channels, uint32_t frames) noexcept {
        if (!m_ready || !m_unit || channels == 0 || channels > SandboxProtocol::kMaxChannels ||
            frames == 0 || frames > m_maxFrames) return false;

        m_parameterError = false;
        m_input = &shared;
        m_channels = channels;
        m_frames = frames;
        auto* output = reinterpret_cast<AudioBufferList*>(m_outputStorage);
        output->mNumberBuffers = channels;
        for (uint32_t channel = 0; channel < channels; ++channel) {
            output->mBuffers[channel].mNumberChannels = 1;
            output->mBuffers[channel].mDataByteSize = frames * sizeof(float);
            output->mBuffers[channel].mData = shared.output[channel];
        }
        // Parameter changes are consumed inside the sandbox worker, immediately
        // before the AU render call.  The host audio callback only appends to a
        // bounded mailbox; it never calls AudioUnitSetParameter directly.
        // AudioUnitSetParameter has no sample-offset argument, so AU changes
        // are applied at the start of this block.  The offset is intentionally
        // validated here and retained in the protocol for formats that support
        // sample-accurate scheduling.
        const uint32_t parameterCount = std::min<uint32_t>(
            shared.parameterChanges.load(std::memory_order_acquire),
            SandboxProtocol::kMaxParameterChanges);
        for (uint32_t index = 0; index < parameterCount; ++index) {
            const auto& change = shared.parameterChange[index];
            if (change.sampleOffset > frames || !std::isfinite(change.value)) continue;
            if (AudioUnitSetParameter(m_unit, static_cast<AudioUnitParameterID>(change.parameterId),
                                      kAudioUnitScope_Global, 0, static_cast<AudioUnitParameterValue>(change.value),
                                      0) != noErr) {
                m_parameterError = true;
            }
        }
        // AU instruments receive MIDI through the MusicDevice API rather
        // than the audio render callback.  Forward the bounded mailbox before
        // rendering so note-on/CC/pitch-bend state belongs to this block.
        if (m_componentType == kAudioUnitType_MusicDevice ||
            m_componentType == kAudioUnitType_MusicEffect) {
            const uint32_t eventCount = std::min<uint32_t>(
                shared.midiEvents.load(std::memory_order_acquire),
                SandboxProtocol::kMaxMidiEvents);
            for (uint32_t index = 0; index < eventCount; ++index) {
                const auto& event = shared.midi[index];
                if (event.size >= 3 && event.size <= sizeof(event.data) &&
                    event.sampleOffset < frames) {
                    (void)MusicDeviceMIDIEvent(
                        m_unit, static_cast<UInt32>(event.data[0]),
                        static_cast<UInt32>(event.data[1]),
                        static_cast<UInt32>(event.data[2]),
                        static_cast<UInt32>(event.sampleOffset));
                }
            }
        }
        AudioUnitRenderActionFlags flags = 0;
        AudioTimeStamp timestamp{};
        timestamp.mFlags = kAudioTimeStampSampleTimeValid;
        const OSStatus status = AudioUnitRender(m_unit, &flags, &timestamp, 0, frames, output);
        m_input = nullptr;
        const bool parameterOk = !m_parameterError;
        m_parameterError = false;
        return status == noErr && parameterOk;
    }

    bool ready() const noexcept { return m_ready; }
    OSType componentType() const noexcept { return m_componentType; }
    OSType componentSubType() const noexcept { return m_componentSubType; }
    OSType componentManufacturer() const noexcept { return m_componentManufacturer; }

    bool saveState(uint8_t* destination, size_t capacity, uint32_t& size) noexcept {
        size = 0;
        if (!m_ready || !m_unit || !destination) return false;
        CFPropertyListRef classInfo = nullptr;
        UInt32 propertySize = sizeof(classInfo);
        if (AudioUnitGetProperty(m_unit, kAudioUnitProperty_ClassInfo,
                                 kAudioUnitScope_Global, 0, &classInfo, &propertySize) != noErr ||
            !classInfo) return false;
        CFDataRef encoded = CFPropertyListCreateData(
            kCFAllocatorDefault, classInfo, kCFPropertyListBinaryFormat_v1_0, 0, nullptr);
        CFRelease(classInfo);
        if (!encoded) return false;
        const CFIndex length = CFDataGetLength(encoded);
        const bool fits = length >= 0 && static_cast<size_t>(length) <= capacity &&
            static_cast<uint64_t>(length) <= std::numeric_limits<uint32_t>::max();
        if (fits && length > 0)
            std::memcpy(destination, CFDataGetBytePtr(encoded), static_cast<size_t>(length));
        if (fits) size = static_cast<uint32_t>(length);
        CFRelease(encoded);
        return fits;
    }

    bool loadState(const uint8_t* data, size_t size) noexcept {
        if (!m_ready || !m_unit || (!data && size) || size == 0 ||
            size > kMaxStateBytes ||
            size > static_cast<size_t>(std::numeric_limits<CFIndex>::max())) return false;
        CFDataRef encoded = CFDataCreate(kCFAllocatorDefault, data, static_cast<CFIndex>(size));
        if (!encoded) return false;
        CFErrorRef error = nullptr;
        CFPropertyListRef classInfo = CFPropertyListCreateWithData(
            kCFAllocatorDefault, encoded, kCFPropertyListMutableContainersAndLeaves,
            nullptr, &error);
        if (error) CFRelease(error);
        CFRelease(encoded);
        if (!classInfo) return false;
        const OSStatus status = AudioUnitSetProperty(
            m_unit, kAudioUnitProperty_ClassInfo, kAudioUnitScope_Global, 0,
            &classInfo, sizeof(classInfo));
        CFRelease(classInfo);
        return status == noErr;
    }

    bool reset() noexcept {
        if (!m_ready || !m_unit) return false;
        return AudioUnitReset(m_unit, kAudioUnitScope_Global, 0) == noErr;
    }

private:
    static OSType fourCC(CFTypeRef value) noexcept {
        if (value && CFGetTypeID(value) == CFNumberGetTypeID()) {
            int32_t number = 0;
            CFNumberGetValue(static_cast<CFNumberRef>(value), kCFNumberSInt32Type, &number);
            return static_cast<OSType>(number);
        }
        if (!value || CFGetTypeID(value) != CFStringGetTypeID()) return 0;
        char text[5]{};
        if (!CFStringGetCString(static_cast<CFStringRef>(value), text, sizeof(text), kCFStringEncodingUTF8))
            return 0;
        return static_cast<OSType>(static_cast<uint8_t>(text[0]) << 24 |
                                   static_cast<uint8_t>(text[1]) << 16 |
                                   static_cast<uint8_t>(text[2]) << 8 |
                                   static_cast<uint8_t>(text[3]));
    }

    bool readDescriptions(std::vector<AudioComponentDescription>& results) const noexcept {
        results.clear();
        if (!m_bundle) return false;
        CFDictionaryRef info = CFBundleGetInfoDictionary(m_bundle);
        if (!info) return false;
        const auto components = static_cast<CFArrayRef>(CFDictionaryGetValue(
            info, CFSTR("AudioComponents")));
        if (!components || CFGetTypeID(components) != CFArrayGetTypeID() || CFArrayGetCount(components) == 0)
            return false;
        const CFIndex count = CFArrayGetCount(components);
        for (CFIndex index = 0; index < count; ++index) {
            const auto component = static_cast<CFDictionaryRef>(CFArrayGetValueAtIndex(components, index));
            if (!component || CFGetTypeID(component) != CFDictionaryGetTypeID()) continue;
            AudioComponentDescription candidate{};
            candidate.componentType = fourCC(CFDictionaryGetValue(component, CFSTR("type")));
            candidate.componentSubType = fourCC(CFDictionaryGetValue(component, CFSTR("subtype")));
            candidate.componentManufacturer = fourCC(CFDictionaryGetValue(component, CFSTR("manufacturer")));
            candidate.componentFlags = 0;
            candidate.componentFlagsMask = 0;
            if (candidate.componentType != 0 && candidate.componentSubType != 0 &&
                candidate.componentManufacturer != 0) {
                results.push_back(candidate);
            }
        }
        return !results.empty();
    }

    static OSStatus inputCallback(void* refCon, AudioUnitRenderActionFlags*,
                                  const AudioTimeStamp*, AudioUnitElement,
                                  UInt32 frames, AudioBufferList* ioData) noexcept {
        auto* self = static_cast<Runtime*>(refCon);
        if (!self || !self->m_input || !ioData || frames > self->m_frames) return static_cast<OSStatus>(-50);
        ioData->mNumberBuffers = self->m_channels;
        for (uint32_t channel = 0; channel < self->m_channels; ++channel) {
            ioData->mBuffers[channel].mNumberChannels = 1;
            ioData->mBuffers[channel].mDataByteSize = frames * sizeof(float);
            ioData->mBuffers[channel].mData = self->m_input->input[channel];
        }
        return noErr;
    }

    void shutdownUnitOnly() noexcept {
        if (m_unit) {
            if (m_initialized) AudioUnitUninitialize(m_unit);
            AudioComponentInstanceDispose(m_unit);
            m_unit = nullptr;
        }
        m_initialized = false;
        m_ready = false;
        m_input = nullptr;
    }

    void shutdown() noexcept {
        shutdownUnitOnly();
        if (m_bundle) {
            CFBundleUnloadExecutable(m_bundle);
            CFRelease(m_bundle);
            m_bundle = nullptr;
        }
        m_component = nullptr;
        m_componentType = 0;
        m_componentSubType = 0;
        m_componentManufacturer = 0;
    }

    CFBundleRef m_bundle = nullptr;
    AudioComponent m_component = nullptr;
    OSType m_componentType = 0;
    OSType m_componentSubType = 0;
    OSType m_componentManufacturer = 0;
    AudioUnit m_unit = nullptr;
    SandboxProtocol::SharedAudioBlock* m_input = nullptr;
    double m_sampleRate = 44100.0;
    uint32_t m_maxFrames = 0;
    uint32_t m_channels = 0;
    uint32_t m_frames = 0;
    bool m_initialized = false;
    bool m_ready = false;
    bool m_parameterError = false;
    static_assert(SandboxProtocol::kMaxChannels >= 1, "AU adapter requires at least one channel");
    // AudioBufferList contains one AudioBuffer inline; reserve the remaining
    // channel descriptors explicitly so stereo/multichannel access is bounded.
    alignas(AudioBufferList) uint8_t m_outputStorage[
        sizeof(AudioBufferList) + sizeof(AudioBuffer) * (SandboxProtocol::kMaxChannels - 1)];
};

} // namespace Aura::Core::Plugins::SandboxAU

#endif
