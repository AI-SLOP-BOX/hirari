#pragma once

#include <string>
#include <cstddef>
#include <cstdint>

namespace Aura::Core::Driver {

#if defined(__APPLE__)
std::string list_core_midi_devices_json();
bool send_core_midi_message(uint32_t uniqueId, const uint8_t* data, size_t size);
bool start_core_midi_input();
void stop_core_midi_input();
std::string poll_core_midi_input_json();
#else
inline std::string list_core_midi_devices_json() { return "[]"; }
inline bool send_core_midi_message(uint32_t, const uint8_t*, size_t) { return false; }
inline bool start_core_midi_input() { return false; }
inline void stop_core_midi_input() {}
inline std::string poll_core_midi_input_json() { return "[]"; }
#endif

} // namespace Aura::Core::Driver
