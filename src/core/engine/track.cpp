#include "track.hpp"
#include "../../scae/AuraAISuite.hpp"

namespace Aura::Core::Engine {

// Keep this translation unit non-empty while the track implementation remains
// header-oriented. This prevents the static archive from containing an
// object with no symbols when build.rs discovers all engine sources.
void track_translation_unit_anchor() noexcept {}

} // namespace Aura::Core::Engine
