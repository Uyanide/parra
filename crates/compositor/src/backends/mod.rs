pub mod niri;

use crate::CompositorBackend;

/// Picks a backend from the environment, in the order listed.
pub fn detect() -> Option<Box<dyn CompositorBackend>> {
    if let Some(backend) = niri::detect() {
        return Some(Box::new(backend));
    }
    None
}

/// Names of every backend that could be selected.
pub const AVAILABLE: &[&str] = &[niri::NAME];
