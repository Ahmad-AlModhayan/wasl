//! Bakes the Google sign-in client (an "installed app" client) into wasld. It comes from the
//! environment (`WASL_GOOGLE_CLIENT_ID`, `WASL_GOOGLE_CLIENT_SECRET`, as CI would set them) or
//! from a file outside the repository: `WASL_GOOGLE_ENV_FILE`, else
//! `~/.wasl-build/google-desktop.env`, with `KEY=value` lines. Without either, wasld builds and
//! says at sign-in that this build has no Google client.

use std::path::PathBuf;

const KEYS: [&str; 2] = ["WASL_GOOGLE_CLIENT_ID", "WASL_GOOGLE_CLIENT_SECRET"];

fn main() {
    for k in KEYS
        .iter()
        .chain(["WASL_GOOGLE_ENV_FILE", "USERPROFILE", "HOME"].iter())
    {
        println!("cargo:rerun-if-env-changed={k}");
    }
    let file = std::env::var_os("WASL_GOOGLE_ENV_FILE")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("USERPROFILE")
                .or_else(|| std::env::var_os("HOME"))
                .map(|h| PathBuf::from(h).join(".wasl-build").join("google-desktop.env"))
        });
    let mut from_file = Vec::new();
    if let Some(f) = &file {
        println!("cargo:rerun-if-changed={}", f.display());
        if let Ok(text) = std::fs::read_to_string(f) {
            for line in text.lines().map(str::trim).filter(|l| !l.starts_with('#')) {
                if let Some((k, v)) = line.split_once('=') {
                    from_file.push((k.trim().to_owned(), v.trim().to_owned()));
                }
            }
        }
    }
    for key in KEYS {
        let value = std::env::var(key)
            .ok()
            .filter(|v| !v.is_empty())
            .or_else(|| from_file.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone()));
        if let Some(v) = value {
            println!("cargo:rustc-env={key}={v}");
        }
    }
}
