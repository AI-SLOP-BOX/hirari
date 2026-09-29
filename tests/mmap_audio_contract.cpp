#include "../src/io/mmap_audio_file.hpp"

#include <type_traits>
#include <utility>

using Hirari::IO::MMapAudioFile;

// Runtime parsing and sample-format contracts now live beside the Rust owner.
// Keep this native target focused on the compatibility adapter's public shape.
static_assert(!std::is_copy_constructible_v<MMapAudioFile>);
static_assert(!std::is_move_constructible_v<MMapAudioFile>);
static_assert(std::is_same_v<decltype(std::declval<const MMapAudioFile&>().getNumSamples()), uint64_t>);
static_assert(std::is_same_v<decltype(std::declval<const MMapAudioFile&>().getNumChannels()), uint32_t>);
static_assert(std::is_same_v<decltype(std::declval<const MMapAudioFile&>().getSample(0, 0)), float>);

int main() { return 0; }
