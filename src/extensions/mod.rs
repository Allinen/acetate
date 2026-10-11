//! The extensions that are loaded (see `acetate_extensions` and docs/admin/extensions.md):
//! those that come with Acetate and the ones in `ACETATE_EXTENSIONS_DIR`.

pub mod fetch;

use std::path::Path;
use std::sync::OnceLock;

use acetate_extensions::Registry;

static LOADED: OnceLock<Registry> = OnceLock::new();

/// Loads the extensions, once, at start (`dir`: the admin's folder). What can't be loaded is
/// said and left out.
pub fn load(dir: Option<&Path>) {
    let registry = match dir {
        Some(dir) => {
            let (registry, problems) = Registry::with_folder(dir);
            for problem in problems {
                tracing::warn!("extension skipped: {problem}");
            }
            registry
        }
        None => Registry::builtin().clone(),
    };
    tracing::info!("extensions: {}", registry.extensions.iter().map(|e| e.extension.name.as_str()).collect::<Vec<_>>().join(", "));
    let _ = LOADED.set(registry);
}

/// The extensions in use: the built-in ones until `load` has been called.
pub fn registry() -> &'static Registry {
    LOADED.get().unwrap_or_else(|| Registry::builtin())
}
