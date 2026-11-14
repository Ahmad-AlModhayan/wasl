//! Built-in rule sets for dependency, build and junk folders (SRS FR-G3).
//!
//! Presets are gitignore lines evaluated below every rule file, so a pair's own `.waslignore` can
//! re-include anything a preset excludes (for example `!build/` in an Android project that keeps
//! a hand-written `build` folder).

use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Preset {
    Node,
    Python,
    Rust,
    AndroidGradle,
    Flutter,
    Docker,
    Editors,
    OsJunk,
}

impl Preset {
    pub const ALL: [Preset; 8] = [
        Preset::Node,
        Preset::Python,
        Preset::Rust,
        Preset::AndroidGradle,
        Preset::Flutter,
        Preset::Docker,
        Preset::Editors,
        Preset::OsJunk,
    ];

    /// Stable identifier used in settings and in `explain` output (`preset:node`).
    pub fn id(self) -> &'static str {
        match self {
            Preset::Node => "node",
            Preset::Python => "python",
            Preset::Rust => "rust",
            Preset::AndroidGradle => "android-gradle",
            Preset::Flutter => "flutter",
            Preset::Docker => "docker",
            Preset::Editors => "editors",
            Preset::OsJunk => "os-junk",
        }
    }

    pub fn from_id(id: &str) -> Option<Preset> {
        Preset::ALL.into_iter().find(|p| p.id() == id)
    }

    /// The preset's rules, in gitignore syntax.
    pub fn lines(self) -> &'static [&'static str] {
        match self {
            Preset::Node => &[
                "node_modules/",
                "bower_components/",
                ".npm/",
                ".pnpm-store/",
                ".yarn/cache/",
                ".yarn/unplugged/",
                ".yarn/install-state.gz",
                "dist/",
                ".next/",
                ".nuxt/",
                ".svelte-kit/",
                ".astro/",
                ".turbo/",
                ".parcel-cache/",
                ".vite/",
                ".angular/cache/",
                ".eslintcache",
            ],
            Preset::Python => &[
                "__pycache__/",
                "*.py[cod]",
                ".venv/",
                "venv/",
                ".tox/",
                ".nox/",
                ".mypy_cache/",
                ".pytest_cache/",
                ".ruff_cache/",
                ".pyre/",
                ".ipynb_checkpoints/",
                "*.egg-info/",
                ".eggs/",
                "__pypackages__/",
            ],
            Preset::Rust => &["target/"],
            Preset::AndroidGradle => &[
                ".gradle/",
                "build/",
                ".cxx/",
                ".externalNativeBuild/",
                "captures/",
                ".kotlin/",
                "local.properties",
            ],
            Preset::Flutter => &[
                ".dart_tool/",
                ".pub-cache/",
                ".pub/",
                "build/",
                ".flutter-plugins",
                ".flutter-plugins-dependencies",
            ],
            Preset::Docker => &[".docker/", "docker-data/", "*.tar.zst.partial"],
            Preset::Editors => &[
                ".idea/workspace.xml",
                ".idea/tasks.xml",
                ".idea/usage.statistics.xml",
                ".idea/shelf/",
                ".idea/caches/",
                ".vscode/.browse.VC.db*",
                ".vscode/ipch/",
                ".vs/",
                ".history/",
                "*.swp",
                "*.swo",
                "*~",
            ],
            Preset::OsJunk => &[
                ".DS_Store",
                "._*",
                ".Spotlight-V100/",
                ".Trashes/",
                ".fseventsd/",
                "Thumbs.db",
                "ehthumbs.db",
                "desktop.ini",
                "$RECYCLE.BIN/",
                ".directory",
                ".Trash-*/",
            ],
        }
    }
}

impl fmt::Display for Preset {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "preset:{}", self.id())
    }
}

#[cfg(test)]
mod tests {
    use super::Preset;

    #[test]
    fn preset_ids_match_proto() {
        let ids: Vec<&str> = Preset::ALL.iter().map(|p| p.id()).collect();
        assert_eq!(ids, wasl_proto::PRESET_IDS);
    }
}
