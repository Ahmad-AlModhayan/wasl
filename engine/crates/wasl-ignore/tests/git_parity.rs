//! SRS FR-G1 acceptance: fixture trees compared with `git check-ignore`. For every file and folder
//! the verdict and the deciding rule (file and line) must match git's. Skipped when git is missing.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::process::{Command, Stdio};
use wasl_ignore::{FileKind, IgnoreRules, RuleOrigin};

struct Fixture {
    /// Rule files: (folder key, contents).
    gitignores: &'static [(&'static str, &'static str)],
    /// Files to create; folders come from their paths.
    files: &'static [&'static str],
}

fn git_available() -> bool {
    Command::new("git")
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success())
}

/// git's view: path → Some((ignored, "file:line")) when a rule matched, None otherwise.
fn git_check(root: &Path, paths: &[String]) -> BTreeMap<String, Option<(bool, String)>> {
    let mut child = Command::new("git")
        .args([
            "-c",
            "core.ignorecase=false",
            "-c",
            "core.excludesFile=",
            "check-ignore",
            "--no-index",
            "--stdin",
            "-v",
            "-n",
        ])
        .current_dir(root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("git runs");
    {
        use std::io::Write;
        let mut stdin = child.stdin.take().unwrap();
        for p in paths {
            writeln!(stdin, "{p}").unwrap();
        }
    }
    let out = child.wait_with_output().unwrap();
    let text = String::from_utf8(out.stdout).unwrap();
    let mut verdicts = BTreeMap::new();
    for line in text.lines() {
        // "<source>:<line>:<pattern>\t<path>" or "::\t<path>"
        let (info, path) = line.split_once('\t').expect("check-ignore -v output");
        let path = path.trim_matches('"').to_owned();
        if info == "::" {
            verdicts.insert(path, None);
            continue;
        }
        let mut parts = info.splitn(3, ':');
        let (file, line_no, pattern) = (
            parts.next().unwrap(),
            parts.next().unwrap(),
            parts.next().unwrap(),
        );
        verdicts.insert(
            path,
            Some((!pattern.starts_with('!'), format!("{file}:{line_no}"))),
        );
    }
    verdicts
}

fn run(fixture: &Fixture) {
    if !git_available() {
        eprintln!("git not found: skipping");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let status = Command::new("git")
        .args(["init", "-q"])
        .current_dir(root)
        .status()
        .unwrap();
    assert!(status.success());

    let mut builder = IgnoreRules::builder().honor_gitignore(true);
    for (folder, contents) in fixture.gitignores {
        let d = root.join(folder);
        fs::create_dir_all(&d).unwrap();
        fs::write(d.join(".gitignore"), contents).unwrap();
        builder = builder.file(folder, FileKind::GitIgnore, contents);
    }
    let rules = builder.build().unwrap();

    // Every file and every folder on the way to it, with whether it is a folder.
    let mut paths: BTreeMap<String, bool> = BTreeMap::new();
    for f in fixture.files {
        let p = root.join(f);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(&p, b"x").unwrap();
        paths.insert((*f).to_owned(), false);
        for (i, _) in f.match_indices('/') {
            paths.insert(f[..i].to_owned(), true);
        }
    }
    let list: Vec<String> = paths.keys().cloned().collect();
    let git = git_check(root, &list);

    let mut mismatches = Vec::new();
    for (path, is_dir) in &paths {
        let ours = rules.check(path, *is_dir);
        let ours_origin = ours.rule.as_ref().map(|r| match &r.origin {
            RuleOrigin::File { file, line } => format!("{file}:{line}"),
            other => format!("{other}"),
        });
        let theirs = git.get(path).cloned().flatten();
        let theirs_ignored = theirs.as_ref().is_some_and(|(ignored, _)| *ignored);
        let theirs_origin = theirs.map(|(_, origin)| origin);
        if ours.ignored != theirs_ignored || ours_origin != theirs_origin {
            mismatches.push(format!(
                "{path}{}: wasl ignored={} by {:?}, git ignored={} by {:?}",
                if *is_dir { "/" } else { "" },
                ours.ignored,
                ours_origin,
                theirs_ignored,
                theirs_origin
            ));
        }
    }
    assert!(
        mismatches.is_empty(),
        "differences from git:\n{}",
        mismatches.join("\n")
    );
}

#[test]
fn patterns_negation_anchoring_and_directory_rules() {
    run(&Fixture {
        gitignores: &[(
            "",
            "# comment\n*.log\n!important.log\n/root-only.txt\nbuild/\ndocs/**/draft.md\n\\#hash.txt\ntrailing-space.txt   \n[Tt]emp/\na/**/z\n*.o\n",
        )],
        files: &[
            "a.log",
            "important.log",
            "deep/x/important.log",
            "deep/x/other.log",
            "root-only.txt",
            "sub/root-only.txt",
            "build/out.bin",
            "sub/build/x.bin",
            "notbuild/build.txt",
            "docs/draft.md",
            "docs/x/y/draft.md",
            "other/docs/draft.md",
            "#hash.txt",
            "trailing-space.txt",
            "temp/t.txt",
            "a/z",
            "a/q/r/z",
            "b/a/z",
            "src/main.o",
            "src/main.rs",
        ],
    });
}

#[test]
fn nearer_files_override_higher_ones() {
    run(&Fixture {
        gitignores: &[
            ("", "*.tmp\n*.log\n/local.txt\n"),
            ("sub", "!keep.log\n/local.txt\nnested/\n*.cache\n!*.keep\n"),
            ("sub/deeper", "!*.tmp\n!*.cache\n*.keep\n"),
        ],
        files: &[
            "x.tmp",
            "sub/x.tmp",
            "sub/deeper/y.tmp",
            "sub/deeper/more/z.tmp",
            "sub/keep.log",
            "sub/other.log",
            "keep.log",
            "local.txt",
            "sub/local.txt",
            "sub/deeper/local.txt",
            "sub/nested/n.txt",
            "nested/n.txt",
            "sub/a.cache",
            "sub/deeper/a.cache",
            "sub/deeper/more/b.cache",
            "sub/a.keep",
            "sub/deeper/a.keep",
        ],
    });
}

#[test]
fn a_negation_cannot_reinclude_a_file_inside_an_ignored_folder() {
    run(&Fixture {
        gitignores: &[("", "cache/\n!cache/keep.txt\nlogs/*\n!logs/keep.txt\n")],
        files: &[
            "cache/keep.txt",
            "cache/drop.txt",
            "logs/keep.txt",
            "logs/drop.txt",
            "logs/sub/x.txt",
        ],
    });
}

#[test]
fn double_star_forms() {
    run(&Fixture {
        gitignores: &[("", "**/generated\nout/**\n**/tmp/**/*.bin\nfoo/**/bar/\n")],
        files: &[
            "generated",
            "x/generated/y.rs",
            "out/a/b.txt",
            "tmp/a.bin",
            "q/tmp/w/e/a.bin",
            "q/tmp/w/e/a.txt",
            "foo/bar/x",
            "foo/a/b/bar/x",
            "foo/bar.txt",
        ],
    });
}
