#pragma once

#include <string>
#include <cstddef>
#include <cstdint>
#include <vector>

namespace Hirari::Core::Driver {

#if defined(__APPLE__)
std::string list_core_midi_devices_json();
bool send_core_midi_message(uint32_t uniqueId, const uint8_t* data, size_t size);
bool start_core_midi_input();
void stop_core_midi_input();
std::string poll_core_midi_input_json();
uint64_t core_midi_host_time_now();
uint64_t core_midi_host_time_delta_samples(uint64_t start, uint64_t event, double sampleRate);
uint64_t core_midi_dropped_input_events();
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
