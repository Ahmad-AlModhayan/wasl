//! The commands. Each returns the process exit code, or a [`Fail`] with its code and message.

use crate::out::{self, Tone, paint};
use crate::{AccountCmd, BinCmd, Cmd, DaemonCmd, DeviceCmd, PairCmd, ServiceCmd, Target, VaultCmd, ViewCmd};
use std::collections::HashMap;
use std::io::{BufRead, IsTerminal, Write};
use std::sync::Arc;
use std::time::{Duration, Instant};
use wasl_cli::{i18n, service, t};
use wasl_ipc::{
    Account, Client, ClientError, DaemonStatus, ErrorKind, Event, LocalChangeKind, Overall, PairState, Paths,
};
use wasl_proto::{
    Action, BinSide, Pair, Phase, PlanSummary, Progress, RunOutcome, RunStatus, SyncMode, Trigger,
};

pub const OK: u8 = 0;
pub const GENERAL: u8 = 1;
pub const USAGE: u8 = 2;
pub const NOT_RUNNING: u8 = 3;
pub const NOT_FOUND: u8 = 4;
pub const AUTH: u8 = 5;

#[derive(Debug)]
pub struct Fail {
    pub code: u8,
    pub message: String,
}

impl Fail {
    pub fn new(code: u8, message: String) -> Fail {
        Fail { code, message }
    }
}

impl From<ClientError> for Fail {
    fn from(e: ClientError) -> Fail {
        match e {
            ClientError::NotRunning => Fail::new(NOT_RUNNING, t!(NotRunning)),
            ClientError::Untrusted => Fail::new(GENERAL, t!(Untrusted)),
            ClientError::Version(v) => Fail::new(GENERAL, t!(WrongProtocol, v, wasl_ipc::PROTOCOL_VERSION)),
            ClientError::Io(e) => Fail::new(GENERAL, t!(ConnectionLost, e)),
            ClientError::Remote(r) => {
                let code = match r.kind {
                    ErrorKind::NotFound => NOT_FOUND,
                    ErrorKind::Auth => AUTH,
                    ErrorKind::Invalid | ErrorKind::Unsupported => USAGE,
                    _ => GENERAL,
                };
                Fail::new(code, r.message)
            }
        }
    }
}

pub(crate) type R<T> = Result<T, Fail>;

fn paths() -> R<Paths> {
    Paths::for_user().map_err(|e| Fail::new(GENERAL, e.to_string()))
}

/// Connects to this user's wasld, and follows its language setting unless `WASL_LANG` is set.
pub(crate) async fn connect() -> R<Client> {
    let c = Client::connect(&paths()?).await?;
    if !i18n::forced()
        && let Ok(Some(lang)) = c.setting("language").await
    {
        i18n::apply_setting(&lang);
    }
    Ok(c)
}

pub async fn run(cmd: Cmd) -> R<u8> {
    match cmd {
        Cmd::Status { json } => status(json).await,
        Cmd::Agent(a) => crate::agent::run(a).await,
        Cmd::Oplog(o) => crate::oplog::run(o).await,
        Cmd::Sync { pair, wait } => sync(pair, wait).await,
        Cmd::Pause(target) => pause(target, true).await,
        Cmd::Resume(target) => pause(target, false).await,
        Cmd::Pair(PairCmd::List { json }) => pair_list(json).await,
        Cmd::Pair(PairCmd::Add {
            local_folder,
            drive_path,
            account,
            name,
            method,
            create_drive_folder,
            gitignore,
            presets,
            hidden,
        }) => {
            let rules = Rules {
                gitignore,
                presets,
                hidden,
            };
            pair_add(
                &local_folder,
                &drive_path,
                account.as_deref(),
                name,
                method.mode(),
                create_drive_folder,
                rules,
            )
            .await
        }
        Cmd::Pair(PairCmd::Remove { pair, yes }) => pair_remove(&pair, yes).await,
        Cmd::Pair(PairCmd::Preview { pair, limit, start }) => pair_preview(&pair, limit, start).await,
        Cmd::Ignore { explain, pair, path } => ignore(explain, &pair, &path).await,
        Cmd::Account(AccountCmd::List { json }) => account_list(json).await,
        Cmd::Account(AccountCmd::Add) => account_add().await,
        Cmd::Account(AccountCmd::Remove { email }) => account_remove(&email).await,
        Cmd::History { pair, limit, json } => history(pair, limit, json).await,
        Cmd::Problems { json } => problems(json).await,
        Cmd::Bin(BinCmd::List { pair, json }) => bin_list(pair, json).await,
        Cmd::Bin(BinCmd::Restore { ids }) => bin_restore(ids).await,
        Cmd::Bin(BinCmd::Empty { yes }) => bin_empty(yes).await,
        Cmd::Daemon(DaemonCmd::Start) => daemon_start().await,
        Cmd::Daemon(DaemonCmd::Stop) => daemon_stop().await,
        Cmd::Daemon(DaemonCmd::Status) => daemon_status().await,
        Cmd::Service(ServiceCmd::Install) => service_install(),
        Cmd::Service(ServiceCmd::Uninstall) => service_uninstall(),
        Cmd::Service(ServiceCmd::Status) => service_status(),
        Cmd::Device(DeviceCmd::List { json }) => device_list(json).await,
        Cmd::Device(DeviceCmd::Remove { device, yes }) => device_remove(&device, yes).await,
        Cmd::Device(DeviceCmd::Connect { address }) => device_connect(&address).await,
        Cmd::Device(DeviceCmd::Add { yes }) => device_pair(None, yes).await,
        Cmd::Device(DeviceCmd::Join { code, yes }) => device_pair(Some(&code), yes).await,
        Cmd::Device(DeviceCmd::Offers { json, all }) => device_offers(json, all).await,
        Cmd::Device(DeviceCmd::Accept {
            offer,
            folder,
            from,
            name,
        }) => device_accept(&offer, &folder, from.as_deref(), name.as_deref()).await,
        Cmd::Device(DeviceCmd::Decline { offer, from }) => device_decline(&offer, from.as_deref()).await,
        Cmd::Device(DeviceCmd::Keep { device }) => device_keep(&device).await,
        Cmd::Pair(PairCmd::Share { pair, code, with }) => pair_share(&pair, code.as_deref(), &with).await,
        Cmd::Pair(PairCmd::Unshare { pair }) => pair_unshare(&pair).await,
        Cmd::Vault(VaultCmd::Decrypt {
            folder,
            output,
            passphrase_stdin,
            overwrite,
        }) => crate::vault::decrypt(&folder, &output, passphrase_stdin, overwrite),
        Cmd::Vault(VaultCmd::List { json }) => vault_list(json).await,
        Cmd::Vault(VaultCmd::Create {
            drive_folder,
            name,
            account,
            passphrase_stdin,
        }) => vault_create(&drive_folder, name, account.as_deref(), passphrase_stdin).await,
        Cmd::Vault(VaultCmd::Unlock {
            vault,
            account,
            passphrase_stdin,
        }) => vault_unlock(&vault, account.as_deref(), passphrase_stdin).await,
        Cmd::Vault(VaultCmd::Lock { vault }) => vault_lock(&vault).await,
        Cmd::Vault(VaultCmd::Upgrade {
            vault,
            drive_folder,
            account,
            passphrase_stdin,
        }) => vault_upgrade(&vault, &drive_folder, account.as_deref(), passphrase_stdin).await,
        Cmd::View(ViewCmd::List { json }) => view_list(json).await,
        Cmd::View(ViewCmd::Add {
            drive_folder,
            local_path,
            name,
            account,
            cache_limit,
        }) => view_add(&drive_folder, &local_path, name, account.as_deref(), cache_limit).await,
        Cmd::View(ViewCmd::Remove {
            view,
            keep_files,
            yes,
        }) => view_remove(&view, keep_files, yes).await,
        Cmd::View(ViewCmd::Pin { path }) => view_pin(&path, true).await,
        Cmd::View(ViewCmd::Unpin { path }) => view_pin(&path, false).await,
        Cmd::View(ViewCmd::Refresh { view }) => view_refresh(&view).await,
        Cmd::Pin { .. } => Err(Fail::new(USAGE, t!(PinLater))),
    }
}

pub(crate) fn print_json<T: serde::Serialize>(value: &T) -> R<u8> {
    let s = serde_json::to_string_pretty(value).map_err(|e| Fail::new(GENERAL, e.to_string()))?;
    println!("{s}");
    Ok(OK)
}

// ------------------------------------------------------------------- pair references

/// Finds a pair by exact id, exact name (any case) or a unique id prefix of 4+ characters.
pub fn find_pair<'a>(pairs: &'a [Pair], reference: &str) -> R<&'a Pair> {
    if let Some(p) = pairs.iter().find(|p| p.id == reference) {
        return Ok(p);
    }
    let lower = reference.to_lowercase();
    let by_name: Vec<&Pair> = pairs.iter().filter(|p| p.name.to_lowercase() == lower).collect();
    let candidates = match by_name.len() {
        1 => return Ok(by_name[0]),
        0 if reference.chars().count() >= 4 => pairs.iter().filter(|p| p.id.starts_with(reference)).collect(),
        0 => Vec::new(),
        _ => by_name,
    };
    match candidates.len() {
        0 => Err(Fail::new(NOT_FOUND, t!(NoSuchPair, reference))),
        1 => Ok(candidates[0]),
        _ => {
            let list: Vec<String> = candidates
                .iter()
                .map(|p| format!("  {}  {}", p.id, p.name))
                .collect();
            Err(Fail::new(USAGE, t!(AmbiguousPair, reference, list.join("\n"))))
        }
    }
}

async fn resolve(c: &Client, reference: &str) -> R<Pair> {
    let pairs = c.pairs().await?;
    find_pair(&pairs, reference).cloned()
}

// ------------------------------------------------------------------- labels

fn method_label(m: SyncMode) -> String {
    match m {
        SyncMode::TwoWay => t!(MethodTwoWay),
        SyncMode::UploadOnly => t!(MethodUploadOnly),
        SyncMode::UploadThenDelete => t!(MethodUploadThenDelete),
        SyncMode::UploadMirror => t!(MethodMirrorUp),
        SyncMode::DownloadOnly => t!(MethodDownloadOnly),
        SyncMode::DownloadThenDelete => t!(MethodDownloadThenDelete),
        SyncMode::DownloadMirror => t!(MethodMirrorDown),
    }
}

fn arrow(m: SyncMode) -> &'static str {
    match (m.uploads(), m.downloads()) {
        (true, true) => "↔",
        (true, false) => "→",
        _ => "←",
    }
}

fn folders(p: &Pair) -> String {
    format!("{} {} {}", p.local_root, arrow(p.method), p.remote_path)
}

fn overall_label(o: Overall) -> (String, Tone) {
    match o {
        Overall::UpToDate => (t!(OverallUpToDate), Tone::Good),
        Overall::Syncing => (t!(OverallSyncing), Tone::Busy),
        Overall::Paused => (t!(OverallPaused), Tone::Quiet),
        Overall::Waiting => (t!(OverallWaiting), Tone::Quiet),
        Overall::NeedsYou => (t!(OverallNeedsYou), Tone::Bad),
        Overall::Idle => (t!(OverallIdle), Tone::Quiet),
    }
}

fn phase_label(p: Phase) -> String {
    match p {
        Phase::Starting => t!(PhaseStarting),
        Phase::ListingRemote => t!(PhaseListingRemote),
        Phase::ScanningLocal => t!(PhaseScanningLocal),
        Phase::Planning => t!(PhasePlanning),
        Phase::Transferring => t!(PhaseTransferring),
        Phase::Deleting => t!(PhaseDeleting),
        Phase::Finishing => t!(PhaseFinishing),
    }
}

fn run_status_label(s: RunStatus) -> (String, Tone) {
    match s {
        RunStatus::Running => (t!(RunRunning), Tone::Busy),
        RunStatus::Success => (t!(RunSuccess), Tone::Good),
        RunStatus::Partial => (t!(RunPartial), Tone::Bad),
        RunStatus::Failed => (t!(RunFailed), Tone::Bad),
        RunStatus::Blocked => (t!(RunBlocked), Tone::Bad),
        RunStatus::Skipped => (t!(RunSkipped), Tone::Quiet),
        RunStatus::Cancelled => (t!(RunCancelled), Tone::Quiet),
    }
}

fn progress_text(p: &Progress) -> String {
    let mut s = phase_label(p.phase);
    if p.total_actions > 0 {
        let pct = p
            .bytes_done
            .saturating_mul(100)
            .checked_div(p.bytes_total)
            .unwrap_or_else(|| p.done_actions.saturating_mul(100) / p.total_actions);
        s.push_str(&format!(" {pct}% · "));
        s.push_str(&t!(FilesOf, p.done_actions, p.total_actions));
        if p.bytes_total > 0 {
            s.push_str(&format!(
                " · {} / {}",
                out::size(p.bytes_done),
                out::size(p.bytes_total)
            ));
        }
    } else if p.checked > 0 {
        s.push_str(" · ");
        s.push_str(&t!(Checked, p.checked));
    }
    s
}

/// One word or phrase for where a pair is, with its tone.
fn pair_state(ps: &PairState, accounts: &[Account]) -> (String, Tone) {
    let p = &ps.pair;
    if ps.running {
        return match &ps.progress {
            Some(pr) => (progress_text(pr), Tone::Busy),
            None => (t!(OverallSyncing), Tone::Busy),
        };
    }
    if !p.enabled {
        return (t!(StateOff), Tone::Quiet);
    }
    if p.paused {
        return (t!(OverallPaused), Tone::Quiet);
    }
    if let Some(a) = accounts
        .iter()
        .find(|a| Some(&a.id) == p.account_id.as_ref() && a.auth_error.is_some())
    {
        return (t!(StateSignIn, a.email), Tone::Bad);
    }
    if ps.blocked || p.status.pending_approval {
        return (t!(StateApprove), Tone::Bad);
    }
    if ps.open_conflicts > 0 {
        return (t!(StateConflicts, ps.open_conflicts), Tone::Bad);
    }
    if p.status.hold_for_preview {
        return (t!(StateWaitingPreview), Tone::Quiet);
    }
    match p.status.last_status {
        Some(RunStatus::Failed) => (
            t!(StateFailed, p.status.last_error.clone().unwrap_or_default()),
            Tone::Bad,
        ),
        Some(RunStatus::Partial) => (t!(RunPartial), Tone::Bad),
        Some(RunStatus::Blocked) => (t!(StateApprove), Tone::Bad),
        None => (t!(StateNeverSynced), Tone::Quiet),
        _ => (t!(OverallUpToDate), Tone::Good),
    }
}

fn changes(uploaded: u32, downloaded: u32, deleted: u32) -> String {
    t!(Changes, uploaded, downloaded, deleted)
}

fn summary_changes(s: &PlanSummary) -> String {
    changes(
        s.uploads,
        s.downloads + s.exports,
        s.delete_local + s.delete_remote,
    )
}

// ------------------------------------------------------------------- status

async fn status(json: bool) -> R<u8> {
    let c = connect().await?;
    let st: DaemonStatus = c.status().await?;
    if json {
        return print_json(&st);
    }
    let (overall, tone) = overall_label(st.overall);
    println!("{}", paint(&t!(StatusHeader, st.version, overall), tone));
    for a in st.accounts.iter().filter(|a| a.auth_error.is_some()) {
        println!("{}", paint(&t!(AccountNeedsSignIn, a.email), Tone::Bad));
    }
    if st.pairs.is_empty() {
        println!("{}", t!(NoPairs));
        return Ok(OK);
    }
    println!();
    let mut rows = vec![vec![t!(ColName), t!(ColFolders), t!(ColState), t!(ColLastSync)]];
    for ps in &st.pairs {
        let (state, tone) = pair_state(ps, &st.accounts);
        rows.push(vec![
            ps.pair.name.clone(),
            folders(&ps.pair),
            out::styled(&state, tone),
            ps.pair
                .status
                .last_success_at
                .map(out::when)
                .unwrap_or_else(|| t!(Never)),
        ]);
    }
    print!("{}", out::table(&rows));
    Ok(OK)
}

// ------------------------------------------------------------------- sync

fn outcome_code(o: &RunOutcome) -> u8 {
    match o.status {
        RunStatus::Success => OK,
        _ if o.error_kind == Some(wasl_proto::ErrorKind::Auth) => AUTH,
        _ => GENERAL,
    }
}

fn outcome_line(name: &str, o: &RunOutcome) -> String {
    let msg = o
        .message
        .clone()
        .or_else(|| o.blocker.as_ref().map(|b| format!("{b:?}")))
        .unwrap_or_default();
    let (line, tone) = match o.status {
        RunStatus::Success => (
            t!(
                OutcomeSuccess,
                name,
                o.summary
                    .as_ref()
                    .map(summary_changes)
                    .unwrap_or_else(|| changes(0, 0, 0))
            ),
            Tone::Good,
        ),
        RunStatus::Partial => (t!(OutcomePartial, name, msg), Tone::Bad),
        RunStatus::Failed => (t!(OutcomeFailed, name, msg), Tone::Bad),
        RunStatus::Blocked => (t!(OutcomeBlocked, name, msg), Tone::Bad),
        RunStatus::Skipped => (t!(OutcomeSkipped, name, msg), Tone::Quiet),
        RunStatus::Cancelled => (t!(OutcomeCancelled, name), Tone::Quiet),
        RunStatus::Running => (t!(OutcomeRunning, name), Tone::Busy),
    };
    paint(line.trim_end_matches([' ', ':']), tone)
}

async fn sync(pair: Option<String>, wait: bool) -> R<u8> {
    run_pairs(pair, wait, Trigger::Manual).await
}

/// Starts one pair (or all) and, with `wait`, follows it to the end. A pair that is already
/// running is followed too, instead of reporting that it did not start.
async fn run_pairs(pair: Option<String>, wait: bool, trigger: Trigger) -> R<u8> {
    let c = connect().await?;
    let pairs = c.pairs().await?;
    let names: HashMap<String, String> = pairs.iter().map(|p| (p.id.clone(), p.name.clone())).collect();
    let target = match &pair {
        Some(r) => Some(find_pair(&pairs, r)?.clone()),
        None if pairs.is_empty() => {
            println!("{}", t!(NoPairs));
            return Ok(OK);
        }
        None => None,
    };
    let mut events = c.subscribe().await?;
    let c = Arc::new(c);
    let mut call = {
        let c = c.clone();
        let id = target.as_ref().map(|p| p.id.clone());
        tokio::spawn(async move {
            match id {
                Some(id) => c.sync(&id, trigger).await.map(|o| vec![o]),
                None => c.sync_all(trigger).await,
            }
        })
    };
    let relevant = |pair_id: &str| target.as_ref().is_none_or(|p| p.id == pair_id);
    let name_of = |pair_id: &str| names.get(pair_id).cloned().unwrap_or_else(|| pair_id.to_owned());
    let tty = out::tty();
    let mut events_open = true;
    let mut last_line: HashMap<String, (Phase, Instant)> = HashMap::new();
    // Set when the pair was already running: the answer came at once, the end comes as an event.
    let mut following = false;
    loop {
        tokio::select! {
            res = &mut call, if !following => {
                let outcomes = res.map_err(|e| Fail::new(GENERAL, e.to_string()))??;
                let busy = target.is_some()
                    && outcomes.first().is_some_and(|o| {
                        o.status == RunStatus::Skipped && o.message.as_deref() == Some(wasl_proto::ALREADY_RUNNING)
                    });
                if busy && wait && events_open {
                    following = true;
                    continue;
                }
                out::finish_progress_line();
                return Ok(report_outcomes(target.as_ref(), &outcomes));
            }
            ev = events.recv(), if events_open => match ev {
                None => events_open = false,
                Some(Event::Progress { progress }) if relevant(&progress.pair_id) => {
                    if !wait {
                        match &target {
                            Some(p) => println!("{}", t!(SyncStarted, p.name)),
                            None => println!("{}", t!(SyncStartedAll)),
                        }
                        println!("{}", t!(SyncFollow));
                        return Ok(OK);
                    }
                    let line = format!("{}: {}", name_of(&progress.pair_id), progress_text(&progress));
                    if tty {
                        out::progress_line(&line);
                    } else {
                        let due = match last_line.get(&progress.pair_id) {
                            Some((phase, at)) => *phase != progress.phase || at.elapsed() >= Duration::from_secs(5),
                            None => true,
                        };
                        if due {
                            println!("{}", out::clean(&line));
                            last_line.insert(progress.pair_id.clone(), (progress.phase, Instant::now()));
                        }
                    }
                }
                Some(Event::RunFinished { pair_id, outcome }) if following && relevant(&pair_id) => {
                    out::finish_progress_line();
                    return Ok(report_outcomes(target.as_ref(), &[outcome]));
                }
                Some(Event::RunFinished { pair_id, outcome }) if wait && target.is_none() => {
                    out::finish_progress_line();
                    println!("{}", outcome_line(&name_of(&pair_id), &outcome));
                }
                Some(Event::ShuttingDown) => {
                    out::finish_progress_line();
                    return Err(Fail::new(GENERAL, t!(DaemonShuttingDown)));
                }
                Some(_) => {}
            }
        }
    }
}

fn report_outcomes(target: Option<&Pair>, outcomes: &[RunOutcome]) -> u8 {
    match target {
        Some(p) => {
            let o = outcomes.first();
            match o {
                Some(o) => {
                    println!("{}", outcome_line(&p.name, o));
                    if o.status == RunStatus::Blocked {
                        println!("{}", t!(SeePreview, p.name));
                    }
                    if o.status == RunStatus::Skipped
                        && o.message.as_deref() == Some(wasl_proto::HOLD_MESSAGE)
                    {
                        println!("{}", t!(StartWithPreview, p.name));
                    }
                    outcome_code(o)
                }
                None => OK,
            }
        }
        None => {
            let ok = outcomes.iter().filter(|o| o.status == RunStatus::Success).count();
            let bad = outcomes.len() - ok;
            println!("{}", t!(SyncAllSummary, ok, bad));
            outcomes.iter().map(outcome_code).max().unwrap_or(OK)
        }
    }
}

// ------------------------------------------------------------------- pause / resume

async fn pause(target: Target, paused: bool) -> R<u8> {
    let c = connect().await?;
    let pairs = c.pairs().await?;
    let chosen: Vec<&Pair> = if target.all {
        pairs.iter().collect()
    } else {
        let r = target.pair.as_deref().unwrap_or_default();
        vec![find_pair(&pairs, r)?]
    };
    if chosen.is_empty() {
        println!("{}", t!(NoPairs));
    }
    for p in chosen {
        c.set_paused(&p.id, paused, None).await?;
        if paused {
            println!("{}", t!(Paused, p.name));
        } else {
            println!("{}", t!(Resumed, p.name));
        }
    }
    Ok(OK)
}

// ------------------------------------------------------------------- pairs

async fn pair_list(json: bool) -> R<u8> {
    let c = connect().await?;
    let pairs = c.pairs().await?;
    if json {
        return print_json(&pairs);
    }
    if pairs.is_empty() {
        println!("{}", t!(NoPairs));
        return Ok(OK);
    }
    let mut rows = vec![vec![
        t!(ColId),
        t!(ColName),
        t!(ColMethod),
        t!(ColLocal),
        t!(ColDrive),
        t!(ColState),
    ]];
    for p in &pairs {
        let state = if !p.enabled {
            t!(StateOff)
        } else if p.paused {
            t!(OverallPaused)
        } else {
            t!(StateOn)
        };
        rows.push(vec![
            out::short_id(&p.id),
            p.name.clone(),
            method_label(p.method),
            p.local_root.clone(),
            p.remote_path.clone(),
            state,
        ]);
    }
    print!("{}", out::table(&rows));
    Ok(OK)
}

/// "Photos" → "My Drive/Photos"; paths already under My Drive or a shared drive stay.
pub fn normalise_drive_path(path: &str) -> String {
    let p = path
        .trim()
        .trim_matches(|c| c == '/' || c == '\\')
        .replace('\\', "/");
    let lower = p.to_lowercase();
    if lower == "my drive" || lower.starts_with("my drive/") || lower.starts_with("shared drives/") {
        p
    } else if p.is_empty() {
        "My Drive".into()
    } else {
        format!("My Drive/{p}")
    }
}

fn new_id() -> R<String> {
    let mut b = [0u8; 16];
    getrandom::fill(&mut b).map_err(|e| Fail::new(GENERAL, e.to_string()))?;
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    let h: String = b.iter().map(|x| format!("{x:02x}")).collect();
    Ok(format!(
        "{}-{}-{}-{}-{}",
        &h[0..8],
        &h[8..12],
        &h[12..16],
        &h[16..20],
        &h[20..32]
    ))
}

/// The account named (by email or id), or the only one signed in.
fn pick_account<'a>(accounts: &'a [Account], account: Option<&str>) -> R<&'a Account> {
    match account {
        Some(email) => accounts
            .iter()
            .find(|a| a.email.eq_ignore_ascii_case(email) || a.id == email)
            .ok_or_else(|| Fail::new(NOT_FOUND, t!(NoSuchAccount, email))),
        None => match accounts {
            [] => Err(Fail::new(AUTH, t!(NoAccounts))),
            [one] => Ok(one),
            many => {
                let list: Vec<String> = many.iter().map(|a| format!("  {}", a.email)).collect();
                Err(Fail::new(USAGE, t!(ChooseAccount, list.join("\n"))))
            }
        },
    }
}

/// What `pair add` leaves out besides `.waslignore` files.
struct Rules {
    gitignore: bool,
    presets: Vec<String>,
    hidden: bool,
}

async fn pair_add(
    local: &str,
    drive_path: &str,
    account: Option<&str>,
    name: Option<String>,
    method: SyncMode,
    create: bool,
    rules: Rules,
) -> R<u8> {
    for p in &rules.presets {
        if !wasl_proto::is_valid_preset_id(p) {
            return Err(Fail::new(USAGE, t!(UnknownPreset, p)));
        }
    }
    let c = connect().await?;
    let folder = std::path::absolute(local).map_err(|e| Fail::new(USAGE, e.to_string()))?;
    if !folder.is_dir() {
        return Err(Fail::new(NOT_FOUND, t!(NoSuchFolder, folder.display())));
    }
    let accounts = c.accounts().await?;
    let account = pick_account(&accounts, account)?;
    let drive_path = normalise_drive_path(drive_path);
    let drive = c.resolve_drive_path(&account.id, &drive_path, create).await?;
    let name = name.unwrap_or_else(|| {
        folder
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| folder.display().to_string())
    });
    let root = folder.display().to_string();
    let mut pair = Pair::new(&new_id()?, &name, &root);
    pair.account_id = Some(account.id.clone());
    pair.drive_folder_id = Some(drive.id);
    pair.drive_id = drive.drive_id;
    pair.remote_path = drive_path;
    pair.method = method;
    pair.honor_gitignore = rules.gitignore;
    pair.presets = rules.presets;
    pair.sync_hidden = rules.hidden;
    let now = out::now_ms();
    pair.created_at = now;
    pair.updated_at = now;
    c.create_pair(pair.clone()).await?;
    println!(
        "{}",
        t!(PairAdded, pair.name, folders(&pair), method_label(method))
    );
    println!("{}", t!(PairAddedNext, pair.name));
    Ok(OK)
}

/// Asks on a terminal; without one, the command needs `--yes`.
pub(crate) fn confirm(question: &str) -> R<bool> {
    if !std::io::stdin().is_terminal() {
        return Err(Fail::new(USAGE, t!(NeedsYes)));
    }
    print!("{question} {} ", t!(YesNo));
    let _ = std::io::stdout().flush();
    let mut answer = String::new();
    std::io::stdin()
        .lock()
        .read_line(&mut answer)
        .map_err(|e| Fail::new(GENERAL, e.to_string()))?;
    let a = answer.trim().to_lowercase();
    Ok(matches!(a.as_str(), "y" | "yes" | "ن" | "نعم"))
}

async fn pair_remove(reference: &str, yes: bool) -> R<u8> {
    let c = connect().await?;
    let p = resolve(&c, reference).await?;
    println!("{}", t!(RemoveWhat, p.name, folders(&p)));
    println!("{}", t!(FilesNotTouched));
    if !yes && !confirm(&t!(RemoveAsk))? {
        println!("{}", t!(NothingChanged));
        return Ok(OK);
    }
    c.delete_pair(&p.id).await?;
    println!("{}", t!(PairRemoved, p.name));
    Ok(OK)
}

fn action_line(a: &Action, sizes: &HashMap<String, u64>) -> Option<(String, String, String)> {
    let size_of = |key: &str| sizes.get(key).map(|s| out::size(*s)).unwrap_or_default();
    Some(match a {
        Action::Upload { key, local, .. } => (t!(ActUpload), key.clone(), out::size(local.size)),
        Action::Download { key, remote, .. } => (
            t!(ActDownload),
            key.clone(),
            remote.size.map(out::size).unwrap_or_default(),
        ),
        Action::Export { key, .. } => (t!(ActExport), key.clone(), String::new()),
        Action::DeleteLocal { key, .. } => (t!(ActDeleteLocal), key.clone(), size_of(key)),
        Action::DeleteRemote { key, .. } => (t!(ActDeleteRemote), key.clone(), size_of(key)),
        Action::Conflict { key, .. } => (t!(ActConflict), key.clone(), String::new()),
        Action::MkdirLocal { key, .. } => (t!(ActMkdirLocal), key.clone(), String::new()),
        Action::MkdirRemote { key, .. } => (t!(ActMkdirRemote), key.clone(), String::new()),
        Action::Skip { key, reason, .. } => (t!(ActSkip), key.clone(), reason.clone()),
        Action::Adopt { .. } | Action::Forget { .. } | Action::RefreshState { .. } => return None,
    })
}

/// `--start`: the plan was shown, so start it (Trigger::Preview also lifts a new pair's hold).
async fn start_after_preview(reference: &str, start: bool) -> R<u8> {
    if !start {
        return Ok(OK);
    }
    println!();
    run_pairs(Some(reference.to_owned()), true, Trigger::Preview).await
}

async fn pair_preview(reference: &str, limit: usize, start: bool) -> R<u8> {
    let c = connect().await?;
    let p = resolve(&c, reference).await?;
    if p.route == wasl_proto::Route::Devices {
        // No Drive plan to show: a pair synced with devices only (an accepted offer) starts here.
        println!("{}", t!(PreviewDevicesOnly, p.name));
        return start_after_preview(reference, start).await;
    }
    let pv = c.preview(&p.id).await?;
    let s = &pv.plan.summary;
    println!("{}", t!(PreviewOf, p.name, folders(&p)));
    println!();
    let rows = vec![
        vec![
            t!(SumUploads),
            t!(FilesSized, s.uploads, out::size(s.upload_bytes)),
        ],
        vec![
            t!(SumDownloads),
            t!(FilesSized, s.downloads + s.exports, out::size(s.download_bytes)),
        ],
        vec![t!(SumDeleteLocal), s.delete_local.to_string()],
        vec![t!(SumDeleteRemote), s.delete_remote.to_string()],
        vec![t!(SumConflicts), s.conflicts.to_string()],
        vec![t!(SumUnchanged), s.unchanged.to_string()],
    ];
    let mut rows = rows;
    // "Then delete" methods remove what they sent: the preview says so (E2E 2026-10).
    if s.moved_after_transfer > 0 {
        let label = match p.method {
            SyncMode::DownloadThenDelete => t!(SumMovedOffDrive),
            _ => t!(SumMovedOffHere),
        };
        rows.insert(4, vec![label, s.moved_after_transfer.to_string()]);
    }
    // No header row: indent every line.
    for line in out::table(
        &[vec![String::new(), String::new()]]
            .into_iter()
            .chain(rows)
            .collect::<Vec<_>>(),
    )
    .lines()
    .skip(1)
    {
        println!("  {line}");
    }
    if pv.plan.blocked || s.blocked_reason.is_some() {
        println!();
        println!(
            "{}",
            paint(
                &t!(PreviewBlocked, s.blocked_reason.clone().unwrap_or_default()),
                Tone::Bad
            )
        );
    }
    for w in &s.warnings {
        println!("{}", t!(Warning, w));
    }
    for n in pv.notes.iter().chain(&pv.infos) {
        println!("{}", t!(Note, n));
    }
    let lines: Vec<(String, String, String)> = pv
        .plan
        .actions
        .iter()
        .filter_map(|a| action_line(a, &pv.sizes))
        .collect();
    println!();
    if lines.is_empty() {
        println!("{}", t!(NothingToDo));
        return start_after_preview(reference, start).await;
    }
    println!("{}", t!(FirstChanges, lines.len().min(limit), lines.len()));
    let rows: Vec<Vec<String>> = std::iter::once(vec![String::new(); 3])
        .chain(lines.into_iter().take(limit).map(|(a, b, c)| vec![a, b, c]))
        .collect();
    for line in out::table(&rows).lines().skip(1) {
        println!("  {line}");
    }
    start_after_preview(reference, start).await
}

// ------------------------------------------------------------------- ignore

/// A path as the engine keys it: relative to the pair's folder, `/`-separated.
pub fn relative_key(local_root: &str, path: &str) -> String {
    let norm = |s: &str| s.replace('\\', "/");
    let root = norm(local_root).trim_end_matches('/').to_owned();
    let p = norm(path);
    let rel = if !root.is_empty()
        && p.len() > root.len()
        && p[..root.len()].eq_ignore_ascii_case(&root)
        && p.as_bytes()[root.len()] == b'/'
    {
        p[root.len() + 1..].to_owned()
    } else {
        p
    };
    rel.trim_start_matches("./").trim_matches('/').to_owned()
}

async fn ignore(explain: bool, reference: &str, path: &str) -> R<u8> {
    if !explain {
        return Err(Fail::new(USAGE, t!(IgnoreNeedsExplain)));
    }
    let c = connect().await?;
    let p = resolve(&c, reference).await?;
    let key = relative_key(&p.local_root, path);
    let e = c.ignore_explain(&p.id, &key).await?;
    if e.ignored {
        println!("{}", t!(IsIgnored, key));
    } else if e.note.is_some() {
        // Explain checked this device only; the note says what Drive may still decide.
        println!("{}", t!(NotIgnoredHere, key));
    } else {
        println!("{}", t!(NotIgnored, key));
    }
    if let Some(rule) = &e.rule {
        println!("  {}", t!(IgnoreRule, rule));
    }
    if let Some(src) = &e.source {
        println!("  {}", t!(IgnoreSource, src));
    }
    if let Some(note) = &e.note {
        println!("  {note}");
    }
    Ok(OK)
}

// ------------------------------------------------------------------- accounts

async fn account_list(json: bool) -> R<u8> {
    let c = connect().await?;
    let accounts = c.accounts().await?;
    if json {
        return print_json(&accounts);
    }
    if accounts.is_empty() {
        println!("{}", t!(NoAccounts));
        return Ok(OK);
    }
    let mut rows = vec![vec![t!(ColAccount), t!(ColState)]];
    for a in &accounts {
        rows.push(vec![
            a.email.clone(),
            match &a.auth_error {
                Some(e) => out::styled(&t!(SignInAgain, e), Tone::Bad),
                None => out::styled(&t!(SignedIn), Tone::Good),
            },
        ]);
    }
    print!("{}", out::table(&rows));
    Ok(OK)
}

async fn account_add() -> R<u8> {
    let c = connect().await?;
    let mut events = c.subscribe().await?;
    let url = c.add_account().await?;
    println!("{}", t!(SignInBrowser));
    println!("{}", t!(SignInUrl, url));
    let wait = async {
        while let Some(ev) = events.recv().await {
            match ev {
                Event::AccountAdded { account } => {
                    println!("{}", t!(AccountAdded, account.email));
                    return Ok(OK);
                }
                Event::AccountFailed { message } => {
                    return Err(Fail::new(AUTH, t!(AccountFailed, message)));
                }
                Event::ShuttingDown => return Err(Fail::new(GENERAL, t!(DaemonShuttingDown))),
                _ => {}
            }
        }
        Err(Fail::new(GENERAL, t!(ConnectionLost, "")))
    };
    match tokio::time::timeout(Duration::from_secs(300), wait).await {
        Ok(r) => r,
        Err(_) => Err(Fail::new(GENERAL, t!(SignInTimeout))),
    }
}

async fn account_remove(email: &str) -> R<u8> {
    let c = connect().await?;
    let accounts = c.accounts().await?;
    let a = accounts
        .iter()
        .find(|a| a.email.eq_ignore_ascii_case(email) || a.id == email)
        .ok_or_else(|| Fail::new(NOT_FOUND, t!(NoSuchAccount, email)))?;
    c.remove_account(&a.id).await?;
    println!("{}", t!(AccountRemoved, a.email));
    Ok(OK)
}

// ------------------------------------------------------------------- history, problems

// ------------------------------------------------------------------------------ devices

async fn device_list(json: bool) -> R<u8> {
    let c = connect().await?;
    let info = c.devices().await?;
    if json {
        return print_json(&info);
    }
    if !info.enabled {
        println!("{}", t!(DevicesOff));
        return Ok(OK);
    }
    let mut rows = vec![vec![t!(ColName), t!(ColId), t!(ColState)]];
    let name_of = |id: &str| {
        info.devices
            .iter()
            .find(|d| d.id == id)
            .map_or_else(|| id.to_string(), |d| i18n::clean(&d.name))
    };
    let mut arrivals = Vec::new();
    for d in &info.devices {
        if !d.known && !d.this_device {
            arrivals.push((
                i18n::clean(&d.name),
                d.added_by.as_deref().map(name_of).unwrap_or_else(|| "?".into()),
            ));
        }
        let state = if !d.known && !d.this_device {
            out::styled(&t!(DeviceNew), Tone::Bad)
        } else if d.this_device {
            out::styled(&t!(ThisDevice), Tone::Good)
        } else if d.connected {
            let how = match d.path {
                Some(wasl_ipc::LinkPath::Lan) => t!(PathLan),
                Some(wasl_ipc::LinkPath::Direct) => t!(PathDirect),
                Some(wasl_ipc::LinkPath::Punched) => t!(PathPunched),
                None => String::new(),
            };
            let label = if how.is_empty() {
                t!(Connected)
            } else {
                format!("{} ({how})", t!(Connected))
            };
            out::styled(&label, Tone::Good)
        } else {
            out::styled(&t!(NotConnected), Tone::Quiet)
        };
        rows.push(vec![i18n::clean(&d.name), d.id.clone(), state]);
    }
    print!("{}", out::table(&rows));
    if let Some(port) = info
        .listen
        .as_deref()
        .and_then(|a| a.parse::<std::net::SocketAddr>().ok())
        .map(|a| a.port())
    {
        println!("{}", t!(ListeningOn, port));
    }
    if info.devices.len() == 1 {
        println!("{}", t!(OnlyThisDevice));
    }
    for (name, by) in arrivals {
        println!("{}", t!(DeviceArrived, by, name));
    }
    Ok(OK)
}

/// A device of the group by ID (any case) or exact name.
fn find_device<'a>(info: &'a wasl_ipc::DevicesInfo, reference: &str) -> R<&'a wasl_ipc::DeviceInfo> {
    info.devices
        .iter()
        .find(|d| d.id.eq_ignore_ascii_case(reference) || d.name == reference)
        .ok_or_else(|| Fail::new(NOT_FOUND, t!(NoSuchDevice, reference)))
}

async fn device_keep(reference: &str) -> R<u8> {
    let c = connect().await?;
    let info = c.devices().await?;
    let d = find_device(&info, reference)?;
    c.keep_device(&d.id).await?;
    println!("{}", t!(DeviceKept, i18n::clean(&d.name)));
    Ok(OK)
}

async fn device_offers(json: bool, all: bool) -> R<u8> {
    let c = connect().await?;
    let offers: Vec<wasl_ipc::OfferInfo> = c
        .offers()
        .await?
        .into_iter()
        .filter(|o| all || o.status == wasl_ipc::OfferStatus::New)
        .collect();
    if json {
        return print_json(&offers);
    }
    if offers.is_empty() {
        println!("{}", t!(NoOffers));
        return Ok(OK);
    }
    let mut rows = vec![vec![t!(ColName), t!(ColFrom), t!(ColFiles), t!(ColCode)]];
    for o in &offers {
        let mut name = i18n::clean(&o.name);
        match o.status {
            wasl_ipc::OfferStatus::New => {}
            wasl_ipc::OfferStatus::Declined => name = format!("{name} ({})", t!(OfferDeclined)),
            wasl_ipc::OfferStatus::Accepted => name = format!("{name} ({})", t!(OfferAccepted)),
        }
        rows.push(vec![
            name,
            i18n::clean(&o.from_name),
            t!(OfferSize, o.files, out::size(o.bytes)),
            o.share.clone(),
        ]);
    }
    print!("{}", out::table(&rows));
    println!("{}", t!(OffersNext));
    Ok(OK)
}

/// The offer `reference` names (its code, a prefix of 4+ characters of it, or its folder name),
/// from the device `from` names when given.
async fn find_offer(c: &wasl_ipc::Client, reference: &str, from: Option<&str>) -> R<wasl_ipc::OfferInfo> {
    let from = match from {
        Some(f) => Some(find_device(&c.devices().await?, f)?.id.clone()),
        None => None,
    };
    let lower = reference.to_lowercase();
    let found: Vec<wasl_ipc::OfferInfo> = c
        .offers()
        .await?
        .into_iter()
        .filter(|o| from.as_ref().is_none_or(|f| *f == o.from_device))
        .filter(|o| {
            o.share == lower
                || (reference.chars().count() >= 4 && o.share.starts_with(&lower))
                || o.name == reference
        })
        .collect();
    match found.len() {
        0 => Err(Fail::new(NOT_FOUND, t!(NoSuchOffer, reference))),
        1 => Ok(found.into_iter().next().expect("one")),
        _ => {
            let list: Vec<String> = found
                .iter()
                .map(|o| {
                    format!(
                        "  {}  {}  {}",
                        o.share,
                        i18n::clean(&o.name),
                        i18n::clean(&o.from_name)
                    )
                })
                .collect();
            Err(Fail::new(USAGE, t!(AmbiguousOffer, reference, list.join("\n"))))
        }
    }
}

async fn device_accept(reference: &str, folder: &str, from: Option<&str>, name: Option<&str>) -> R<u8> {
    let c = connect().await?;
    let o = find_offer(&c, reference, from).await?;
    let root = std::path::absolute(folder).map_err(|e| Fail::new(USAGE, e.to_string()))?;
    let pair = c
        .accept_offer(&o.share, &o.from_device, &root.to_string_lossy(), name)
        .await?;
    println!(
        "{}",
        t!(
            OfferAcceptedNext,
            i18n::clean(&pair.name),
            root.display(),
            pair.id
        )
    );
    Ok(OK)
}

async fn device_decline(reference: &str, from: Option<&str>) -> R<u8> {
    let c = connect().await?;
    let o = find_offer(&c, reference, from).await?;
    c.decline_offer(&o.share, &o.from_device).await?;
    println!(
        "{}",
        t!(OfferDeclinedDone, i18n::clean(&o.name), i18n::clean(&o.from_name))
    );
    Ok(OK)
}

async fn device_remove(reference: &str, yes: bool) -> R<u8> {
    let c = connect().await?;
    let info = c.devices().await?;
    let d = find_device(&info, reference)?;
    println!("{}", t!(RemoveDeviceWhat, i18n::clean(&d.name), d.id));
    if !yes && !confirm(&t!(RemoveAsk))? {
        println!("{}", t!(NothingChanged));
        return Ok(OK);
    }
    c.remove_device(&d.id).await?;
    println!("{}", t!(DeviceRemoved, i18n::clean(&d.name)));
    Ok(OK)
}

/// Adds a device (`code` is `None`: this computer shows the code) or joins one: waits for both
/// devices to prove the code, asks whether the four words match, and waits for the other side.
async fn device_pair(code: Option<&str>, yes: bool) -> R<u8> {
    use wasl_ipc::PairingStage;
    let c = connect().await?;
    match code {
        None => {
            let s = c.invite_device().await?;
            println!("{}", t!(PairShowCode));
            println!();
            println!("    {}", s.code.unwrap_or_default());
            println!();
            println!("{}", t!(PairCodeNext));
        }
        Some(code) => {
            c.join_device(code).await?;
            println!("{}", t!(PairReaching));
        }
    }
    let wait_for = |want: &'static [PairingStage]| {
        let c = &c;
        async move {
            loop {
                let s = c.pairing_status().await?;
                if want.contains(&s.stage) || s.stage == PairingStage::Failed {
                    return Ok::<_, Fail>(s);
                }
                tokio::time::sleep(Duration::from_millis(300)).await;
            }
        }
    };
    let s = wait_for(&[PairingStage::Confirm]).await?;
    if s.stage == PairingStage::Failed {
        return Err(Fail::new(GENERAL, s.message.unwrap_or_default()));
    }
    println!("{}", t!(PairWords));
    println!();
    println!("    {}", s.words.join("  "));
    println!();
    let same = yes || confirm(&t!(PairWordsAsk))?;
    let s = c.confirm_pairing(same).await?;
    if !same {
        println!("{}", t!(PairNotAdded));
        return Ok(GENERAL);
    }
    if s.stage != PairingStage::Paired {
        println!("{}", t!(PairWaitingOther));
    }
    let s = wait_for(&[PairingStage::Paired]).await?;
    match s.stage {
        PairingStage::Paired => {
            println!(
                "{}",
                t!(PairDone, i18n::clean(s.peer_name.as_deref().unwrap_or("?")))
            );
            Ok(OK)
        }
        _ => Err(Fail::new(GENERAL, s.message.unwrap_or_default())),
    }
}

async fn device_connect(address: &str) -> R<u8> {
    let c = connect().await?;
    let id = c.connect_device(address).await?;
    println!("{}", t!(DeviceConnected, id, address));
    Ok(OK)
}

async fn pair_share(reference: &str, code: Option<&str>, with: &[String]) -> R<u8> {
    let c = connect().await?;
    let p = resolve(&c, reference).await?;
    let devices = if with.is_empty() {
        Vec::new()
    } else {
        let info = c.devices().await?;
        with.iter()
            .map(|w| find_device(&info, w).map(|d| (d.id.clone(), i18n::clean(&d.name))))
            .collect::<R<Vec<_>>>()?
    };
    let share = c.share_pair(&p.id, code).await?;
    if !devices.is_empty() {
        let ids = devices.iter().map(|(id, _)| id.clone()).collect();
        c.share_with(&p.id, false, ids).await?;
        let names: Vec<&str> = devices.iter().map(|(_, n)| n.as_str()).collect();
        println!("{}", t!(PairOffered, p.name, names.join(", ")));
        return Ok(OK);
    }
    if code.is_some() {
        println!("{}", t!(PairJoinedShare, p.name));
    } else {
        println!("{}", t!(PairShared, p.name));
        println!("{}", t!(ShareCodeNext, share));
    }
    Ok(OK)
}

async fn pair_unshare(reference: &str) -> R<u8> {
    let c = connect().await?;
    let p = resolve(&c, reference).await?;
    c.unshare_pair(&p.id).await?;
    println!("{}", t!(PairUnshared, p.name));
    Ok(OK)
}

// ------------------------------------------------------------------- vaults

/// Finds a vault by exact id, exact name (any case) or a unique id prefix of 4+ characters;
/// `None` when nothing matches.
pub fn find_vault<'a>(
    vaults: &'a [wasl_ipc::VaultInfo],
    reference: &str,
) -> R<Option<&'a wasl_ipc::VaultInfo>> {
    if let Some(v) = vaults.iter().find(|v| v.id == reference) {
        return Ok(Some(v));
    }
    let lower = reference.to_lowercase();
    let by_name: Vec<&wasl_ipc::VaultInfo> =
        vaults.iter().filter(|v| v.name.to_lowercase() == lower).collect();
    let candidates = match by_name.len() {
        1 => return Ok(Some(by_name[0])),
        0 if reference.chars().count() >= 4 => {
            vaults.iter().filter(|v| v.id.starts_with(reference)).collect()
        }
        0 => Vec::new(),
        _ => by_name,
    };
    match candidates.len() {
        0 => Ok(None),
        1 => Ok(Some(candidates[0])),
        _ => {
            let list: Vec<String> = candidates
                .iter()
                .map(|v| format!("  {}  {}", v.id, i18n::clean(&v.name)))
                .collect();
            Err(Fail::new(USAGE, t!(AmbiguousVault, reference, list.join("\n"))))
        }
    }
}

/// A daemon refusal, with a wrong passphrase said in the person's language.
fn vault_fail(e: ClientError) -> Fail {
    match &e {
        ClientError::Remote(r)
            if r.kind == ErrorKind::Invalid && r.message.starts_with("Wrong passphrase") =>
        {
            Fail::new(AUTH, t!(VaultWrongPass))
        }
        _ => Fail::from(e),
    }
}

fn wasl_passphrase(p: zeroize::Zeroizing<String>) -> R<wasl_ipc::Passphrase> {
    if p.is_empty() {
        return Err(Fail::new(USAGE, t!(VaultPassEmpty)));
    }
    Ok(wasl_ipc::Passphrase::new(p.as_str()))
}

async fn vault_list(json: bool) -> R<u8> {
    let c = connect().await?;
    let vaults = c.vaults().await?;
    if json {
        return print_json(&vaults);
    }
    if vaults.is_empty() {
        println!("{}", t!(NoVaults));
        return Ok(OK);
    }
    let mut rows = vec![vec![
        t!(ColId),
        t!(ColName),
        t!(ColFormat),
        t!(ColState),
        t!(ColDrive),
    ]];
    for v in &vaults {
        rows.push(vec![
            out::short_id(&v.id),
            i18n::clean(&v.name),
            if v.format.is_empty() {
                format!("v{}", v.version)
            } else {
                format!("v{}", v.format)
            },
            if v.unlocked {
                out::styled(&t!(VaultUnlockedHere), Tone::Good)
            } else {
                out::styled(&t!(VaultLockedHere), Tone::Quiet)
            },
            i18n::clean(&v.remote_path),
        ]);
    }
    print!("{}", out::table(&rows));
    Ok(OK)
}

async fn vault_create(
    drive_folder: &str,
    name: Option<String>,
    account: Option<&str>,
    passphrase_stdin: bool,
) -> R<u8> {
    let c = connect().await?;
    let accounts = c.accounts().await?;
    let account = pick_account(&accounts, account)?;
    let drive_path = normalise_drive_path(drive_folder);
    let Some((parent, folder_name)) = drive_path
        .rsplit_once('/')
        .filter(|(p, n)| !n.trim().is_empty() && !p.eq_ignore_ascii_case("shared drives"))
    else {
        return Err(Fail::new(USAGE, t!(VaultNeedsFolder)));
    };
    // The parent must exist; only the vault's own folder is made.
    let parent = c.resolve_drive_path(&account.id, parent, false).await?;
    let name = name.unwrap_or_else(|| folder_name.to_owned());
    if !passphrase_stdin {
        println!("{}", t!(VaultLoseWarning));
    }
    let passphrase = wasl_passphrase(crate::vault::read_new_passphrase(passphrase_stdin)?)?;
    if passphrase.expose().chars().count() < 8 {
        return Err(Fail::new(USAGE, t!(VaultPassShort)));
    }
    let folder = c.resolve_drive_path(&account.id, &drive_path, true).await?;
    println!("{}", t!(VaultMakingKeys));
    let made = c
        .create_vault(wasl_ipc::NewVault {
            account_id: account.id.clone(),
            drive_id: folder.drive_id.or(parent.drive_id),
            parent_id: parent.id,
            folder_id: Some(folder.id),
            remote_path: drive_path.clone(),
            name,
            passphrase,
        })
        .await?;
    println!(
        "{}",
        t!(VaultCreated, i18n::clean(&made.name), i18n::clean(&drive_path))
    );
    Ok(OK)
}

async fn vault_unlock(reference: &str, account: Option<&str>, passphrase_stdin: bool) -> R<u8> {
    let c = connect().await?;
    let vaults = c.vaults().await?;
    let known = find_vault(&vaults, reference)?.cloned();
    let folder = match &known {
        Some(_) => None,
        None => {
            let accounts = c.accounts().await?;
            let account = pick_account(&accounts, account)?;
            let drive_path = normalise_drive_path(reference);
            let found = c
                .resolve_drive_path(&account.id, &drive_path, false)
                .await
                .map_err(|e| match e.kind() {
                    Some(ErrorKind::NotFound) => Fail::new(NOT_FOUND, t!(NoSuchVault, reference)),
                    _ => Fail::from(e),
                })?;
            Some(wasl_ipc::VaultFolder {
                account_id: account.id.clone(),
                folder_id: found.id,
                drive_id: found.drive_id,
                remote_path: drive_path,
            })
        }
    };
    let passphrase = wasl_passphrase(crate::vault::read_passphrase(passphrase_stdin)?)?;
    println!("{}", t!(VaultChecking));
    let opened = match (known, folder) {
        (Some(v), _) => c.unlock_vault(&v.id, passphrase).await,
        (None, Some(f)) => c.unlock_vault_folder(f, passphrase).await,
        (None, None) => return Err(Fail::new(NOT_FOUND, t!(NoSuchVault, reference))),
    }
    .map_err(vault_fail)?;
    println!("{}", t!(VaultUnlocked, i18n::clean(&opened.name)));
    Ok(OK)
}

async fn vault_upgrade(
    reference: &str,
    drive_folder: &str,
    account: Option<&str>,
    passphrase_stdin: bool,
) -> R<u8> {
    let c = connect().await?;
    let vaults = c.vaults().await?;
    let v =
        find_vault(&vaults, reference)?.ok_or_else(|| Fail::new(NOT_FOUND, t!(NoSuchVault, reference)))?;
    let accounts = c.accounts().await?;
    let account = pick_account(&accounts, account)?;
    let drive_path = normalise_drive_path(drive_folder);
    let Some((parent, _)) = drive_path
        .rsplit_once('/')
        .filter(|(p, n)| !n.trim().is_empty() && !p.eq_ignore_ascii_case("shared drives"))
    else {
        return Err(Fail::new(USAGE, t!(VaultNeedsFolder)));
    };
    c.resolve_drive_path(&account.id, parent, false).await?;
    // The new vault keeps the old vault's passphrase (wasld checks it against the old vault), so
    // this asks once for the passphrase people already type, not for a new one.
    let passphrase = wasl_passphrase(crate::vault::read_passphrase(passphrase_stdin)?)?;
    let folder = c.resolve_drive_path(&account.id, &drive_path, true).await?;
    println!("{}", t!(VaultUpgrading, i18n::clean(&v.name)));
    let r = c
        .upgrade_vault(
            &v.id,
            wasl_ipc::VaultFolder {
                account_id: account.id.clone(),
                folder_id: folder.id,
                drive_id: folder.drive_id,
                remote_path: drive_path.clone(),
            },
            passphrase,
        )
        .await?;
    println!(
        "{}",
        t!(
            VaultUpgraded,
            i18n::clean(&v.name),
            i18n::clean(&drive_path),
            r.files,
            out::size(r.bytes),
            r.already_there
        )
    );
    for key in r.left_out.iter().chain(&r.failed) {
        println!("  {}", t!(VaultNotCopied, i18n::clean(key)));
    }
    println!("{}", t!(VaultUpgradeNext));
    Ok(if r.left_out.is_empty() && r.failed.is_empty() {
        OK
    } else {
        GENERAL
    })
}

async fn vault_lock(reference: &str) -> R<u8> {
    let c = connect().await?;
    let vaults = c.vaults().await?;
    let v =
        find_vault(&vaults, reference)?.ok_or_else(|| Fail::new(NOT_FOUND, t!(NoSuchVault, reference)))?;
    c.lock_vault(&v.id).await?;
    println!("{}", t!(VaultLocked, i18n::clean(&v.name)));
    Ok(OK)
}

// ------------------------------------------------------------------- views

/// Finds a view by exact id, exact name (any case) or a unique id prefix of 4+ characters.
pub fn find_view<'a>(views: &'a [wasl_ipc::View], reference: &str) -> R<&'a wasl_ipc::View> {
    if let Some(v) = views.iter().find(|v| v.id == reference) {
        return Ok(v);
    }
    let lower = reference.to_lowercase();
    let by_name: Vec<&wasl_ipc::View> = views.iter().filter(|v| v.name.to_lowercase() == lower).collect();
    let candidates = match by_name.len() {
        1 => return Ok(by_name[0]),
        0 if reference.chars().count() >= 4 => views.iter().filter(|v| v.id.starts_with(reference)).collect(),
        0 => Vec::new(),
        _ => by_name,
    };
    match candidates.len() {
        0 => Err(Fail::new(NOT_FOUND, t!(NoSuchView, reference))),
        1 => Ok(candidates[0]),
        _ => {
            let list: Vec<String> = candidates
                .iter()
                .map(|v| format!("  {}  {}", v.id, i18n::clean(&v.name)))
                .collect();
            Err(Fail::new(USAGE, t!(AmbiguousView, reference, list.join("\n"))))
        }
    }
}

fn view_state(s: &wasl_ipc::ViewState) -> String {
    let v = &s.view;
    if !v.enabled {
        out::styled(&t!(StateOff), Tone::Quiet)
    } else if let Some(e) = &v.last_error {
        out::styled(&t!(ViewNotShowing, i18n::clean(e)), Tone::Bad)
    } else if s.mounted {
        let when = v.last_refresh_at.map_or_else(|| t!(Never), out::when);
        out::styled(&t!(ViewShowing, s.files, when), Tone::Good)
    } else {
        out::styled(&t!(ViewStarting), Tone::Busy)
    }
}

async fn view_list(json: bool) -> R<u8> {
    let c = connect().await?;
    let info = c.views().await?;
    if json {
        return print_json(&info);
    }
    if !info.supported {
        println!("{}", t!(ViewsUnsupported));
        return Ok(OK);
    }
    if info.views.is_empty() {
        println!("{}", t!(NoViews));
        return Ok(OK);
    }
    let mut rows = vec![vec![
        t!(ColId),
        t!(ColName),
        t!(ColDrive),
        t!(ColLocal),
        t!(ColState),
        t!(ColOnComputer),
    ]];
    for s in &info.views {
        let v = &s.view;
        rows.push(vec![
            out::short_id(&v.id),
            i18n::clean(&v.name),
            i18n::clean(&v.remote_path),
            i18n::clean(&v.local_path),
            view_state(s),
            t!(
                CacheUse,
                out::size(s.cache_used),
                out::size(v.cache_limit),
                out::size(s.pinned_bytes)
            ),
        ]);
    }
    print!("{}", out::table(&rows));
    for s in info.views.iter().filter(|s| !s.local_changes.is_empty()) {
        println!();
        println!(
            "{}",
            t!(ViewLocalChanges, i18n::clean(&s.view.name), s.local_changes.len())
        );
        for ch in s.local_changes.iter().take(20) {
            let why = match ch.kind {
                Some(LocalChangeKind::OnlyHere) => t!(ChangeOnlyHere),
                Some(LocalChangeKind::ChangedHere) => t!(ChangeChangedHere),
                Some(LocalChangeKind::Differs) => t!(ChangeDiffers),
                Some(LocalChangeKind::Unsupported) => t!(ChangeUnsupported),
                Some(LocalChangeKind::Failed) => t!(ChangeFailed, i18n::clean(&ch.reason)),
                Some(LocalChangeKind::Restored) => t!(ChangeRestored, i18n::clean(&ch.reason)),
                None => i18n::clean(&ch.reason),
            };
            println!("  {}  ({why})", i18n::clean(&ch.path));
        }
    }
    Ok(OK)
}

async fn view_add(
    drive_folder: &str,
    local: &str,
    name: Option<String>,
    account: Option<&str>,
    cache_limit: Option<u64>,
) -> R<u8> {
    let c = connect().await?;
    let folder = std::path::absolute(local).map_err(|e| Fail::new(USAGE, e.to_string()))?;
    let accounts = c.accounts().await?;
    let account = pick_account(&accounts, account)?;
    let drive_path = normalise_drive_path(drive_folder);
    let drive = c.resolve_drive_path(&account.id, &drive_path, false).await?;
    let name = name.unwrap_or_else(|| {
        drive_path
            .rsplit('/')
            .next()
            .filter(|n| !n.is_empty())
            .unwrap_or("Drive")
            .to_owned()
    });
    let view = c
        .add_view(wasl_ipc::NewView {
            name,
            account_id: account.id.clone(),
            drive_folder_id: drive.id,
            drive_id: drive.drive_id,
            remote_path: drive_path.clone(),
            local_path: folder.display().to_string(),
            cache_limit,
        })
        .await?;
    println!(
        "{}",
        t!(
            ViewAdded,
            i18n::clean(&view.name),
            i18n::clean(&drive_path),
            i18n::clean(&view.local_path)
        )
    );
    match &view.last_error {
        Some(e) => {
            println!("{}", t!(ViewAddedNotYet, i18n::clean(e)));
            Ok(GENERAL)
        }
        None => Ok(OK),
    }
}

async fn view_remove(reference: &str, keep_files: bool, yes: bool) -> R<u8> {
    let c = connect().await?;
    let views: Vec<wasl_ipc::View> = c.views().await?.views.into_iter().map(|s| s.view).collect();
    let v = find_view(&views, reference)?;
    println!(
        "{}",
        t!(ViewRemoveWhat, i18n::clean(&v.name), i18n::clean(&v.local_path))
    );
    println!(
        "{}",
        if keep_files {
            t!(ViewRemoveKeep)
        } else {
            t!(ViewRemoveFiles)
        }
    );
    if !yes && !confirm(&t!(RemoveAsk))? {
        println!("{}", t!(NothingChanged));
        return Ok(OK);
    }
    c.remove_view(&v.id, keep_files).await?;
    println!("{}", t!(ViewRemoved, i18n::clean(&v.name)));
    Ok(OK)
}

async fn view_pin(path: &str, pinned: bool) -> R<u8> {
    let c = connect().await?;
    let full = std::path::absolute(path).map_err(|e| Fail::new(USAGE, e.to_string()))?;
    let shown = i18n::clean(&full.display().to_string());
    let full = full.display().to_string();
    if pinned {
        c.pin_path(&full).await?;
        println!("{}", t!(ViewPinned, shown));
    } else {
        c.unpin_path(&full).await?;
        println!("{}", t!(ViewUnpinned, shown));
    }
    Ok(OK)
}

async fn view_refresh(reference: &str) -> R<u8> {
    let c = connect().await?;
    let views: Vec<wasl_ipc::View> = c.views().await?.views.into_iter().map(|s| s.view).collect();
    let v = find_view(&views, reference)?;
    c.refresh_view(&v.id).await?;
    println!("{}", t!(ViewRefreshed, i18n::clean(&v.name)));
    Ok(OK)
}

async fn history(pair: Option<String>, limit: u32, json: bool) -> R<u8> {
    let c = connect().await?;
    let pairs = c.pairs().await?;
    let only = match &pair {
        Some(r) => Some(find_pair(&pairs, r)?.id.clone()),
        None => None,
    };
    let runs = c.runs(only.as_deref(), limit).await?;
    if json {
        return print_json(&runs);
    }
    if runs.is_empty() {
        println!("{}", t!(NoHistory));
        return Ok(OK);
    }
    let names: HashMap<&str, &str> = pairs.iter().map(|p| (p.id.as_str(), p.name.as_str())).collect();
    let mut rows = vec![vec![
        t!(ColStarted),
        t!(ColPair),
        t!(ColResult),
        t!(ColChanges),
        t!(ColMessage),
    ]];
    for r in &runs {
        let (status, tone) = run_status_label(r.status);
        rows.push(vec![
            out::when(r.started_at),
            names
                .get(r.pair_id.as_str())
                .copied()
                .unwrap_or(&r.pair_id)
                .to_owned(),
            out::styled(&status, tone),
            changes(
                r.stats.uploaded,
                r.stats.downloaded + r.stats.exported,
                r.stats.deleted_local + r.stats.deleted_remote,
            ),
            r.message.clone().unwrap_or_default(),
        ]);
    }
    print!("{}", out::table(&rows));
    Ok(OK)
}

#[derive(serde::Serialize)]
struct PairProblem {
    pair_id: String,
    name: String,
    problem: &'static str,
    message: Option<String>,
}

#[derive(serde::Serialize)]
struct Problems {
    accounts: Vec<Account>,
    pairs: Vec<PairProblem>,
    conflicts: Vec<wasl_proto::ConflictRecord>,
    /// Agent changesets waiting for a decision (docs/AGENT_CHANGES.md).
    agent_changes: Vec<wasl_proto::Changeset>,
}

async fn problems(json: bool) -> R<u8> {
    let c = connect().await?;
    let st = c.status().await?;
    let conflicts = c.open_conflicts(None).await?;
    let accounts: Vec<Account> = st
        .accounts
        .iter()
        .filter(|a| a.auth_error.is_some())
        .cloned()
        .collect();
    let mut pairs = Vec::new();
    for ps in &st.pairs {
        let p = &ps.pair;
        let problem =
            if ps.blocked || p.status.pending_approval || p.status.last_status == Some(RunStatus::Blocked) {
                "blocked"
            } else if p.status.last_status == Some(RunStatus::Failed) {
                "failed"
            } else {
                continue;
            };
        pairs.push(PairProblem {
            pair_id: p.id.clone(),
            name: p.name.clone(),
            problem,
            message: p.status.last_error.clone(),
        });
    }
    let agent_changes: Vec<wasl_proto::Changeset> = c
        .agent_changesets(None, false)
        .await?
        .into_iter()
        .filter(|cs| cs.status == wasl_proto::ChangesetStatus::Held)
        .collect();
    let found = Problems {
        accounts,
        pairs,
        conflicts,
        agent_changes,
    };
    if json {
        return print_json(&found);
    }
    if found.accounts.is_empty()
        && found.pairs.is_empty()
        && found.conflicts.is_empty()
        && found.agent_changes.is_empty()
    {
        println!("{}", paint(&t!(NoProblems), Tone::Good));
        return Ok(OK);
    }
    for a in &found.accounts {
        println!("{}", t!(AccountNeedsSignIn, a.email));
    }
    for p in &found.pairs {
        let msg = p.message.clone().unwrap_or_default();
        if p.problem == "blocked" {
            println!("{}", t!(ProblemBlocked, p.name));
        } else {
            println!("{}", t!(ProblemFailed, p.name, msg).trim_end_matches([' ', ':']));
        }
    }
    if !found.agent_changes.is_empty() {
        println!("{}", t!(ProblemAgentChanges, found.agent_changes.len()));
    }
    if !found.conflicts.is_empty() {
        let names: HashMap<&str, &str> = st
            .pairs
            .iter()
            .map(|p| (p.pair.id.as_str(), p.pair.name.as_str()))
            .collect();
        println!();
        println!("{}", t!(ConflictsHeader, found.conflicts.len()));
        let mut rows = vec![vec![t!(ColPair), t!(ColPath), t!(ColDetected)]];
        for k in &found.conflicts {
            rows.push(vec![
                names
                    .get(k.pair_id.as_str())
                    .copied()
                    .unwrap_or(&k.pair_id)
                    .to_owned(),
                k.key.clone(),
                out::when(k.detected_at),
            ]);
        }
        print!("{}", out::table(&rows));
    }
    Ok(OK)
}

// ------------------------------------------------------------------- bin

async fn bin_list(pair: Option<String>, json: bool) -> R<u8> {
    let c = connect().await?;
    let pairs = c.pairs().await?;
    let only = match &pair {
        Some(r) => Some(find_pair(&pairs, r)?.id.clone()),
        None => None,
    };
    let entries = c.bin_entries(only.as_deref()).await?;
    if json {
        return print_json(&entries);
    }
    if entries.is_empty() {
        println!("{}", t!(BinEmptyAlready));
        return Ok(OK);
    }
    let names: HashMap<&str, &str> = pairs.iter().map(|p| (p.id.as_str(), p.name.as_str())).collect();
    let mut rows = vec![vec![
        t!(ColId),
        t!(ColPair),
        t!(ColWhere),
        t!(ColPath),
        t!(ColDeleted),
        t!(ColSize),
    ]];
    for b in &entries {
        let e = &b.entry;
        rows.push(vec![
            e.id.clone(),
            e.pair_name
                .clone()
                .or_else(|| names.get(e.pair_id.as_str()).map(|s| (*s).to_owned()))
                .unwrap_or_else(|| e.pair_id.clone()),
            match e.side {
                BinSide::Local => t!(ThisDevice),
                BinSide::Drive => "Drive".to_owned(),
            },
            e.key.clone(),
            out::when(e.deleted_at),
            out::size(e.size),
        ]);
    }
    print!("{}", out::table(&rows));
    Ok(OK)
}

async fn bin_restore(ids: Vec<String>) -> R<u8> {
    let c = connect().await?;
    let r = c.restore_bin(ids).await?;
    for b in &r.restored {
        println!("{}", t!(Restored, b.entry.key));
    }
    for f in &r.failed {
        eprintln!("{}", paint(&t!(RestoreFailed, f.entry_id, f.message), Tone::Bad));
    }
    Ok(if r.failed.is_empty() { OK } else { GENERAL })
}

async fn bin_empty(yes: bool) -> R<u8> {
    let c = connect().await?;
    if !yes && !confirm(&t!(EmptyAsk))? {
        println!("{}", t!(NothingChanged));
        return Ok(OK);
    }
    let n = c.empty_bin().await?;
    println!("{}", t!(BinEmptied, n));
    Ok(OK)
}

// ------------------------------------------------------------------- daemon, service

async fn daemon_start() -> R<u8> {
    let paths = paths()?;
    if let Ok(c) = Client::connect(&paths).await {
        println!("{}", t!(DaemonAlreadyRunning, c.daemon_version));
        return Ok(OK);
    }
    let exe = service::find_daemon().ok_or_else(|| Fail::new(GENERAL, t!(DaemonNotFound)))?;
    let mut cmd = std::process::Command::new(&exe);
    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        cmd.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    let mut child = cmd
        .spawn()
        .map_err(|e| Fail::new(GENERAL, t!(DaemonSpawnFailed, exe.display(), e)))?;
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if let Ok(c) = Client::connect(&paths).await {
            println!("{}", t!(DaemonStarted, c.daemon_version));
            return Ok(OK);
        }
        if let Ok(Some(code)) = child.try_wait() {
            return Err(Fail::new(GENERAL, t!(DaemonExited, code)));
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    Err(Fail::new(GENERAL, t!(DaemonStartTimeout)))
}

async fn daemon_stop() -> R<u8> {
    let paths = paths()?;
    let c = Client::connect(&paths).await?;
    match c.shutdown().await {
        // The daemon may go before its answer arrives.
        Ok(()) | Err(ClientError::Io(_)) => {}
        Err(e) => return Err(e.into()),
    }
    drop(c);
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if Client::connect(&paths).await.is_err() {
            println!("{}", t!(DaemonStopped));
            return Ok(OK);
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    Err(Fail::new(GENERAL, t!(DaemonStopTimeout)))
}

async fn daemon_status() -> R<u8> {
    let c = connect().await?;
    println!("{}", t!(DaemonRunning, c.daemon_version));
    Ok(OK)
}

fn service_install() -> R<u8> {
    let exe = service::find_daemon().ok_or_else(|| Fail::new(GENERAL, t!(DaemonNotFound)))?;
    service::install(&exe).map_err(|e| Fail::new(GENERAL, t!(ServiceFailed, e)))?;
    println!("{}", t!(ServiceInstalled, exe.display()));
    Ok(OK)
}

fn service_uninstall() -> R<u8> {
    service::uninstall().map_err(|e| Fail::new(GENERAL, t!(ServiceFailed, e)))?;
    println!("{}", t!(ServiceUninstalled));
    Ok(OK)
}

fn service_status() -> R<u8> {
    match service::status().map_err(|e| Fail::new(GENERAL, t!(ServiceFailed, e)))? {
        service::ServiceState::Installed { location } => {
            println!("{}", t!(ServiceIsInstalled, location));
        }
        service::ServiceState::NotInstalled => println!("{}", t!(ServiceNotInstalled)),
    }
    Ok(OK)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pair(id: &str, name: &str) -> Pair {
        Pair::new(id, name, "/x")
    }

    #[test]
    fn pairs_are_found_by_id_name_or_prefix() {
        let pairs = vec![
            pair("abcd1234", "Photos"),
            pair("abcd9999", "Work"),
            pair("ffff0000", "work"),
        ];
        assert_eq!("abcd1234", find_pair(&pairs, "abcd1234").unwrap().id);
        assert_eq!("abcd1234", find_pair(&pairs, "photos").unwrap().id);
        assert_eq!("ffff0000", find_pair(&pairs, "ffff").unwrap().id);
        // Too short for a prefix.
        assert_eq!(NOT_FOUND, find_pair(&pairs, "fff").unwrap_err().code);
        // Two prefixes, two names.
        assert_eq!(USAGE, find_pair(&pairs, "abcd").unwrap_err().code);
        assert_eq!(USAGE, find_pair(&pairs, "WORK").unwrap_err().code);
    }

    #[test]
    fn drive_paths_default_to_my_drive() {
        assert_eq!("My Drive/Photos", normalise_drive_path("Photos"));
        assert_eq!("My Drive/Photos", normalise_drive_path("/Photos/"));
        assert_eq!("My Drive/a/b", normalise_drive_path("My Drive/a/b"));
        assert_eq!(
            "Shared drives/Team/x",
            normalise_drive_path("Shared drives/Team/x")
        );
        assert_eq!("My Drive", normalise_drive_path(""));
    }

    #[test]
    fn paths_become_keys_relative_to_the_folder() {
        assert_eq!(
            "a/b.txt",
            relative_key(r"C:\Users\me\Docs", r"C:\Users\me\Docs\a\b.txt")
        );
        assert_eq!("a/b.txt", relative_key("/home/me/Docs/", "/home/me/Docs/a/b.txt"));
        assert_eq!(
            "node_modules/x",
            relative_key("/home/me/Docs", "./node_modules/x")
        );
        assert_eq!(
            "/other/x",
            "/".to_owned() + &relative_key("/home/me/Docs", "/other/x")
        );
    }

    fn view(id: &str, name: &str) -> wasl_ipc::View {
        wasl_ipc::View {
            id: id.into(),
            name: name.into(),
            account_id: "me@example.com".into(),
            drive_folder_id: "F".into(),
            drive_id: None,
            remote_path: "My Drive/x".into(),
            local_path: "/x".into(),
            vault_id: None,
            cache_limit: 0,
            enabled: true,
            created_at: 0,
            last_refresh_at: None,
            last_error: None,
        }
    }

    #[test]
    fn views_are_found_by_id_name_or_prefix() {
        let views = vec![view("abcd1234", "Photos"), view("abcd9999", "Work")];
        assert_eq!("abcd1234", find_view(&views, "photos").unwrap().id);
        assert_eq!("abcd9999", find_view(&views, "abcd9").unwrap().id);
        assert_eq!(USAGE, find_view(&views, "abcd").unwrap_err().code);
        assert_eq!(NOT_FOUND, find_view(&views, "Music").unwrap_err().code);
    }

    #[test]
    fn sizes_parse_in_binary_units() {
        use crate::parse_size;
        assert_eq!(Ok(20 << 30), parse_size("20G"));
        assert_eq!(Ok(20 << 30), parse_size("20GiB"));
        assert_eq!(Ok(500 << 20), parse_size("500m"));
        assert_eq!(Ok(500 << 20), parse_size(" 500 MB "));
        assert_eq!(Ok(3 << 29), parse_size("1.5G"));
        assert_eq!(Ok(4096), parse_size("4096"));
        assert_eq!(Ok(64 << 10), parse_size("64K"));
        assert!(parse_size("lots").is_err());
        assert!(parse_size("-1G").is_err());
        assert!(parse_size("").is_err());
    }

    #[test]
    fn view_commands_parse() {
        use crate::{Cli, ViewCmd};
        use clap::Parser;
        let cli = Cli::try_parse_from([
            "wasl",
            "view",
            "add",
            "Photos",
            "/home/me/Photos on demand",
            "--name",
            "Photos",
            "--cache-limit",
            "2G",
        ])
        .unwrap();
        match cli.cmd {
            Cmd::View(ViewCmd::Add {
                drive_folder,
                local_path,
                name,
                account,
                cache_limit,
            }) => {
                assert_eq!("Photos", drive_folder);
                assert_eq!("/home/me/Photos on demand", local_path);
                assert_eq!(Some("Photos".into()), name);
                assert_eq!(None, account);
                assert_eq!(Some(2 << 30), cache_limit);
            }
            other => panic!("{other:?}"),
        }
        // #148: the words can be confirmed without a terminal, as the "add --yes" hint says.
        let cli = Cli::try_parse_from(["wasl", "device", "add", "--yes"]).unwrap();
        assert!(matches!(cli.cmd, Cmd::Device(DeviceCmd::Add { yes: true })));
        let cli = Cli::try_parse_from(["wasl", "device", "join", "ABCD-EFGH", "--yes"]).unwrap();
        assert!(matches!(cli.cmd, Cmd::Device(DeviceCmd::Join { yes: true, .. })));
        let cli = Cli::try_parse_from(["wasl", "view", "remove", "Photos", "--keep-files", "--yes"]).unwrap();
        assert!(matches!(
            cli.cmd,
            Cmd::View(ViewCmd::Remove {
                keep_files: true,
                yes: true,
                ..
            })
        ));
        let cli = Cli::try_parse_from(["wasl", "view", "pin", "a/b.jpg"]).unwrap();
        assert!(matches!(cli.cmd, Cmd::View(ViewCmd::Pin { path }) if path == "a/b.jpg"));
        let cli = Cli::try_parse_from(["wasl", "view", "unpin", "a"]).unwrap();
        assert!(matches!(cli.cmd, Cmd::View(ViewCmd::Unpin { .. })));
        assert!(
            Cli::try_parse_from(["wasl", "view", "add", "Photos", "/x", "--cache-limit", "huge"]).is_err()
        );
        assert!(Cli::try_parse_from(["wasl", "view", "list", "--json"]).is_ok());
    }

    fn vault(id: &str, name: &str) -> wasl_ipc::VaultInfo {
        wasl_ipc::VaultInfo {
            id: id.into(),
            name: name.into(),
            version: 2,
            format: "2.0".into(),
            unlocked: false,
            account_id: None,
            drive_folder_id: None,
            drive_id: None,
            remote_path: String::new(),
            created_at: 0,
        }
    }

    #[test]
    fn vaults_are_found_by_id_name_or_prefix_and_otherwise_not_at_all() {
        let vaults = vec![vault("abcd1234", "Private"), vault("abcd9999", "Taxes")];
        assert_eq!("abcd1234", find_vault(&vaults, "private").unwrap().unwrap().id);
        assert_eq!("abcd9999", find_vault(&vaults, "abcd9").unwrap().unwrap().id);
        assert_eq!(USAGE, find_vault(&vaults, "abcd").unwrap_err().code);
        // Not a vault here: `wasl vault unlock` then takes it as a Drive folder.
        assert!(find_vault(&vaults, "My Drive/Old vault").unwrap().is_none());
    }

    #[test]
    fn vault_commands_parse_and_never_take_a_passphrase_argument() {
        use crate::{Cli, VaultCmd};
        use clap::Parser;
        let cli =
            Cli::try_parse_from(["wasl", "vault", "create", "My Drive/Private", "--name", "Mine"]).unwrap();
        match cli.cmd {
            Cmd::Vault(VaultCmd::Create {
                drive_folder,
                name,
                account,
                passphrase_stdin,
            }) => {
                assert_eq!("My Drive/Private", drive_folder);
                assert_eq!(Some("Mine".into()), name);
                assert_eq!(None, account);
                assert!(!passphrase_stdin);
            }
            other => panic!("{other:?}"),
        }
        let cli = Cli::try_parse_from(["wasl", "vault", "unlock", "Private", "--passphrase-stdin"]).unwrap();
        assert!(matches!(
            cli.cmd,
            Cmd::Vault(VaultCmd::Unlock { vault, passphrase_stdin: true, .. }) if vault == "Private"
        ));
        let cli = Cli::try_parse_from(["wasl", "vault", "lock", "abcd"]).unwrap();
        assert!(matches!(cli.cmd, Cmd::Vault(VaultCmd::Lock { vault }) if vault == "abcd"));
        let cli = Cli::try_parse_from([
            "wasl",
            "vault",
            "upgrade",
            "Private",
            "My Drive/Private 2.1",
            "--passphrase-stdin",
        ])
        .unwrap();
        assert!(
            matches!(cli.cmd, Cmd::Vault(VaultCmd::Upgrade { vault, drive_folder, passphrase_stdin: true, .. })
            if vault == "Private" && drive_folder == "My Drive/Private 2.1")
        );
        assert!(Cli::try_parse_from(["wasl", "vault", "list", "--json"]).is_ok());
        for cmd in ["create", "unlock"] {
            assert!(
                Cli::try_parse_from(["wasl", "vault", cmd, "Private", "--passphrase", "secret"]).is_err(),
                "{cmd}"
            );
        }
        assert!(Cli::try_parse_from(["wasl", "vault", "unlock", "Private", "extra secret"]).is_err());
    }

    #[test]
    fn ids_look_like_uuids() {
        let id = new_id().unwrap();
        assert_eq!(36, id.len());
        assert_eq!(Some('4'), id.chars().nth(14));
    }
}
