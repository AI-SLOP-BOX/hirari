#pragma once

#include <cstdint>
#include "../rust_ffi.hpp"

namespace Hirari::Core::Engine {

/** Native compatibility façade for the Rust-owned scalar parameter registry. */
class ParamTree {
public:
    static ParamTree& getInstance() {
        static ParamTree instance;
        return instance;
    }

    ~ParamTree() { hirari_param_tree_destroy(m_state); }

    bool setParam(uint32_t id, float value) noexcept {
        return hirari_param_tree_set(m_state, id, value);
    }

    float getParam(uint32_t id, float fallback = 0.0f) const noexcept {
        return hirari_param_tree_get(m_state, id, fallback);
    }

    void clear() noexcept { hirari_param_tree_clear(m_state); }

    ParamTree(const ParamTree&) = delete;
    ParamTree& operator=(const ParamTree&) = delete;

private:
    ParamTree() : m_state(hirari_param_tree_create()) {}

    void* m_state;
};

} // namespace Hirari::Core::Engine
