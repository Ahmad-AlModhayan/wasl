//! Rule files, presets, pair patterns and explain (SRS FR-G1, FR-G2, FR-G3, FR-G6, FR-G7, FR-G8).

use wasl_ignore::legacy::{PairRule, PathFilter};
use wasl_ignore::{FileKind, IgnoreRules, Preset, RuleOrigin};

fn file_origin(file: &str, line: usize) -> RuleOrigin {
    RuleOrigin::File {
        file: file.to_owned(),
        line,
    }
}

#[test]
fn waslignore_applies_at_any_depth() {
    let rules = IgnoreRules::builder()
        .file("", FileKind::WaslIgnore, "*.tmp\n")
        .file("app/src", FileKind::WaslIgnore, "generated/\n")
        .build()
        .unwrap();
    assert!(rules.is_ignored("a.tmp", false));
    assert!(rules.is_ignored("app/src/x/a.tmp", false));
    assert!(rules.is_ignored("app/src/generated", true));
    assert!(rules.is_ignored("app/src/deep/generated/r.java", false));
    assert!(
        !rules.is_ignored("app/generated/r.java", false),
        "rules in app/src don't reach app/"
    );
    assert!(!rules.is_ignored("app/src/main.rs", false));
}

#[test]
fn gitignore_counts_only_when_the_pair_honours_it() {
    let build = |honor| {
        IgnoreRules::builder()
            .file("", FileKind::GitIgnore, "secrets.env\n")
            .honor_gitignore(honor)
            .build()
            .unwrap()
    };
    assert!(!build(false).is_ignored("secrets.env", false));
    assert!(build(true).is_ignored("secrets.env", false));
}

#[test]
fn waslignore_wins_over_gitignore_in_the_same_folder() {
    let rules = IgnoreRules::builder()
        .honor_gitignore(true)
        .file("", FileKind::GitIgnore, "dist/\n*.lock\n")
        .file("", FileKind::WaslIgnore, "!dist/\n")
        .build()
        .unwrap();
    assert!(
        !rules.is_ignored("dist/app.js", false),
        "sync the build output git ignores"
    );
    assert!(rules.is_ignored("Cargo.lock", false));
}

#[test]
fn presets_sit_below_every_rule_file() {
    let rules = IgnoreRules::builder()
        .presets([Preset::Node])
        .file("", FileKind::WaslIgnore, "!dist/\n")
        .build()
        .unwrap();
    assert!(rules.is_ignored("node_modules", true));
    assert!(rules.is_ignored("web/node_modules/react/index.js", false));
    assert!(
        !rules.is_ignored("dist/bundle.js", false),
        "a rule file re-includes what a preset excludes"
    );
}

#[test]
fn every_preset_covers_its_usual_folders() {
    let cases: [(Preset, &[(&str, bool)]); 8] = [
        (
            Preset::Node,
            &[
                ("node_modules/x/index.js", false),
                ("dist/app.js", false),
                (".next/cache/a", false),
            ],
        ),
        (
            Preset::Python,
            &[
                ("pkg/__pycache__/m.cpython-312.pyc", false),
                (".venv/bin/python", false),
                ("m.pyc", false),
            ],
        ),
        (
            Preset::Rust,
            &[("target/debug/app", false), ("crates/x/target/release/x", false)],
        ),
        (
            Preset::AndroidGradle,
            &[
                (".gradle/8.9/x", false),
                ("app/build/outputs/a.apk", false),
                ("local.properties", false),
            ],
        ),
        (
            Preset::Flutter,
            &[(".dart_tool/package_config.json", false), ("build/app/x", false)],
        ),
        (Preset::Docker, &[(".docker/config.json", false)]),
        (
            Preset::Editors,
            &[
                (".idea/workspace.xml", false),
                (".vs/x", false),
                ("notes.txt~", false),
            ],
        ),
        (
            Preset::OsJunk,
            &[
                (".DS_Store", false),
                ("Photos/Thumbs.db", false),
                ("desktop.ini", false),
                ("._a.jpg", false),
            ],
        ),
    ];
    for (preset, paths) in cases {
        let rules = IgnoreRules::builder().presets([preset]).build().unwrap();
        for (path, is_dir) in paths {
            assert!(rules.is_ignored(path, *is_dir), "{preset} should ignore {path}");
        }
        for keep in ["src/main.rs", "README.md", "app/src/main/AndroidManifest.xml"] {
            assert!(!rules.is_ignored(keep, false), "{preset} must not ignore {keep}");
        }
    }
    // Shared project settings stay; only per-user editor state goes.
    let editors = IgnoreRules::builder().presets([Preset::Editors]).build().unwrap();
    assert!(!editors.is_ignored(".idea/codeStyles/Project.xml", false));
    assert!(!editors.is_ignored(".vscode/settings.json", false));
}

#[test]
fn preset_ids_round_trip() {
    for p in Preset::ALL {
        assert_eq!(Some(p), Preset::from_id(p.id()));
    }
    assert_eq!(None, Preset::from_id("cobol"));
}

#[test]
fn pair_patterns_from_1_1_4_exclude_on_top_of_rule_files() {
    let filter = PathFilter::new(&["*.jpg"], &["*.bak"], &["Private"], false);
    let rules = IgnoreRules::builder()
        .file("", FileKind::WaslIgnore, "!*.bak\n")
        .pair_patterns(filter)
        .build()
        .unwrap();
    assert!(
        rules.is_ignored("a.bak", false),
        "a rule file cannot re-include what the pair excludes"
    );
    assert!(
        rules.is_ignored("notes.txt", false),
        "includes keep only matching files"
    );
    assert!(!rules.is_ignored("p.jpg", false));
    assert!(
        rules.is_ignored("Private/p.jpg", false),
        "excluded subfolders take their files along"
    );
    assert!(rules.is_ignored(".hidden/p.jpg", false));
    let v = rules.check("Private/p.jpg", false);
    assert_eq!(RuleOrigin::PairPatterns, v.rule.as_ref().unwrap().origin);
    assert_eq!(
        Some(&PairRule::ExcludedSubfolder("Private".into())),
        v.rule.as_ref().unwrap().pair_rule.as_ref()
    );
    assert_eq!(Some("Private"), v.rule.unwrap().via_parent.as_deref());
    // The rule that fired is named, not guessed from the path.
    let why = |key: &str| rules.check(key, false).rule.and_then(|m| m.pair_rule);
    assert_eq!(Some(PairRule::Hidden), why(".hidden/p.jpg"));
    assert_eq!(Some(PairRule::Exclude("*.bak".into())), why("a.bak"));
    assert_eq!(Some(PairRule::NotIncluded), why("notes.txt"));
}

#[test]
fn rule_files_always_sync() {
    let rules = IgnoreRules::builder()
        .honor_gitignore(true)
        .file("", FileKind::WaslIgnore, ".*\n*.txt\n")
        .pair_patterns(PathFilter::new::<&str>(&[], &[], &[], false)) // hides dot files
        .build()
        .unwrap();
    assert!(!rules.is_ignored(".waslignore", false));
    assert!(!rules.is_ignored("app/.waslignore", false));
    assert!(!rules.is_ignored(".gitignore", false));
    assert!(rules.is_ignored("other.txt", false));
    // ...unless they sit inside an ignored folder, which never syncs at all.
    let nested = IgnoreRules::builder().presets([Preset::Node]).build().unwrap();
    assert!(nested.is_ignored("node_modules/pkg/.gitignore", false));
}

#[test]
fn explain_names_the_file_line_and_pattern() {
    let rules = IgnoreRules::builder()
        .file("", FileKind::WaslIgnore, "# build output\n*.log\n!keep.log\n")
        .file("web", FileKind::WaslIgnore, "cache/\n")
        .presets([Preset::Node])
        .build()
        .unwrap();

    let v = rules.check("x.log", false);
    assert!(v.ignored);
    let m = v.rule.unwrap();
    assert_eq!(file_origin(".waslignore", 2), m.origin);
    assert_eq!("*.log", m.pattern);
    assert_eq!(None, m.via_parent);
    assert_eq!(".waslignore:2", m.origin.to_string());

    let v = rules.check("keep.log", false);
    assert!(!v.ignored, "re-included");
    assert_eq!(file_origin(".waslignore", 3), v.rule.unwrap().origin);

    let v = rules.check("web/cache/a/b.bin", false);
    let m = v.rule.unwrap();
    assert_eq!(file_origin("web/.waslignore", 1), m.origin);
    assert_eq!(Some("web/cache"), m.via_parent.as_deref());

    let v = rules.check("node_modules/react/index.js", false);
    let m = v.rule.unwrap();
    assert_eq!(
        RuleOrigin::Preset {
            preset: Preset::Node,
            line: 1
        },
        m.origin
    );
    assert_eq!("preset:node:1", m.origin.to_string());

    assert_eq!(None, rules.check("src/main.rs", false).rule);
}

#[test]
fn matching_is_case_sensitive_like_git() {
    let rules = IgnoreRules::builder()
        .file("", FileKind::WaslIgnore, "Build/\n")
        .build()
        .unwrap();
    assert!(rules.is_ignored("Build", true));
    assert!(!rules.is_ignored("build", true));
}

#[test]
fn the_pair_root_is_never_ignored() {
    let rules = IgnoreRules::builder()
        .file("", FileKind::WaslIgnore, "*\n")
        .build()
        .unwrap();
    assert!(!rules.is_ignored("", true));
    assert!(!rules.is_ignored("/", true));
}

#[test]
fn rule_files_in_folders_with_unusual_names_still_decide() {
    // Drive accepts tabs, colons and newlines in folder names; the verdict must not depend on them.
    for folder in ["tab\there", "colon:here", "new\nline", "f\t3\tx"] {
        let rules = IgnoreRules::builder()
            .file(folder, FileKind::WaslIgnore, "secrets.env\n")
            .build()
            .unwrap();
        let v = rules.check(&format!("{folder}/secrets.env"), false);
        assert!(v.ignored, "{folder:?}");
        assert_eq!(
            file_origin(&format!("{folder}/.waslignore"), 1),
            v.rule.unwrap().origin
        );
    }
}
