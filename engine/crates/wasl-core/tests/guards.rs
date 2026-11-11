//! Boundary tests for the safety guards at the SRS FR-F3 defaults (more than 20 files, or more than
//! 5% of a side once at least `PERCENT_FLOOR` files are deleted) and FR-F2 (first merge).

use wasl_core::guards::{GuardConfig, GuardInput, PERCENT_FLOOR, evaluate};
use wasl_core::{BlockKind, BlockSide};

fn drive_deletes(n: u32, side_files: u32) -> GuardInput {
    GuardInput {
        baseline_files: side_files,
        local_files: side_files - n,
        remote_files: side_files,
        delete_local_files: 0,
        delete_remote_files: n,
    }
}

fn device_deletes(n: u32, side_files: u32) -> GuardInput {
    GuardInput {
        baseline_files: side_files,
        local_files: side_files,
        remote_files: side_files - n,
        delete_local_files: n,
        delete_remote_files: 0,
    }
}

#[test]
fn defaults_match_the_srs() {
    let d = GuardConfig::default();
    assert!(d.mass_delete_guard && d.first_merge_guard && !d.approved_once);
    assert_eq!(
        (20, 5),
        (d.mass_delete_min_files, d.mass_delete_threshold_percent)
    );
    assert_eq!(5, PERCENT_FLOOR);
}

#[test]
fn count_rule_blocks_above_twenty_and_not_at_twenty() {
    // 10,000 files on the side, so the 5% rule (500 files) never fires here.
    let cfg = GuardConfig::default();
    assert_eq!(None, evaluate(&cfg, &drive_deletes(20, 10_000)));
    let b = evaluate(&cfg, &drive_deletes(21, 10_000)).expect("21 deletions block");
    assert_eq!(
        (BlockKind::Deletions, Some(BlockSide::Drive), 21, 0),
        (b.kind, b.side, b.count, b.percent)
    );
    assert_eq!(None, evaluate(&cfg, &device_deletes(20, 10_000)));
    assert_eq!(
        Some(BlockSide::Device),
        evaluate(&cfg, &device_deletes(21, 10_000)).unwrap().side
    );
}

#[test]
fn percent_rule_blocks_above_five_percent_and_not_at_five_percent() {
    let cfg = GuardConfig::default();
    // 5 of 100 is exactly 5%: not more than 5%.
    assert_eq!(None, evaluate(&cfg, &drive_deletes(5, 100)));
    // 6 of 100 is 6%.
    let b = evaluate(&cfg, &drive_deletes(6, 100)).expect("6% blocks");
    assert_eq!(6, b.percent);
    // 5 of 99 is 5.05%: more than 5%.
    assert!(evaluate(&cfg, &drive_deletes(5, 99)).is_some());
}

#[test]
fn percent_rule_ignores_batches_below_the_floor() {
    let cfg = GuardConfig::default();
    // 4 of 10 is 40% but only 4 files.
    assert_eq!(None, evaluate(&cfg, &drive_deletes(4, 10)));
    // 5 of 10 reaches the floor.
    assert_eq!(50, evaluate(&cfg, &drive_deletes(5, 10)).unwrap().percent);
}

#[test]
fn the_side_total_is_the_larger_of_baseline_and_current_files() {
    let cfg = GuardConfig::default();
    // 6 deletions; the baseline knew 100 files but Drive now lists 200: 6 of 200 = 3%.
    let t = GuardInput {
        baseline_files: 100,
        local_files: 94,
        remote_files: 200,
        delete_local_files: 0,
        delete_remote_files: 6,
    };
    assert_eq!(None, evaluate(&cfg, &t));
}

#[test]
fn wipe_rule_beats_thresholds_and_needs_a_previous_sync() {
    let cfg = GuardConfig::default();
    let wiped = GuardInput {
        baseline_files: 1,
        local_files: 0,
        remote_files: 1,
        delete_remote_files: 1,
        ..Default::default()
    };
    assert_eq!(BlockKind::Wipe, evaluate(&cfg, &wiped).unwrap().kind);
    let never_synced = GuardInput {
        baseline_files: 0,
        ..wiped
    };
    assert_eq!(None, evaluate(&cfg, &never_synced));
}

#[test]
fn device_side_block_takes_precedence_over_drive_side() {
    let cfg = GuardConfig::default();
    let both = GuardInput {
        baseline_files: 100,
        local_files: 70,
        remote_files: 70,
        delete_local_files: 30,
        delete_remote_files: 30,
    };
    assert_eq!(Some(BlockSide::Device), evaluate(&cfg, &both).unwrap().side);
}

#[test]
fn first_merge_needs_both_sides_non_empty_and_no_baseline() {
    let cfg = GuardConfig::default();
    let merge = GuardInput {
        baseline_files: 0,
        local_files: 1,
        remote_files: 1,
        ..Default::default()
    };
    assert_eq!(BlockKind::FirstMerge, evaluate(&cfg, &merge).unwrap().kind);
    assert_eq!(
        None,
        evaluate(
            &cfg,
            &GuardInput {
                local_files: 0,
                ..merge
            }
        )
    );
    assert_eq!(
        None,
        evaluate(
            &cfg,
            &GuardInput {
                remote_files: 0,
                ..merge
            }
        )
    );
    assert_eq!(
        None,
        evaluate(
            &cfg,
            &GuardInput {
                baseline_files: 1,
                ..merge
            }
        )
    );
}

#[test]
fn approval_lifts_every_guard_once() {
    let cfg = GuardConfig {
        approved_once: true,
        ..GuardConfig::default()
    };
    assert_eq!(None, evaluate(&cfg, &drive_deletes(500, 1000)));
    let merge = GuardInput {
        baseline_files: 0,
        local_files: 1,
        remote_files: 1,
        ..Default::default()
    };
    assert_eq!(None, evaluate(&cfg, &merge));
}

#[test]
fn disabled_mass_delete_guard_still_allows_the_first_merge_guard() {
    let cfg = GuardConfig {
        mass_delete_guard: false,
        ..GuardConfig::default()
    };
    assert_eq!(None, evaluate(&cfg, &drive_deletes(500, 1000)));
    let merge = GuardInput {
        baseline_files: 0,
        local_files: 1,
        remote_files: 1,
        ..Default::default()
    };
    assert!(evaluate(&cfg, &merge).is_some());
}
