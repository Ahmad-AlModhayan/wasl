//! Files on demand through the cache: only what is read is fetched, seeking costs one block,
//! sequential readers get read-ahead, the cache keeps to its budget and pins are never evicted.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use wasl_vfs::{BLOCK, BlockCache, FileRef, Hydrator, ReadAhead, Result, Source, VfsError};

/// Content is a function of (version, offset); every request is recorded.
#[derive(Default)]
struct Fake {
    calls: Mutex<Vec<(u64, u64)>>,
    /// Requests are widened to multiples of this (a chunked source), when set.
    chunk: Option<u64>,
    short: bool,
}

fn byte(version: &str, at: u64) -> u8 {
    (at.wrapping_mul(31) ^ version.len() as u64 ^ (at >> 20)) as u8
}

impl Fake {
    fn calls(&self) -> Vec<(u64, u64)> {
        self.calls.lock().unwrap().clone()
    }
    fn fetched(&self) -> u64 {
        self.calls().iter().map(|(_, l)| l).sum()
    }
}

#[async_trait]
impl Source for Fake {
    async fn read(&self, f: &FileRef, offset: u64, len: u64) -> Result<Vec<u8>> {
        self.calls.lock().unwrap().push((offset, len));
        let end = (offset + len).min(f.size);
        let mut v: Vec<u8> = (offset..end).map(|i| byte(&f.version, i)).collect();
        if self.short {
            v.pop();
        }
        Ok(v)
    }

    fn natural_range(&self, _f: &FileRef, offset: u64, len: u64) -> (u64, u64) {
        match self.chunk {
            Some(c) => {
                let from = offset / c * c;
                (from, (offset + len).div_ceil(c) * c - from)
            }
            None => (offset, len),
        }
    }
}

fn file(size: u64, version: &str) -> FileRef {
    FileRef {
        id: "file-1".into(),
        version: version.into(),
        size,
    }
}

fn expected(f: &FileRef, offset: u64, len: u64) -> Vec<u8> {
    (offset..(offset + len).min(f.size))
        .map(|i| byte(&f.version, i))
        .collect()
}

fn setup(limit: u64, fake: Fake) -> (Hydrator, Arc<Fake>, Arc<BlockCache>, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let cache = Arc::new(BlockCache::open(dir.path().join("cache"), limit).unwrap());
    let fake = Arc::new(fake);
    let h = Hydrator::new(fake.clone(), cache.clone(), ReadAhead { blocks: 4, after: 2 });
    (h, fake, cache, dir)
}

#[tokio::test]
async fn a_read_fetches_its_blocks_once() {
    let (h, fake, _c, _d) = setup(1 << 30, Fake::default());
    let f = file(5 * BLOCK + 123, "v1");
    // Across a block boundary.
    let got = h.read(&f, BLOCK - 10, 20).await.unwrap();
    assert_eq!(expected(&f, BLOCK - 10, 20), got);
    assert_eq!(
        vec![(0, 2 * BLOCK)],
        fake.calls(),
        "the two blocks, in one request"
    );
    let again = h.read(&f, BLOCK - 5, 10).await.unwrap();
    assert_eq!(expected(&f, BLOCK - 5, 10), again);
    assert_eq!(1, fake.calls().len(), "the second read came from the cache");
    // The tail block is short; reads past the end are cut, and beyond it refused.
    let tail = h.read(&f, 5 * BLOCK + 100, 1000).await.unwrap();
    assert_eq!(expected(&f, 5 * BLOCK + 100, 23), tail);
    assert!(matches!(
        h.read(&f, f.size + 1, 1).await,
        Err(VfsError::OutOfRange)
    ));
    assert!(h.read(&f, f.size, 10).await.unwrap().is_empty());
}

#[tokio::test]
async fn seeking_in_a_huge_file_costs_one_block() {
    // FR-O2: open a 20 GB file and seek: one block, not the file.
    let (h, fake, _c, _d) = setup(1 << 30, Fake::default());
    let f = file(20 << 30, "v1");
    let at = 15 << 30;
    assert_eq!(expected(&f, at + 7, 100), h.read(&f, at + 7, 100).await.unwrap());
    assert_eq!(BLOCK, fake.fetched());
}

#[tokio::test]
async fn sequential_readers_get_read_ahead() {
    let (h, fake, _c, _d) = setup(1 << 30, Fake::default());
    let f = file(64 * BLOCK, "v1");
    for i in 0..3 {
        h.read(&f, i * BLOCK, BLOCK).await.unwrap();
    }
    // The third read in order also fetched four blocks ahead.
    assert_eq!(7 * BLOCK, fake.fetched());
    let before = fake.calls().len();
    for i in 3..7 {
        assert_eq!(
            expected(&f, i * BLOCK, BLOCK),
            h.read(&f, i * BLOCK, BLOCK).await.unwrap()
        );
    }
    assert!(fake.calls().len() <= before + 4);
    // A jump resets it.
    let n = fake.fetched();
    h.read(&f, 40 * BLOCK, 10).await.unwrap();
    assert_eq!(n + BLOCK, fake.fetched());
}

#[tokio::test]
async fn the_cache_keeps_to_its_budget_and_never_evicts_pins() {
    let (h, fake, cache, _d) = setup(3 * BLOCK, Fake::default());
    let pinned = FileRef {
        id: "keep".into(),
        version: "v1".into(),
        size: 5 * BLOCK,
    };
    h.pin(&pinned).await.unwrap();
    assert!(h.complete(&pinned));
    let other = file(10 * BLOCK, "v1");
    for i in 0..10 {
        h.read(&other, i * BLOCK, 1).await.unwrap();
    }
    let (total, pinned_bytes) = cache.usage();
    assert_eq!(5 * BLOCK, pinned_bytes, "every pinned block stays");
    assert!(
        total - pinned_bytes <= 3 * BLOCK,
        "unpinned blocks keep to the budget"
    );
    assert!(h.complete(&pinned));
    let calls = fake.calls().len();
    h.read(&pinned, 0, 5 * BLOCK).await.unwrap();
    assert_eq!(
        calls,
        fake.calls().len(),
        "a pinned file reads without the network"
    );

    // Unpinned, it is ordinary again.
    h.unpin(&pinned).unwrap();
    assert_eq!(0, cache.usage().1);
    assert!(cache.usage().0 <= 3 * BLOCK);
}

#[tokio::test]
async fn a_lower_limit_evicts_now_and_pins_stay() {
    let (h, _fake, cache, _d) = setup(8 * BLOCK, Fake::default());
    let pinned = FileRef {
        id: "keep".into(),
        version: "v1".into(),
        size: 2 * BLOCK,
    };
    h.pin(&pinned).await.unwrap();
    let other = file(6 * BLOCK, "v1");
    h.read(&other, 0, 6 * BLOCK).await.unwrap();
    assert_eq!(8 * BLOCK, cache.usage().0);

    cache.set_limit(BLOCK);
    assert_eq!(BLOCK, cache.limit());
    let (total, pinned_bytes) = cache.usage();
    assert_eq!(2 * BLOCK, pinned_bytes);
    assert!(total - pinned_bytes <= BLOCK);
    assert!(h.complete(&pinned));
}

#[tokio::test]
async fn a_new_version_never_reads_old_blocks_and_keeps_its_pin() {
    let (h, fake, cache, _d) = setup(1 << 30, Fake::default());
    let v1 = file(2 * BLOCK, "v1");
    h.pin(&v1).await.unwrap();
    let v2 = file(2 * BLOCK, "v22");
    assert_eq!(expected(&v2, 0, 10), h.read(&v2, 0, 10).await.unwrap());
    assert_ne!(expected(&v1, 0, 10), expected(&v2, 0, 10));
    assert!(cache.is_pinned("file-1"));
    let n = fake.fetched();
    h.pin(&v2).await.unwrap();
    assert!(fake.fetched() > n);
    assert!(h.complete(&v2));
}

#[tokio::test]
async fn forgetting_unpinned_blocks_keeps_only_pins_also_after_a_restart() {
    let dir = tempfile::tempdir().unwrap();
    let kept = file(2 * BLOCK, "v1");
    let other = FileRef {
        id: "file-2".into(),
        version: "v1".into(),
        size: 3 * BLOCK,
    };
    let cache = Arc::new(BlockCache::open(dir.path().join("c"), 1 << 30).unwrap());
    let h = Hydrator::new(Arc::new(Fake::default()), cache.clone(), ReadAhead::default());
    h.pin(&kept).await.unwrap();
    h.read(&other, 0, 3 * BLOCK).await.unwrap();
    assert_eq!((5 * BLOCK, 2 * BLOCK), cache.usage());
    cache.forget_unpinned();
    assert_eq!((2 * BLOCK, 2 * BLOCK), cache.usage());
    assert!(h.complete(&kept) && !h.complete(&other));
    drop((h, cache));
    // Gone from the disk too, not only from the index.
    let cache = BlockCache::open(dir.path().join("c"), 1 << 30).unwrap();
    assert_eq!((2 * BLOCK, 2 * BLOCK), cache.usage());
    cache.forget_unpinned();
    assert_eq!((2 * BLOCK, 2 * BLOCK), cache.usage());
}

#[tokio::test]
async fn blocks_and_pins_survive_a_restart() {
    let dir = tempfile::tempdir().unwrap();
    let f = file(3 * BLOCK, "v1");
    {
        let cache = Arc::new(BlockCache::open(dir.path().join("c"), 1 << 30).unwrap());
        let h = Hydrator::new(Arc::new(Fake::default()), cache, ReadAhead::default());
        h.pin(&f).await.unwrap();
    }
    let cache = Arc::new(BlockCache::open(dir.path().join("c"), 0).unwrap());
    assert_eq!(
        (3 * BLOCK, 3 * BLOCK),
        cache.usage(),
        "pinned blocks kept even with no budget"
    );
    let fake = Arc::new(Fake::default());
    let h = Hydrator::new(fake.clone(), cache, ReadAhead::default());
    assert_eq!(
        expected(&f, 0, 3 * BLOCK),
        h.read(&f, 0, 3 * BLOCK).await.unwrap()
    );
    assert!(fake.calls().is_empty());
}

#[tokio::test]
async fn a_chunked_source_is_fetched_by_whole_chunks() {
    let (h, fake, _c, _d) = setup(
        1 << 30,
        Fake {
            chunk: Some(4 * BLOCK),
            ..Fake::default()
        },
    );
    let f = file(20 * BLOCK, "v1");
    assert_eq!(
        expected(&f, 5 * BLOCK + 3, 10),
        h.read(&f, 5 * BLOCK + 3, 10).await.unwrap()
    );
    assert_eq!(
        vec![(4 * BLOCK, 4 * BLOCK)],
        fake.calls(),
        "the chunk around the read"
    );
    h.read(&f, 7 * BLOCK, 10).await.unwrap();
    assert_eq!(1, fake.calls().len(), "the rest of that chunk is cached");
}

#[tokio::test]
async fn a_short_answer_is_an_error_not_a_hole() {
    let (h, _fake, cache, _d) = setup(
        1 << 30,
        Fake {
            short: true,
            ..Fake::default()
        },
    );
    let f = file(2 * BLOCK, "v1");
    assert!(matches!(h.read(&f, 0, 10).await, Err(VfsError::Integrity(_))));
    assert_eq!(0, cache.usage().0, "nothing half-read is cached");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn readers_fetching_the_same_block_at_once_all_succeed() {
    let (h, _fake, _c, _d) = setup(1 << 30, Fake::default());
    let h = Arc::new(h);
    let f = file(4 * BLOCK, "v1");
    let tasks: Vec<_> = (0..16)
        .map(|_| {
            let (h, f) = (h.clone(), f.clone());
            tokio::spawn(async move { h.read(&f, BLOCK + 5, 100).await })
        })
        .collect();
    for t in tasks {
        assert_eq!(expected(&f, BLOCK + 5, 100), t.await.unwrap().unwrap());
    }
}
