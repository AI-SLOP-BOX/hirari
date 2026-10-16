//! Launch gate: the application window must boot on the WGPU backend.
//!
//! The UI requires WGPU 29 at startup, and the adapter probe passes on any
//! machine with a GPU, so backend *selection* succeeds either way. Renderer
//! creation is deferred to window creation: with no WGPU-capable renderer
//! compiled in, the request falls through to a suspended OpenGL renderer and
//! the first frame dies inside the event loop with "WGPU 29.x rendering is
//! not supported with an OpenGL renderer". Only booting the real binary
//! through a short event-loop run catches that.
//!
//! The child runs on its own main thread, which matters: on macOS the winit
//! event loop must be created on the main thread, so no in-process unit test
//! can boot the backend. macOS-only for the same reason a window server is
//! required; the shipping platform is macOS and a GPU-backed adapter is
//! required (this is a launch gate, not a software-rendering test).
//!
//! The window opens on screen for the duration of the run. That is the
//! point: this test observes the real startup path, not a mock of it.

#[cfg(target_os = "macos")]
#[test]
fn wgpu_backend_boots_the_application_window() {
    let binary = option_env!("CARGO_BIN_EXE_hirari_ui")
        .or(option_env!("CARGO_BIN_EXE_hirari-ui"))
        .expect("integration tests must resolve the application binary");
    let output = std::process::Command::new(binary)
        .env("HIRARI_SMOKE_BOOT_MS", "1500")
        .output()
        .expect("application binary must launch");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "boot must exit cleanly\n--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}"
    );
    assert!(
        !stderr.contains("not supported with an OpenGL renderer"),
        "WGPU request must not fall through to the OpenGL renderer\n--- stderr ---\n{stderr}"
    );
    assert!(
        stdout.contains("HIRARI_SMOKE_BOOT_OK"),
        "event loop must run to the timed shutdown\n--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}"
    );
}
