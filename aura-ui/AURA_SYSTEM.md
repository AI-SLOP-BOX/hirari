# AURA STUDIO PRO | Architectural Manifesto (v17.5)

## 1. The "Sanctuary" Doctrine (License and Fault Boundaries)
Aura-owned engine and UI source is MIT-licensed. The packaged desktop UI uses
Slint under its royalty-free application license and includes the required
`AboutSlint` attribution. External plugins retain their own licenses. Aura
uses a strict **Process Separation** model for external plugins so that
untrusted code cannot compromise the realtime engine or the UI process; this
boundary is an engineering and distribution boundary, not a relicensing of
Aura source.
- **Physical Sandboxing**: Plugins (VST/AU/CLAP) are executed in a dedicated memory space via the `AuraBridge` process.
- **IPC over Shared Memory**: Audio buffers are transferred via zero-copy shared memory regions to maintain low latency while keeping the worker lifecycle independently recoverable.
- **UI Sovereignty**: Plugin UIs are managed by the host as independent windows, ensuring that legacy or unstable VST interfaces never compromise the Slint main render thread.

## 2. "Crash-Resistant" Orchestration
- **Fault Isolation**: A plugin failure (SIGSEGV/SIGILL) only terminates the `AuraBridge` child process. The main DAW engine continues recording and processing without interruption.
- **Auto-Recovery**: The `AuraBridge` can be warm-restarted within a single audio buffer period (1.4ms targets) to minimize disruption.

## 3. The Industrial Stack
| Layer | Technology | Role |
| :--- | :--- | :--- |
| **Nucleus** (Engine) | Rust + SIMD / WGPU | Zero-copy audio processing & spectral analysis. |
| **Face** (UI) | Slint (royalty-free application license) | High-fidelity, declarative studio interface with `AboutSlint` attribution. |
| **Appendages** (Plugins) | AuraBridge (C++/Rust) | Sandboxed VST/CLAP hosting via IPC. |

## 4. Performance Goals (2026 Standards)
- **Shared Memory Zero-Copy**: Eliminating memory-copy overhead for cross-process buffers.
- **WGPU Context Sharing**: Modern CLAP plugins share GPU contexts for zero-latency UI rendering.
- **Deterministic Synchronization**: Atomic property updates between the Rust UI and the Sandboxed Bridge.

---
*Codified for Industrial Stability & Legal Integrity*
