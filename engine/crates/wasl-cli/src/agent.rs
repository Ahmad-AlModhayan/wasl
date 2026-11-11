//! `wasl agent`: what AI agents changed in synced folders, held until accepted or reverted
//! (docs/AGENT_CHANGES.md).

use crate::AgentCmd;
use crate::cmd::{Fail, GENERAL, NOT_FOUND, OK, R, USAGE, connect, print_json};
use crate::out::{self, Tone, paint};
use std::io::Read;
use std::path::{Path, PathBuf};
use wasl_cli::{i18n, t};
use wasl_ipc::Client;
use wasl_store::{Changeset, ChangesetStatus, EntryKind};

pub async fn run(cmd: AgentCmd) -> R<u8> {
    match cmd {
        AgentCmd::Run {
            agent,
            dir,
            label,
            command,
        } => agent_run(&agent, dir, label, &command).await,
        AgentCmd::Begin {
            agent,
            dir,
            label,
            session,
        } => {
            let c = connect().await?;
            let cs = c
                .agent_begin(&folder(dir)?, &agent, label.as_deref(), session.as_deref())
                .await?;
            println!("{}", t!(AgentBegun, cs.actor, scope_phrase(&cs), cs.id));
            Ok(OK)
        }
        AgentCmd::Save { path, session } => {
            let c = connect().await?;
            c.agent_save(&absolute(&path)?, session.as_deref()).await?;
            Ok(OK)
        }
        AgentCmd::End { id, session, dir } => {
            let c = connect().await?;
            let id = match id {
                Some(r) => Some(find(&c, &r).await?.id),
                None => None,
            };
            let dir = match (&id, &session, dir) {
                (None, None, d) => Some(folder(d)?),
                (_, _, d) => d.map(|d| folder(Some(d))).transpose()?,
            };
            match c
                .agent_end(id.as_deref(), session.as_deref(), dir.as_deref())
                .await?
            {
                Some(cs) => report_end(&c, &cs).await,
                None => println!("{}", t!(AgentNothing)),
            }
            Ok(OK)
        }
        AgentCmd::List { all, json } => list(all, json).await,
        AgentCmd::Show { id, diff, json } => show(&id, diff, json).await,
        AgentCmd::Accept { id } => {
            // Only the person decides: an agent must not accept its own work.
            if let Some(var) = inside_agent() {
                return Err(Fail::new(USAGE, t!(AgentAcceptInsideAgent, var)));
            }
            let c = connect().await?;
            let cs = find(&c, &id).await?;
            // The daemon accepts only once the person confirms in the Wasl app's dialog, which it
            // opens now: a terminal answer could come from any program (docs/AGENT_CHANGES.md
            // §5). The daemon checks the marker too (it is None here): the check above is for a
            // clear message, the daemon's for clients that skip it.
            let asked = c.agent_accept_as(&cs.id, Some("cli"), inside_agent()).await?;
            if let Some(secs) = asked.retry_in_secs {
                // Declined or unanswered a moment ago: no new dialog until the wait is over.
                eprintln!(
                    "{}",
                    paint(&t!(AgentTryAgainIn, secs.div_ceil(60).max(1)), Tone::Busy)
                );
                return Ok(GENERAL);
            }
            println!("{}", t!(AgentConfirmInApp, asked.code));
            let deadline =
                std::time::Instant::now() + std::time::Duration::from_secs(asked.expires_in_secs + 5);
            loop {
                let status = c.agent_changeset(&cs.id).await?.changeset.status;
                if status == ChangesetStatus::Accepted {
                    println!("{}", paint(&t!(AgentAccepted), Tone::Good));
                    return Ok(OK);
                }
                if status != ChangesetStatus::Held || std::time::Instant::now() >= deadline {
                    eprintln!("{}", paint(&t!(AgentNotConfirmed), Tone::Busy));
                    return Ok(GENERAL);
                }
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            }
        }
        AgentCmd::Revert { id } => {
            let c = connect().await?;
            let cs = find(&c, &id).await?;
            let r = c.agent_revert(&cs.id).await?;
            println!("{}", paint(&t!(AgentReverted, r.reverted.len()), Tone::Good));
            for s in &r.skipped {
                println!("{}", paint(&t!(AgentSkipped, s), Tone::Busy));
            }
            for (p, why) in &r.failed {
                eprintln!("{}", paint(&t!(AgentRevertFailed, p, why), Tone::Bad));
            }
            Ok(if r.failed.is_empty() { OK } else { GENERAL })
        }
        AgentCmd::Watch { dir, unignore } => watch(dir, &unignore).await,
        AgentCmd::Unwatch { dir } => {
            let c = connect().await?;
            let target = match dir {
                Some(d) => d,
                None => folder(None)?,
            };
            let w = c.agent_unwatch(&target).await?;
            println!("{}", t!(AgentUnwatched, w.root));
            Ok(OK)
        }
        AgentCmd::Watches { json } => {
            let c = connect().await?;
            let watches = c.agent_watches().await?;
            if json {
                return print_json(&watches);
            }
            if watches.is_empty() {
                println!("{}", t!(AgentNoWatches));
            }
            for w in &watches {
                println!("{}  {}", w.root, paint(&w.id, Tone::Quiet));
                println!("  {}", ignores_line(&w.ignores));
            }
            Ok(OK)
        }
        AgentCmd::Hooks { agent } => {
            hooks(&agent)?;
            Ok(OK)
        }
        AgentCmd::Hook { agent, event } => Ok(hook(&agent, &event).await),
    }
}

fn ignores_line(ignores: &[String]) -> String {
    if ignores.is_empty() {
        t!(AgentWatchNoDefaults)
    } else {
        t!(AgentWatchDefaults, ignores.join(", "))
    }
}

/// `wasl agent watch`: adds the folder, or on a watched one changes only its default ignores.
async fn watch(dir: Option<PathBuf>, unignore: &[String]) -> R<u8> {
    let c = connect().await?;
    let root = folder(dir)?;
    let known = c.agent_watches().await?;
    let canonical = std::fs::canonicalize(&root)
        .map(|p| p.to_string_lossy().into_owned())
        .ok();
    let existing = known
        .into_iter()
        .find(|w| w.root == root || canonical.as_deref() == Some(w.root.as_str()));
    let w = match existing {
        Some(w) if !unignore.is_empty() => w,
        Some(w) => return Err(Fail::new(USAGE, t!(AgentWatchedAlready, w.root))),
        None => {
            let w = c.agent_watch(&root).await?;
            println!("{}", paint(&t!(AgentWatching, w.root), Tone::Good));
            w
        }
    };
    let mut ignores = w.ignores.clone();
    if !unignore.is_empty() {
        let norm = |p: &str| p.trim_end_matches('/').to_owned();
        if let Some(bad) = unignore
            .iter()
            .find(|u| !ignores.iter().any(|i| norm(i) == norm(u)))
        {
            return Err(Fail::new(USAGE, t!(AgentNotADefault, bad, w.ignores.join(", "))));
        }
        ignores.retain(|i| !unignore.iter().any(|u| norm(i) == norm(u)));
        c.agent_watch_ignores(&w.id, ignores.clone()).await?;
    }
    println!("{}", ignores_line(&ignores));
    Ok(OK)
}

/// Set in the environment of a process an agent runs: `agent run` sets the first, Claude Code and
/// Gemini CLI set the others in their shells. Not a security boundary (any program of this user can
/// reach wasld), but it stops an agent from accepting its own changes by ordinary means.
const AGENT_MARKERS: [&str; 3] = ["WASL_AGENT_SESSION", "CLAUDECODE", "GEMINI_CLI"];

fn inside_agent() -> Option<&'static str> {
    AGENT_MARKERS
        .into_iter()
        .find(|v| std::env::var_os(v).is_some_and(|x| !x.is_empty()))
}

/// A full path, without Windows' `\\?\` prefix (what pairs record).
fn absolute(p: &Path) -> R<String> {
    let a = std::path::absolute(p).map_err(|e| Fail::new(USAGE, e.to_string()))?;
    Ok(a.to_string_lossy().into_owned())
}

/// The folder a session works in: the one given, else the current one.
fn folder(dir: Option<PathBuf>) -> R<String> {
    match dir {
        Some(d) => absolute(&d),
        None => absolute(&std::env::current_dir().map_err(|e| Fail::new(GENERAL, e.to_string()))?),
    }
}

fn scope_label(cs: &Changeset) -> String {
    if cs.scope.is_empty() {
        "/".into()
    } else {
        cs.scope.clone()
    }
}

/// The scope in a sentence: "/" alone read as a folder name in "working in /" (E2E 2026-10).
fn scope_phrase(cs: &Changeset) -> String {
    if cs.scope.is_empty() {
        t!(AgentWholePair)
    } else {
        cs.scope.clone()
    }
}

/// A changeset by id or a unique id prefix of 4+ characters.
async fn find(c: &Client, reference: &str) -> R<Changeset> {
    let all = c.agent_changesets(None, true).await?;
    if let Some(cs) = all.iter().find(|cs| cs.id == reference) {
        return Ok(cs.clone());
    }
    let hits: Vec<&Changeset> = if reference.chars().count() >= 4 {
        all.iter().filter(|cs| cs.id.starts_with(reference)).collect()
    } else {
        Vec::new()
    };
    match hits.len() {
        1 => Ok(hits[0].clone()),
        0 => Err(Fail::new(NOT_FOUND, t!(AgentNoSuch, reference))),
        _ => Err(Fail::new(USAGE, t!(AgentAmbiguous, reference))),
    }
}

fn status_label(s: ChangesetStatus) -> String {
    match s {
        ChangesetStatus::Open => t!(AgentStOpen),
        ChangesetStatus::Held => paint(&t!(AgentStHeld), Tone::Busy),
        ChangesetStatus::Accepted => t!(AgentStAccepted),
        ChangesetStatus::Reverted => t!(AgentStReverted),
        ChangesetStatus::PartlyReverted => t!(AgentStPartly),
    }
}

fn kind_label(k: EntryKind) -> String {
    match k {
        EntryKind::Added => paint(&t!(AgentKAdded), Tone::Good),
        EntryKind::Changed => paint(&t!(AgentKChanged), Tone::Busy),
        EntryKind::Deleted => paint(&t!(AgentKDeleted), Tone::Bad),
    }
}

async fn report_end(c: &Client, cs: &Changeset) {
    let n = c
        .agent_changeset(&cs.id)
        .await
        .map(|d| d.entries.len())
        .unwrap_or(0);
    println!(
        "{}",
        paint(
            &t!(AgentHeld, cs.actor, n, scope_phrase(cs), cs.id.clone()),
            Tone::Busy
        )
    );
}

async fn list(all: bool, json: bool) -> R<u8> {
    let c = connect().await?;
    let sets = c.agent_changesets(None, all).await?;
    if json {
        return print_json(&sets);
    }
    if sets.is_empty() {
        println!("{}", t!(AgentNone));
        return Ok(OK);
    }
    let pairs = c.pairs().await?;
    let mut rows = vec![vec![
        t!(ColId),
        t!(ColAgent),
        t!(ColPair),
        t!(ColPath),
        t!(ColStarted),
        t!(ColState),
        t!(ColChanges),
    ]];
    for cs in &sets {
        let n = c
            .agent_changeset(&cs.id)
            .await
            .map(|d| d.entries.len().to_string())
            .unwrap_or_default();
        rows.push(vec![
            cs.id.clone(),
            cs.actor.trim_start_matches("agent:").to_owned(),
            pairs
                .iter()
                .find(|p| Some(&p.id) == cs.pair_id.as_ref())
                .map(|p| p.name.clone())
                .or_else(|| cs.pair_id.clone())
                .unwrap_or_default(),
            scope_label(cs),
            out::when(cs.started_at),
            status_label(cs.status),
            n,
        ]);
    }
    print!("{}", out::table(&rows));
    Ok(OK)
}

async fn show(id: &str, diff: bool, json: bool) -> R<u8> {
    let c = connect().await?;
    let cs = find(&c, id).await?;
    let d = c.agent_changeset(&cs.id).await?;
    if json {
        return print_json(&d);
    }
    let label = d.changeset.label.as_deref().map(i18n::clean).unwrap_or_default();
    println!(
        "{}  {}  {}  {}  {}",
        cs.id.clone(),
        cs.actor,
        i18n::clean(&d.pair_name),
        scope_label(&cs),
        status_label(cs.status)
    );
    if !label.is_empty() {
        println!("  {label}");
    }
    let mut rows = vec![vec![t!(ColChange), t!(ColPath), t!(ColSize)]];
    for e in &d.entries {
        let size = match (e.before_size, e.after_size) {
            (Some(a), Some(b)) => format!("{} → {}", out::size(a), out::size(b)),
            (None, Some(b)) => out::size(b),
            (Some(a), None) => out::size(a),
            _ => String::new(),
        };
        let path = if e.is_dir {
            format!("{}/", e.rel)
        } else {
            e.rel.clone()
        };
        rows.push(vec![kind_label(e.kind), path, size]);
    }
    print!("{}", out::table(&rows));
    if diff {
        for e in d.entries.iter().filter(|e| !e.is_dir) {
            let text = c.agent_diff(&cs.id, &e.rel).await?;
            for line in text.lines() {
                let tone = if line.starts_with('+') && !line.starts_with("+++") {
                    Some(Tone::Good)
                } else if line.starts_with('-') && !line.starts_with("---") {
                    Some(Tone::Bad)
                } else {
                    None
                };
                let line = i18n::clean(line);
                match tone {
                    Some(t) => println!("{}", paint(&line, t)),
                    None => println!("{line}"),
                }
            }
        }
    }
    Ok(OK)
}

/// Runs an agent in a session: begins in the folder, runs the command, ends when it exits.
async fn agent_run(agent: &str, dir: Option<PathBuf>, label: Option<String>, command: &[String]) -> R<u8> {
    let Some((program, args)) = command.split_first() else {
        return Err(Fail::new(USAGE, t!(AgentNoCommand)));
    };
    let dir = folder(dir)?;
    let c = connect().await?;
    let label = label.unwrap_or_else(|| command.join(" "));
    let cs = c.agent_begin(&dir, agent, Some(&label), None).await?;
    eprintln!("{}", t!(AgentBegun, cs.actor, scope_phrase(&cs), cs.id.clone()));
    let status = std::process::Command::new(program)
        .args(args)
        .env("WASL_AGENT_SESSION", &cs.id)
        .current_dir(&dir)
        .status();
    // The session ends however the agent did.
    let ended = c.agent_end(Some(&cs.id), None, None).await?;
    match ended {
        Some(cs) => report_end(&c, &cs).await,
        None => eprintln!("{}", t!(AgentNothing)),
    }
    match status {
        Ok(s) => Ok(s.code().and_then(|c| u8::try_from(c).ok()).unwrap_or(GENERAL)),
        Err(e) => Err(Fail::new(GENERAL, t!(AgentRunFailed, program, e))),
    }
}

fn hook_command(agent: &str, event: &str) -> String {
    let exe = std::env::current_exe()
        .ok()
        .map(|p| p.to_string_lossy().replace('\\', "/"))
        .unwrap_or_else(|| "wasl".into());
    format!("\"{exe}\" agent hook {agent} {event}")
}

/// Prints the hooks block for the agent's settings.
fn hooks(agent: &str) -> R<()> {
    if agent != "claude-code" {
        return Err(Fail::new(USAGE, t!(AgentHooksUnknown, agent)));
    }
    let cmd = |event: &str| serde_json::json!([{ "hooks": [{ "type": "command", "command": hook_command(agent, event) }] }]);
    let block = serde_json::json!({
        "hooks": {
            "UserPromptSubmit": cmd("prompt"),
            "PreToolUse": [{
                "matcher": "Edit|Write|MultiEdit|NotebookEdit",
                "hooks": [{ "type": "command", "command": hook_command(agent, "pre-tool") }]
            }],
            "Stop": cmd("stop"),
            "SessionEnd": cmd("stop"),
        }
    });
    println!(
        "{}",
        serde_json::to_string_pretty(&block).map_err(|e| Fail::new(GENERAL, e.to_string()))?
    );
    eprintln!("{}", t!(AgentHooksWhere));
    Ok(())
}

/// Called by Claude Code with its hook JSON on stdin. Never blocks the agent: always exit 0,
/// nothing on stdout (a prompt hook's stdout would reach the model).
async fn hook(agent: &str, event: &str) -> u8 {
    if let Err(e) = hook_inner(agent, event).await {
        eprintln!("{}", t!(AgentHookFailed, e.message));
    }
    OK
}

async fn hook_inner(agent: &str, event: &str) -> R<()> {
    let mut input = String::new();
    std::io::stdin()
        .read_to_string(&mut input)
        .map_err(|e| Fail::new(GENERAL, e.to_string()))?;
    let v: serde_json::Value =
        serde_json::from_str(&input).map_err(|e| Fail::new(USAGE, t!(AgentHookBadInput, e)))?;
    let field = |k: &str| v.get(k).and_then(|x| x.as_str()).map(str::to_owned);
    let session = field("session_id");
    let c = match connect().await {
        Ok(c) => c,
        // No wasld: the agent goes on unwatched.
        Err(f) => return Err(f),
    };
    match event {
        "prompt" => {
            let Some(cwd) = field("cwd") else {
                return Ok(());
            };
            let label: Option<String> = field("prompt").map(|p| {
                let one_line = p.split_whitespace().collect::<Vec<_>>().join(" ");
                one_line.chars().take(120).collect()
            });
            match c
                .agent_begin(&cwd, agent, label.as_deref(), session.as_deref())
                .await
            {
                // A folder Wasl does not sync is not watched.
                Err(wasl_ipc::ClientError::Remote(r)) if r.kind == wasl_ipc::ErrorKind::NotFound => Ok(()),
                other => other.map(|_| ()).map_err(Into::into),
            }
        }
        "pre-tool" => {
            let input = v.get("tool_input");
            let path = ["file_path", "notebook_path"]
                .iter()
                .find_map(|k| input.and_then(|i| i.get(*k)).and_then(|x| x.as_str()));
            let Some(path) = path else {
                return Ok(());
            };
            let path = match (Path::new(path).is_absolute(), field("cwd")) {
                (false, Some(cwd)) => Path::new(&cwd).join(path).to_string_lossy().into_owned(),
                _ => path.to_owned(),
            };
            c.agent_save(&path, session.as_deref()).await?;
            Ok(())
        }
        "stop" => {
            if session.is_some() {
                c.agent_end(None, session.as_deref(), None).await?;
            }
            Ok(())
        }
        other => Err(Fail::new(USAGE, t!(AgentHookUnknownEvent, other))),
    }
}
