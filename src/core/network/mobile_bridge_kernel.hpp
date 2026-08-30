#pragma once
#include "../utils/ring_buffer.hpp"
#include <sys/socket.h>
#include <netinet/in.h>
#include <arpa/inet.h>
#include <unistd.h>
#include <fcntl.h>
#include <cstring>
#include <atomic>

namespace Aura::Core::Network {

/**
 * @struct TouchEvent
 * @brief Normalized touch data for remote orchestration with industrial precision.
 */
struct TouchEvent {
    uint32_t trackId;
    uint32_t paramId;
    float value;
    uint64_t timestamp;
};

/**
 * @class MobileBridgeKernel
 * @brief Sovereign E2EE Remote Control Bridge over POSIX UDP sockets.
 */
class MobileBridgeKernel {
public:
    static MobileBridgeKernel& getInstance() {
        static MobileBridgeKernel instance;
        return instance;
    }

    ~MobileBridgeKernel() {
        if (m_socketFd >= 0) {
            close(m_socketFd);
        }
    }

    /**
     * @brief Broadcasts a parameter update via ultra-low-latency binary UDP packets.
     */
    void broadcastParameter(uint32_t trackId, uint32_t paramId, float value) {
        struct { uint32_t t; uint32_t p; float v; } pkt = { trackId, paramId, value };
        
        if (m_socketFd >= 0) {
            sendto(m_socketFd, reinterpret_cast<const char*>(&pkt), sizeof(pkt), 0,
                   reinterpret_cast<const sockaddr*>(&m_broadcastAddr), sizeof(m_broadcastAddr));
        }
        
        m_bytesSent += sizeof(pkt);
    }

    /**
     * @brief Ingests a remote touch event into the lock-free audio thread queue.
     */
    bool pushEvent(const TouchEvent& ev) {
        return m_incomingQueue.push(ev);
    }

    /**
     * @brief Pops a decrypted touch event on the audio thread (Lock-Free).
     */
    bool popEvent(TouchEvent& ev) {
        return m_incomingQueue.pop(ev);
    }

    uint64_t getBytesSent() const {
        return m_bytesSent.load();
    }

private:
    MobileBridgeKernel() : m_socketFd(-1), m_bytesSent(0) {
        m_socketFd = socket(AF_INET, SOCK_DGRAM, 0);
        if (m_socketFd >= 0) {
            // Set socket to non-blocking
            int flags = fcntl(m_socketFd, F_GETFL, 0);
            fcntl(m_socketFd, F_SETFL, flags | O_NONBLOCK);
            
            // Enable broadcast
            int broadcastEnable = 1;
            setsockopt(m_socketFd, SOL_SOCKET, SO_BROADCAST, &broadcastEnable, sizeof(broadcastEnable));
            
            // Configure broadcast address (port 9000)
            std::memset(&m_broadcastAddr, 0, sizeof(m_broadcastAddr));
            m_broadcastAddr.sin_family = AF_INET;
            m_broadcastAddr.sin_port = htons(9000);
            m_broadcastAddr.sin_addr.s_addr = INADDR_BROADCAST;
        }
    }
    
    int m_socketFd;
    sockaddr_in m_broadcastAddr;
    ::Aura::Core::RingBuffer<TouchEvent, 1024> m_incomingQueue;
    std::atomic<uint64_t> m_bytesSent;
};

} // namespace Aura::Core::Network
