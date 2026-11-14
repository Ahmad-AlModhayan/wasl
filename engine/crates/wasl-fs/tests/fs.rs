//! Path safety, the plain file system back end, hashing and the scanner.

use proptest::prelude::*;
use std::fs;
use std::io::Read;
use std::path::Path;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use wasl_fs::{FsEntry, LocalFs, ReadablePath, StdFs, check_rel, check_rel_on, hash_reader, scan};
use wasl_ignore::{FileKind, IgnoreRules, Preset};

fn setup() -> (tempfile::TempDir, StdFs) {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir_all(dir.path().join("pair")).unwrap();
    let fs = StdFs::new(dir.path().join("pair"), dir.path().join("bin"));
    (dir, fs)
}

fn write(root: &Path, rel: &str, content: &str) {
    let p = root.join(rel);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, content).unwrap();
}

fn read(fs: &StdFs, rel: &str) -> String {
    let mut s = String::new();
    fs.open_read(rel).unwrap().read_to_string(&mut s).unwrap();
    s
}

// ---------------------------------------------------------------- path safety

#[test]
fn unsafe_paths_are_refused() {
    for bad in [
        "..",
        "../x",
        "a/../../x",
        "a/./b",
        "/etc/passwd",
        "a//b",
        "a/",
        "a\\..\\b",
        "C:/Windows",
        "c:x",
        "a\0b",
    ] {
        assert!(check_rel(bad).is_err(), "{bad:?} was accepted");
    }
    for ok in ["", "a", "a/b.txt", "a..b/c", ".hidden/x", "名前/ملف.txt", "a：b"] {
        assert!(check_rel(ok).is_ok(), "{ok:?} was refused");
    }
    // "..." is a plain name on Unix; Windows strips trailing dots, so it is refused there.
    assert!(check_rel_on("...", false).is_ok());
    assert!(check_rel_on("...", true).is_err());
}

#[test]
fn windows_rules_refuse_device_names_streams_and_trailing_dots() {
    for bad in [
        "CON",
        "con.txt",
        "a/NUL",
        "Aux.tar.gz",
        "com1",
        "LPT9.log",
        "COM\u{b9}",
        "prn ",
        "a/b.",
        "a/b ",
        "file:stream",
        "dir:x/a",
    ] {
        assert!(
            check_rel_on(bad, true).is_err(),
            "{bad:?} was accepted on Windows"
        );
    }
    for elsewhere in ["CON", "con.txt", "a/NUL", "com1", "file:stream"] {
        assert!(
            check_rel_on(elsewhere, false).is_ok(),
            "{elsewhere:?} is a fine name elsewhere"
        );
    }
    for short in ["PROGRA~1", "REPORT~1.DOC", "a/MYDOC~12", "x~9.txt"] {
        assert!(
            check_rel_on(short, true).is_err(),
            "{short:?} could alias another file on Windows"
        );
        assert!(check_rel_on(short, false).is_ok());
    }
    for ok in [
        "CONSOLE",
        "con-notes.txt",
        "COM10",
        "LPT",
        "nul2",
        "a.b/c",
        "x/.hidden",
        "~1",
        "notes~v2.txt",
        "LONGERNAME~1.txt",
        "a~1.html5",
    ] {
        assert!(check_rel_on(ok, true).is_ok(), "{ok:?} was refused on Windows");
    }
}

#[test]
fn every_operation_checks_its_path() {
    let (_d, fs) = setup();
    assert!(fs.list("../").is_err());
    assert!(fs.stat("../x").is_err());
    assert!(fs.open_read("../x").is_err());
    assert!(fs.readable_path("/etc/passwd").is_err());
    assert!(fs.staging_path("a/../../x").is_err());
    assert!(fs.mkdir("..").is_err());
    assert!(fs.remove_dir_if_empty("..").is_err());
    assert!(fs.remove_file("../x").is_err());
    assert!(fs.remove_file("").is_err());
    assert!(fs.rename("a", "../b").is_err());
    assert!(fs.move_to_bin("../x").is_err());
    assert!(
        fs.restore_from_bin("../x", "a").is_err(),
        "bin locations are single names"
    );
    assert!(fs.purge_from_bin("../../pair/a").is_err());
}

proptest! {
    /// Whatever check_rel accepts stays inside the root when joined.
    #[test]
    fn accepted_paths_never_leave_the_root(parts in proptest::collection::vec("[a-z.]{1,3}|\\.\\.|\\.", 1..6)) {
        let rel = parts.join("/");
        if check_rel(&rel).is_ok() {
            let root = Path::new("/r/pair");
            let joined = root.join(&rel);
            let mut depth = 0i32;
            for c in Path::new(&rel).components() {
                match c {
                    std::path::Component::Normal(_) => depth += 1,
                    _ => prop_assert!(false, "{rel:?} has a non-normal component"),
                }
            }
            prop_assert!(depth > 0 && joined.starts_with(root));
        }
    }
}

// ---------------------------------------------------------------- StdFs

#[test]
fn list_skips_part_files_and_reports_sizes() {
    let (d, fs) = setup();
    let root = d.path().join("pair");
    write(&root, "a.txt", "hello");
    write(&root, "b.txt.wasl-part", "partial");
    write(&root, "c.drivesync-part", "partial");
    fs::create_dir(root.join("sub")).unwrap();
    let names: Vec<(String, bool, u64)> = fs
        .list("")
        .unwrap()
        .into_iter()
        .map(|e| (e.name, e.is_dir, e.size))
        .collect();
    assert_eq!(vec![("a.txt".into(), false, 5), ("sub".into(), true, 0)], names);
}

#[cfg(unix)]
#[test]
fn symlinks_are_not_followed() {
    let (d, fs) = setup();
    let root = d.path().join("pair");
    let outside = d.path().join("outside");
    fs::create_dir_all(&outside).unwrap();
    fs::write(outside.join("secret.txt"), "s").unwrap();
    std::os::unix::fs::symlink(&outside, root.join("escape")).unwrap();
    std::os::unix::fs::symlink(outside.join("secret.txt"), root.join("link.txt")).unwrap();
    assert!(fs.list("").unwrap().is_empty());
    assert_eq!(None, fs.stat("escape").unwrap());
    assert_eq!(None, fs.stat("escape/secret.txt").unwrap());
    // No operation goes through a link, whether it was there at scan time or appeared later.
    assert!(fs.open_read("link.txt").is_err());
    assert!(fs.open_read("escape/secret.txt").is_err());
    assert!(fs.staging_path("escape/new.txt").is_err());
    assert!(fs.mkdir("escape/sub").is_err());
    assert!(fs.move_to_bin("escape/secret.txt").is_err());
    let staged = d.path().join("staged");
    fs::write(&staged, "x").unwrap();
    assert!(fs.install(&staged, "escape/planted.txt", None).is_err());
    assert!(!outside.join("planted.txt").exists());
    assert!(outside.join("secret.txt").exists());
}

#[cfg(windows)]
#[test]
fn junctions_are_not_followed() {
    let (d, fs) = setup();
    let root = d.path().join("pair");
    let outside = d.path().join("outside");
    fs::create_dir_all(&outside).unwrap();
    let out = std::process::Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(root.join("escape"))
        .arg(&outside)
        .output()
        .unwrap();
    assert!(out.status.success(), "mklink /J failed: {out:?}");
    assert!(fs.list("").unwrap().is_empty());
    let staged = d.path().join("staged");
    fs::write(&staged, "x").unwrap();
    assert!(fs.install(&staged, "escape/planted.txt", None).is_err());
    assert!(!outside.join("planted.txt").exists());
}

#[test]
fn install_replaces_atomically_and_sets_mtime() {
    let (d, fs) = setup();
    let staged = fs.staging_path("docs/a.txt").unwrap();
    assert!(
        staged.ends_with("a.txt.wasl-part"),
        "staged next to the target: {}",
        staged.display()
    );
    fs::create_dir_all(staged.parent().unwrap()).unwrap();
    fs::write(&staged, "v1").unwrap();
    let e = fs
        .install(&staged, "docs/a.txt", Some(1_700_000_000_123))
        .unwrap();
    assert_eq!(
        FsEntry {
            name: "a.txt".into(),
            is_dir: false,
            size: 2,
            mtime_ms: 1_700_000_000_123
        },
        e
    );
    assert!(!staged.exists());
    assert_eq!("v1", read(&fs, "docs/a.txt"));
    assert!(d.path().join("pair/docs/a.txt").is_file());
}

#[test]
fn bin_round_trip_is_byte_identical() {
    let (d, fs) = setup();
    let root = d.path().join("pair");
    let content = "مرحبا\u{0}binary\u{ff}";
    write(&root, "x/report.bin", content);
    let loc = fs.move_to_bin("x/report.bin").unwrap();
    assert_eq!(None, fs.stat("x/report.bin").unwrap());
    assert!(!loc.contains('/'));
    fs.restore_from_bin(&loc, "x/report.bin").unwrap();
    assert_eq!(content, read(&fs, "x/report.bin"));
    // Two files with the same name binned in a row get distinct locations.
    let a = fs.move_to_bin("x/report.bin").unwrap();
    write(&root, "x/report.bin", "again");
    let b = fs.move_to_bin("x/report.bin").unwrap();
    assert_ne!(a, b);
    fs.purge_from_bin(&a).unwrap();
    fs.purge_from_bin(&a).unwrap(); // idempotent
    fs.restore_from_bin(&b, "x/report.bin").unwrap();
    assert_eq!("again", read(&fs, "x/report.bin"));
}

#[test]
fn restore_never_overwrites() {
    let (d, fs) = setup();
    write(&d.path().join("pair"), "a.txt", "old");
    let loc = fs.move_to_bin("a.txt").unwrap();
    write(&d.path().join("pair"), "a.txt", "new");
    assert!(fs.restore_from_bin(&loc, "a.txt").is_err());
    assert_eq!("new", read(&fs, "a.txt"));
}

#[test]
fn rename_never_overwrites() {
    let (d, fs) = setup();
    let root = d.path().join("pair");
    write(&root, "a.txt", "a");
    write(&root, "b.txt", "b");
    assert!(fs.rename("a.txt", "b.txt").is_err());
    fs.rename("a.txt", "c.txt").unwrap();
    assert_eq!("a", read(&fs, "c.txt"));
}

#[test]
fn remove_dir_only_when_empty_and_never_the_root() {
    let (d, fs) = setup();
    let root = d.path().join("pair");
    write(&root, "full/a.txt", "a");
    write(&root, "parts/x.wasl-part", "p");
    fs::create_dir_all(root.join("empty")).unwrap();
    assert!(!fs.remove_dir_if_empty("full").unwrap());
    assert!(
        fs.remove_dir_if_empty("parts").unwrap(),
        "leftover part files don't keep a folder"
    );
    assert!(fs.remove_dir_if_empty("empty").unwrap());
    assert!(!fs.remove_dir_if_empty("gone").unwrap());
    assert!(!fs.remove_dir_if_empty("").unwrap());
    assert!(root.is_dir());
}

// ---------------------------------------------------------------- hashing

#[test]
fn hashes_match_published_test_vectors() {
    let empty = hash_reader(&b""[..]).unwrap();
    assert_eq!("d41d8cd98f00b204e9800998ecf8427e", empty.md5);
    assert_eq!(
        "af1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f3262",
        empty.blake3
    );
    let abc = hash_reader(&b"abc"[..]).unwrap();
    assert_eq!("900150983cd24fb0d6963f7d28e17f72", abc.md5);
    assert_eq!(
        "6437b3ac38465133ffb63b75273a8db548c558465d79db03fd359c6cd5bd9d85",
        abc.blake3
    );
    assert_eq!(3, abc.size);
}

#[test]
fn hashing_a_large_input_matches_hashing_it_whole() {
    let data: Vec<u8> = (0..3_000_000u32).map(|i| (i * 31 % 251) as u8).collect();
    let streamed = hash_reader(std::io::Cursor::new(&data)).unwrap();
    assert_eq!(blake3::hash(&data).to_hex().to_string(), streamed.blake3);
    assert_eq!(data.len() as u64, streamed.size);
}

// ---------------------------------------------------------------- scanner

/// Counts directory listings, to prove ignored folders are never entered.
struct Counting<'a> {
    inner: &'a StdFs,
    listed: Mutex<Vec<String>>,
    fail: Option<&'static str>,
    calls: AtomicUsize,
}

impl LocalFs for Counting<'_> {
    fn list(&self, dir: &str) -> std::io::Result<Vec<FsEntry>> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        self.listed.lock().unwrap().push(dir.to_owned());
        if self.fail == Some(dir) {
            return Err(std::io::Error::new(std::io::ErrorKind::PermissionDenied, "no"));
        }
        self.inner.list(dir)
    }
    fn stat(&self, rel: &str) -> std::io::Result<Option<FsEntry>> {
        self.inner.stat(rel)
    }
    fn open_read(&self, rel: &str) -> std::io::Result<Box<dyn Read + Send>> {
        self.inner.open_read(rel)
    }
    fn readable_path(&self, rel: &str) -> std::io::Result<ReadablePath> {
        self.inner.readable_path(rel)
    }
    fn staging_path(&self, rel: &str) -> std::io::Result<std::path::PathBuf> {
        self.inner.staging_path(rel)
    }
    fn install(&self, s: &Path, rel: &str, m: Option<i64>) -> std::io::Result<FsEntry> {
        self.inner.install(s, rel, m)
    }
    fn mkdir(&self, rel: &str) -> std::io::Result<()> {
        self.inner.mkdir(rel)
    }
    fn remove_file(&self, rel: &str) -> std::io::Result<()> {
        self.inner.remove_file(rel)
    }
    fn remove_dir_if_empty(&self, rel: &str) -> std::io::Result<bool> {
        self.inner.remove_dir_if_empty(rel)
    }
    fn rename(&self, a: &str, b: &str) -> std::io::Result<()> {
        self.inner.rename(a, b)
    }
    fn move_to_bin(&self, rel: &str) -> std::io::Result<String> {
        self.inner.move_to_bin(rel)
    }
    fn restore_from_bin(&self, l: &str, rel: &str) -> std::io::Result<()> {
        self.inner.restore_from_bin(l, rel)
    }
    fn purge_from_bin(&self, l: &str) -> std::io::Result<()> {
        self.inner.purge_from_bin(l)
    }
}

fn counting<'a>(fs: &'a StdFs, fail: Option<&'static str>) -> Counting<'a> {
    Counting {
        inner: fs,
        listed: Mutex::new(Vec::new()),
        fail,
        calls: AtomicUsize::new(0),
    }
}

fn rels(out: &wasl_fs::ScanOutcome) -> Vec<String> {
    let mut v: Vec<String> = out.entries.iter().map(|e| e.rel.clone()).collect();
    v.sort();
    v
}

#[test]
fn scan_prunes_ignored_folders_without_entering_them() {
    let (d, fs) = setup();
    let root = d.path().join("pair");
    write(&root, "web/src/app.js", "x");
    write(&root, "web/node_modules/react/index.js", "x");
    write(&root, "web/node_modules/react/deep/a.js", "x");
    write(&root, "web/.waslignore", "*.log\n");
    write(&root, "web/debug.log", "x");
    write(&root, "notes.log", "x");
    let c = counting(&fs, None);
    let builder = IgnoreRules::builder().presets([Preset::Node]);
    let out = scan(&c, builder, &|rel| rel.to_owned(), || false).unwrap();
    assert_eq!(
        vec!["notes.log", "web", "web/.waslignore", "web/src", "web/src/app.js"],
        rels(&out),
        "nested .waslignore applies to its folder only; node_modules pruned"
    );
    assert!(
        !c.listed
            .lock()
            .unwrap()
            .iter()
            .any(|d| d.contains("node_modules")),
        "never listed"
    );
    assert_eq!(1, out.rule_files.len());
    assert_eq!(
        ("web".to_owned(), FileKind::WaslIgnore),
        (out.rule_files[0].0.clone(), out.rule_files[0].1)
    );
    assert!(out.rules.is_ignored("web/x.log", false));
}

#[test]
fn scan_reports_unreadable_folders_and_fails_on_an_unreadable_root() {
    let (d, fs) = setup();
    let root = d.path().join("pair");
    write(&root, "ok/a.txt", "x");
    write(&root, "locked/b.txt", "x");
    let out = scan(
        &counting(&fs, Some("locked")),
        IgnoreRules::builder(),
        &|r| r.to_owned(),
        || false,
    )
    .unwrap();
    assert_eq!(vec!["locked".to_owned()], out.unreadable);
    assert_eq!(vec!["locked", "ok", "ok/a.txt"], rels(&out));
    assert!(
        scan(
            &counting(&fs, Some("")),
            IgnoreRules::builder(),
            &|r| r.to_owned(),
            || false
        )
        .is_err()
    );
}

#[test]
fn scan_matches_rules_on_canonical_keys() {
    let (d, fs) = setup();
    write(&d.path().join("pair"), "a：b/secret.txt", "x");
    // The local name a：b stands for the Drive name a:b.
    let key_of = |rel: &str| rel.replace('：', ":");
    let builder = IgnoreRules::builder().file("", FileKind::WaslIgnore, "a:b/\n");
    let out = scan(&fs, builder, &key_of, || false).unwrap();
    assert!(out.entries.is_empty(), "{:?}", rels(&out));
}

#[test]
fn scan_can_be_cancelled() {
    let (d, fs) = setup();
    write(&d.path().join("pair"), "a/b.txt", "x");
    let err = scan(&fs, IgnoreRules::builder(), &|r| r.to_owned(), || true)
        .err()
        .unwrap();
    assert_eq!(std::io::ErrorKind::Interrupted, err.kind());
}

/// SRS NFR-3: scanning 100,000 unchanged files takes under 2 s on a desktop SSD. Creating the tree
/// takes a while, so this runs on request: `cargo test --release -p wasl-fs -- --ignored`.
#[test]
#[ignore]
fn scans_100k_files_within_budget() {
    let (d, fs) = setup();
    let root = d.path().join("pair");
    for i in 0..100_000u32 {
        let dir = root.join(format!("d{}/s{}", i % 100, i % 1000));
        if i < 1000 {
            fs::create_dir_all(&dir).unwrap();
        }
        fs::write(dir.join(format!("f{i}.bin")), b"x").unwrap();
    }
    let start = std::time::Instant::now();
    let out = scan(&fs, IgnoreRules::builder(), &|r| r.to_owned(), || false).unwrap();
    let elapsed = start.elapsed();
    eprintln!("scanned {} entries in {elapsed:?}", out.entries.len());
    assert_eq!(100_000 + 100 + 1000, out.entries.len());
    assert!(elapsed.as_secs_f64() < 2.0, "{elapsed:?}");
}

/// The engine opens a new StdFs per operation: files with one name binned at once by different
/// instances must all survive in the bin.
#[test]
fn files_with_one_name_binned_at_once_never_replace_each_other() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("root");
    let bin = dir.path().join("bin");
    let mut locations = Vec::new();
    for i in 0..200 {
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("a.txt"), format!("v{i}")).unwrap();
        let fs = StdFs::new(&root, &bin);
        locations.push(fs.move_to_bin("a.txt").unwrap());
    }
    let fs = StdFs::new(&root, &bin);
    for (i, loc) in locations.iter().enumerate() {
        fs.restore_from_bin(loc, "a.txt").unwrap();
        assert_eq!(
            format!("v{i}"),
            std::fs::read_to_string(root.join("a.txt")).unwrap()
        );
        std::fs::remove_file(root.join("a.txt")).unwrap();
    }
}

/// ADR-0004: a case-only rename of a file onto itself works where the folder folds case; a rename
/// onto another file whose name differs only in normalisation (two files on NTFS and Linux) is
/// refused, never an overwrite.
#[test]
fn rename_in_place_only_onto_the_same_file() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("p");
    fs::create_dir_all(&root).unwrap();
    let fs_ = StdFs::new(&root, dir.path().join("bin"));
    fs::write(root.join("Foo.txt"), "foo").unwrap();
    fs_.rename("Foo.txt", "foo.txt").unwrap();
    assert!(fs_.stat("foo.txt").unwrap().is_some());

    let (nfc, nfd) = ("caf\u{e9}.txt", "cafe\u{301}.txt");
    fs::write(root.join(nfc), "nfc").unwrap();
    fs::write(root.join(nfd), "nfd").unwrap();
    if fs_.is_same_file(nfc, nfd).unwrap() {
        return; // a folder that folds normalisation (APFS) holds one file
    }
    assert!(fs_.rename(nfc, nfd).is_err());
    assert_eq!("nfd", fs::read_to_string(root.join(nfd)).unwrap());
}

/// Some network drives (WebDAV, some NAS) report every file as the same file. Two names the folder
/// lists exactly are two entries, whatever the file identity says: never a rename "in place".
/// Emulated with two hard links whose names differ only in normalisation.
#[test]
fn two_listed_names_are_never_one_file_for_a_rename() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("p");
    fs::create_dir_all(&root).unwrap();
    let fs_ = StdFs::new(&root, dir.path().join("bin"));
    let (nfc, nfd) = ("caf\u{e9}.txt", "cafe\u{301}.txt");
    fs::write(root.join(nfc), "one").unwrap();
    if fs::hard_link(root.join(nfc), root.join(nfd)).is_err() {
        return; // a folder that folds normalisation (APFS) cannot hold both names
    }
    assert!(!fs_.is_same_file(nfc, nfd).unwrap());
    assert!(fs_.rename(nfc, nfd).is_err());
    assert!(root.join(nfc).exists() && root.join(nfd).exists());
}
