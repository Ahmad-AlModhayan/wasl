//! Helpers shared by the store tests.

/// A folder path that is absolute on this OS (`/name`, or `C:\name` on Windows), so moving a pair
/// between two of them reads as a move everywhere (`wasl_core::paths::comparable`).
pub fn root(name: &str) -> String {
    if cfg!(windows) {
        format!(r"C:\{name}")
    } else {
        format!("/{name}")
    }
}
