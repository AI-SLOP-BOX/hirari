# HIRARI DAW | System Architecture (v17.5)

## 1. The "Sanctuary" Doctrine (License and Fault Boundaries)
Hirari-owned engine and UI source is MIT-licensed. The packaged desktop UI uses
Slint under its royalty-free application license and includes the required
`AboutSlint` attribution. External plugins retain their own licenses. Hirari
uses a strict **Process Separation** model for external plugins so that
untrusted code cannot compromise the realtime engine or the UI process; this
boundary is an engineering and distribution boundary, not a relicensing of
Hirari source.
- **Physical Sandboxing**: Plugins (VST/AU/CLAP) are executed in a dedicated memory space via the `HirariBridge` process.
- **IPC over Shared Memory**: Audio buffers are transferred via zero-copy shared memory regions to maintain low latency while keeping the worker lifecycle independently recoverable.
- **UI Sovereignty**: Plugin UIs are managed by the host as independent windows, ensuring that legacy or unstable VST interfaces never compromise the Slint main render thread.

## 2. "Crash-Resistant" Orchestration
- **Fault Isolation**: A plugin failure (SIGSEGV/SIGILL) only terminates the `HirariBridge` child process. The main DAW engine continues recording and processing without interruption.
- **Auto-Recovery**: The `HirariBridge` can be warm-restarted within a single audio buffer period (1.4ms targets) to minimize disruption.

## 3. The Industrial Stack
| Layer | Technology | Role |
| :--- | :--- | :--- |
| **Nucleus** (Engine) | C++ audio engine and DSP, exposed through the Rust core bridge | Realtime audio graph, DSP, and project operations. |
| **Face** (UI) | Rust/Slint application in `hirari-ui` | Desktop interface and command client with `AboutSlint` attribution. |
| **Appendages** (Plugins) | Isolated plugin worker and format adapters | Sandboxed plugin hosting over IPC. |

The C++ helper headers used by graphics components and native contracts live
under `src/graphics/ui_components/support`. The superseded C++ window and
workspace implementation is archived under `archive/legacy-cpp-ui`; neither
directory provides the desktop application's UI, which is owned by `hirari-ui`.

## 4. Performance Goals (2026 Standards)
- **Shared Memory Zero-Copy**: Eliminating memory-copy overhead for cross-process buffers.
- **WGPU Context Sharing**: Modern CLAP plugins share GPU contexts for zero-latency UI rendering.
- **Deterministic Synchronization**: Atomic property updates between the Rust UI and the Sandboxed Bridge.

---
*Codified for Industrial Stability & Legal Integrity*
