# AURA STUDIO PRO | Architectural Manifesto (v17.5)

## 1. The "Sanctuary" Doctrine (License Sovereignty)
To protect the integrity of the **GPLv3 Core** (Engine/UI), Aura adopts a strict **Process Separation** model for external plugins. 
- **Physical Sandboxing**: Plugins (VST/AU/CLAP) are executed in a dedicated memory space via the `AuraBridge` process.
- **IPC over Shared Memory**: Audio buffers are transferred via zero-copy shared memory regions to maintain ultra-low latency without legal "linking" contamination.
- **UI Sovereignty**: Plugin UIs are managed by the host as independent windows, ensuring that legacy or unstable VST interfaces never compromise the Slint/Vizia main render thread.

## 2. "Crash-Resistant" Orchestration
- **Fault Isolation**: A plugin failure (SIGSEGV/SIGILL) only terminates the `AuraBridge` child process. The main DAW engine continues recording and processing without interruption.
- **Auto-Recovery**: The `AuraBridge` can be warm-restarted within a single audio buffer period (1.4ms targets) to minimize disruption.

## 3. The Industrial Stack
| Layer | Technology | Role |
| :--- | :--- | :--- |
| **Nucleus** (Engine) | Rust + SIMD / WGPU | Zero-copy audio processing & spectral analysis. |
| **Face** (UI) | Slint / Vizia (GPLv3) | High-fidelity, declarative studio interface. |
| **Appendages** (Plugins) | AuraBridge (C++/Rust) | Sandboxed VST/CLAP hosting via IPC. |

## 4. Performance Goals (2026 Standards)
- **Shared Memory Zero-Copy**: Eliminating memory-copy overhead for cross-process buffers.
- **WGPU Context Sharing**: Modern CLAP plugins share GPU contexts for zero-latency UI rendering.
- **Deterministic Synchronization**: Atomic property updates between the Rust UI and the Sandboxed Bridge.

---
*Codified for Industrial Stability & Legal Integrity*
