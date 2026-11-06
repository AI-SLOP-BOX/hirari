// The project-load session, driven through the engine it replaced. These tests
// hold the three promises the deleted C++ loader made: a saved document takes
// over the graph, one hydration runs at a time per engine, and a document that
// cannot be read leaves the open project published.

static PROJECT_LOAD_SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

struct ProjectLoadEngine {
    _guard: std::sync::MutexGuard<'static, ()>,
    engine: Box<crate::AudioEngine>,
    path: String,
}

impl ProjectLoadEngine {
    fn new(label: &str) -> Self {
        let _guard = native_engine_test_guard();
        let engine = crate::new_audio_engine();
        assert!(!false);
        let unique =
            PROJECT_LOAD_SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let path = std::env::temp_dir()
            .join(format!(
                "hirari-project-load-{label}-{}-{}.hirari",
                std::process::id(),
                unique
            ))
            .to_string_lossy()
            .into_owned();
        Self { _guard, engine, path }
    }

    fn core(&self) -> &crate::AudioEngine {
        &*self.engine
    }

    fn host(&self) -> *mut std::ffi::c_void {
        crate::audio_engine_delegate_ext::host(self.core())
    }

    fn layout(&self) -> serde_json::Value {
        serde_json::from_str(&self.core().project_layout_json())
            .expect("native layout must be valid JSON")
    }

    fn save(&self) -> bool {
        let bytes = self.path.as_bytes();
        unsafe {
            crate::layout_gather::hirari_project_save_from_host(
                self.host(),
                bytes.as_ptr(),
                bytes.len(),
            )
        }
    }

    fn load_at(&self, path: &str) -> bool {
        let bytes = path.as_bytes();
        unsafe {
            crate::project_load_host::hirari_project_load_from_host(
                self.host(),
                bytes.as_ptr(),
                bytes.len(),
            )
        }
    }

    fn load(&self) -> bool {
        self.load_at(&self.path)
    }

    fn cleanup(&self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

#[test]
fn a_saved_document_takes_over_the_graph() {
    let fixture = ProjectLoadEngine::new("roundtrip");
    let saved = fixture.core().add_track(0);
    assert_ne!(saved, 0);
    assert!(fixture.core().set_track_name(saved, "saved-state"));
    assert!(fixture.save());

    let current = fixture.core().add_track(0);
    assert_ne!(current, 0);
    assert!(fixture.core().set_track_name(current, "current-state"));
    assert_eq!(fixture.layout().as_array().unwrap().len(), 2);

    assert!(fixture.load(), "the document must load");
    let layout = fixture.layout();
    let tracks = layout.as_array().unwrap();
    assert_eq!(tracks.len(), 1, "the loaded document replaces the graph");
    assert_eq!(tracks[0]["name"], "saved-state");
    assert_eq!(
        tracks[0]["id"].as_u64(),
        Some(u64::from(saved)),
        "a track keeps the id the document carries"
    );
    fixture.cleanup();
}

#[test]
fn one_hydration_runs_per_session() {
    let fixture = ProjectLoadEngine::new("guard");
    let track = fixture.core().add_track(0);
    assert_ne!(track, 0);
    assert!(fixture.core().set_track_name(track, "published"));
    assert!(fixture.save());

    let before = fixture.layout();
    let flag = crate::engine_session::handle_of_host(
        fixture.host(),
        crate::engine_session::H_HYDRATION_GUARD,
    )
    .cast::<std::sync::atomic::AtomicBool>();
    assert!(!flag.is_null(), "the session bag carries the hydration flag");
    unsafe { (*flag).store(true, std::sync::atomic::Ordering::Release) };

    assert!(!fixture.load(), "a second hydration is refused, not queued");
    assert_eq!(fixture.layout().to_string(), before.to_string());

    unsafe { (*flag).store(false, std::sync::atomic::Ordering::Release) };
    assert!(fixture.load(), "the flag releases with the transaction");
    fixture.cleanup();
}

#[test]
fn an_unreadable_document_leaves_the_published_project_alone() {
    let fixture = ProjectLoadEngine::new("unreadable");
    let track = fixture.core().add_track(0);
    assert_ne!(track, 0);
    assert!(fixture.core().set_track_name(track, "keep-me"));
    let before = fixture.layout().to_string();

    let missing = std::env::temp_dir()
        .join(format!(
            "hirari-project-load-missing-{}.hirari",
            std::process::id()
        ))
        .to_string_lossy()
        .into_owned();
    assert!(!fixture.load_at(&missing));

    std::fs::write(&fixture.path, b"not an ARUA container").unwrap();
    assert!(!fixture.load());
    assert_eq!(fixture.layout().to_string(), before);
    fixture.cleanup();
}
