use std::sync::{Arc, Mutex, OnceLock};

/// `SessionResourceCleanup = (sessionId?: string) => void`
pub type SessionResourceCleanup = Arc<dyn Fn(Option<&str>) + Send + Sync>;

fn cleanups() -> &'static Mutex<Vec<(u64, SessionResourceCleanup)>> {
    static CLEANUPS: OnceLock<Mutex<Vec<(u64, SessionResourceCleanup)>>> = OnceLock::new();
    CLEANUPS.get_or_init(|| Mutex::new(Vec::new()))
}

/// Handle returned by [`register_session_resource_cleanup`]; dropping it does nothing,
/// calling [`SessionResourceCleanupHandle::unregister`] removes the cleanup.
#[derive(Debug, Clone, Copy)]
pub struct SessionResourceCleanupHandle(u64);

impl SessionResourceCleanupHandle {
    pub fn unregister(self) {
        let mut cleanups = cleanups().lock().expect("poisoned");
        cleanups.retain(|(id, _)| *id != self.0);
    }
}

/// `registerSessionResourceCleanup(cleanup)`
pub fn register_session_resource_cleanup(
    cleanup: SessionResourceCleanup,
) -> SessionResourceCleanupHandle {
    static NEXT_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let id = NEXT_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    cleanups().lock().expect("poisoned").push((id, cleanup));
    SessionResourceCleanupHandle(id)
}

/// `cleanupSessionResources(sessionId?)`
/// cannot fail, so every one of them runs and nothing is collected.
pub fn cleanup_session_resources(session_id: Option<&str>) {
    let registered: Vec<SessionResourceCleanup> = cleanups()
        .lock()
        .expect("poisoned")
        .iter()
        .map(|(_, cleanup)| cleanup.clone())
        .collect();
    for cleanup in registered {
        cleanup(session_id);
    }
}
