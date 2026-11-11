//! The update check (SRS FR-R5, #196): a plain check against the version file the Wasl site publishes
//! (`site/public/version.json`), at most once a day plus whenever Settings › About asks. It never installs
//! anything and sends nothing but the request itself; when a newer version is out, About shows it with a
//! link to the download page. A failed check is silent, apart from "Couldn't check" in About.

use serde::{Deserialize, Serialize};

/// The site's version file.
pub const VERSION_URL: &str = "https://wasl.kahflabs.com/version.json";
/// Download pages are on the Wasl site; any other address in the file makes the check fail.
const SITE: &str = "https://wasl.kahflabs.com/";
/// Once a day.
pub const EVERY_MS: i64 = 24 * 60 * 60 * 1000;
/// After a check that failed, the next one is due within the hour, not the next day (#205).
pub const RETRY_MS: i64 = 60 * 60 * 1000;
/// The most of the site's file that is read; it is a few hundred bytes (#205).
pub const MAX_BYTES: usize = 64 * 1024;

/// What the site's file says.
#[derive(Debug, Deserialize)]
struct SiteVersion {
    version: String,
    url: SiteUrl,
}

#[derive(Debug, Deserialize)]
struct SiteUrl {
    en: String,
    ar: String,
}

/// A check's answer, for About.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Update {
    /// This is the newest version.
    Current,
    /// A newer version is out, and its download page in the window's language.
    Newer { version: String, url: String },
    /// The file couldn't be read or made no sense.
    Failed,
}

/// The last check, kept in the app's config folder so a restart doesn't check again within the day.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LastCheck {
    pub at_ms: i64,
    pub version: String,
    pub url_en: String,
    pub url_ar: String,
}

/// "0.4.2" → [0, 4, 2], and "0.5.0-beta.1" → [0, 5, 0] with the pre-release tag ["beta", "1"].
fn parts(v: &str) -> Option<(Vec<u64>, Option<Vec<&str>>)> {
    let v = v.trim().trim_start_matches('v');
    let (core, pre) = match v.split_once('-') {
        Some((c, p)) => (c, Some(p.split('.').collect::<Vec<_>>())),
        None => (v, None),
    };
    if pre.as_ref().is_some_and(|p| p.iter().any(|id| id.is_empty())) {
        return None;
    }
    let nums = core.split('.').map(|p| p.parse::<u64>().ok()).collect::<Option<Vec<_>>>()?;
    (!nums.is_empty()).then_some((nums, pre))
}

/// Two pre-release tags of one version, as semver orders them: identifier by identifier, numbers by value and
/// below words, words by their ASCII text, and a tag that is the start of a longer one first.
fn cmp_pre(a: &[&str], b: &[&str]) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    for (x, y) in a.iter().zip(b) {
        let o = match (x.parse::<u64>(), y.parse::<u64>()) {
            (Ok(m), Ok(n)) => m.cmp(&n),
            (Ok(_), Err(_)) => Ordering::Less,
            (Err(_), Ok(_)) => Ordering::Greater,
            (Err(_), Err(_)) => x.cmp(y),
        };
        if o != Ordering::Equal {
            return o;
        }
    }
    a.len().cmp(&b.len())
}

/// Whether `latest` is newer than `current`. Versions that can't be read are never newer.
pub fn is_newer(current: &str, latest: &str) -> bool {
    let (Some((mut c, c_pre)), Some((mut l, l_pre))) = (parts(current), parts(latest)) else {
        return false;
    };
    let n = c.len().max(l.len());
    c.resize(n, 0);
    l.resize(n, 0);
    match l.cmp(&c) {
        std::cmp::Ordering::Greater => true,
        std::cmp::Ordering::Less => false,
        // A release is newer than its pre-releases.
        std::cmp::Ordering::Equal => match (c_pre, l_pre) {
            (Some(_), None) => true,
            (Some(c), Some(l)) => cmp_pre(&l, &c).is_gt(),
            (None, _) => false,
        },
    }
}

/// Reads the site's file. None when it isn't the file's shape, or a download page isn't on the Wasl site.
pub fn parse(body: &str, now_ms: i64) -> Option<LastCheck> {
    let v: SiteVersion = serde_json::from_str(body).ok()?;
    parts(&v.version)?;
    let (url_en, url_ar) = (site_url(&v.url.en)?, site_url(&v.url.ar)?);
    Some(LastCheck { at_ms: now_ms, version: v.version, url_en, url_ar })
}

/// Whether the daily check is due.
pub fn due(last_at_ms: Option<i64>, now_ms: i64) -> bool {
    last_at_ms.is_none_or(|at| now_ms - at >= EVERY_MS || now_ms < at)
}

/// What About says, from the last good check (None: the last check failed).
pub fn answer(current: &str, last: Option<&LastCheck>, lang: &str) -> Update {
    match last {
        None => Update::Failed,
        Some(l) if is_newer(current, &l.version) => Update::Newer {
            version: l.version.clone(),
            url: if lang == "ar" { l.url_ar.clone() } else { l.url_en.clone() },
        },
        Some(_) => Update::Current,
    }
}

/// A page on the Wasl site, re-built from its checked path: `https`, the host exactly `wasl.kahflabs.com`, no
/// user, port, query or fragment, and a path of `[A-Za-z0-9/._-]` only, with no `..`. Anything else is None.
/// Every address Wasl opens in the browser passes here first: `explorer`, `open` and `xdg-open` get no quotes,
/// `^`, `%`, commas, spaces or control characters (#199 review).
pub fn site_url(u: &str) -> Option<String> {
    let path = u.strip_prefix(SITE)?;
    let plain = path.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '.' | '_' | '-'));
    if !plain || path.split('/').any(|seg| seg == "..") || path.starts_with('/') {
        return None;
    }
    Some(format!("{SITE}{path}"))
}

/// What the config folder keeps: when the last check ran, whether it failed, and the last answer that worked.
/// A failed check keeps the good answer before it, so a newer version already found isn't forgotten (#207 review).
#[derive(Debug, Default, Serialize, Deserialize)]
struct Kept {
    at_ms: Option<i64>,
    last: Option<LastCheck>,
    /// The check at `at_ms` failed. Absent from a file written before 0.5: there, no `last` meant a failure.
    #[serde(default)]
    failed: bool,
}

impl Kept {
    /// Whether a check is due: a day after one that worked, an hour after one that failed.
    fn due(&self, now_ms: i64) -> bool {
        match self.at_ms {
            Some(at) if self.failed || self.last.is_none() => now_ms - at >= RETRY_MS || now_ms < at,
            at => due(at, now_ms),
        }
    }

    /// The check that just ran, at `now_ms`: its answer if it worked, else the last good one, marked failed.
    fn after(self, fresh: Option<LastCheck>, now_ms: i64) -> Kept {
        let failed = fresh.is_none();
        Kept { at_ms: Some(now_ms), last: fresh.or(self.last), failed }
    }

    /// What About says: after a failed check, a newer version the last good check found, else Failed.
    fn answer(&self, current: &str, lang: &str) -> Update {
        match answer(current, self.last.as_ref(), lang) {
            Update::Current if self.failed => Update::Failed,
            a => a,
        }
    }
}

const KEPT_FILE: &str = "update-check.json";

fn kept_file(app: &tauri::AppHandle) -> Option<std::path::PathBuf> {
    use tauri::Manager;
    app.path().app_config_dir().ok().map(|d| d.join(KEPT_FILE))
}

fn read_kept(file: Option<&std::path::Path>) -> Kept {
    file.and_then(|f| std::fs::read_to_string(f).ok())
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |t| t.as_millis() as i64)
}

/// Adds a piece of the body to `body`. False, adding nothing, when the body would pass `MAX_BYTES`.
fn take(body: &mut Vec<u8>, chunk: &[u8]) -> bool {
    if body.len() + chunk.len() > MAX_BYTES {
        return false;
    }
    body.extend_from_slice(chunk);
    true
}

/// Reads the site's file: a plain GET with no cookies or identifiers, given up after 10 seconds or past
/// `MAX_BYTES`.
async fn fetch() -> Option<String> {
    let http = reqwest::Client::builder().timeout(std::time::Duration::from_secs(10)).build().ok()?;
    let mut res = http.get(VERSION_URL).send().await.ok()?;
    if !res.status().is_success() || res.content_length().is_some_and(|n| n > MAX_BYTES as u64) {
        return None;
    }
    let mut body = Vec::new();
    while let Some(chunk) = res.chunk().await.ok()? {
        if !take(&mut body, &chunk) {
            return None;
        }
    }
    String::from_utf8(body).ok()
}

/// The update check: from the kept answer while it is under a day old, unless `now` (About's "Check for
/// updates"); otherwise from the site. Never an error: a check that fails says Failed.
#[tauri::command]
pub async fn check_update(app: tauri::AppHandle, lang: String, now: bool) -> Update {
    let file = kept_file(&app);
    let kept = read_kept(file.as_deref());
    let current = env!("CARGO_PKG_VERSION");
    let t = now_ms();
    if !now && !kept.due(t) {
        return kept.answer(current, &lang);
    }
    let kept = kept.after(fetch().await.and_then(|body| parse(&body, t)), t);
    if let Some(f) = file
        && let (Some(dir), Ok(json)) = (f.parent(), serde_json::to_string(&kept))
    {
        let _ = std::fs::create_dir_all(dir);
        let _ = std::fs::write(&f, json);
    }
    kept.answer(current, &lang)
}

/// Opens the download page the last check found, in the window's language. The window sends no address: it
/// comes from the kept check, and is checked again on the way out (`site_url`).
#[tauri::command]
pub async fn open_download(app: tauri::AppHandle, lang: String) -> Result<(), crate::commands::UiError> {
    let kept = read_kept(kept_file(&app).as_deref());
    let url = kept
        .last
        .map(|l| if lang == "ar" { l.url_ar } else { l.url_en })
        .ok_or_else(|| crate::commands::UiError::new("not_found", "No newer version was found"))?;
    crate::commands::open_in_browser(url).await
}

#[cfg(test)]
mod tests {
    use super::*;

    const FILE: &str = r#"{"version":"0.5.0","url":{"en":"https://wasl.kahflabs.com/","ar":"https://wasl.kahflabs.com/ar/"}}"#;

    #[test]
    fn a_newer_version_is_newer_by_number_not_by_text() {
        assert!(is_newer("0.4.0", "0.5.0"));
        assert!(is_newer("0.9.0", "0.10.0"));
        assert!(is_newer("0.4", "0.4.1"));
        assert!(is_newer("0.5.0-beta.1", "0.5.0"));
        assert!(!is_newer("0.5.0", "0.5.0"));
        assert!(!is_newer("0.5.0", "0.4.9"));
        assert!(!is_newer("0.5.0", "0.5.0-beta.1"));
        assert!(!is_newer("0.4.0", "soon"));
        assert!(!is_newer("0.4.0", ""));
        // Pre-releases of one version compare by their tags, as semver does (#205).
        assert!(is_newer("0.5.0-beta.1", "0.5.0-beta.2"));
        assert!(!is_newer("0.5.0-beta.2", "0.5.0-beta.1"));
        assert!(is_newer("0.5.0-beta.9", "0.5.0-beta.10"));
        assert!(is_newer("0.5.0-alpha", "0.5.0-beta"));
        assert!(is_newer("0.5.0-beta", "0.5.0-beta.1"));
        assert!(is_newer("0.5.0-1", "0.5.0-beta"));
        assert!(!is_newer("0.5.0-beta.1", "0.5.0-beta.1"));
    }

    #[test]
    fn the_site_file_gives_the_version_and_the_download_page_in_the_window_language() {
        let last = parse(FILE, 7).unwrap();
        assert_eq!(7, last.at_ms);
        assert_eq!(
            Update::Newer { version: "0.5.0".into(), url: "https://wasl.kahflabs.com/ar/".into() },
            answer("0.4.0", Some(&last), "ar")
        );
        assert_eq!(Update::Current, answer("0.5.0", Some(&last), "en"));
    }

    #[test]
    fn a_file_that_makes_no_sense_or_points_off_the_site_is_a_failed_check() {
        assert!(parse("<html>Not found</html>", 0).is_none());
        assert!(parse(r#"{"version":"0.5.0"}"#, 0).is_none());
        assert!(parse(&FILE.replace("0.5.0", "latest"), 0).is_none());
        assert!(parse(&FILE.replace("https://wasl.kahflabs.com/ar/", "https://example.com/"), 0).is_none());
        assert_eq!(Update::Failed, answer("0.4.0", None, "en"));
    }

    #[test]
    fn the_check_runs_once_a_day() {
        assert!(due(None, 0));
        assert!(!due(Some(1_000), 1_000 + EVERY_MS - 1));
        assert!(due(Some(1_000), 1_000 + EVERY_MS));
        // A clock set back doesn't stop the checks.
        assert!(due(Some(1_000), 500));
    }

    #[test]
    fn a_failed_check_is_tried_again_within_the_hour() {
        let kept = |last: Option<LastCheck>| Kept { at_ms: Some(1_000), failed: last.is_none(), last };
        let ok = parse(FILE, 1_000);
        assert!(!kept(None).due(1_000 + RETRY_MS - 1));
        assert!(kept(None).due(1_000 + RETRY_MS));
        assert!(!kept(ok.clone()).due(1_000 + RETRY_MS));
        assert!(kept(ok).due(1_000 + EVERY_MS));
        assert!(Kept::default().due(0));
    }

    #[test]
    fn a_failed_check_keeps_the_newer_version_it_already_knew() {
        let good = Kept::default().after(parse(FILE, 1_000), 1_000);
        let failed = good.after(None, 2_000);
        assert!(failed.failed);
        assert_eq!(Some(2_000), failed.at_ms);
        assert_eq!("0.5.0", failed.last.as_ref().unwrap().version);
        assert!(matches!(failed.answer("0.4.0", "en"), Update::Newer { .. }));
        // Up to date before, then a failure: About says it couldn't check, not that this is the newest.
        assert_eq!(Update::Failed, failed.answer("0.5.0", "en"));
        assert_eq!(Update::Failed, Kept::default().after(None, 0).answer("0.4.0", "en"));
        // The next good check clears the failure.
        assert!(!failed.after(parse(FILE, 3_000), 3_000).failed);
    }

    #[test]
    fn the_site_file_is_read_up_to_64_kb() {
        let mut body = Vec::new();
        assert!(take(&mut body, FILE.as_bytes()));
        assert!(take(&mut body, &vec![b' '; MAX_BYTES - FILE.len()]));
        assert_eq!(MAX_BYTES, body.len());
        assert!(!take(&mut body, b" "));
    }

    #[test]
    fn only_plain_https_pages_on_the_wasl_site_open() {
        assert_eq!(site_url("https://wasl.kahflabs.com/ar/").as_deref(), Some("https://wasl.kahflabs.com/ar/"));
        assert_eq!(site_url("https://wasl.kahflabs.com/").as_deref(), Some("https://wasl.kahflabs.com/"));
        assert_eq!(
            site_url("https://wasl.kahflabs.com/download/wasl-0.5.0_x64.msi").as_deref(),
            Some("https://wasl.kahflabs.com/download/wasl-0.5.0_x64.msi")
        );
        for bad in [
            "https://wasl.kahflabs.com/\",\"&calc",
            "https://wasl.kahflabs.com/'x",
            "https://wasl.kahflabs.com/^x",
            "https://wasl.kahflabs.com/%22",
            "https://wasl.kahflabs.com/a,b",
            "https://wasl.kahflabs.com/a\nb",
            "https://wasl.kahflabs.com/a\u{7}b",
            "https://wasl.kahflabs.com/a b",
            "https://wasl.kahflabs.com/?q=1",
            "https://wasl.kahflabs.com/#download",
            "https://wasl.kahflabs.com/../x",
            "https://wasl.kahflabs.com",
            "https://wasl.kahflabs.com.evil.example/",
            "https://evil.example/",
            "https://me@wasl.kahflabs.com/",
            "https://wasl.kahflabs.com@evil.example/",
            "https://wasl.kahflabs.com:8443/",
            "http://wasl.kahflabs.com/",
            "HTTPS://wasl.kahflabs.com/",
            "file:///C:/Windows",
            "",
        ] {
            assert_eq!(site_url(bad), None, "{bad:?}");
        }
    }
}
