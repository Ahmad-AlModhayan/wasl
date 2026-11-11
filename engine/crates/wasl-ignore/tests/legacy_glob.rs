//! Port of `core/GlobTest.kt` (classes `GlobTest` and `PathFilterTest`, Android 1.1.4).

use wasl_ignore::legacy::{Glob, GlobError, PathFilter};

fn matches(pattern: &str, path: &str) -> bool {
    Glob::compile(pattern).unwrap().matches(path, false)
}

fn assert_matches(pattern: &str, paths: &[&str]) {
    for p in paths {
        assert!(matches(pattern, p), "'{pattern}' should match '{p}'");
    }
}

fn assert_no_match(pattern: &str, paths: &[&str]) {
    for p in paths {
        assert!(!matches(pattern, p), "'{pattern}' should NOT match '{p}'");
    }
}

mod glob {
    use super::*;

    // ------------------------------------------------------------ operators

    #[test]
    fn question_mark_matches_exactly_one_character_but_never_slash() {
        assert_matches("/a?c.txt", &["abc.txt", "a-c.txt", "aXc.txt"]);
        assert_no_match("/a?c.txt", &["ac.txt", "abbc.txt", "a/c.txt"]);
    }

    #[test]
    fn star_matches_any_run_of_characters_except_slash() {
        assert_matches("/*.jpg", &["a.jpg", "photo 1.jpg", ".jpg"]);
        assert_no_match("/*.jpg", &["sub/a.jpg", "a.jpeg", "ajpg"]);
        assert_matches("/IMG_*", &["IMG_", "IMG_0001.jpg"]);
    }

    #[test]
    fn double_star_with_slash_matches_zero_or_more_directories() {
        assert_matches("/a/**/b.txt", &["a/b.txt", "a/x/b.txt", "a/x/y/z/b.txt"]);
        assert_no_match("/a/**/b.txt", &["b.txt", "a/b.txt/c", "q/a/b.txt"]);
    }

    #[test]
    fn double_star_at_end_matches_whole_subtree_but_not_the_folder_itself() {
        assert_matches("/cache/**", &["cache/x", "cache/x/y.bin"]);
        assert_no_match("/cache/**", &["cache", "cache2/x"]);
    }

    #[test]
    fn double_star_without_slash_matches_across_directories() {
        assert_matches("/foo**", &["foo", "foobar", "foo/bar/baz"]);
        assert_no_match("/foo**", &["xfoo"]);
    }

    #[test]
    fn character_class_matches_one_listed_character() {
        assert_matches("/f[abc].txt", &["fa.txt", "fb.txt", "fc.txt"]);
        assert_no_match("/f[abc].txt", &["fd.txt", "f.txt", "fab.txt"]);
        assert_matches("/img[0-9].png", &["img0.png", "img7.png"]);
        assert_no_match("/img[0-9].png", &["imga.png", "img10.png"]);
    }

    #[test]
    fn negated_character_class_matches_anything_but_listed_characters() {
        assert_matches("/f[!abc].txt", &["fd.txt", "fz.txt", "f1.txt"]);
        assert_no_match("/f[!abc].txt", &["fa.txt", "fb.txt", "fc.txt", "f.txt"]);
        // '^' is accepted as an alias for '!'
        assert_matches("/f[^abc].txt", &["fd.txt"]);
        assert_no_match("/f[^abc].txt", &["fa.txt"]);
    }

    #[test]
    fn unterminated_character_class_is_treated_as_literal_bracket() {
        assert_matches("/f[a.txt", &["f[a.txt"]);
        assert_no_match("/f[a.txt", &["fa.txt"]);
    }

    #[test]
    fn numeric_range_expands_to_every_integer_inclusive() {
        assert_matches("/img{1..9}.jpg", &["img1.jpg", "img5.jpg", "img9.jpg"]);
        assert_no_match("/img{1..9}.jpg", &["img0.jpg", "img10.jpg", "img.jpg"]);
        assert_matches("/img{8..12}.jpg", &["img8.jpg", "img10.jpg", "img12.jpg"]);
        assert_no_match("/img{8..12}.jpg", &["img7.jpg", "img13.jpg"]);
        // reversed bounds are normalised
        assert_matches("/v{3..1}", &["v1", "v2", "v3"]);
        assert_no_match("/v{3..1}", &["v4"]);
    }

    #[test]
    fn numeric_range_larger_than_two_thousand_is_kept_literal() {
        assert_matches("/n{1..5000}", &["n{1..5000}"]);
        assert_no_match("/n{1..5000}", &["n7"]);
    }

    #[test]
    fn alternation_matches_any_listed_option() {
        assert_matches("/*.{jpg,png}", &["a.jpg", "b.png"]);
        assert_no_match("/*.{jpg,png}", &["a.gif", "a.jpgpng"]);
        assert_matches("/{Camera,Screenshots}/*", &["Camera/x.jpg", "Screenshots/y.png"]);
        assert_no_match("/{Camera,Screenshots}/*", &["Downloads/x.jpg"]);
    }

    #[test]
    fn brace_without_range_or_comma_is_literal() {
        assert_matches("/a{b}c", &["a{b}c"]);
        assert_no_match("/a{b}c", &["abc"]);
    }

    // ------------------------------------------------------------ anchoring

    #[test]
    fn pattern_without_leading_slash_matches_at_any_depth() {
        assert_matches("*.jpg", &["a.jpg", "x/a.jpg", "x/y/z/a.jpg"]);
        assert_matches("thumbs.db", &["thumbs.db", "Pictures/thumbs.db"]);
        assert_no_match("thumbs.db", &["mythumbs.db", "thumbs.db/x"]);
    }

    #[test]
    fn leading_slash_anchors_to_pair_root() {
        assert_matches("/*.jpg", &["a.jpg"]);
        assert_no_match("/*.jpg", &["x/a.jpg"]);
        assert_matches("/build", &["build"]);
        assert_no_match("/build", &["x/build", "build/x"]);
    }

    #[test]
    fn explicit_double_star_prefix_is_not_doubled() {
        assert_matches(
            "**/node_modules",
            &["node_modules", "a/node_modules", "a/b/node_modules"],
        );
        assert_no_match("**/node_modules", &["node_modules/x"]);
    }

    #[test]
    fn trailing_slash_matches_directories_only() {
        let g = Glob::compile("build/").unwrap();
        assert!(g.dir_only());
        assert!(g.matches("build", true));
        assert!(g.matches("src/build", true));
        assert!(
            !g.matches("build", false),
            "a file named 'build' is not a directory match"
        );
        // the glob itself does not match children: pruning is the scanner's job (see PathFilter tests)
        assert!(!g.matches("build/out.bin", false));
    }

    #[test]
    fn pattern_without_trailing_slash_matches_files_and_directories() {
        let g = Glob::compile("build").unwrap();
        assert!(!g.dir_only());
        assert!(g.matches("build", true));
        assert!(g.matches("build", false));
    }

    #[test]
    fn leading_slash_on_the_path_is_ignored() {
        assert_matches("/a.jpg", &["/a.jpg"]);
        assert_matches("*.jpg", &["/x/a.jpg"]);
    }

    // ------------------------------------------------------------ case and escaping

    #[test]
    fn matching_is_case_insensitive() {
        assert_matches("*.JPG", &["photo.jpg", "PHOTO.Jpg"]);
        assert_matches("/Photos/*", &["photos/a.txt", "PHOTOS/b.txt"]);
        assert_matches("/[a-c]x", &["Bx"]);
        assert_matches("/{Camera,DCIM}/*", &["camera/x", "dcim/y"]);
    }

    #[test]
    fn matching_is_case_insensitive_for_non_ascii() {
        assert_matches("/ärger.txt", &["ÄRGER.txt"]);
    }

    #[test]
    fn regex_meta_characters_are_treated_literally() {
        assert_matches("/a+b(c).txt", &["a+b(c).txt"]);
        assert_no_match("/a+b(c).txt", &["aab(c).txt", "abc.txt"]);
        assert_matches("/price$.txt", &["price$.txt"]);
        assert_matches("/x|y.txt", &["x|y.txt"]);
        assert_no_match("/x|y.txt", &["x", "y.txt"]);
        assert_no_match("/a.txt", &["aXtxt"]);
        assert_matches("/back\\slash", &["back\\slash"]);
    }

    // ------------------------------------------------------------ compile

    #[test]
    fn compile_rejects_empty_pattern() {
        assert_eq!(GlobError::Empty, Glob::compile("   ").unwrap_err());
    }

    #[test]
    fn compile_trims_whitespace_and_keeps_raw_pattern() {
        let g = Glob::compile("  *.jpg  ").unwrap();
        assert_eq!("  *.jpg  ", g.pattern());
        assert!(g.matches("a.jpg", false));
    }
}

mod path_filter {
    use super::*;

    fn filter(
        includes: &[&str],
        excludes: &[&str],
        excluded_subfolders: &[&str],
        sync_hidden: bool,
    ) -> PathFilter {
        PathFilter::new(includes, excludes, excluded_subfolders, sync_hidden)
    }

    fn excluding(excludes: &[&str]) -> PathFilter {
        filter(&[], excludes, &[], false)
    }

    #[test]
    fn no_patterns_accepts_everything_visible() {
        let f = filter(&[], &[], &[], false);
        assert!(f.accept("a.txt", false));
        assert!(f.accept("dir", true));
        assert!(f.accept("dir/sub/file.bin", false));
    }

    #[test]
    fn root_path_is_always_accepted() {
        assert!(excluding(&["**"]).accept("", true));
        assert!(excluding(&["**"]).accept("/", true));
    }

    #[test]
    fn hidden_entries_are_rejected_unless_sync_hidden() {
        let f = filter(&[], &[], &[], false);
        assert!(!f.accept(".git", true));
        assert!(!f.accept(".nomedia", false));
        assert!(!f.accept("a/.thumbnails/x.jpg", false));
        assert!(!f.accept(".hidden/visible.txt", false));
        assert!(f.accept("a.b/visible.txt", false));

        let h = filter(&[], &[], &[], true);
        assert!(h.accept(".git", true));
        assert!(h.accept(".nomedia", false));
        assert!(h.accept("a/.thumbnails/x.jpg", false));
    }

    #[test]
    fn internal_part_and_tmp_files_are_always_rejected() {
        let f = filter(&["**"], &[], &[], true);
        assert!(!f.accept("photo.jpg.drivesync-part", false));
        assert!(!f.accept("sub/photo.jpg.drivesync-part", false));
        assert!(!f.accept("x.drivesync-tmp", false));
        assert!(!f.accept("photo.jpg.drivesync-part", true));
        assert!(f.accept("photo.jpg", false));
        // only the suffix is internal; a directory containing the marker in the middle is fine
        assert!(f.accept("drivesync-part-notes.txt", false));
    }

    #[test]
    fn includes_apply_to_files_only_so_directories_are_still_traversed() {
        let f = filter(&["*.jpg"], &[], &[], false);
        assert!(f.accept("a.jpg", false));
        assert!(f.accept("deep/a.JPG", false));
        assert!(!f.accept("a.png", false));
        assert!(!f.accept("deep/notes.txt", false));
        // directories are never filtered by includes, otherwise nothing below them could match
        assert!(f.accept("deep", true));
        assert!(f.accept("deep/deeper", true));
    }

    #[test]
    fn excludes_reject_matching_files() {
        let f = excluding(&["*.tmp", "~*"]);
        assert!(!f.accept("a.tmp", false));
        assert!(!f.accept("sub/a.tmp", false));
        assert!(!f.accept("~lock.docx", false));
        assert!(f.accept("a.txt", false));
    }

    #[test]
    fn exclude_wins_over_include() {
        let f = filter(&["*.txt"], &["secret*"], &[], false);
        assert!(f.accept("notes.txt", false));
        assert!(!f.accept("secret.txt", false));
        assert!(!f.accept("a/secret-plan.txt", false));
    }

    #[test]
    fn directory_exclude_prunes_the_directory_itself() {
        let f = excluding(&["node_modules/"]);
        assert!(!f.accept("node_modules", true), "directory must be pruned");
        assert!(!f.accept("a/b/node_modules", true));
        // a file that happens to have the folder's name is not affected by a dir-only pattern
        assert!(f.accept("node_modules", false));
        // children are not matched by the pattern: the scanner prunes at the directory, so it never asks
        assert!(f.accept("node_modules/x.js", false));
    }

    #[test]
    fn exclude_pattern_without_slash_prunes_directories_and_files() {
        let f = excluding(&["build"]);
        assert!(!f.accept("build", true));
        assert!(!f.accept("build", false));
        assert!(!f.accept("app/build", true));
        assert!(f.accept("builds", true));
    }

    #[test]
    fn subtree_exclude_rejects_everything_under_the_pattern() {
        let f = excluding(&["cache/**"]);
        assert!(!f.accept("cache/a.bin", false));
        assert!(!f.accept("x/cache/a/b.bin", false));
        assert!(!f.accept("cache/sub", true));
        assert!(
            f.accept("cache", true),
            "the folder itself is not matched by 'cache/**'"
        );
    }

    #[test]
    fn excluded_subfolders_match_prefix_case_insensitively() {
        let f = filter(&[], &[], &["Photos/Raw", "/Videos/"], false);
        assert!(!f.accept("Photos/Raw", true));
        assert!(!f.accept("photos/raw", true));
        assert!(!f.accept("Photos/Raw/img.dng", false));
        assert!(!f.accept("PHOTOS/RAW/deep/img.dng", false));
        assert!(!f.accept("Videos", true));
        assert!(!f.accept("videos/clip.mp4", false));
        assert!(
            f.accept("Photos/Rawr", true),
            "prefix must end at a path separator"
        );
        assert!(f.accept("Photos", true));
        assert!(f.accept("Photos/Edited/img.jpg", false));
        assert!(f.accept("Other/Photos/Raw/img.dng", false));
    }

    #[test]
    fn blank_or_invalid_patterns_are_ignored() {
        let f = filter(&["   ", ""], &[" "], &["/", " "], false);
        assert!(f.accept("anything.bin", false));
        assert!(f.accept("dir", true));
    }

    #[test]
    fn patterns_are_trimmed() {
        let f = excluding(&["  *.tmp  "]);
        assert!(!f.accept("a.tmp", false));
    }
}
