//! `wasl`: the command line of Wasl (SRS FR-A3). Every command talks to `wasld` over the local
//! protocol; messages are in English or Arabic (NFR-8).

mod agent;
mod cmd;
mod oplog;
mod out;
mod vault;

use clap::{Args, Parser, Subcommand, ValueEnum};
use std::process::ExitCode;
use wasl_cli::{i18n, t};

#[derive(Parser, Debug)]
#[command(
    name = "wasl",
    version,
    about = "Wasl: sync folders on this computer with Google Drive",
    long_about = None,
)]
pub struct Cli {
    #[command(subcommand)]
    pub cmd: Cmd,
}

#[derive(Subcommand, Debug)]
pub enum Cmd {
    /// The daemon and every folder pair at a glance.
    Status {
        /// Machine-readable output.
        #[arg(long)]
        json: bool,
    },
    /// Syncs one folder pair now, or all of them.
    ///
    /// Without --wait, returns once the run has started (or as soon as it ended, when that is
    /// quicker), with exit code 1 if it ended badly. With --wait, shows progress until every run
    /// ends; the exit code is non-zero if any did not succeed.
    Sync {
        /// Pair id, id prefix (4+ characters) or name. All pairs when left out.
        pair: Option<String>,
        /// Show progress and wait until the run ends.
        #[arg(long)]
        wait: bool,
    },
    /// Pauses a folder pair (or all of them) until resumed.
    Pause(Target),
    /// Resumes a paused folder pair (or all of them).
    Resume(Target),
    /// Lists, adds, removes and previews folder pairs.
    #[command(subcommand)]
    Pair(PairCmd),
    /// Explains whether a path in a pair is ignored, and by which rule.
    Ignore {
        /// Required: explain the rule that decides.
        #[arg(long)]
        explain: bool,
        /// Pair id, id prefix or name.
        pair: String,
        /// A path inside the pair's folder (relative, or absolute under it).
        path: String,
    },
    /// Google accounts.
    #[command(subcommand)]
    Account(AccountCmd),
    /// Past sync runs, newest first.
    History {
        /// Only this pair.
        pair: Option<String>,
        /// How many runs to show.
        #[arg(long, default_value_t = 20)]
        limit: u32,
        #[arg(long)]
        json: bool,
    },
    /// What needs you: conflicts, failed or stopped pairs, accounts to sign in again.
    Problems {
        #[arg(long)]
        json: bool,
    },
    /// The recycle bin: files Wasl deleted or replaced, kept 30 days.
    #[command(subcommand)]
    Bin(BinCmd),
    /// Starts, stops or checks the wasld daemon.
    #[command(subcommand)]
    Daemon(DaemonCmd),
    /// Starts wasld when you sign in to this computer.
    #[command(subcommand)]
    Service(ServiceCmd),
    /// Your devices: list them, remove one, or reach one at an address.
    #[command(subcommand)]
    Device(DeviceCmd),
    /// Encrypted vaults: make one, unlock or lock it on this computer, or recover the files of a
    /// vault folder downloaded from Drive.
    #[command(subcommand)]
    Vault(VaultCmd),
    /// Read-only views of Drive folders: files show here at full size and download only when
    /// opened. Nothing written in a view is uploaded.
    #[command(subcommand)]
    View(ViewCmd),
    /// What AI agents changed in your synced folders: held on this computer until you accept or
    /// revert it (docs/AGENT_CHANGES.md).
    #[command(subcommand)]
    Agent(AgentCmd),
    /// The op-log: every actor's changesets, read-only (docs/adr/0002-one-op-log.md). Accepting
    /// stays in the Wasl window.
    #[command(subcommand)]
    Oplog(OplogCmd),
    /// Files on demand: see `wasl view pin`.
    Pin {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true, hide = true)]
        args: Vec<String>,
    },
}

#[derive(Args, Debug)]
#[group(required = true, multiple = false)]
pub struct Target {
    /// Pair id, id prefix (4+ characters) or name.
    pub pair: Option<String>,
    /// Every pair.
    #[arg(long)]
    pub all: bool,
}

#[derive(Subcommand, Debug)]
pub enum PairCmd {
    /// Lists the folder pairs.
    List {
        #[arg(long)]
        json: bool,
    },
    /// Pairs a folder on this computer with a Drive folder.
    Add {
        /// The folder on this computer.
        local_folder: String,
        /// The Drive folder, e.g. "My Drive/Photos" (a path without "My Drive/" is taken under it).
        drive_path: String,
        /// The Google account (email). Needed only when more than one is signed in.
        #[arg(long)]
        account: Option<String>,
        /// The pair's name. Defaults to the folder's name.
        #[arg(long)]
        name: Option<String>,
        /// How files move.
        #[arg(long, value_enum, default_value_t = Method::TwoWay)]
        method: Method,
        /// Create the Drive folder when it does not exist yet.
        #[arg(long)]
        create_drive_folder: bool,
        /// Leave out what the folder's .gitignore files leave out of git.
        #[arg(long)]
        gitignore: bool,
        /// Leave out a kind of project's build and dependency folders (repeat for more): node,
        /// python, rust, android-gradle, flutter, docker, editors, os-junk.
        #[arg(long = "preset", value_name = "PRESET")]
        presets: Vec<String>,
        /// Sync hidden files and folders too (names starting with a dot, such as .github).
        #[arg(long)]
        hidden: bool,
    },
    /// Removes a folder pair. Files on both sides are not touched.
    Remove {
        pair: String,
        /// Do not ask for confirmation.
        #[arg(long)]
        yes: bool,
    },
    /// Syncs a pair with your other devices too. With --with, only with the devices named, which
    /// are offered the folder (they accept it there: wasl device offers). Without it, prints a
    /// share code; on the other device run this with --code to sync its folder with this one.
    Share {
        pair: String,
        /// The code shown on the device that shared the folder first.
        #[arg(long)]
        code: Option<String>,
        /// A device to share with and offer the folder to (ID or name); repeat for more. Replaces
        /// the pair's list, and "all my devices" with it.
        #[arg(long = "with", value_name = "DEVICE")]
        with: Vec<String>,
    },
    /// Stops syncing a pair with your devices.
    Unshare { pair: String },
    /// Shows what the next sync would do, without doing it.
    Preview {
        pair: String,
        /// How many changes to list.
        #[arg(long, default_value_t = 20)]
        limit: usize,
        /// After showing the plan, start the sync (a new pair's first sync starts this way).
        #[arg(long)]
        start: bool,
    },
}

#[derive(Subcommand, Debug)]
pub enum OplogCmd {
    /// Changesets, newest first.
    ///
    /// Example: wasl oplog list --actor agent:claude-code --status staged --since 7d
    List {
        /// Only this actor's, e.g. agent:claude-code.
        #[arg(long)]
        actor: Option<String>,
        /// Only those in this state: open, staged, committed, discarded, reverted, partly_reverted.
        #[arg(long)]
        status: Option<String>,
        /// Only those opened since: a date, an RFC 3339 time, or 30m, 24h, 7d ago.
        #[arg(long)]
        since: Option<String>,
        /// Only those in this pair (name or id).
        #[arg(long)]
        pair: Option<String>,
        /// How many to show, the newest (at most 1000).
        #[arg(long, default_value_t = 100)]
        limit: u32,
        /// The read format of ADR-0002, with a version field.
        #[arg(long)]
        json: bool,
    },
    /// One changeset with its operations: <device>:<id>, or <id> for this device.
    Show {
        id: String,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand, Debug)]
pub enum AgentCmd {
    /// Runs an agent in a session: its changes are held until you accept or revert them.
    ///
    /// Example: wasl agent run --as gemini -- gemini
    Run {
        /// The agent's name, e.g. claude-code, gemini, raqeeb.
        #[arg(long = "as", value_name = "NAME")]
        agent: String,
        /// The folder it works in (inside a synced folder). Defaults to the current folder.
        #[arg(long)]
        dir: Option<std::path::PathBuf>,
        /// A note shown with the changeset. Defaults to the command.
        #[arg(long)]
        label: Option<String>,
        /// The agent's command and its arguments, after `--`.
        #[arg(trailing_var_arg = true, allow_hyphen_values = true, required = true)]
        command: Vec<String>,
    },
    /// Starts an agent session in a folder (for scripts and agents that call Wasl themselves).
    Begin {
        #[arg(long = "as", value_name = "NAME")]
        agent: String,
        /// Defaults to the current folder.
        #[arg(long)]
        dir: Option<std::path::PathBuf>,
        #[arg(long)]
        label: Option<String>,
        /// The agent's own session id: beginning again with it continues the open session.
        #[arg(long)]
        session: Option<String>,
    },
    /// Keeps a file's current content before an agent writes it.
    Save {
        path: std::path::PathBuf,
        #[arg(long)]
        session: Option<String>,
    },
    /// Ends a session: what it changed is held for you. Without arguments, the session that began
    /// in the current folder.
    End {
        /// The changeset id (or a prefix of 4+ characters).
        id: Option<String>,
        #[arg(long)]
        session: Option<String>,
        #[arg(long)]
        dir: Option<std::path::PathBuf>,
    },
    /// Changesets waiting for you (and working sessions); with --all, past ones too.
    List {
        #[arg(long)]
        all: bool,
        #[arg(long)]
        json: bool,
    },
    /// What a changeset changed, file by file.
    Show {
        id: String,
        /// Show the changes line by line (text files up to 1 MB).
        #[arg(long)]
        diff: bool,
        #[arg(long)]
        json: bool,
    },
    /// Keeps an agent's changes: they sync now.
    Accept { id: String },
    /// Puts back everything a changeset changed. The agent's versions go to the recycle bin.
    Revert { id: String },
    /// Adds a folder to agent undo's watch list: every agent session snapshots it at its start.
    /// `.waslignore` applies; node_modules/, .venv/, target/ and build/ are ignored until you
    /// remove them with --unignore. A disk's top folder and Wasl's data folder are refused.
    ///
    /// Example: wasl agent watch ~/code/site --unignore build/
    Watch {
        /// Defaults to the current folder.
        dir: Option<std::path::PathBuf>,
        /// A default ignore to stop applying (repeatable). On a watched folder, changes only that.
        #[arg(long, value_name = "PATTERN")]
        unignore: Vec<String>,
    },
    /// Takes a folder off the watch list (by path or watch id). Earlier snapshots stay.
    Unwatch {
        /// Defaults to the current folder.
        dir: Option<String>,
    },
    /// The watched folders and the default ignores each still applies.
    Watches {
        #[arg(long)]
        json: bool,
    },
    /// Prints the hooks to add to an agent's settings (claude-code).
    Hooks { agent: String },
    /// Called by an agent's hooks; reads the hook's JSON on standard input.
    #[command(hide = true)]
    Hook { agent: String, event: String },
}

#[derive(Subcommand, Debug)]
pub enum DeviceCmd {
    /// Lists this computer and the other devices of your group.
    List {
        #[arg(long)]
        json: bool,
    },
    /// Removes a device from your group. It can never sync with your devices again.
    Remove {
        /// The device ID (WASL-…) or name.
        device: String,
        /// Do not ask for confirmation.
        #[arg(long)]
        yes: bool,
    },
    /// Connects to one of your devices at an address (host:port), e.g. over Tailscale.
    Connect { address: String },
    /// Adds a new device: shows a code to enter on it (both must be on the same network).
    Add {
        /// Confirm the four words without asking (compare them on both screens first).
        #[arg(long)]
        yes: bool,
    },
    /// Joins your other device's group with the code it shows.
    Join {
        code: String,
        /// Confirm the four words without asking (compare them on both screens first).
        #[arg(long)]
        yes: bool,
    },
    /// Lists the folders your other devices offer this computer.
    Offers {
        #[arg(long)]
        json: bool,
        /// Also list declined and accepted offers.
        #[arg(long)]
        all: bool,
    },
    /// Accepts a folder another device offers: syncs it with a folder on this computer. Like any
    /// new pair, nothing moves until you have seen its first preview.
    Accept {
        /// The offer: its code (or its first characters) or its folder name.
        offer: String,
        /// The folder on this computer.
        folder: String,
        /// The device offering it (ID or name), when more than one offers it.
        #[arg(long)]
        from: Option<String>,
        /// A name for the new pair (default: the offered name).
        #[arg(long)]
        name: Option<String>,
    },
    /// Hides a folder another device offers this computer.
    Decline {
        offer: String,
        #[arg(long)]
        from: Option<String>,
    },
    /// Keeps a device another of your devices added: it gets the folders shared with all your
    /// devices here.
    Keep {
        /// The device ID (WASL-…) or name.
        device: String,
    },
}

#[derive(Subcommand, Debug)]
pub enum ViewCmd {
    /// Lists the views.
    List {
        #[arg(long)]
        json: bool,
    },
    /// Shows a Drive folder at a new or empty folder on this computer, read-only.
    Add {
        /// The Drive folder, e.g. "My Drive/Photos" (a path without "My Drive/" is taken under it).
        drive_folder: String,
        /// Where the view appears on this computer (created when missing; must be empty).
        local_path: String,
        /// The view's name. Defaults to the Drive folder's name.
        #[arg(long)]
        name: Option<String>,
        /// The Google account (email). Needed only when more than one is signed in.
        #[arg(long)]
        account: Option<String>,
        /// Space for files that are not pinned, e.g. 20G or 500M (default 20G).
        #[arg(long, value_parser = parse_size)]
        cache_limit: Option<u64>,
    },
    /// Removes a view. Pinned files stay as ordinary files; nothing on Drive changes.
    Remove {
        /// The view's id, id prefix (4+ characters) or name.
        view: String,
        /// Download every file first and keep them all as ordinary files.
        #[arg(long)]
        keep_files: bool,
        /// Do not ask for confirmation.
        #[arg(long)]
        yes: bool,
    },
    /// Keeps a file, or every file of a folder, of a view on this computer.
    Pin {
        /// A file or folder inside a view's folder.
        path: String,
    },
    /// Lets a pinned file go from this computer when space is needed (it stays on Drive).
    Unpin {
        /// A file or folder inside a view's folder.
        path: String,
    },
    /// Reads a view's Drive folder again now.
    Refresh {
        /// The view's id, id prefix (4+ characters) or name.
        view: String,
    },
}

/// "20G", "500M", "1.5G", "64K" or plain bytes (binary units: G is GiB).
pub fn parse_size(s: &str) -> Result<u64, String> {
    let t = s.trim();
    let lower = t.to_ascii_lowercase();
    let body = lower
        .strip_suffix("ib")
        .or_else(|| lower.strip_suffix('b'))
        .unwrap_or(&lower);
    let (num, mult) = match body.chars().last() {
        Some('k') => (&body[..body.len() - 1], 1u64 << 10),
        Some('m') => (&body[..body.len() - 1], 1 << 20),
        Some('g') => (&body[..body.len() - 1], 1 << 30),
        Some('t') => (&body[..body.len() - 1], 1 << 40),
        _ => (body, 1),
    };
    let bad = || t!(BadSize, t);
    let n: f64 = num.trim().parse().map_err(|_| bad())?;
    if !n.is_finite() || n < 0.0 || n * mult as f64 > u64::MAX as f64 {
        return Err(bad());
    }
    Ok((n * mult as f64).round() as u64)
}

#[derive(Subcommand, Debug)]
pub enum VaultCmd {
    /// Lists the vaults this computer knows, and whether each is unlocked here.
    List {
        #[arg(long)]
        json: bool,
    },
    /// Makes a new encrypted vault (format v2) in a new or empty Drive folder and unlocks it on
    /// this computer. Asks for the passphrase twice, without showing it.
    Create {
        /// The vault's folder, e.g. "My Drive/Private" (made when missing; its parent must exist).
        drive_folder: String,
        /// The vault's name. Defaults to the folder's name.
        #[arg(long)]
        name: Option<String>,
        /// The Google account (email). Needed only when more than one is signed in.
        #[arg(long)]
        account: Option<String>,
        /// Read the passphrase from the first line of standard input instead of asking.
        #[arg(long)]
        passphrase_stdin: bool,
    },
    /// Unlocks a vault on this computer, so its pairs sync and its views show their files. Name a
    /// vault this computer knows, or a Drive folder that holds one (made on another device).
    Unlock {
        /// The vault's id, id prefix (4+ characters) or name, or its Drive folder.
        vault: String,
        /// The Google account (email), for a Drive folder. Needed only when more than one is
        /// signed in.
        #[arg(long)]
        account: Option<String>,
        /// Read the passphrase from the first line of standard input instead of asking.
        #[arg(long)]
        passphrase_stdin: bool,
    },
    /// Copies a vault of an older format (v1 or 2.0) into a new vault of the current format
    /// (2.1) in a new or empty Drive folder, with the same passphrase. The old vault and its pairs
    /// stay as they are; move them over afterwards (docs/ENCRYPTION.md §11.12). Run it again to
    /// finish an interrupted copy.
    Upgrade {
        /// The vault's id, id prefix (4+ characters) or name. It must be unlocked here.
        vault: String,
        /// The new vault's folder, e.g. "My Drive/Private 2.1" (made when missing; its parent
        /// must exist).
        drive_folder: String,
        /// The Google account (email). Needed only when more than one is signed in.
        #[arg(long)]
        account: Option<String>,
        /// Read the vault's passphrase (the new vault keeps it) from the first line of standard
        /// input instead of asking.
        #[arg(long)]
        passphrase_stdin: bool,
    },
    /// Forgets a vault's keys on this computer: its pairs and views wait until it is unlocked
    /// again. Nothing on Drive changes.
    Lock {
        /// The vault's id, id prefix (4+ characters) or name.
        vault: String,
    },
    /// Decrypts a whole vault folder (format v1 or v2) downloaded from Google Drive, with only
    /// the passphrase. Works without wasld and without a network.
    Decrypt {
        /// The downloaded vault folder (the one holding vault.json).
        folder: std::path::PathBuf,
        /// Where the decrypted files go (created when missing).
        output: std::path::PathBuf,
        /// Read the passphrase from the first line of standard input instead of asking.
        #[arg(long)]
        passphrase_stdin: bool,
        /// Replace files that already exist in the output folder.
        #[arg(long)]
        overwrite: bool,
    },
}

#[derive(ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum Method {
    TwoWay,
    UploadOnly,
    /// Uploads, then removes each uploaded file from this computer (to the recycle bin).
    UploadThenDelete,
    DownloadOnly,
    /// Downloads, then moves each downloaded file to Drive's bin.
    DownloadThenDelete,
    MirrorUp,
    MirrorDown,
}

impl Method {
    pub fn mode(self) -> wasl_core::SyncMode {
        use wasl_core::SyncMode;
        match self {
            Method::TwoWay => SyncMode::TwoWay,
            Method::UploadOnly => SyncMode::UploadOnly,
            Method::UploadThenDelete => SyncMode::UploadThenDelete,
            Method::DownloadOnly => SyncMode::DownloadOnly,
            Method::DownloadThenDelete => SyncMode::DownloadThenDelete,
            Method::MirrorUp => SyncMode::UploadMirror,
            Method::MirrorDown => SyncMode::DownloadMirror,
        }
    }
}

#[derive(Subcommand, Debug)]
pub enum AccountCmd {
    /// Lists the signed-in Google accounts.
    List {
        #[arg(long)]
        json: bool,
    },
    /// Signs in to a Google account in the browser.
    Add,
    /// Signs out of a Google account.
    Remove { email: String },
}

#[derive(Subcommand, Debug)]
pub enum BinCmd {
    /// Lists the recycle bin.
    List {
        /// Only this pair.
        pair: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Puts files back where they were.
    Restore {
        #[arg(required = true)]
        ids: Vec<String>,
    },
    /// Deletes everything in the recycle bin for good.
    Empty {
        #[arg(long)]
        yes: bool,
    },
}

#[derive(Subcommand, Debug)]
pub enum DaemonCmd {
    /// Starts wasld in the background.
    Start,
    /// Stops wasld (running syncs are cancelled cleanly).
    Stop,
    /// Whether wasld is running.
    Status,
}

#[derive(Subcommand, Debug)]
pub enum ServiceCmd {
    /// Start wasld at every sign-in from now on.
    Install,
    /// Stop starting wasld at sign-in.
    Uninstall,
    /// Whether wasld starts at sign-in.
    Status,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    i18n::init_from_env();
    let rt = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("{}", t!(ErrorLine, e));
            return ExitCode::from(cmd::GENERAL);
        }
    };
    match rt.block_on(cmd::run(cli.cmd)) {
        Ok(code) => ExitCode::from(code),
        Err(fail) => {
            out::finish_progress_line();
            eprintln!("{}", t!(ErrorLine, fail.message));
            ExitCode::from(fail.code)
        }
    }
}
