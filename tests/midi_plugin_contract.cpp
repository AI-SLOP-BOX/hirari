#include <cassert>
#include <filesystem>
#include <fstream>
#include <string>
#include <vector>

#include "../src/core/midi_buffer.hpp"
#include "../src/core/plugins/plugin_host.hpp"

int main() {
    Aura::Core::MidiBuffer midi;
    midi.addNoteOn(1, 60, 100, static_cast<uint64_t>(UINT32_MAX) + 50u);
    assert(midi.size() == 1);
    uint32_t offset = 0;
    uint32_t size = 0;
    uint8_t data[8] = {};
    Aura::Core::MidiBuffer::Iterator iterator(midi);
    assert(iterator.getNextEvent(offset, data, size));
    assert(offset == UINT32_MAX && size == 3 && data[1] == 60);
    midi.clear();
    assert(midi.size() == 0 && !midi.overflowed() && midi.droppedEvents() == 0);

    const auto root = std::filesystem::temp_directory_path() / "aura-plugin-scan-contract";
    std::error_code ec;
    std::filesystem::create_directories(root / "nested", ec);
    std::ofstream(root / "Synth.vst3").put('\0');
    std::ofstream(root / "nested" / "Drums.clap").put('\0');
    std::ofstream(root / "notes.txt").put('\0');
    const auto plugins = Aura::Core::Plugins::PluginScanner::scanFolders({root.string()});
    assert(plugins.size() == 2);
    assert(plugins[0].binaryPath < plugins[1].binaryPath);
    std::filesystem::remove_all(root, ec);
    return 0;
}
