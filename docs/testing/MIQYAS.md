# Wasl under Miqyas

How Miqyas (Kahf's test and benchmark runner) starts, drives and tears down Wasl on one machine, with
no Google account and no person. Issue #49.

## What there is

- **A test build.** `wasld` built with the Cargo feature `miqyas`, and a harness binary,
  `wasl-miqyas`, that exists only in that build. The `wasl` CLI is the released one, unchanged.
- **Two devices on one machine.** `wasl-miqyas up` starts device A and device B, each a test
  `wasld` with its own data folder and socket, pairs them, and syncs a devices-only pair at reach
  `lan` over loopback.
- **A test**: `cargo test -p wasld --features miqyas --test miqyas`. 100 files made on A appear on
  B with the same bytes; teardown leaves no process and no folder.

## Build

```sh
tools/miqyas/build.sh            # refuses uncommitted changes; --allow-dirty to build anyway
```

It builds from the current commit, release profile, into `engine/target/miqyas/release` (set
`WASL_MIQYAS_TARGET` to move it), apart from `engine/target/release` so a test build is never
packaged. It prints the commit and the versions; `wasld --version` says `(miqyas test build)`.
To pin a release, check out its tag first.

## Run

```sh
bin=engine/target/miqyas/release
root=$(mktemp -d "${TMPDIR:-/tmp}/wasl-miqyas.XXXXXX")
$bin/wasl-miqyas up --root "$root"          # JSON: the devices, their folders, pids, ports
# ... make files in "$root/a/folder" ...
$bin/wasl-miqyas wait --root "$root"        # JSON: {"in_sync":true,"elapsed_ms":…,"files":…,"bytes":…}
WASL_HOME="$root/b/home" $bin/wasl status --json
$bin/wasl-miqyas down --root "$root"        # stops both daemons, removes the root
```

`tools/miqyas/sync-100.sh [BIN_DIR]` does exactly this with 100 files.

Layout under the root: `a/home` and `b/home` (each daemon's data folder and socket),
`a/folder` and `b/folder` (the synced folders), `a/wasld.log` and `b/wasld.log`, and
`miqyas.json` (what `up` printed). Keep the root short: the socket path must stay under 104
bytes on macOS. The default temp folder is fine.

### Commands

| Command | Does |
|---|---|
| `up [--root DIR] [--wasld PATH]` | Starts A and B, pairs them, makes the pair on A, offers it to B, accepts it on B, starts both from their preview, waits for both first syncs. `DIR` must be missing or empty (default: a new folder in the temp folder). `wasld` defaults to the one next to `wasl-miqyas`. |
| `wait [--root DIR] [--timeout SECS]` | Until both folders hold the same files with the same bytes and neither device is syncing; exit 1 after the timeout (default 120). |
| `down [--root DIR]` | Asks both daemons to stop, kills one that doesn't within 10 s, removes `DIR`. Refuses a folder `up` didn't make. |

`--root` can also come from `WASL_MIQYAS_ROOT`.

### Environment

| Variable | Read by | Meaning |
|---|---|---|
| `WASL_HOME` | `wasld`, `wasl` | The device's data folder (`$root/a/home`); `wasld --home DIR` does the same. The test `wasld` refuses to start without one. |
| `WASL_MIQYAS_ROOT` | `wasl-miqyas` | The root, when `--root` isn't given. |
| `WASL_MIQYAS_LISTEN` | test `wasld` | The device listen address. Default `127.0.0.1:0` (any free port). `up` leaves the default. |
| `WASL_MIQYAS_NAME` | test `wasld` | The device name (`up` sets "Miqyas A" and "Miqyas B"). |
| `WASL_MIQYAS_DRIVE_URL` | test `wasld` | A fake Drive's base URL for later scenarios. Default `http://127.0.0.1:9`, where nothing answers. |
| `WASL_MIQYAS_TARGET` | `build.sh` | The Cargo target folder for the test build. |

## What Miqyas can measure

- **Time to in-sync**: `elapsed_ms` from `wait`, counted from when `wait` starts. Start it right
  after the last file is written. It includes Wasl's own delays: a change waits until its folder
  is quiet for 5 s, and the scheduler looks every 2 s (`wasld::schedule`), so a small change takes
  about 5 to 7 s. That is what a user gets; compare Syncthing with its own watcher delay as shipped
  (10 s by default) or set both alike.
- **Files and bytes in the folder**: `files` and `bytes` from `wait`.
- **State**: `WASL_HOME=… wasl status --json` per device (pairs, `running`, `last_status`) and
  `wasl device list --json` (connected, path `lan`).
- **Processes**: the pids in `up`'s JSON, for CPU and memory sampling.

Not there yet: bytes sent over the network, and per-run history for device runs (`wasl history`
lists Drive runs only). Miqyas can count loopback traffic on the two ports in `up`'s JSON
(`listen`) until Wasl reports it.

## It never touches the real Wasl

The test `wasld`:

- refuses to start without `--home` or `WASL_HOME`, and refuses a folder that is, holds or sits
  inside this user's real Wasl folders: `~/Library/Application Support/Wasl` on macOS,
  `%LOCALAPPDATA%\Wasl` on Windows, `~/.local/share/wasl` and `$XDG_RUNTIME_DIR/wasl` on Linux
  (`Paths::user_default` in `wasl-ipc`);
- keeps its device key and tokens in `miqyas-secrets.json` in its own folder, never in the OS
  keychain, so it never reads or replaces the real device's key;
- mounts no views, so on macOS it never takes the File Provider socket in Wasl.app's group
  container;
- listens on loopback only, with LAN discovery off and no STUN servers, and points Drive and
  sign-in at a dead loopback port.

Never point `WASL_HOME` at the real folder, and never run `wasl-miqyas down` on a folder you
care about: it removes only a root that has `up`'s marker file, but anything inside goes with it.

## How pairing without a person is locked out of release

Pairing is the production flow: A opens a code, B joins with it, both prove the secret, both
show four words. A person checks the words match on both screens and confirms on both;
`wasl-miqyas` reads the words from both daemons, confirms both only when they match, and
cancels otherwise. Nothing in `wasld` or `wasl` changed for it.

The harness exists only in a build with the `miqyas` feature: `wasl-miqyas` is a binary with
`required-features = ["miqyas"]`, and no release or packaging build turns the feature on. A
`wasld` built with it says so in `--version`, serves only a test folder and never the user's
keychain, so it can't stand in for an installed Wasl.
