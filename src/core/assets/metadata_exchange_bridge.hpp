#pragma once
#include <vector>
#include <string>

namespace Aura::Core::Assets {

/**
 * @struct SceneObject
 * @brief Metadata for a VFX scene object.
 */
struct SceneObject {
    std::string id;
    float posX, posY, posZ;
    float velocity;
};

/**
 * @class MetadataExchangeBridge
 * @brief Exchanges USD/scene metadata between DAW and VFX engine.
 */
class MetadataExchangeBridge {
public:
    static MetadataExchangeBridge& getInstance() {
        static MetadataExchangeBridge instance;
        return instance;
    }

    /**
     * @brief Updates local scene objects from the VFX engine's state.
     */
    void updateFromVFX(const std::vector<SceneObject>& objects) {
        // INDUSTRIAL: In a real implementation, this would parse 
        // USD (Universal Scene Description) updates or Hydra stream 
        // data to synchronize the 3D mixer's sources with video objects.
    }

private:
    MetadataExchangeBridge() = default;
};

} // namespace Aura::Core::Assets
