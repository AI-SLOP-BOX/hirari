/*
 * Aura DAW Ultimate - Sovereign Plugin SDK
 * Copyright (c) 2024-2026 Aura DAW Project. All rights reserved.
 */

#pragma once

#include <string>
#include <vector>
#include <memory>
#include <cstdint>
#include <cmath>
#include <unordered_set>
#include <algorithm>
#include "audio_buffer.hpp"
#include "midi_buffer.hpp"

namespace Aura::SDK {

inline constexpr uint32_t kSDKVersion = 1;

struct ParameterDescriptor {
    uint32_t id = 0;
    std::string name;
    float minValue = 0.0f;
    float maxValue = 1.0f;
    float defaultValue = 0.0f;
    bool automatable = true;
};

struct AutomationPoint {
    uint64_t sample = 0;
    float value = 0.0f;
};

struct ComponentDescriptor {
    std::string identifier;
    uint32_t inputChannels = 2;
    uint32_t outputChannels = 2;
    bool isSidechain = false;

    bool isValid() const {
        return !identifier.empty() && inputChannels > 0 && outputChannels > 0
            && inputChannels <= 32 && outputChannels <= 32;
    }
};

/// Deterministic, allocation-free-after-build parameter automation evaluator.
class ParameterAutomation {
public:
    bool setPoints(std::vector<AutomationPoint> points) {
        for (const auto& point : points) {
            if (!std::isfinite(point.value)) return false;
        }
        std::stable_sort(points.begin(), points.end(), [](const auto& left, const auto& right) {
            return left.sample < right.sample;
        });
        for (std::size_t i = 1; i < points.size(); ++i) {
            if (points[i - 1].sample == points[i].sample) return false;
        }
        m_points = std::move(points);
        return true;
    }

    float valueAt(uint64_t sample, float fallback) const {
        if (m_points.empty()) return fallback;
        if (sample <= m_points.front().sample) return m_points.front().value;
        if (sample >= m_points.back().sample) return m_points.back().value;
        const auto upper = std::upper_bound(m_points.begin(), m_points.end(), sample,
            [](uint64_t position, const AutomationPoint& point) { return position < point.sample; });
        const auto& right = *upper;
        const auto& left = *(upper - 1);
        const double span = static_cast<double>(right.sample - left.sample);
        const float t = static_cast<float>(static_cast<double>(sample - left.sample) / span);
        return left.value + (right.value - left.value) * t;
    }

    const std::vector<AutomationPoint>& points() const { return m_points; }

private:
    std::vector<AutomationPoint> m_points;
};

struct PluginDescriptor {
    uint32_t sdkVersion = kSDKVersion;
    std::string identifier;
    std::string name;
    std::string vendor;
    std::string version;
    uint32_t stateSchemaVersion = 1;
    uint32_t guiStateSchemaVersion = 1;
    uint32_t inputChannels = 2;
    uint32_t outputChannels = 2;
    uint32_t sidechainChannels = 0;
    bool acceptsMidi = false;
    bool producesMidi = false;
    bool supportsSidechain = false;
    std::vector<ParameterDescriptor> parameters;
    std::vector<ComponentDescriptor> components;

    bool isValid() const {
        if (sdkVersion == 0 || identifier.empty() || name.empty() ||
            vendor.empty() || version.empty() || inputChannels == 0 ||
            outputChannels == 0 || inputChannels > 32 || outputChannels > 32 ||
            sidechainChannels > 32 || (supportsSidechain && sidechainChannels == 0) ||
            stateSchemaVersion == 0 || guiStateSchemaVersion == 0) return false;
        std::unordered_set<uint32_t> parameterIds;
        std::unordered_set<std::string> componentIds;
        for (const auto& component : components) {
            if (!component.isValid() || !componentIds.insert(component.identifier).second) return false;
            if (component.isSidechain && !supportsSidechain) return false;
        }
        for (const auto& parameter : parameters) {
            if (parameter.name.empty() || !parameterIds.insert(parameter.id).second ||
                !std::isfinite(parameter.minValue) || !std::isfinite(parameter.maxValue) ||
                !std::isfinite(parameter.defaultValue) || parameter.minValue > parameter.maxValue ||
                parameter.defaultValue < parameter.minValue ||
                parameter.defaultValue > parameter.maxValue) return false;
        }
        return true;
    }

    std::string stateCacheKey() const {
        return identifier + "@" + version + ":state-" +
               std::to_string(stateSchemaVersion) + ":gui-" +
               std::to_string(guiStateSchemaVersion);
    }
};

struct ProcessContext {
    double sampleRate;
    uint32_t bufferSize;
    double bpm;
    uint64_t timelinePos;
};

/**
 * @class IProcessor
 * @brief Base interface for industrial Aura DSP plugins.
 */
class IProcessor {
public:
    virtual ~IProcessor() = default;

    virtual std::string getName() const = 0;
    virtual void prepareToPlay(double sampleRate, uint32_t bufferSize) = 0;
    virtual void process(::Aura::Core::AudioBuffer& buffer, 
                         ::Aura::Core::MidiBuffer& midi, 
                         const ProcessContext& context) = 0;
    virtual void reset() = 0;

    // State blobs are opaque to the host and may contain plugin-defined
    // versioned data. Empty defaults preserve source compatibility for
    // processors that do not expose state yet.
    virtual std::vector<uint8_t> saveState() const { return {}; }
    virtual bool loadState(const std::vector<uint8_t>&) { return true; }
    virtual std::vector<uint8_t> saveGuiState() const { return {}; }
    virtual bool loadGuiState(const std::vector<uint8_t>&) { return true; }
    virtual float getParameter(uint32_t) const { return 0.0f; }
    virtual bool setParameter(uint32_t, float) { return false; }
    
    virtual bool isBypassed() const { return m_bypassed; }
    virtual void setBypassed(bool b) { m_bypassed = b; }

protected:
    bool m_bypassed = false;
};

/** Stable creation boundary used by host discovery and future VST3/AU/CLAP
 * adapters. Ownership of a processor returned by create() belongs to the
 * caller and must be released through destroy(). */
class IPluginFactory {
public:
    virtual ~IPluginFactory() = default;
    virtual PluginDescriptor getDescriptor() const = 0;
    virtual std::unique_ptr<IProcessor> create() const = 0;
    virtual void destroy(std::unique_ptr<IProcessor> processor) const {
        processor.reset();
    }
};

} // namespace Aura::SDK
