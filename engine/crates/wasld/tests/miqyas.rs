//! The Miqyas harness end to end (feature `miqyas`, docs/testing/MIQYAS.md): two test daemons on
//! one machine, paired without a person, sync 100 files over loopback, and leave nothing behind.

use std::path::Path;
use std::process::Command;

const HARNESS: &str = env!("CARGO_BIN_EXE_wasl-miqyas");

fn harness(args: &[&str]) -> serde_json::Value {
    let out = Command::new(HARNESS).args(args).output().unwrap();
    assert!(
        out.status.success(),
        "wasl-miqyas {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap()
}

#[cfg(unix)]
fn alive(pid: u64) -> bool {
    Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

#[cfg(windows)]
fn alive(pid: u64) -> bool {
    let out = Command::new("tasklist")
        .args(["/FI", &format!("PID eq {pid}"), "/NH"])
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout).contains(&pid.to_string())
}

/// Stops the daemons when the test fails before its own `down`.
struct Down(String);

impl Drop for Down {
    fn drop(&mut self) {
        if Path::new(&self.0).exists() {
            let _ = Command::new(HARNESS).args(["down", "--root", &self.0]).output();
        }
    }
}

#[test]
fn a_hundred_files_made_on_a_reach_b_and_teardown_leaves_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("run");
    let root_s = root.to_str().unwrap();
    let _down = Down(root_s.to_owned());
    let state = harness(&["up", "--root", root_s]);
    let devices = state["devices"].as_array().unwrap();
    let folder = |i: usize| Path::new(devices[i]["folder"].as_str().unwrap()).to_path_buf();
    let pids: Vec<u64> = devices.iter().map(|d| d["pid"].as_u64().unwrap()).collect();
    assert!(pids.iter().all(|p| alive(*p)));
    assert_ne!(
        devices[0]["device_id"], devices[1]["device_id"],
        "two identities, not the keychain's one"
    );

    let (a, b) = (folder(0), folder(1));
    std::fs::create_dir_all(a.join("نصوص")).unwrap();
    for i in 0..100 {
        let dir = if i % 10 == 0 {
            a.join("نصوص")
        } else {
            a.clone()
        };
        std::fs::write(
            dir.join(format!("file-{i:03}.txt")),
            format!("file {i} ").repeat(i + 1),
        )
        .unwrap();
    }
    let waited = harness(&["wait", "--root", root_s, "--timeout", "120"]);
    assert_eq!(true, waited["in_sync"]);
    assert_eq!(100, waited["files"]);
    for i in 0..100 {
        let rel = if i % 10 == 0 {
            format!("نصوص/file-{i:03}.txt")
        } else {
            format!("file-{i:03}.txt")
        };
        assert_eq!(
            std::fs::read(a.join(&rel)).unwrap(),
            std::fs::read(b.join(&rel)).unwrap(),
            "{rel}"
        );
    }

    harness(&["down", "--root", root_s]);
    assert!(!root.exists(), "the root is removed");
    assert!(pids.iter().all(|p| !alive(*p)), "both daemons stopped");
}

#[test]
fn down_removes_only_a_root_up_made() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("keep.txt"), b"mine").unwrap();
    let out = Command::new(HARNESS)
        .args(["down", "--root", tmp.path().to_str().unwrap()])
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(tmp.path().join("keep.txt").exists());
}
