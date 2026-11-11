//! Safety guards that stop a plan and ask (SRS FR-F2, FR-F3).

use crate::model::{BlockKind, BlockSide};
use serde::{Deserialize, Serialize};

/// The percentage rule only applies to batches of at least this many files, so deleting one file
/// from a small folder doesn't stop the sync.
pub const PERCENT_FLOOR: u32 = 5;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct GuardConfig {
    /// Stop when a plan deletes more than `mass_delete_min_files` files or more than
    /// `mass_delete_threshold_percent` of a side.
    pub mass_delete_guard: bool,
    pub mass_delete_min_files: u32,
    pub mass_delete_threshold_percent: u32,
    /// Stop before the first sync merges two folders that both already have files.
    pub first_merge_guard: bool,
    /// The user approved this run once (from the preview or the "Needs you" prompt).
    pub approved_once: bool,
}

impl Default for GuardConfig {
    /// The defaults of SRS FR-F3: more than 20 files or more than 5%.
    fn default() -> Self {
        GuardConfig {
            mass_delete_guard: true,
            mass_delete_min_files: 20,
            mass_delete_threshold_percent: 5,
            first_merge_guard: true,
            approved_once: false,
        }
    }
}

/// File counts a plan is judged on. Folders are not counted.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct GuardInput {
    pub baseline_files: u32,
    pub local_files: u32,
    pub remote_files: u32,
    pub delete_local_files: u32,
    pub delete_remote_files: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Block {
    pub kind: BlockKind,
    pub side: Option<BlockSide>,
    pub count: u32,
    pub percent: u32,
    pub reason: String,
}

/// Integer percentage, 0 when `total` is 0.
pub fn pct(n: u32, total: u32) -> u32 {
    if total == 0 {
        0
    } else {
        (u64::from(n) * 100 / u64::from(total)) as u32
    }
}

/// The guard that stops this plan, if any. Later rules take precedence, as in 1.1.4: a wipe beats
/// a mass deletion, and a device-side deletion beats a Drive-side one.
pub fn evaluate(cfg: &GuardConfig, t: &GuardInput) -> Option<Block> {
    if cfg.approved_once {
        return None;
    }
    let mut block = None;
    if cfg.mass_delete_guard {
        let exceeds = |n: u32, total: u32| {
            n > cfg.mass_delete_min_files
                || (n >= PERCENT_FLOOR
                    && total > 0
                    && u64::from(n) * 100 > u64::from(cfg.mass_delete_threshold_percent) * u64::from(total))
        };
        let remote_total = t.baseline_files.max(t.remote_files);
        let local_total = t.baseline_files.max(t.local_files);
        if exceeds(t.delete_remote_files, remote_total) {
            let (n, p) = (t.delete_remote_files, pct(t.delete_remote_files, remote_total));
            block = Some(Block {
                kind: BlockKind::Deletions,
                side: Some(BlockSide::Drive),
                count: n,
                percent: p,
                reason: format!(
                    "This sync would delete {n} files on Drive, {p}% of the folder. Drive still has every file until you decide."
                ),
            });
        }
        if exceeds(t.delete_local_files, local_total) {
            let (n, p) = (t.delete_local_files, pct(t.delete_local_files, local_total));
            block = Some(Block {
                kind: BlockKind::Deletions,
                side: Some(BlockSide::Device),
                count: n,
                percent: p,
                reason: format!(
                    "This sync would delete {n} files on this device, {p}% of the folder. This device still has every file until you decide."
                ),
            });
        }
        if t.baseline_files > 0 && t.local_files == 0 && t.delete_remote_files > 0 {
            block = Some(Block {
                kind: BlockKind::Wipe,
                side: Some(BlockSide::Drive),
                count: t.delete_remote_files,
                percent: 100,
                reason: "The folder on this device is empty but was synced before. Wasl won't empty Drive to match it."
                    .to_owned(),
            });
        }
        if t.baseline_files > 0 && t.remote_files == 0 && t.delete_local_files > 0 {
            block = Some(Block {
                kind: BlockKind::Wipe,
                side: Some(BlockSide::Device),
                count: t.delete_local_files,
                percent: 100,
                reason: "The Drive folder is empty but was synced before. Wasl won't empty this device's folder to match it."
                    .to_owned(),
            });
        }
    }
    if block.is_none()
        && cfg.first_merge_guard
        && t.baseline_files == 0
        && t.local_files > 0
        && t.remote_files > 0
    {
        block = Some(Block {
            kind: BlockKind::FirstMerge,
            side: None,
            count: 0,
            percent: 0,
            reason: first_merge_reason(t),
        });
    }
    block
}

/// What a first sync of two folders that both have files does. Two-way and one-way methods delete
/// nothing; a mirror makes one side match the other, and says how many files that removes.
fn first_merge_reason(t: &GuardInput) -> String {
    let mut s = format!(
        "The first sync merges two folders that both have files: {} on this device and {} on Drive.",
        t.local_files, t.remote_files
    );
    match (t.delete_local_files, t.delete_remote_files) {
        (0, 0) => s.push_str(" Nothing is deleted."),
        (here, drive) => {
            if here > 0 {
                s.push_str(&format!(
                    " {here} files only on this device go to its recycle bin, so it matches Drive."
                ));
            }
            if drive > 0 {
                s.push_str(&format!(
                    " {drive} files only on Drive go to Drive's bin, so it matches this device."
                ));
            }
        }
    }
    s
}
