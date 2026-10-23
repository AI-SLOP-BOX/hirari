#pragma once

#include <string>
#include <cstddef>
#include <cstdint>
#include <vector>

namespace Hirari::Core::Driver {

#if defined(__APPLE__)

extern "C" {
char* hirari_core_midi_list_devices_json();
bool hirari_core_midi_send_message(uint32_t uniqueId, const uint8_t* data, size_t size);
bool hirari_core_midi_start_input();
void hirari_core_midi_stop_input();
char* hirari_core_midi_poll_input_json();
uint64_t hirari_core_midi_host_time_now();
uint64_t hirari_core_midi_host_time_delta_samples(uint64_t start, uint64_t event,
                                                  double sampleRate);
uint64_t hirari_core_midi_dropped_input_events();
void hirari_core_midi_string_free(char* value);
}

inline std::string take_core_midi_string(char* value) {
    std::string result = value ? std::string(value) : std::string{};
    hirari_core_midi_string_free(value);
    return result;
}

inline std::string list_core_midi_devices_json() {
    return take_core_midi_string(hirari_core_midi_list_devices_json());
}
inline bool send_core_midi_message(uint32_t uniqueId, const uint8_t* data, size_t size) {
    return hirari_core_midi_send_message(uniqueId, data, size);
}
inline bool start_core_midi_input() { return hirari_core_midi_start_input(); }
inline void stop_core_midi_input() { hirari_core_midi_stop_input(); }
inline std::string poll_core_midi_input_json() {
    return take_core_midi_string(hirari_core_midi_poll_input_json());
}
inline uint64_t core_midi_host_time_now() { return hirari_core_midi_host_time_now(); }
inline uint64_t core_midi_host_time_delta_samples(uint64_t start, uint64_t event,
                                                  double sampleRate) {
    return hirari_core_midi_host_time_delta_samples(start, event, sampleRate);
}
inline uint64_t core_midi_dropped_input_events() {
    return hirari_core_midi_dropped_input_events();
}

#else
inline std::string list_core_midi_devices_json() { return "[]"; }
inline bool send_core_midi_message(uint32_t, const uint8_t*, size_t) { return false; }
inline bool start_core_midi_input() { return false; }
inline void stop_core_midi_input() {}
inline std::string poll_core_midi_input_json() { return "[]"; }
inline uint64_t core_midi_host_time_now() { return 0; }
inline uint64_t core_midi_host_time_delta_samples(uint64_t, uint64_t, double) { return 0; }
inline uint64_t core_midi_dropped_input_events() { return 0; }
#endif

} // namespace Hirari::Core::Driver
