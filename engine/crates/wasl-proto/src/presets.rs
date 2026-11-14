//! Built-in ignore preset identifiers shared with the CLI and daemon.

/// Known ignore preset identifiers accepted by `wasl pair add --preset`.
pub const PRESET_IDS: [&str; 8] = [
    "node",
    "python",
    "rust",
    "android-gradle",
    "flutter",
    "docker",
    "editors",
    "os-junk",
];

/// Returns true if `id` is a recognized built-in ignore preset identifier.
pub fn is_valid_preset_id(id: &str) -> bool {
    PRESET_IDS.contains(&id)
}
