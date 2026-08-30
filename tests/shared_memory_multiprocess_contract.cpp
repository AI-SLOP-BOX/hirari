#include "src/network/shared_memory_bridge.hpp"

#include <array>
#include <cstdlib>
#include <cstring>
#include <sys/wait.h>
#include <unistd.h>

namespace {
constexpr const char* kConsumerMode = "--consumer";
}

int main(int argc, char** argv) {
    if (argc == 3 && std::strcmp(argv[1], kConsumerMode) == 0) {
        auto& bridge = Aura::Network::SharedMemoryBridge::getInstance();
        if (!bridge.openExisting(argv[2])) return 10;

        Aura::Core::Plugins::MidiExtendedMessageRing::Message message;
        if (!bridge.popExtendedMidi(message) || message.size != 300 ||
            message.sampleOffset != 1234 || message.articulationId != 7 ||
            message.data.front() != 0xf0 || message.data[299] != 0xf7) return 11;

        const std::array<uint8_t, 2> ack{0x90, 0x7f};
        if (!bridge.pushExtendedMidi(5678, 9, ack.data(), ack.size())) return 12;
        bridge.stop();
        return 0;
    }

    if (argc != 1) return 1;
    auto& bridge = Aura::Network::SharedMemoryBridge::getInstance();

    // A same-sized but stale segment must not be accepted as a current
    // SharedState. The consumer also must not unlink a segment it did not
    // create.
    const std::string staleName = "/aura_shared_state_stale_" +
        std::to_string(static_cast<unsigned long long>(::getpid()));
    const int staleFd = shm_open(staleName.c_str(), O_CREAT | O_EXCL | O_RDWR, 0600);
    if (staleFd == -1 || ftruncate(staleFd, sizeof(Aura::Network::SharedMemoryBridge::SharedState)) != 0) {
        if (staleFd >= 0) close(staleFd);
        shm_unlink(staleName.c_str());
        return 8;
    }
    void* staleMap = mmap(nullptr, sizeof(Aura::Network::SharedMemoryBridge::SharedState),
                          PROT_READ | PROT_WRITE, MAP_SHARED, staleFd, 0);
    if (staleMap == MAP_FAILED) {
        close(staleFd);
        shm_unlink(staleName.c_str());
        return 9;
    }
    auto* staleState = ::new (staleMap) Aura::Network::SharedMemoryBridge::SharedState{};
    if (staleState->protocolVersion.load(std::memory_order_acquire) != 0 ||
        bridge.openExisting(staleName)) {
        staleState->~SharedState();
        munmap(staleMap, sizeof(*staleState));
        close(staleFd);
        shm_unlink(staleName.c_str());
        return 13;
    }
    staleState->~SharedState();
    munmap(staleMap, sizeof(*staleState));
    close(staleFd);
    shm_unlink(staleName.c_str());

    const std::array<uint8_t, 300> payload = [] {
        std::array<uint8_t, 300> value{};
        value.front() = 0xf0;
        value.back() = 0xf7;
        return value;
    }();
    unsigned rounds = 32;
    if (const char* configured = std::getenv("AURA_SHARED_MEMORY_ROUNDS")) {
        const unsigned parsed = static_cast<unsigned>(std::strtoul(configured, nullptr, 10));
        if (parsed > 0) rounds = parsed;
    }

    for (unsigned round = 0; round < rounds; ++round) {
        if (!bridge.start()) return 2;
        if (!bridge.pushExtendedMidi(1234, 7, payload.data(), payload.size())) {
            bridge.stop();
            return 3;
        }

        const pid_t child = fork();
        if (child == -1) {
            bridge.stop();
            return 4;
        }
        if (child == 0) {
            execl(argv[0], argv[0], kConsumerMode, bridge.segmentName().c_str(), nullptr);
            _exit(127);
        }

        int status = 0;
        if (waitpid(child, &status, 0) != child || !WIFEXITED(status) || WEXITSTATUS(status) != 0) {
            bridge.stop();
            return 5;
        }

        Aura::Core::Plugins::MidiExtendedMessageRing::Message ack;
        if (!bridge.popExtendedMidi(ack) || ack.size != 2 || ack.sampleOffset != 5678 ||
            ack.articulationId != 9 || ack.data[0] != 0x90 || ack.data[1] != 0x7f) {
            bridge.stop();
            return 6;
        }
        bridge.stop();
        if (!bridge.segmentName().empty()) return 7;
    }
    return 0;
}
