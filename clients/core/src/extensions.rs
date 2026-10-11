//! The extensions the server has (`GET /api/extensions`), for reading pasted links and for
//! the forms of a platform's sources. Until the server's are here, the ones that come with
//! Acetate.

use std::sync::{Arc, OnceLock, RwLock};

pub use acetate_extensions::Registry;

fn cell() -> &'static RwLock<Arc<Registry>> {
    static CURRENT: OnceLock<RwLock<Arc<Registry>>> = OnceLock::new();
    CURRENT.get_or_init(|| RwLock::new(Arc::new(Registry::builtin().clone())))
}

/// The extensions in use.
pub fn current() -> Arc<Registry> {
    cell().read().unwrap().clone()
}

/// The server's extensions arrived.
pub fn set(registry: Registry) {
    *cell().write().unwrap() = Arc::new(registry);
}
