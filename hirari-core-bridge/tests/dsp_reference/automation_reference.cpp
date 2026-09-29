// Frozen C++ reference for Track's realtime automation cursor and interpolation.
#include <algorithm>
#include <cmath>
#include <cstddef>
#include <cstdint>

namespace {
struct AutomationPoint {
    double time;
    float value;
    float curve;
};
}

extern "C" float hirari_automation_reference_evaluate(
    const AutomationPoint* points, size_t count, double time, size_t* hint) {
    if (count == 0) return 0.0f;
    if (count == 1) return points[0].value;
    if (time <= points[0].time) {
        *hint = 0;
        return points[0].value;
    }
    if (time >= points[count - 1].time) {
        *hint = count - 1;
        return points[count - 1].value;
    }
    size_t index = *hint;
    if (index >= count - 1) index = 0;
    if (!(points[index].time <= time && time < points[index + 1].time)) {
        if (index + 2 < count && points[index + 1].time <= time && time < points[index + 2].time) {
            ++index;
        } else {
            auto it = std::upper_bound(points, points + count, time,
                [](double t, const AutomationPoint& point) { return t < point.time; });
            index = static_cast<size_t>(std::distance(points, it) - 1);
        }
        *hint = index;
    }
    const auto& a = points[index];
    const auto& b = points[index + 1];
    const double range = b.time - a.time;
    if (range <= 1.0e-9) return a.value;
    const float amount = std::clamp(static_cast<float>((time - a.time) / range), 0.0f, 1.0f);
    const float curve = std::clamp(a.curve, -1.0f, 1.0f);
    const float shaped = std::clamp(
        amount + curve * amount * (1.0f - amount) * (1.0f - 2.0f * amount), 0.0f, 1.0f);
    return a.value + (b.value - a.value) * shaped;
}

extern "C" void hirari_automation_reference_apply_stereo_block(
    const AutomationPoint* gain_points, size_t gain_count, size_t* gain_hint,
    const AutomationPoint* pan_points, size_t pan_count, size_t* pan_hint,
    double start_time, float* left, float* right,
    float* dry_left, float* dry_right, uint32_t frames) {
    if (gain_count != 0) {
        for (uint32_t sample = 0; sample < frames; ++sample) {
            const float gain = std::clamp(hirari_automation_reference_evaluate(
                gain_points, gain_count, start_time + sample, gain_hint), 0.0f, 2.0f);
            left[sample] *= gain;
            right[sample] *= gain;
            if (dry_left && dry_right) {
                dry_left[sample] *= gain;
                dry_right[sample] *= gain;
            }
        }
    }
    if (pan_count != 0) {
        constexpr float pi = 3.14159265358979323846f;
        for (uint32_t sample = 0; sample < frames; ++sample) {
            const float pan = std::clamp(hirari_automation_reference_evaluate(
                pan_points, pan_count, start_time + sample, pan_hint), -1.0f, 1.0f);
            const float angle = (pan + 1.0f) * 0.25f * pi;
            const float left_gain = std::cos(angle) * 1.41421356237f;
            const float right_gain = std::sin(angle) * 1.41421356237f;
            left[sample] *= left_gain;
            right[sample] *= right_gain;
            if (dry_left && dry_right) {
                dry_left[sample] *= left_gain;
                dry_right[sample] *= right_gain;
            }
        }
    }
}

namespace {
struct PluginAutomationLaneView {
    uint32_t processor_index;
    uint32_t parameter_id;
    const AutomationPoint* points;
    size_t point_count;
};
struct TimedParameterEvent {
    uint32_t processor_index;
    uint32_t parameter_id;
    uint32_t sample_offset;
    float normalized_value;
};
}

extern "C" size_t hirari_plugin_automation_reference_render_block(
    const PluginAutomationLaneView* lanes, size_t lane_count,
    uint64_t playhead, uint32_t frames,
    TimedParameterEvent* output, size_t capacity) {
    const size_t automated_lane_count = static_cast<size_t>(std::count_if(
        lanes, lanes + lane_count,
        [](const auto& lane) { return lane.point_count != 0; }));
    if (automated_lane_count == 0 || frames == 0 || capacity == 0) return 0;
    const size_t per_lane_budget = std::max<size_t>(1, capacity / automated_lane_count);
    const uint64_t block_end = playhead <= UINT64_MAX - frames ? playhead + frames : UINT64_MAX;
    size_t count = 0;
    const auto append = [&](const auto& lane, uint32_t offset, float value) {
        if (count >= capacity) return;
        output[count++] = {lane.processor_index, lane.parameter_id, offset,
                           std::clamp(value, 0.0f, 1.0f)};
    };
    for (size_t lane_index = 0; lane_index < lane_count; ++lane_index) {
        const auto& lane = lanes[lane_index];
        if (lane.point_count == 0) continue;
        const auto* points = lane.points;
        const size_t point_count_all = lane.point_count;
        size_t hint = 0;
        const float initial = std::clamp(hirari_automation_reference_evaluate(
            points, point_count_all, static_cast<double>(playhead), &hint), 0.0f, 1.0f);
        const size_t lane_event_start = count;
        append(lane, 0, initial);
        size_t remaining = per_lane_budget > 0 ? per_lane_budget - 1 : 0;
        if (remaining == 0 || frames <= 1) continue;
        const auto* first = std::upper_bound(points, points + point_count_all,
            static_cast<double>(playhead), [](double time, const auto& point) { return time < point.time; });
        const auto* after = std::lower_bound(first, points + point_count_all,
            static_cast<double>(block_end), [](const auto& point, double time) { return point.time < time; });
        const size_t point_count = static_cast<size_t>(after - first);
        const size_t point_slots = std::min(point_count, remaining);
        if (point_count <= point_slots) {
            for (auto point = first; point != after; ++point) {
                if (point->time < 0.0 || point->time > static_cast<double>(UINT64_MAX)) continue;
                const uint64_t sample = static_cast<uint64_t>(point->time);
                if (sample > playhead && sample < block_end)
                    append(lane, static_cast<uint32_t>(sample - playhead), point->value);
            }
        } else if (point_slots == 1) {
            const auto* point = after - 1;
            const uint64_t sample = static_cast<uint64_t>(point->time);
            append(lane, static_cast<uint32_t>(sample - playhead), point->value);
        } else if (point_slots > 1) {
            for (size_t selected = 0; selected < point_slots; ++selected) {
                const size_t point_index = selected * (point_count - 1) / (point_slots - 1);
                const auto* point = first + static_cast<ptrdiff_t>(point_index);
                const uint64_t sample = static_cast<uint64_t>(point->time);
                append(lane, static_cast<uint32_t>(sample - playhead), point->value);
            }
        }
        remaining -= point_slots;
        const size_t grid_slots = std::min<size_t>(remaining, frames - 1);
        for (size_t grid = 1; grid <= grid_slots; ++grid) {
            const uint32_t offset = static_cast<uint32_t>(
                (grid * static_cast<uint64_t>(frames - 1)) / grid_slots);
            const uint64_t sample = playhead + offset;
            const auto* exact = std::lower_bound(points, points + point_count_all,
                static_cast<double>(sample), [](const auto& point, double time) { return point.time < time; });
            if (exact != points + point_count_all && exact->time == static_cast<double>(sample)) continue;
            const float value = hirari_automation_reference_evaluate(
                points, point_count_all, static_cast<double>(sample), &hint);
            append(lane, offset, value);
        }
        if (grid_slots == 0 && count - lane_event_start < per_lane_budget && frames > 1) {
            const uint32_t offset = frames - 1;
            const uint64_t sample = playhead + offset;
            const auto* exact = std::lower_bound(points, points + point_count_all,
                static_cast<double>(sample), [](const auto& point, double time) { return point.time < time; });
            if (exact == points + point_count_all || exact->time != static_cast<double>(sample)) {
                const float value = hirari_automation_reference_evaluate(
                    points, point_count_all, static_cast<double>(sample), &hint);
                append(lane, offset, value);
            }
        }
    }
    std::sort(output, output + count, [](const auto& left, const auto& right) {
        if (left.sample_offset != right.sample_offset) return left.sample_offset < right.sample_offset;
        if (left.processor_index != right.processor_index) return left.processor_index < right.processor_index;
        return left.parameter_id < right.parameter_id;
    });
    return count;
}
