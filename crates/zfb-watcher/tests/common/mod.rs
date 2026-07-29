//! Shared helpers for the backend-parameterized integration tests
//! (`smoke.rs`, `extra_paths.rs`, `real_fs_behavior.rs`).

use std::time::Duration;

use zfb_watcher::WatchBackend;

/// The poll backend at a short test interval — NOT the production
/// [`zfb_watcher::DEFAULT_POLL_INTERVAL`] (500ms). 100ms keeps every
/// condition-keyed wait budget comfortable (worst-case observation cost is
/// one interval + the debounce window) without busy-scanning the tempdir.
pub fn poll_backend() -> WatchBackend {
    WatchBackend::Poll {
        interval: Duration::from_millis(100),
    }
}
