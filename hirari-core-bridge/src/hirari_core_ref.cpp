#include "hirari-core-bridge/src/lib.rs.h"
#include <string>
#include <iostream>
#include <cmath>
#include <algorithm>

namespace Hirari::Core::BridgeFFI {

void HIRARI_LOG(uint32_t level, const std::string& msg) {
    ::Hirari::Core::Bridge::report_hirari_log(level, rust::Str(msg));
}

} // namespace Hirari::Core::BridgeFFI

#include "dsp/spatial/metal_audio_kernel.hpp"
#include "core/midi_buffer.hpp"
#include "core/midi_region.hpp"
#include "core/status_queue.hpp"
#include "core/concurrency/audio_task_manager.hpp"
#include "core/concurrency/thread_pool.hpp"
#include "scae/stem_extraction_pipeline.hpp"
#include "dsp/effects/chord_trigger.hpp"

namespace Hirari::Core::Bridge {

void initialize_gpu() {
    // Explicitly seed the GPU kernel before the first block arrives
    ::Hirari::DSP::Spatial::MetalAudioKernel::getInstance().initialize();
}

} // namespace Hirari::Core::Bridge

// Frozen reference for the pre-migration AnalogSaturator / Oversampler2x path.
extern "C" void hirari_analog_saturator_frozen_reference(
    const float* input_left, const float* input_right,
    float* output_left, float* output_right, uint32_t frames,
    float drive, float warmth, uint32_t model) {
    constexpr float a1 = 0.12967654578647f;
    constexpr float a2 = 0.48418923434341f;
    constexpr float dc_cut = 0.995f;
    float phase_l1 = 0.0f, phase_l2 = 0.0f, phase_r1 = 0.0f, phase_r2 = 0.0f;
    float dc_l = 0.0f, dc_r = 0.0f;
    const float pre_gain = std::pow(10.0f, std::clamp(std::isfinite(drive) ? drive : 0.0f, 0.0f, 4.0f) * 12.0f / 20.0f);
    const float safe_warmth = std::clamp(std::isfinite(warmth) ? warmth : 0.5f, 0.0f, 1.0f);
    const float post_gain = 1.0f / std::max(1.0f, pre_gain * 0.35f);
    const auto shape = [safe_warmth, model](float x) {
        if (model == 1) {
            const float ax = std::abs(x);
            return ax < 1.0f ? x * (1.5f - 0.5f * x * x) : (x > 0.0f ? 1.0f : -1.0f);
        }
        if (model == 2) return std::tanh(x);
        const float bias = safe_warmth * 0.25f;
        return (x + bias) / (1.0f + std::abs(x + bias)) - bias / (1.0f + std::abs(bias));
    };
    for (uint32_t i = 0; i < frames; ++i) {
        const float in_l = std::isfinite(input_left[i]) ? input_left[i] : 0.0f;
        const float in_r = std::isfinite(input_right[i]) ? input_right[i] : 0.0f;
        const float up_l1 = a1 * (in_l * pre_gain - phase_l1) + phase_l1; phase_l1 = up_l1;
        const float up_l2 = a2 * (in_l * pre_gain - phase_l2) + phase_l2; phase_l2 = up_l2;
        const float up_r1 = a1 * (in_r * pre_gain - phase_r1) + phase_r1; phase_r1 = up_r1;
        const float up_r2 = a2 * (in_r * pre_gain - phase_r2) + phase_r2; phase_r2 = up_r2;
        const float wet_l = (shape(up_l1) * post_gain + shape(up_l2) * post_gain) * 0.5f;
        const float wet_r = (shape(up_r1) * post_gain + shape(up_r2) * post_gain) * 0.5f;
        dc_l = dc_cut * dc_l + (1.0f - dc_cut) * wet_l;
        dc_r = dc_cut * dc_r + (1.0f - dc_cut) * wet_r;
        const float out_l = wet_l - dc_l, out_r = wet_r - dc_r;
        output_left[i] = std::isfinite(out_l) ? out_l : 0.0f;
        output_right[i] = std::isfinite(out_r) ? out_r : 0.0f;
    }
}

extern "C" bool hirari_midi_buffer_cpp_wrapper_smoke() {
    Hirari::Core::MidiBuffer buffer;
    const uint8_t noteOn[] = {0x90, 60, 100};
    const uint8_t noteOff[] = {0x80, 60, 0};
    buffer.addEvent(12, noteOn, sizeof(noteOn), 3);
    buffer.addEvent(12, noteOff, sizeof(noteOff), 0);
    if (buffer.size() != 2 || buffer.remainingCapacity() !=
            Hirari::Core::MidiBuffer::kMaxEventsPerBlock - 2) return false;
    buffer.sort();
    if (buffer.getEvents()[0].data[0] != 0x80 ||
        buffer.getEvents()[1].data[0] != 0x90) return false;
    for (size_t index = 0; index < Hirari::Core::MidiBuffer::kMaxEventsPerBlock; ++index)
        buffer.addEvent(index, noteOn, sizeof(noteOn), 0);
    if (!buffer.overflowed() || buffer.droppedEvents() == 0 ||
        buffer.takeDroppedEvents() == 0 || buffer.droppedEvents() != 0) return false;
    buffer.clear();
    return buffer.size() == 0 && !buffer.overflowed() &&
        buffer.takeOversizeEvents() == 0 && buffer.takeExtendedEvents() == 0;
}

extern "C" bool hirari_status_queue_cpp_wrapper_smoke() {
    auto& queue = Hirari::Core::StatusQueue::getInstance();
    Hirari::Core::StatusQueue::Message message{};
    queue.pushFromAudio(Hirari::Core::StatusQueue::Severity::Warning, "C++ bridge status");
    if (!queue.pop(message) || message.severity != Hirari::Core::StatusQueue::Severity::Warning ||
        std::string_view(message.text) != "C++ bridge status") return false;
    for (size_t index = 0; index < 256; ++index) {
        queue.pushFromAudio(Hirari::Core::StatusQueue::Severity::Info, "bounded");
    }
    queue.pushFromAudio(Hirari::Core::StatusQueue::Severity::Error, "overflow");
    const bool counted = queue.takeDroppedCount() == 1;
    while (queue.pop(message)) {}
    return counted;
}

extern "C" bool hirari_audio_task_scheduler_cpp_smoke() {
    auto& scheduler = Hirari::Core::Concurrency::AudioTaskStealingScheduler::getInstance();
    scheduler.start(2);
    std::atomic<uint64_t> sum{0};
    scheduler.parallel_for(0, 128, [&sum](uint32_t value) {
        sum.fetch_add(value, std::memory_order_relaxed);
    });
    std::atomic<uint64_t> rawSum{0};
    scheduler.parallel_for(0, 16, [](uint32_t value, void* data) {
        static_cast<std::atomic<uint64_t>*>(data)->fetch_add(value, std::memory_order_relaxed);
    }, &rawSum);
    std::atomic<uint64_t> dataSum{0};
    scheduler.parallel_for_with_data<int>(
        0, 8,
        [&dataSum](uint32_t value, void*, int**, uint32_t, uint32_t) {
            dataSum.fetch_add(value, std::memory_order_relaxed);
        },
        nullptr, static_cast<int**>(nullptr), 0, 0);
    scheduler.stop();
    return sum.load(std::memory_order_relaxed) == (127ull * 128ull) / 2ull &&
        rawSum.load(std::memory_order_relaxed) == (15ull * 16ull) / 2ull &&
        dataSum.load(std::memory_order_relaxed) == (7ull * 8ull) / 2ull;
}

extern "C" bool hirari_rust_thread_pool_cpp_smoke() {
    try {
        std::atomic<uint64_t> drained{0};
        bool valuesCorrect = false;
        {
            Hirari::Core::Concurrency::ThreadPool pool(2);
            auto value = pool.enqueue([](int left, int right) { return left + right; }, 19, 23);
            auto failure = pool.enqueue([]() -> int { throw std::runtime_error("future exception contract"); });
            for (uint32_t index = 0; index < 256; ++index) {
                (void)pool.enqueue([&drained] { drained.fetch_add(1, std::memory_order_relaxed); });
            }
            bool exceptionPropagated = false;
            try { (void)failure.get(); }
            catch (const std::runtime_error&) { exceptionPropagated = true; }
            valuesCorrect = value.get() == 42 && exceptionPropagated;
        }
        // The local pool drains accepted work before its Rust workers join.
        return valuesCorrect && drained.load(std::memory_order_relaxed) == 256;
    } catch (...) {
        return false;
    }
}

extern "C" bool hirari_scae_stem_pipeline_cpp_smoke() {
    using Pipeline = Hirari::SCAE::Intelligence::StemExtractionPipeline;
    using Stems = std::map<std::string, std::vector<float>>;
    auto promise = std::make_shared<std::promise<Stems>>();
    auto result = promise->get_future();
    auto samples = std::make_shared<std::vector<float>>(256);
    for (size_t index = 0; index < samples->size(); ++index) {
        (*samples)[index] = std::sin(static_cast<float>(index) * 0.11f);
    }
    Pipeline::Task task{7, samples, 44100.0, [promise](Stems&& stems) {
        promise->set_value(std::move(stems));
    }};
    Pipeline::getInstance().enqueue(std::move(task));
    if (result.wait_for(std::chrono::seconds(5)) != std::future_status::ready) return false;
    const auto stems = result.get();
    for (const char* name : {"vocals", "drums", "bass", "other"}) {
        const auto found = stems.find(name);
        if (found == stems.end() || found->second.size() != samples->size()) return false;
    }
    return true;
}

extern "C" bool hirari_midi_region_cpp_wrapper_smoke() {
    Hirari::Core::MidiRegion region(1, "Rust-owned notes");
    region.addNote(Hirari::Core::MIDINote{60, 80, 0.2, 0.5});
    region.addNote(Hirari::Core::MIDINote{60, 90, 1.24, 0.5});
    region.addNote(Hirari::Core::MIDINote{72, 100, 2.0, 0.25});
    region.transpose(12, 1.0, 1.5);
    region.setMutedAt(1.24, 72, true);
    std::vector<Hirari::Core::MIDINote> notes;
    if (!region.copyProcessedNotes(notes) || notes.size() != 3 ||
        notes[1].pitch != 72 || notes[1].velocity != 0) return false;
    region.removeNotesAt(1.24, 72, 0.01);
    if (!region.copyProcessedNotes(notes) || notes.size() != 2) return false;
    const std::vector<Hirari::Core::MIDINote> invalid{{64, 90, 0.5, 0.0}};
    if (region.replaceNotes(invalid) || !region.copyProcessedNotes(notes) || notes.size() != 2)
        return false;
    return region.updateNote(0, Hirari::Core::MIDINote{61, 88, 0.25, 0.75}) &&
        region.copyProcessedNotes(notes) && notes.size() == 2 && notes[0].pitch == 61;
}

extern "C" bool hirari_chord_trigger_cpp_wrapper_smoke() {
    Hirari::DSP::Effects::ChordTrigger effect;
    Hirari::Core::AudioBuffer audio;
    Hirari::Core::MidiBuffer midi;
    Hirari::DSP::ProcessContext context{};
    effect.prepareToPlay(48'000.0, 64);
    effect.setStrumMs(1.0f);
    Hirari::DSP::Effects::ChordTrigger restored;
    if (!restored.setState(effect.getState()) ||
        std::abs(restored.getParameter(0) - effect.getParameter(0)) > 1.0e-6f) return false;
    midi.addNoteOn(2, 60, 96, 100, 7);
    midi.addNoteOff(2, 60, 200);
    effect.process(audio, midi, context);
    if (midi.size() != 6) return false;
    const auto* events = midi.getEvents();
    return events[0].sampleOffset == 100 && events[0].data[0] == 0x91 &&
        events[0].data[1] == 60 && events[0].articulationId == 7 &&
        events[1].sampleOffset == 148 && events[1].data[1] == 64 &&
        events[2].sampleOffset == 196 && events[2].data[1] == 67 &&
        events[3].sampleOffset == 200 && events[3].data[0] == 0x81 &&
        events[3].data[1] == 60 && events[3].articulationId == 0 &&
        events[4].data[1] == 64 && events[5].data[1] == 67;
}
