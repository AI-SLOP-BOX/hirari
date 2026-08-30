# Aura Plugin SDK

Aura plugins are loaded through the stable C++ interface in
`src/external/aura_sdk.hpp`.  A plugin exports one C symbol:
`createInstance()`.  The host owns the returned instance and calls
`initialize`, `process`, and the parameter methods from the realtime-safe
adapter.

Build the example from the repository root:

```sh
c++ -std=c++20 -I. -fsyntax-only sdk/examples/gain_plugin.cpp
```

The SDK deliberately keeps the ABI small: audio buffers are planar, state is
controlled by the host parameter contract, and the plugin must not allocate or
throw from `process`.
