#pragma once

#include <algorithm>
#include <cstddef>
#include <cstdint>
#include <string>
#include <vector>
#include "../rust_ffi.hpp"

namespace Hirari::Core::Mixing {

static_assert(sizeof(HirariControlRoomSpeakerSnapshot) == 140);
static_assert(sizeof(HirariControlRoomCueSnapshot) == 24);

class ControlRoom {
public:
    struct SpeakerSet { std::string name; float gain = 1.0f; bool enabled = true; };
    struct CueMix {
        uint32_t id = 0;
        float gain = 1.0f;
        bool enabled = true;
        uint32_t busTrackId = 0;
        uint32_t outputChannel = 0;
        bool clickEnabled = false;
    };
    static constexpr uint32_t kMaxCueMixes = 32;

    ControlRoom() : m_state(hirari_control_room_state_create()) {}
    ~ControlRoom() { hirari_control_room_state_destroy(m_state); }
    ControlRoom(const ControlRoom&) = delete;
    ControlRoom& operator=(const ControlRoom&) = delete;
    ControlRoom(ControlRoom&&) = delete;
    ControlRoom& operator=(ControlRoom&&) = delete;

    void resetForProject() { hirari_control_room_reset(m_state); }
    bool addSpeakerSet(std::string name, float gain = 1.0f) {
        return hirari_control_room_add_speaker(
            m_state, reinterpret_cast<const uint8_t*>(name.data()), name.size(), gain);
    }
    bool selectSpeakerSet(std::size_t index) noexcept {
        return hirari_control_room_select_speaker(m_state, index);
    }
    bool removeSpeakerSet(std::size_t index) {
        return hirari_control_room_remove_speaker(m_state, index);
    }
    bool renameSpeakerSet(std::size_t index, std::string name) {
        return hirari_control_room_rename_speaker(
            m_state, index, reinterpret_cast<const uint8_t*>(name.data()), name.size());
    }
    bool setSpeakerGain(std::size_t index, float gain) {
        return hirari_control_room_set_speaker_gain(m_state, index, gain);
    }
    bool setSpeakerEnabled(std::size_t index, bool enabled) {
        return hirari_control_room_set_speaker_enabled(m_state, index, enabled);
    }
    void setDim(bool enabled) noexcept { hirari_control_room_set_dim(m_state, enabled); }
    bool setDimReductionDb(float db) noexcept { return hirari_control_room_set_dim_db(m_state, db); }
    float dimReductionDb() const noexcept { return hirari_control_room_dim_db(m_state); }
    bool isDimmed() const noexcept { return hirari_control_room_is_dimmed(m_state); }
    void setTalkback(bool enabled, float gain = 1.0f) noexcept {
        hirari_control_room_set_talkback(m_state, enabled, gain);
    }
    bool talkbackEnabled() const noexcept { return hirari_control_room_talkback_enabled(m_state); }
    bool setTalkbackInputChannel(uint32_t channel) noexcept {
        return hirari_control_room_set_talkback_channel(m_state, channel);
    }
    uint32_t talkbackInputChannel() const noexcept {
        return hirari_control_room_talkback_channel(m_state);
    }
    float monitorGain() const noexcept { return hirari_control_room_monitor_gain(m_state); }
    void processMonitor(float* left, float* right, std::size_t frames) const noexcept {
        hirari_control_room_process_monitor_state(
            m_state, left, right, nullptr, static_cast<uint32_t>(frames));
    }
    void processMonitorWithTalkback(float* left, float* right, const float* talkback,
                                    std::size_t frames) const noexcept {
        hirari_control_room_process_monitor_state(
            m_state, left, right, talkback, static_cast<uint32_t>(frames));
    }
    bool upsertCueMix(uint32_t id, float gain, bool enabled = true) {
        return hirari_control_room_upsert_cue(m_state, id, gain, enabled);
    }
    bool removeCueMix(uint32_t id) { return hirari_control_room_remove_cue(m_state, id); }
    bool setCueMixEnabled(uint32_t id, bool enabled) {
        return hirari_control_room_set_cue_enabled(m_state, id, enabled);
    }
    bool setCueMixBusTrack(uint32_t id, uint32_t busTrackId) {
        return hirari_control_room_set_cue_bus(m_state, id, busTrackId);
    }
    bool setCueMixOutputChannel(uint32_t id, uint32_t outputChannel) {
        return hirari_control_room_set_cue_output(m_state, id, outputChannel);
    }
    bool setCueMixClickEnabled(uint32_t id, bool enabled) {
        return hirari_control_room_set_cue_click(m_state, id, enabled);
    }
    bool selectCueMix(uint32_t id) noexcept { return hirari_control_room_select_cue(m_state, id); }
    uint32_t activeCueMixId() const noexcept { return hirari_control_room_active_cue_id(m_state); }
    uint32_t activeCueMixBusTrackId() const noexcept { return hirari_control_room_active_cue_bus(m_state); }
    uint32_t cueBusTrackId(uint32_t id) const noexcept { return hirari_control_room_cue_bus(m_state, id); }
    float activeCueMixGain() const noexcept { return hirari_control_room_active_cue_gain(m_state); }
    uint32_t activeCueMixOutputChannel() const noexcept { return hirari_control_room_active_cue_output(m_state); }
    bool activeCueMixClickEnabled() const noexcept { return hirari_control_room_active_cue_click(m_state); }
    bool isCueBus(uint32_t id) const noexcept { return hirari_control_room_is_cue_bus(m_state, id); }
    float cueGain(uint32_t id) const noexcept { return hirari_control_room_cue_gain(m_state, id); }
    bool validate() const noexcept { return hirari_control_room_validate(m_state); }

    const std::vector<SpeakerSet>& speakerSets() const {
        m_speakerSnapshot = loadSpeakerSnapshot();
        return m_speakerSnapshot;
    }
    std::vector<SpeakerSet> speakerSetsSnapshot() const { return loadSpeakerSnapshot(); }
    const std::vector<CueMix>& cueMixes() const {
        m_cueSnapshot = loadCueSnapshot();
        return m_cueSnapshot;
    }
    std::vector<CueMix> cueMixesSnapshot() const { return loadCueSnapshot(); }
    std::size_t activeSpeakerSet() const noexcept { return hirari_control_room_active_speaker(m_state); }

private:
    std::vector<SpeakerSet> loadSpeakerSnapshot() const {
        const size_t count = hirari_control_room_speaker_snapshot(m_state, nullptr, 0);
        std::vector<HirariControlRoomSpeakerSnapshot> raw(count);
        if (count != 0) {
            raw.resize(hirari_control_room_speaker_snapshot(m_state, raw.data(), raw.size()));
        }
        std::vector<SpeakerSet> result;
        result.reserve(raw.size());
        for (const auto& item : raw) {
            const auto length = std::min<std::size_t>(item.name_length, sizeof(item.name));
            result.push_back({std::string(reinterpret_cast<const char*>(item.name), length),
                              item.gain, item.enabled != 0});
        }
        return result;
    }
    std::vector<CueMix> loadCueSnapshot() const {
        const size_t count = hirari_control_room_cue_snapshot(m_state, nullptr, 0);
        std::vector<HirariControlRoomCueSnapshot> raw(count);
        if (count != 0) {
            raw.resize(hirari_control_room_cue_snapshot(m_state, raw.data(), raw.size()));
        }
        std::vector<CueMix> result;
        result.reserve(raw.size());
        for (const auto& item : raw) {
            result.push_back({item.id, item.gain, item.enabled != 0, item.bus_track_id,
                              item.output_channel, item.click_enabled != 0});
        }
        return result;
    }

    void* m_state = nullptr;
    mutable std::vector<SpeakerSet> m_speakerSnapshot;
    mutable std::vector<CueMix> m_cueSnapshot;
};

} // namespace Hirari::Core::Mixing
