//! Notifications (#71): only when a decision is needed or something failed, never for a routine sync. Four
//! categories: a conflict, a stopped sync (a failed pair or a Drive sign-in), deletions held by the guard, and a
//! pairing request (a device asking to pair, or offering a folder). They come from here, beside the daemon's
//! connection, so they work with every window hidden.
//!
//! What is shown is decided by [`Gate`], a pure function of what needs you now, what needed you at the last check and
//! the clock: one notification per pair and event while it lasts, the same one at most once every quarter of an
//! hour, and at most a few a minute, the rest folded into one "N things need you". The operating system can't
//! replace a shown notification on macOS or Windows through tauri-plugin-notification, so Wasl never sends a second
//! one for the same thing instead. Clicking one opens the window at that pair or screen.
//!
//! Settings › Notifications turns all of them, or each category, off (`notify`, `notify_<category>`; on when unset).

use crate::daemon::Daemon;
use serde::Deserialize;
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Manager};
use tauri_plugin_notification::NotificationExt;
use wasl_core::RunStatus;
use wasl_ipc::{DaemonStatus, OfferInfo, OfferStatus, PairingStage, PairingStatus};

/// The same pair and event notifies again only after this long (a pair that keeps failing and retrying).
const KEY_QUIET: Duration = Duration::from_secs(15 * 60);
/// Where the last this many notifications lead; older ones open the window as it is.
const ROUTES_KEPT: i32 = 64;
/// At most this many notifications in [`BURST_WINDOW`]; more fold into one summary.
const BURST: usize = 3;
const BURST_WINDOW: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Category {
    Conflict,
    Failed,
    Deletion,
    Pairing,
}

impl Category {
    /// Its switch in Settings › Notifications.
    fn setting(self) -> &'static str {
        match self {
            Category::Conflict => "notify_conflict",
            Category::Failed => "notify_failed",
            Category::Deletion => "notify_deletion",
            Category::Pairing => "notify_pairing",
        }
    }
}

/// One thing that needs you, as a notification would name it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Alert {
    /// One per pair (or account, offer, pairing) and event: `conflict:<pair>`, `failed:<pair>`, `auth:<account>`,
    /// `held:<pair>`, `offer:<share>:<device>`, `pairing`.
    pub key: String,
    pub category: Category,
    /// What the words say: a pair's name, an account's email or an offered folder's name.
    pub name: String,
    /// The device asking, for pairing requests.
    pub device: String,
    /// Where the window opens on a click (`tray://open`): `problems`, `devices`, `settings` or `pair:<id>`.
    pub route: String,
}

/// What [`Gate::admit`] lets through.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Shown {
    One(Alert),
    /// This many more need you; one notification for all of them.
    Summary(usize),
}

/// Decides which alerts become notifications (see the module comment).
#[derive(Debug, Default)]
pub struct Gate {
    /// What needed you at the last check, shown or not. None before the first check.
    active: Option<HashSet<String>>,
    /// When each key last notified.
    last_shown: HashMap<String, Instant>,
    /// When the recent notifications went out, oldest first.
    recent: VecDeque<Instant>,
}

impl Gate {
    /// The notifications to show for what needs you `now`. Only alerts new since the last check count; the first
    /// check after Wasl starts only takes note, so starting never shows a pile of old problems (the tray's badge
    /// says them). Switched-off categories and alerts while the window is in front (`quiet`) are noted, not shown.
    pub fn admit(&mut self, alerts: Vec<Alert>, enabled: impl Fn(Category) -> bool, quiet: bool, now: Instant) -> Vec<Shown> {
        let keys: HashSet<String> = alerts.iter().map(|a| a.key.clone()).collect();
        let Some(before) = self.active.replace(keys) else {
            return Vec::new();
        };
        self.last_shown.retain(|_, at| now.saturating_duration_since(*at) < KEY_QUIET);
        while self.recent.front().is_some_and(|at| now.saturating_duration_since(*at) >= BURST_WINDOW) {
            self.recent.pop_front();
        }
        let fresh: Vec<Alert> = alerts
            .into_iter()
            .filter(|a| !before.contains(&a.key) && !self.last_shown.contains_key(&a.key) && enabled(a.category))
            .collect();
        if quiet || fresh.is_empty() {
            return Vec::new();
        }
        for alert in &fresh {
            self.last_shown.insert(alert.key.clone(), now);
        }
        // Over the limit, the alerts that don't fit stay in the tray's badge and the window.
        let room = BURST.saturating_sub(self.recent.len());
        let shown: Vec<Shown> = if fresh.len() <= room {
            fresh.into_iter().map(Shown::One).collect()
        } else if room == 0 {
            Vec::new()
        } else {
            // All but the last free slot as themselves, the rest in one summary.
            let mut shown: Vec<Shown> = fresh[..room - 1].iter().cloned().map(Shown::One).collect();
            shown.push(Shown::Summary(fresh.len() - (room - 1)));
            shown
        };
        self.recent.extend(std::iter::repeat_n(now, shown.len()));
        shown
    }
}

/// What needs a decision now, one alert per pair (its loudest state, as the window ranks them: held deletions,
/// then a failure, then conflicts), per account that must sign in again, per new offer and for a pairing request.
/// A failed pair keeps its alert while it retries (its last run still failed), so one failure streak is one alert
/// and notifies once, not again every quarter hour.
pub fn alerts(status: &DaemonStatus, offers: &[OfferInfo], pairing: Option<&PairingStatus>) -> Vec<Alert> {
    let alert = |key: String, category, name: &str, device: &str, route: String| Alert {
        key,
        category,
        name: name.to_owned(),
        device: device.to_owned(),
        route,
    };
    let mut out = Vec::new();
    let signed_out: HashSet<&str> = status
        .accounts
        .iter()
        .filter(|a| a.auth_error.is_some())
        .map(|a| a.id.as_str())
        .collect();
    // One per account, not one per Drive pair it stops.
    for a in status.accounts.iter().filter(|a| a.auth_error.is_some()) {
        out.push(alert(format!("auth:{}", a.id), Category::Failed, &a.email, "", "settings".into()));
    }
    for p in &status.pairs {
        let (id, st) = (&p.pair.id, &p.pair.status);
        let route = format!("pair:{id}");
        if st.pending_approval || p.blocked {
            out.push(alert(format!("held:{id}"), Category::Deletion, &p.pair.name, "", route));
        } else if st.last_status == Some(RunStatus::Failed) {
            let account_out = p.pair.account_id.as_deref().is_some_and(|a| signed_out.contains(a));
            if !account_out {
                out.push(alert(format!("failed:{id}"), Category::Failed, &p.pair.name, "", route));
            }
        } else if p.open_conflicts > 0 {
            out.push(alert(format!("conflict:{id}"), Category::Conflict, &p.pair.name, "", "problems".into()));
        }
    }
    for o in offers.iter().filter(|o| o.status == OfferStatus::New) {
        let key = format!("offer:{}:{}", o.share, o.from_device);
        out.push(alert(key, Category::Pairing, &o.name, &o.from_name, "devices".into()));
    }
    if let Some(p) = pairing.filter(|p| p.stage == PairingStage::Confirm && !p.confirmed_here) {
        let device = p.peer_name.as_deref().or(p.peer_id.as_deref()).unwrap_or("");
        out.push(alert("pairing".into(), Category::Pairing, "", device, "devices".into()));
    }
    out
}

/// The notifications' words, from the window, in the person's language (sent with the tray's, `set_tray_text`).
/// `{name}`, `{device}` and `{count}` are filled in here.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NotifyText {
    pub conflict_title: String,
    pub conflict_body: String,
    pub failed_title: String,
    pub failed_body: String,
    pub auth_title: String,
    pub auth_body: String,
    pub deletion_title: String,
    pub deletion_body: String,
    pub offer_title: String,
    pub offer_body: String,
    pub pairing_title: String,
    pub pairing_body: String,
    pub summary_title: String,
    pub summary_body: String,
}

impl NotifyText {
    /// Title and body for one notification.
    fn words(&self, shown: &Shown) -> (String, String) {
        let fill = |s: &str, a: Option<&Alert>, count: usize| {
            let (name, device) = a.map_or(("", ""), |a| (a.name.as_str(), a.device.as_str()));
            s.replace("{name}", name).replace("{device}", device).replace("{count}", &count.to_string())
        };
        let (title, body, a, count) = match shown {
            Shown::Summary(n) => (&self.summary_title, &self.summary_body, None, *n),
            Shown::One(a) => {
                let (t, b) = match (a.category, a.key.split(':').next().unwrap_or("")) {
                    (Category::Failed, "auth") => (&self.auth_title, &self.auth_body),
                    (Category::Failed, _) => (&self.failed_title, &self.failed_body),
                    (Category::Conflict, _) => (&self.conflict_title, &self.conflict_body),
                    (Category::Deletion, _) => (&self.deletion_title, &self.deletion_body),
                    (Category::Pairing, "offer") => (&self.offer_title, &self.offer_body),
                    (Category::Pairing, _) => (&self.pairing_title, &self.pairing_body),
                };
                (t, b, Some(a), 0)
            }
        };
        (fill(title.as_str(), a, count), fill(body.as_str(), a, count))
    }
}

/// The app's notifier: the gate, the words, and where each shown notification leads.
#[derive(Default)]
pub struct Notifier {
    gate: Mutex<Gate>,
    text: Mutex<Option<NotifyText>>,
    routes: Mutex<HashMap<i32, String>>,
    next_id: AtomicI32,
}

/// New words from the window (its language changed, or it started).
pub fn set_text(app: &AppHandle, text: Option<NotifyText>) {
    if let (Some(n), Some(text)) = (app.try_state::<Notifier>(), text) {
        *n.text.lock().unwrap_or_else(PoisonError::into_inner) = Some(text);
    }
}

/// Clicking a notification opens the window at its pair or screen. Called once, at setup.
pub fn listen(app: &AppHandle) {
    let handle = app.clone();
    let _ = app.notification().on_action(move |performed| {
        let route = performed.notification().and_then(|n| {
            let n_id = n.id();
            handle
                .try_state::<Notifier>()
                .and_then(|state| state.routes.lock().unwrap_or_else(PoisonError::into_inner).get(&n_id).cloned())
        });
        crate::popover::open_in_main(&handle, route);
    });
}

/// After the daemon's status changed: notify for what newly needs you. Nothing until the window has sent the words:
/// the gate isn't consulted before then, so nothing it would show is marked as shown and lost. With no pairs yet,
/// only pairing requests, offers and a Drive sign-out can need you; the pair-based alerts simply don't arise.
pub async fn check(app: &AppHandle, d: &Daemon) {
    let (Some(notifier), Some(client), Some(status)) = (app.try_state::<Notifier>(), d.client().await, d.status())
    else {
        return;
    };
    let Some(text) = notifier.text.lock().unwrap_or_else(PoisonError::into_inner).clone() else {
        return;
    };
    let offers = client.offers().await.unwrap_or_default();
    let pairing = client.pairing_status().await.ok();
    let found = alerts(&status, &offers, pairing.as_ref());
    // Unset means on.
    let on = |value: Option<String>| value.as_deref() != Some("false");
    let all = on(client.setting("notify").await.ok().flatten());
    let mut enabled = HashMap::new();
    for c in [Category::Conflict, Category::Failed, Category::Deletion, Category::Pairing] {
        enabled.insert(c, all && on(client.setting(c.setting()).await.ok().flatten()));
    }
    // The window in front already shows it.
    let quiet = app
        .get_webview_window("main")
        .is_some_and(|w| w.is_visible().unwrap_or(false) && w.is_focused().unwrap_or(false));
    let shown = notifier.gate.lock().unwrap_or_else(PoisonError::into_inner).admit(
        found,
        |c| enabled.get(&c).copied().unwrap_or(true),
        quiet,
        Instant::now(),
    );
    for s in shown {
        let (title, body) = text.words(&s);
        let route = match &s {
            Shown::One(a) => a.route.clone(),
            Shown::Summary(_) => "problems".into(),
        };
        let id = notifier.next_id.fetch_add(1, Ordering::Relaxed);
        {
            let mut routes = notifier.routes.lock().unwrap_or_else(PoisonError::into_inner);
            routes.insert(id, route);
            routes.remove(&id.wrapping_sub(ROUTES_KEPT));
        }
        let _ = app.notification().builder().id(id).title(title).body(body).show();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wasl_ipc::{Overall, PairState};
    use wasl_store::Pair;

    fn alert(key: &str, category: Category) -> Alert {
        Alert { key: key.into(), category, name: "kahf-site".into(), device: String::new(), route: "problems".into() }
    }

    fn keys(shown: &[Shown]) -> Vec<String> {
        shown
            .iter()
            .map(|s| match s {
                Shown::One(a) => a.key.clone(),
                Shown::Summary(n) => format!("+{n}"),
            })
            .collect()
    }

    /// A gate past its first check, at `t0`.
    fn started(t0: Instant) -> Gate {
        let mut g = Gate::default();
        assert!(g.admit(vec![], |_| true, false, t0).is_empty());
        g
    }

    #[test]
    fn the_first_check_only_takes_note_of_what_already_needs_you() {
        let t0 = Instant::now();
        let mut g = Gate::default();
        assert!(g.admit(vec![alert("conflict:a", Category::Conflict)], |_| true, false, t0).is_empty());
        let later = t0 + Duration::from_secs(5);
        assert!(g.admit(vec![alert("conflict:a", Category::Conflict)], |_| true, false, later).is_empty());
    }

    #[test]
    fn a_new_alert_notifies_once_while_it_lasts() {
        let t0 = Instant::now();
        let mut g = started(t0);
        let a = vec![alert("conflict:a", Category::Conflict)];
        assert_eq!(keys(&g.admit(a.clone(), |_| true, false, t0)), ["conflict:a"]);
        for s in 1..10 {
            assert!(g.admit(a.clone(), |_| true, false, t0 + Duration::from_secs(s * 30)).is_empty());
        }
    }

    #[test]
    fn the_same_pair_and_event_waits_a_quarter_hour_before_notifying_again() {
        let t0 = Instant::now();
        let mut g = started(t0);
        let a = vec![alert("failed:a", Category::Failed)];
        assert_eq!(g.admit(a.clone(), |_| true, false, t0).len(), 1);
        // It clears and fails again five minutes later: no second notification.
        assert!(g.admit(vec![], |_| true, false, t0 + Duration::from_secs(60)).is_empty());
        assert!(g.admit(a.clone(), |_| true, false, t0 + Duration::from_secs(300)).is_empty());
        // Again after the quarter hour: it notifies.
        assert!(g.admit(vec![], |_| true, false, t0 + Duration::from_secs(600)).is_empty());
        assert_eq!(g.admit(a, |_| true, false, t0 + KEY_QUIET + Duration::from_secs(60)).len(), 1);
    }

    #[test]
    fn different_events_on_the_same_pair_each_notify() {
        let t0 = Instant::now();
        let mut g = started(t0);
        assert_eq!(g.admit(vec![alert("conflict:a", Category::Conflict)], |_| true, false, t0).len(), 1);
        let both = vec![alert("conflict:a", Category::Conflict), alert("held:a", Category::Deletion)];
        assert_eq!(keys(&g.admit(both, |_| true, false, t0 + Duration::from_secs(1))), ["held:a"]);
    }

    #[test]
    fn many_at_once_show_two_and_fold_the_rest_into_one_summary() {
        let t0 = Instant::now();
        let mut g = started(t0);
        let many: Vec<Alert> = (0..6).map(|i| alert(&format!("failed:{i}"), Category::Failed)).collect();
        assert_eq!(keys(&g.admit(many, |_| true, false, t0)), ["failed:0", "failed:1", "+4"]);
    }

    #[test]
    fn exactly_as_many_as_the_limit_show_each() {
        let t0 = Instant::now();
        let mut g = started(t0);
        let three: Vec<Alert> = (0..3).map(|i| alert(&format!("held:{i}"), Category::Deletion)).collect();
        assert_eq!(keys(&g.admit(three, |_| true, false, t0)), ["held:0", "held:1", "held:2"]);
    }

    #[test]
    fn the_rate_limit_holds_across_checks_and_frees_up_after_a_minute() {
        let t0 = Instant::now();
        let mut g = started(t0);
        let mut seen = Vec::new();
        for i in 0..3 {
            seen.push(alert(&format!("conflict:{i}"), Category::Conflict));
            assert_eq!(g.admit(seen.clone(), |_| true, false, t0 + Duration::from_secs(i)).len(), 1);
        }
        seen.push(alert("conflict:3", Category::Conflict));
        assert!(g.admit(seen.clone(), |_| true, false, t0 + Duration::from_secs(10)).is_empty());
        seen.push(alert("conflict:4", Category::Conflict));
        let after = t0 + BURST_WINDOW + Duration::from_secs(5);
        assert_eq!(keys(&g.admit(seen, |_| true, false, after)), ["conflict:4"]);
    }

    #[test]
    fn a_switched_off_category_is_noted_and_never_shown_later() {
        let t0 = Instant::now();
        let mut g = started(t0);
        let a = vec![alert("offer:s:d", Category::Pairing), alert("conflict:a", Category::Conflict)];
        let shown = g.admit(a.clone(), |c| c != Category::Pairing, false, t0);
        assert_eq!(keys(&shown), ["conflict:a"]);
        // Switched on again while the offer still waits: it isn't new, so nothing shows.
        assert!(g.admit(a, |_| true, false, t0 + Duration::from_secs(5)).is_empty());
    }

    #[test]
    fn nothing_shows_while_the_window_is_in_front() {
        let t0 = Instant::now();
        let mut g = started(t0);
        let a = vec![alert("held:a", Category::Deletion)];
        assert!(g.admit(a.clone(), |_| true, true, t0).is_empty());
        assert!(g.admit(a, |_| true, false, t0 + Duration::from_secs(5)).is_empty());
    }

    fn status(pairs: Vec<PairState>, accounts: Vec<wasl_ipc::Account>) -> DaemonStatus {
        DaemonStatus { version: "0".into(), protocol: 1, accounts, pairs, overall: Overall::Idle, agent_changes: 0 }
    }

    fn pair_state(id: &str, last: Option<RunStatus>, running: bool) -> PairState {
        let mut pair = Pair::new(id, "kahf-site", "/tmp/kahf-site");
        pair.status.last_status = last;
        PairState {
            pair,
            running,
            progress: None,
            open_conflicts: 0,
            blocked: false,
            folder_bytes: None,
            skipped: None,
            peers: Vec::new(),
            free_bytes: None,
        }
    }

    #[test]
    fn a_failed_pair_keeps_its_alert_while_it_retries() {
        let failing = status(vec![pair_state("a", Some(RunStatus::Failed), true)], vec![]);
        let found = alerts(&failing, &[], None);
        assert_eq!(found.iter().map(|a| a.key.as_str()).collect::<Vec<_>>(), ["failed:a"]);
        // So the gate sees one streak: the first check notifies, the retries don't, even past the quarter hour.
        let t0 = Instant::now();
        let mut g = started(t0);
        assert_eq!(g.admit(found.clone(), |_| true, false, t0).len(), 1);
        assert!(g.admit(found, |_| true, false, t0 + KEY_QUIET + Duration::from_secs(60)).is_empty());
    }

    #[test]
    fn with_no_pairs_a_pairing_request_an_offer_and_a_drive_sign_out_still_notify() {
        let account = wasl_ipc::Account { id: "1".into(), email: "drive account".into(), auth_error: Some("expired".into()) };
        let offer = OfferInfo {
            share: "s".into(),
            from_device: "d".into(),
            from_name: "Laptop".into(),
            name: "Projects".into(),
            files: 0,
            bytes: 0,
            first_seen: 0,
            status: OfferStatus::New,
        };
        let pairing = PairingStatus {
            stage: PairingStage::Confirm,
            inviting: true,
            code: None,
            expires_at: None,
            words: vec![],
            peer_name: Some("Laptop".into()),
            peer_id: None,
            confirmed_here: false,
            message: None,
        };
        let found = alerts(&status(vec![], vec![account]), &[offer], Some(&pairing));
        let keys: Vec<&str> = found.iter().map(|a| a.key.as_str()).collect();
        assert_eq!(keys, ["auth:1", "offer:s:d", "pairing"]);
        let t0 = Instant::now();
        let mut g = started(t0);
        assert_eq!(g.admit(found, |_| true, false, t0).len(), 3);
    }

    #[test]
    fn the_words_fill_in_the_pair_device_and_count() {
        let w = |s: &str| s.to_owned();
        let text = NotifyText {
            conflict_title: w("{name}: a conflict"),
            conflict_body: w("b"),
            failed_title: w("{name} stopped syncing"),
            failed_body: w("b"),
            auth_title: w("Sign in again"),
            auth_body: w("{name}"),
            deletion_title: w("{name}: deletions held"),
            deletion_body: w("b"),
            offer_title: w("{device} offers you {name}"),
            offer_body: w("b"),
            pairing_title: w("{device} wants to pair"),
            pairing_body: w("b"),
            summary_title: w("{count} things need you"),
            summary_body: w("b"),
        };
        let mut offer = alert("offer:s:d", Category::Pairing);
        offer.name = "Projects".into();
        offer.device = "Laptop".into();
        assert_eq!(text.words(&Shown::One(offer)).0, "Laptop offers you Projects");
        assert_eq!(text.words(&Shown::One(alert("auth:1", Category::Failed))).0, "Sign in again");
        assert_eq!(text.words(&Shown::One(alert("failed:a", Category::Failed))).0, "kahf-site stopped syncing");
        assert_eq!(text.words(&Shown::Summary(4)).0, "4 things need you");
    }
}
