//! Port of `core/NameMapperTest.kt` (Android 1.1.4). One Rust test per Kotlin test.

use wasl_core::names::{conflict_name, needs_mapping, to_local_name};

// ---------------------------------------------------------------- needs_mapping

#[test]
fn ordinary_names_do_not_need_mapping() {
    for name in [
        "Report 2026.docx",
        "résumé.pdf",
        "日本語ファイル.txt",
        "file-name_v2 (1).tar.gz",
        "IMG_20260930-140301.jpg",
        ".hidden",
        "no-extension",
        "a b c",
        "emoji 😀.png",
        "trailing.",
    ] {
        assert!(!needs_mapping(name), "'{name}' should not need mapping");
    }
}

#[test]
fn names_with_fat_invalid_characters_need_mapping() {
    for name in ["a:b", "a\"b", "a*b", "a<b", "a>b", "a?b", "a\\b", "a|b"] {
        assert!(needs_mapping(name), "'{name}' should need mapping");
    }
}

#[test]
fn special_names_need_mapping() {
    assert!(needs_mapping("."));
    assert!(needs_mapping(".."));
    assert!(needs_mapping(""));
    assert!(needs_mapping("tab\tname"));
    assert!(needs_mapping("new\nline"));
    assert!(needs_mapping(&"a".repeat(256)));
    assert!(!needs_mapping(&"a".repeat(255)));
    // byte length counts, not character length: 128 two-byte characters = 256 bytes
    assert!(needs_mapping(&"é".repeat(128)));
    assert!(!needs_mapping(&"é".repeat(127)));
}

// ---------------------------------------------------------------- to_local_name

#[test]
fn ordinary_name_is_returned_unchanged() {
    assert_eq!("Report 2026.docx", to_local_name("Report 2026.docx"));
}

#[test]
fn fat_invalid_characters_are_mapped_to_full_width_equivalents() {
    assert_eq!("a＂b", to_local_name("a\"b"));
    assert_eq!("a＊b", to_local_name("a*b"));
    assert_eq!("a：b", to_local_name("a:b"));
    assert_eq!("a＜b", to_local_name("a<b"));
    assert_eq!("a＞b", to_local_name("a>b"));
    assert_eq!("a？b", to_local_name("a?b"));
    assert_eq!("a＼b", to_local_name("a\\b"));
    assert_eq!("a｜b", to_local_name("a|b"));
    // '&' is legal on FAT and must survive untouched
    assert_eq!(
        "Q&A： what？ ＜draft＞ ＊final＊ ｜ v2＂.txt",
        to_local_name("Q&A: what? <draft> *final* | v2\".txt")
    );
}

#[test]
fn control_characters_are_replaced_by_symbol_for_null() {
    assert_eq!("a␀b␀c", to_local_name("a\u{0001}b\nc"));
    assert_eq!("tab␀sep", to_local_name("tab\tsep"));
}

#[test]
fn dot_and_dot_dot_get_a_prefix() {
    assert_eq!("＿.", to_local_name("."));
    assert_eq!("＿..", to_local_name(".."));
    assert_eq!("...", to_local_name("..."));
}

#[test]
fn empty_name_becomes_unnamed() {
    assert_eq!("＿unnamed", to_local_name(""));
}

#[test]
fn overlong_name_is_truncated_but_keeps_extension() {
    let out = to_local_name(&format!("{}.txt", "a".repeat(300)));
    assert_eq!(format!("{}.txt", "a".repeat(251)), out);
    assert_eq!(255, out.len());
}

#[test]
fn overlong_multi_byte_name_is_truncated_by_bytes_not_characters() {
    let out = to_local_name(&format!("{}.jpg", "é".repeat(200)));
    assert!(out.ends_with(".jpg"));
    assert!(out.starts_with('é'));
    assert!(out.len() <= 255);
    // 125 * 2 bytes + ".jpg" (4 bytes) = 254: the largest fit, never cutting a character in half
    assert_eq!(format!("{}.jpg", "é".repeat(125)), out);
}

#[test]
fn overlong_name_without_extension_is_simply_truncated() {
    assert_eq!("b".repeat(255), to_local_name(&"b".repeat(400)));
}

#[test]
fn overlong_name_with_long_extension_does_not_preserve_the_extension() {
    let ext = "x".repeat(20); // 20 chars: not treated as an extension (limit is < 16)
    let out = to_local_name(&format!("{}.{ext}", "a".repeat(300)));
    assert_eq!("a".repeat(255), out);
}

#[test]
fn overlong_name_with_invalid_characters_is_mapped_then_truncated() {
    let out = to_local_name(&format!("{}.mp4", "a:b".repeat(120)));
    assert!(out.ends_with(".mp4"));
    assert!(!out.contains(':'));
    assert!(out.contains('：'));
    assert!(out.len() <= 255);
}

// ---------------------------------------------------------------- conflict_name

#[test]
fn conflict_name_keeps_extension() {
    assert_eq!(
        "report (conflicted copy from Pixel, 2026-09-30 14-03).docx",
        conflict_name("report.docx", "Pixel", "2026-09-30 14-03")
    );
}

#[test]
fn conflict_name_without_extension() {
    assert_eq!(
        "README (conflicted copy from Pixel, 2026-09-30 14-03)",
        conflict_name("README", "Pixel", "2026-09-30 14-03")
    );
}

#[test]
fn conflict_name_dot_file_has_no_extension() {
    assert_eq!(
        ".bashrc (conflicted copy from Pixel, s)",
        conflict_name(".bashrc", "Pixel", "s")
    );
}

#[test]
fn conflict_name_uses_only_the_last_extension() {
    assert_eq!(
        "archive.tar (conflicted copy from Pixel, s).gz",
        conflict_name("archive.tar.gz", "Pixel", "s")
    );
}

#[test]
fn conflict_name_long_suffix_is_not_an_extension() {
    assert_eq!(
        "notes.averyveryverylongext (conflicted copy from Pixel, s)",
        conflict_name("notes.averyveryverylongext", "Pixel", "s")
    );
    // 11 characters after the dot is still an extension
    assert_eq!(
        "n (conflicted copy from Pixel, s).abcdefghijk",
        conflict_name("n.abcdefghijk", "Pixel", "s")
    );
}

#[test]
fn conflict_name_trailing_dot_is_not_an_extension() {
    assert_eq!(
        "name. (conflicted copy from Pixel, s)",
        conflict_name("name.", "Pixel", "s")
    );
}

// ---------------------------------------------------------------- beyond 1.1.4

#[test]
fn slash_in_a_drive_name_never_leaves_the_folder() {
    assert!(needs_mapping("a/b"));
    assert_eq!("a／b", to_local_name("a/b"));
    assert_eq!("..／..／etc", to_local_name("../../etc"));
    assert!(!to_local_name("/").contains('/'));
}

#[test]
fn conflict_name_device_and_stamp_cannot_add_path_components() {
    let out = conflict_name("a.txt", "../Pixel/x", "2026/10/19");
    assert!(!out.contains('/'), "{out}");
    assert_eq!("a (conflicted copy from ..／Pixel／x, 2026／10／19).txt", out);
}
