#include "src/io/persistence/project_decoder.hpp"

#include <cassert>

int main() {
    Aura::Core::Engine::TimelineSystem timeline;
    auto existing = std::make_shared<Aura::Core::Engine::Track>(
        77, "Existing", Aura::Core::Engine::Track::Type::Audio);
    timeline.addTrack(existing);

    const std::string malformed = R"({
      "tracks": [
        {
          "id": 12,
          "name": "Lead \"Vocal\"",
          "type": 1,
          "volume": 0.5,
          "pan": 0.0,
          "muted": true,
          "solo": false,
          "output_bus": 0,
          "audio_regions": [
            { "length": 64, "name": "phrase", "id": 9001, "start": 0,
              "clip_gain": 0.75, "warp_ratio": 1.25 }
          ]
        },
        {
          "id": 12,
          "volume": 0.2
        }
      ]
    })";
    assert(!Aura::IO::Persistence::ProjectDecoder::decode(malformed, timeline));
    assert(timeline.getTracks().size() == 1);
    assert(timeline.getTracks().front()->getId() == 77);

    const std::string valid = R"({
      "tracks": [
        {
          "id": 12,
          "name": "Lead \"Vocal\"",
          "type": 1,
          "volume": 0.5,
          "pan": 0.0,
          "muted": true,
          "solo": false,
          "output_bus": 0,
          "audio_regions": [
            { "length": 64, "name": "phrase", "id": 9001, "start": 0,
              "clip_gain": 0.75, "warp_ratio": 1.25 }
          ]
        },
        {
          "id": 13,
          "volume": 0.2,
          "pan": -0.1,
          "output_bus": 0
        }
      ]
    })";
    assert(Aura::IO::Persistence::ProjectDecoder::decode(valid, timeline));
    assert(timeline.getTracks().size() == 2);
    assert(timeline.getTracks()[0]->getId() == 12);
    assert(timeline.getTracks()[1]->getId() == 13);
    assert(timeline.getTracks()[0]->getName() == "Lead \"Vocal\"");
    assert(timeline.getTracks()[0]->getType() == Aura::Core::Engine::Track::Type::Midi);
    assert(timeline.getTracks()[0]->isMuted());
    assert(timeline.getTracks()[0]->getRegions().size() == 1);
    assert(timeline.getTracks()[0]->getRegions()[0].id == 9001);
    assert(timeline.getTracks()[0]->getRegions()[0].clipGain == 0.75f);
    assert(timeline.getTracks()[0]->getRegions()[0].warpRatio == 1.25);

    // A present region field with the wrong type must fail closed. Silently
    // ignoring it would turn malformed input into a different project.
    const std::string wrongRegionType = R"({
      "tracks": [{"id": 21, "name": "Broken", "audio_regions": {"id": 1}}]
    })";
    assert(!Aura::IO::Persistence::ProjectDecoder::decode(wrongRegionType, timeline));
    assert(timeline.getTracks().size() == 2);
    assert(timeline.getTracks()[0]->getId() == 12);

    const std::string invalidRegionValue = R"({
      "tracks": [{"id": 22, "audio_regions": [
        {"id": 2, "start": 0, "length": 8, "clip_gain": "not-a-number"}
      ]}]
    })";
    assert(!Aura::IO::Persistence::ProjectDecoder::decode(invalidRegionValue, timeline));
    assert(timeline.getTracks().size() == 2);

    auto duplicateA = std::make_shared<Aura::Core::Engine::Track>(31, "A", Aura::Core::Engine::Track::Type::Audio);
    auto duplicateB = std::make_shared<Aura::Core::Engine::Track>(31, "B", Aura::Core::Engine::Track::Type::Audio);
    assert(!timeline.replaceTracks({duplicateA, duplicateB}));
    assert(timeline.getTracks().size() == 2);
    return 0;
}
