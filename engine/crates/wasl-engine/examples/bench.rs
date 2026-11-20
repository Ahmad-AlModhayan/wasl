//! Reproducible throughput and latency benchmarks for the Wasl sync engine.
//!
//! Run in release mode:
//! ```sh
//! cd engine
//! cargo run --release -p wasl-engine --example bench
//! ```
//!
//! Covers five subsystems with deterministic synthetic workloads:
//! 1. Three-way Drive merge planner (`wasl-core::Planner`) on 100,000 keys.
//! 2. N-way version-vector planner (`wasl-core::nway`) on 100,000 entries.
//! 3. Two-tier FastCDC + BLAKE3 chunking and delta amplification (`wasl-chunk`).
//! 4. Vault v2.1 encryption, decryption, padding overhead, and random-access range read (`wasl-crypto`).
//! 5. End-to-end P2P sync over QUIC + TLS 1.3 (loopback) with adaptive zstd compression,
//!    incremental mid-file delta sync, and 1,000 small files through safe-apply + SQLite (`wasl-engine` + `wasl-net`).

use std::collections::HashMap;
use std::io;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::{Duration, Instant};

use wasl_chunk::{ChunkIndex, ChunkParams, chunk_reader, plan_fetch};
use wasl_core::guards::GuardConfig;
use wasl_core::nway::{ConflictMode, Index, Scanned, plan_with};
use wasl_core::{
    BaselineEntry, ConflictPolicy, DocsFormat, LocalEntry, PlanInput, Planner, PlannerConfig, RemoteEntry,
    Route, SyncMode,
};
use wasl_crypto::v2::{
    ChunkId, FileId, Format, RangeReader, VaultKeysV2, decrypt_file, encrypt_stream, encrypt_update,
    plan_update,
};
use wasl_engine::{Clock, DeviceService, DeviceSync};
use wasl_fs::{LocalFs, StdFs};
use wasl_net::compress::{LEVEL_INTERNET, LEVEL_LAN, LEVEL_METERED};
use wasl_net::{DeviceKey, Endpoint, ShareId, Trust};
use wasl_store::{Pair, SharedStore, Store};

type BenchResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

const MIB: usize = 1 << 20;
const KIB: usize = 1 << 10;
const SHARE: ShareId = ShareId([0x42; 16]);

struct BenchClock(AtomicI64);

impl Clock for BenchClock {
    fn now_ms(&self) -> i64 {
        self.0.fetch_add(1000, Ordering::SeqCst)
    }
}

fn median(mut samples: Vec<Duration>) -> Duration {
    samples.sort();
    samples[samples.len() / 2]
}

fn mib_per_sec(bytes: usize, elapsed: Duration) -> f64 {
    let secs = elapsed.as_secs_f64();
    if secs == 0.0 {
        return 0.0;
    }
    (bytes as f64 / MIB as f64) / secs
}

/// Deterministic text-like corpus (compresses like source code / Markdown / JSON).
fn prose(len: usize, seed: u64) -> Vec<u8> {
    const WORDS: &[&[u8]] = &[
        b"sync",
        b"folder",
        b"wasl",
        b"device",
        b"chunk",
        b"drive",
        b"vault",
        b"merge",
        b"vector",
        b"commit",
        b"journal",
        b"baseline",
        b"conflict",
        b"stream",
        b"verify",
        b"snapshot",
    ];
    let mut x = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
    let mut out = Vec::with_capacity(len + 16);
    while out.len() < len {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        out.extend_from_slice(WORDS[(x as usize) % WORDS.len()]);
        out.push(if x.is_multiple_of(11) { b'\n' } else { b' ' });
    }
    out.truncate(len);
    out
}

/// Deterministic high-entropy bytes (incompressible stand-in for photos / video / archives).
fn noise(len: usize, seed: u64) -> Vec<u8> {
    let mut x = seed.wrapping_mul(0xBF58_476D_1CE4_E5B9) | 1;
    let mut out = Vec::with_capacity(len + 8);
    while out.len() < len {
        x ^= x >> 30;
        x = x.wrapping_mul(0xBF58_476D_1CE4_E5B9);
        x ^= x >> 27;
        x = x.wrapping_mul(0x94D0_49BB_1331_11EB);
        x ^= x >> 31;
        out.extend_from_slice(&x.to_le_bytes());
    }
    out.truncate(len);
    out
}

struct BenchDevice {
    key: DeviceKey,
    root: PathBuf,
    sync: DeviceSync,
    service: Arc<DeviceService>,
    _dir: tempfile::TempDir,
}

impl BenchDevice {
    fn new(seed: u8) -> BenchResult<Self> {
        let dir = tempfile::tempdir()?;
        let root = dir.path().join("folder");
        let bin = dir.path().join("bin");
        std::fs::create_dir_all(&root)?;
        std::fs::create_dir_all(&bin)?;
        let store = SharedStore::new(Store::open_in_memory()?);
        let root_str = root.to_str().ok_or("non-utf8 tempdir path")?;
        let mut pair = Pair::new("bench", "Bench", root_str);
        pair.route = Route::Devices;
        store.lock().create_pair(&pair)?;
        store.lock().set_share_id("bench", &SHARE.to_hex())?;
        for peer_seed in 1..=4u8 {
            let id = DeviceKey::from_seed(&[peer_seed; 32]).id().to_string();
            store.lock().set_device_known(&id, true)?;
        }
        let fs: Arc<dyn LocalFs> = Arc::new(StdFs::new(&root, &bin));
        let key = DeviceKey::from_seed(&[seed; 32]);
        let clock = Arc::new(BenchClock(AtomicI64::new(1_800_000_000_000)));
        let sync = DeviceSync::new(store.clone(), fs.clone(), pair, SHARE, key.id(), clock);
        let open = fs.clone();
        let service = Arc::new(DeviceService::new(store, move |_| Ok(open.clone()), |_, _, _| {}));
        Ok(BenchDevice {
            key,
            root,
            sync,
            service,
            _dir: dir,
        })
    }

    fn write(&self, rel: &str, bytes: &[u8]) -> BenchResult {
        let p = self.root.join(rel);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(p, bytes)?;
        Ok(())
    }
}

async fn quic_pair(
    a: &BenchDevice,
    b: &BenchDevice,
) -> BenchResult<(Endpoint, Endpoint, wasl_net::QuicLink)> {
    let only = |k: &DeviceKey| -> Trust {
        let pk = k.public();
        Arc::new(move |p: &[u8; 32]| *p == pk)
    };
    let loopback: SocketAddr = "127.0.0.1:0".parse()?;
    let server = Endpoint::bind(loopback, &a.key, only(&b.key))?;
    server.serve(a.service.clone());
    let client = Endpoint::bind(loopback, &b.key, only(&a.key))?;
    let link = client.connect(server.local_addr()?, Some(a.key.id())).await?;
    Ok((server, client, link))
}

fn bench_three_way_planner() {
    let count = 100_000u32;
    let mut input = PlanInput::default();
    for i in 0..count {
        let key = format!("d{}/sub{}/file{i}.bin", i % 100, i % 1000);
        let changed = i % 50 == 0;
        input.baseline.insert(
            key.clone(),
            BaselineEntry {
                key: key.clone(),
                local_rel: key.clone(),
                is_dir: false,
                local_size: 1000,
                local_mtime: 1,
                local_hash: Some("h".into()),
                remote_id: Some(format!("id{i}")),
                remote_md5: Some("h".into()),
                remote_size: Some(1000),
                remote_mtime: Some(1),
                remote_version: Some(1),
                export_mime: None,
            },
        );
        input.local.insert(
            key.clone(),
            LocalEntry {
                rel: key.clone(),
                is_dir: false,
                size: 1000,
                mtime: if changed { 2 } else { 1 },
            },
        );
        input.remote.insert(
            key.clone(),
            RemoteEntry {
                key: key.clone(),
                id: format!("id{i}"),
                name: format!("file{i}.bin"),
                is_dir: false,
                size: Some(1000),
                md5: Some("h".into()),
                mtime: 1,
                version: Some(1),
                google_mime: None,
                parent_id: None,
            },
        );
    }
    let planner = Planner::new(PlannerConfig {
        mode: SyncMode::TwoWay,
        conflict_policy: ConflictPolicy::KeepBoth,
        propagate_deletions: true,
        max_file_size: 0,
        docs_export: true,
        docs_format: DocsFormat::Office,
        guards: GuardConfig::default(),
        name_rules: Default::default(),
    });
    let mut samples = Vec::with_capacity(5);
    let mut uploads = 0;
    for _ in 0..5 {
        let t = Instant::now();
        let plan = planner.plan_with(&input, |_| "h2".into());
        samples.push(t.elapsed());
        uploads = plan.summary.uploads;
    }
    let med = median(samples);
    let keys_per_sec = f64::from(count) / med.as_secs_f64();
    println!(
        "| Three-way Drive planner (`wasl-core`) | 100,000 keys (2% changed) | {:.1} ms | {:.0}k keys/s ({uploads} uploads) |",
        med.as_secs_f64() * 1_000.0,
        keys_per_sec / 1_000.0
    );
}

fn bench_nway_planner() {
    let count = 100_000u32;
    let mut local = Index::new(1);
    let mut remote_idx = Index::new(2);
    let base_scan: Vec<Scanned> = (0..count)
        .map(|i| Scanned {
            key: format!("d{}/sub{}/file{i}.bin", i % 100, i % 1000),
            is_dir: false,
            size: 1000,
            mtime: 1_000,
            hash: Some(format!("h{i:08x}")),
        })
        .collect();
    local.apply_scan(&base_scan, 1_000);
    for (k, entry) in &local.entries {
        remote_idx.record(k, entry.version.clone());
    }
    let mut remote_scan = base_scan;
    for (i, item) in remote_scan.iter_mut().enumerate() {
        if i % 50 == 0 {
            item.mtime = 2_000;
            item.hash = Some(format!("new{i:08x}"));
        }
    }
    remote_idx.apply_scan(&remote_scan, 2_000);
    let remote_entries: Vec<(String, _)> = remote_idx
        .entries
        .iter()
        .map(|(k, e)| (k.clone(), e.version.clone()))
        .collect();

    let mut samples = Vec::with_capacity(5);
    let mut steps_count = 0;
    for _ in 0..5 {
        let t = Instant::now();
        let steps = plan_with(&local, &remote_entries, ConflictMode::KeepBoth);
        samples.push(t.elapsed());
        steps_count = steps.len();
    }
    let med = median(samples);
    let keys_per_sec = f64::from(count) / med.as_secs_f64();
    println!(
        "| N-way version-vector planner (`wasl-core::nway`) | 100,000 keys (2% changed) | {:.1} ms | {:.0}k keys/s ({steps_count} steps) |",
        med.as_secs_f64() * 1_000.0,
        keys_per_sec / 1_000.0
    );
}

fn bench_chunking() -> BenchResult {
    // Medium tier (16/64/256 KiB) on 16 MiB
    let buf16 = prose(16 * MIB, 11);
    let mut samples = Vec::with_capacity(5);
    let mut med_chunks = 0usize;
    for _ in 0..5 {
        let t = Instant::now();
        let list = chunk_reader(&buf16[..], &ChunkParams::MEDIUM)?;
        samples.push(t.elapsed());
        med_chunks = list.chunks.len();
    }
    let med = median(samples);
    println!(
        "| FastCDC + BLAKE3 medium tier (`wasl-chunk`) | 16 MiB (16/64/256 KiB) | {:.1} ms | {:.0} MiB/s ({med_chunks} chunks) |",
        med.as_secs_f64() * 1_000.0,
        mib_per_sec(buf16.len(), med)
    );

    // Large tier (1/4/16 MiB) on 64 MiB
    let buf64 = noise(64 * MIB, 22);
    let mut samples = Vec::with_capacity(5);
    let mut large_chunks = 0usize;
    for _ in 0..5 {
        let t = Instant::now();
        let list = chunk_reader(&buf64[..], &ChunkParams::DEFAULT)?;
        samples.push(t.elapsed());
        large_chunks = list.chunks.len();
    }
    let med = median(samples);
    println!(
        "| FastCDC + BLAKE3 large tier (`wasl-chunk`) | 64 MiB (1/4/16 MiB) | {:.1} ms | {:.0} MiB/s ({large_chunks} chunks) |",
        med.as_secs_f64() * 1_000.0,
        mib_per_sec(buf64.len(), med)
    );

    // Delta amplification: 4 KiB mid-file edit in 10 MiB file (medium tier)
    let orig10 = prose(10 * MIB, 33);
    let mut edited10 = orig10.clone();
    let patch4k = noise(4 * KIB, 34);
    edited10[5 * MIB..5 * MIB + 4 * KIB].copy_from_slice(&patch4k);
    let old_list = chunk_reader(&orig10[..], &ChunkParams::MEDIUM)?;
    let new_list = chunk_reader(&edited10[..], &ChunkParams::MEDIUM)?;
    let mut idx = ChunkIndex::new();
    idx.add("doc.md", &old_list);
    let plan = plan_fetch(&new_list, &idx);
    let fetch_bytes = plan.bytes_to_fetch();
    let fetch_chunks = plan.to_fetch().len();
    let chunk_word = if fetch_chunks == 1 { "chunk" } else { "chunks" };
    println!(
        "| FastCDC delta (4 KiB edit in 10 MiB, medium tier) | 10 MiB file | {:.1} KiB fetched | {:.2}% of file ({fetch_chunks} {chunk_word} fetched) |",
        fetch_bytes as f64 / KIB as f64,
        100.0 * fetch_bytes as f64 / (10 * MIB) as f64,
    );

    // Delta amplification: 1 MiB mid-file edit in 64 MiB file (large tier)
    let mut edited64 = buf64.clone();
    let patch1m = noise(MIB, 35);
    edited64[32 * MIB..33 * MIB].copy_from_slice(&patch1m);
    let old64 = chunk_reader(&buf64[..], &ChunkParams::DEFAULT)?;
    let new64 = chunk_reader(&edited64[..], &ChunkParams::DEFAULT)?;
    let mut idx64 = ChunkIndex::new();
    idx64.add("archive.bin", &old64);
    let plan64 = plan_fetch(&new64, &idx64);
    let fetch64_bytes = plan64.bytes_to_fetch();
    let fetch64_chunks = plan64.to_fetch().len();
    let chunk64_word = if fetch64_chunks == 1 { "chunk" } else { "chunks" };
    println!(
        "| FastCDC delta (1 MiB edit in 64 MiB, large tier) | 64 MiB file | {:.2} MiB fetched | {:.2}% of file ({fetch64_chunks} {chunk64_word} fetched) |",
        fetch64_bytes as f64 / MIB as f64,
        100.0 * fetch64_bytes as f64 / (64 * MIB) as f64,
    );
    Ok(())
}

fn bench_vault_v21() -> BenchResult {
    let keys = VaultKeysV2::from_ikm(&[0x5A; 32]);
    let file_id = FileId([0x11; 16]);
    let data = noise(64 * MIB, 44);
    let mut sealed_store: HashMap<ChunkId, Vec<u8>> = HashMap::new();

    // Encrypt 64 MiB with Vault 2.1 (keyed FastCDC + Padme padding + AES-256-GCM)
    let mut enc_samples = Vec::with_capacity(3);
    let mut manifest = None;
    let mut sealed_bytes = 0usize;
    for _ in 0..3 {
        sealed_store.clear();
        let mut total_sealed = 0usize;
        let t = Instant::now();
        let m = encrypt_stream(
            &keys,
            Format::V2_1,
            file_id,
            1_800_000_000_000,
            &data[..],
            |pending| {
                let sealed = pending.seal()?;
                total_sealed += sealed.len();
                sealed_store.insert(*pending.id(), sealed);
                Ok(())
            },
        )?;
        enc_samples.push(t.elapsed());
        sealed_bytes = total_sealed;
        manifest = Some(m);
    }
    let manifest = manifest.ok_or("vault encryption produced no manifest")?;
    let enc_med = median(enc_samples);
    let overhead_pct = 100.0 * (sealed_bytes as f64 - data.len() as f64) / data.len() as f64;
    println!(
        "| Vault v2.1 encrypt (`wasl-crypto`, keyed CDC + Padme + AES-256-GCM) | 64 MiB ({} chunks) | {:.1} ms | {:.0} MiB/s (+{:.2}% ciphertext/pad overhead) |",
        manifest.chunks.len(),
        enc_med.as_secs_f64() * 1_000.0,
        mib_per_sec(data.len(), enc_med),
        overhead_pct
    );

    // Decrypt 64 MiB (full streaming + BLAKE3 verification)
    let mut dec_samples = Vec::with_capacity(3);
    for _ in 0..3 {
        let mut sink = io::sink();
        let t = Instant::now();
        let n = decrypt_file(
            &keys,
            &manifest,
            |id| {
                sealed_store
                    .get(id)
                    .cloned()
                    .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "missing chunk"))
            },
            &mut sink,
        )?;
        dec_samples.push(t.elapsed());
        if n != data.len() as u64 {
            return Err("vault decrypt size mismatch".into());
        }
    }
    let dec_med = median(dec_samples);
    println!(
        "| Vault v2.1 decrypt + verify (`wasl-crypto`) | 64 MiB | {:.1} ms | {:.0} MiB/s |",
        dec_med.as_secs_f64() * 1_000.0,
        mib_per_sec(data.len(), dec_med)
    );

    // Random-access range read at 48 MiB offset (64 KiB read)
    let reader = RangeReader::new(&keys, &manifest);
    let mut fetched_chunks = 0usize;
    let mut fetched_bytes = 0usize;
    let t = Instant::now();
    let slice = reader.read(48 * MIB as u64, 64 * KIB as u64, |id| {
        let bytes = sealed_store
            .get(id)
            .cloned()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "missing chunk"))?;
        fetched_chunks += 1;
        fetched_bytes += bytes.len();
        Ok(bytes)
    })?;
    let seek_elapsed = t.elapsed();
    if slice != data[48 * MIB..48 * MIB + 64 * KIB] {
        return Err("vault range read mismatch".into());
    }
    println!(
        "| Vault v2.1 random-access read (64 KiB @ 48 MiB offset) | 64 MiB file | {:.2} ms | {fetched_chunks}/{} chunks fetched ({:.2} MiB) |",
        seek_elapsed.as_secs_f64() * 1_000.0,
        manifest.chunks.len(),
        fetched_bytes as f64 / MIB as f64
    );

    // Vault v2.1 delta update: 1 MiB mid-file edit in 64 MiB
    let mut edited = data;
    let patch = noise(MIB, 45);
    edited[32 * MIB..33 * MIB].copy_from_slice(&patch);
    let new_manifest = encrypt_update(
        &keys,
        Format::V2_1,
        &manifest,
        1_800_000_001_000,
        &edited[..],
        |_| Ok(()),
    )?;
    let plan = plan_update(&manifest, &new_manifest);
    println!(
        "| Vault v2.1 delta update (1 MiB edit in 64 MiB) | 64 MiB file | {:.2} MiB to upload | {:.2}% of file ({} new chunks) |",
        plan.upload_bytes() as f64 / MIB as f64,
        100.0 * plan.upload_bytes() as f64 / (64 * MIB) as f64,
        plan.upload.len()
    );
    Ok(())
}

async fn bench_quic_sync() -> BenchResult {
    // Workload A: Mixed initial sync (46.875 MiB prose + 15.625 MiB noise across 80 files = 62.5 MiB)
    let a = BenchDevice::new(1)?;
    for i in 0..60u64 {
        let data = prose(800 * KIB, 100 + i);
        a.write(&format!("docs/section-{}/file-{i:02}.md", i % 6), &data)?;
    }
    for i in 0..20u64 {
        let data = noise(800 * KIB, 500 + i);
        a.write(&format!("media/clip-{i:02}.bin"), &data)?;
    }
    let scan_t = Instant::now();
    a.sync.scan().await?;
    let scan_elapsed = scan_t.elapsed();
    let payload_bytes = 80 * 800 * KIB;
    println!(
        "| Local scan + chunk index (`wasl-engine`) | {:.1} MiB (80 mixed files) | {:.1} ms | {:.0} MiB/s |",
        payload_bytes as f64 / MIB as f64,
        scan_elapsed.as_secs_f64() * 1_000.0,
        mib_per_sec(payload_bytes, scan_elapsed)
    );

    for (label, level) in [
        ("raw (level 0)", 0u8),
        ("LAN (zstd level 1)", LEVEL_LAN),
        ("internet (zstd level 3)", LEVEL_INTERNET),
        ("metered (zstd level 9)", LEVEL_METERED),
    ] {
        let b = BenchDevice::new(2)?;
        let (_s, _c, link) = quic_pair(&a, &b).await?;
        link.set_compression(level);
        b.sync.scan().await?;
        let t = Instant::now();
        let report = b.sync.pull(&link).await?;
        let elapsed = t.elapsed();
        let (raw, wire) = link.chunk_bytes();
        if report.deferred != 0 || report.rejected != 0 {
            return Err(format!("sync had deferred/rejected entries: {report:?}").into());
        }
        println!(
            "| P2P QUIC initial sync — {label} | {:.1} MiB mixed (75% text, 25% media) | {:.1} ms | {:.0} MiB/s payload · {:.1} MiB wire ({:.1}%) |",
            raw as f64 / MIB as f64,
            elapsed.as_secs_f64() * 1_000.0,
            mib_per_sec(raw as usize, elapsed),
            wire as f64 / MIB as f64,
            100.0 * wire as f64 / raw.max(1) as f64
        );
    }

    // Workload B: Incremental delta sync over QUIC (4 KiB mid-file edit in 4 x 8 MiB files = 32 MiB)
    let a_delta = BenchDevice::new(1)?;
    let b_delta = BenchDevice::new(2)?;
    let mut docs = Vec::with_capacity(4);
    for i in 0..4u64 {
        let doc = prose(8 * MIB, 900 + i);
        a_delta.write(&format!("notes/doc-{i}.md"), &doc)?;
        docs.push(doc);
    }
    a_delta.sync.scan().await?;
    let (_s, _c, link) = quic_pair(&a_delta, &b_delta).await?;
    link.set_compression(LEVEL_LAN);
    b_delta.sync.scan().await?;
    b_delta.sync.pull(&link).await?;
    let (before_raw, before_wire) = link.chunk_bytes();

    for (i, doc) in docs.iter_mut().enumerate() {
        let patch = prose(4 * KIB, 1200 + i as u64);
        doc[4 * MIB..4 * MIB + 4 * KIB].copy_from_slice(&patch);
        a_delta.write(&format!("notes/doc-{i}.md"), doc)?;
    }
    a_delta.sync.scan().await?;
    let t = Instant::now();
    let report = b_delta.sync.pull(&link).await?;
    let elapsed = t.elapsed();
    let (after_raw, after_wire) = link.chunk_bytes();
    let delta_raw = after_raw - before_raw;
    let delta_wire = after_wire - before_wire;
    if report.deferred != 0 || report.rejected != 0 {
        return Err(format!("delta sync failed: {report:?}").into());
    }
    println!(
        "| P2P QUIC incremental delta (4x 4 KiB edits in 32 MiB) | 32 MiB modified files | {:.1} ms | {:.1} KiB raw · {:.1} KiB wire ({:.2}% of whole files) |",
        elapsed.as_secs_f64() * 1_000.0,
        delta_raw as f64 / KIB as f64,
        delta_wire as f64 / KIB as f64,
        100.0 * delta_wire as f64 / (32 * MIB) as f64
    );

    // Workload C: 1,000 small files (4 KiB each = 4 MiB) over QUIC + safe-apply (fsync + rename) + SQLite
    let a_small = BenchDevice::new(1)?;
    let b_small = BenchDevice::new(2)?;
    for i in 0..1_000u64 {
        let content = prose(4 * KIB, 2000 + i);
        a_small.write(&format!("tree/d{:02}/file-{i:04}.txt", i % 20), &content)?;
    }
    a_small.sync.scan().await?;
    let (_s, _c, link) = quic_pair(&a_small, &b_small).await?;
    link.set_compression(LEVEL_LAN);
    b_small.sync.scan().await?;
    let t = Instant::now();
    let report = b_small.sync.pull(&link).await?;
    let elapsed = t.elapsed();
    if report.deferred != 0 || report.rejected != 0 {
        return Err(format!("small-file sync failed: {report:?}").into());
    }
    let files_per_sec = 1_000.0 / elapsed.as_secs_f64();
    println!(
        "| P2P QUIC small-file sync (fsync + atomic rename + SQLite) | 1,000 x 4 KiB files (4 MiB) | {:.1} ms | {:.0} files/s |",
        elapsed.as_secs_f64() * 1_000.0,
        files_per_sec
    );
    Ok(())
}

#[tokio::main(flavor = "multi_thread")]
async fn main() -> BenchResult {
    let profile = if cfg!(debug_assertions) {
        "debug"
    } else {
        "release"
    };
    println!(
        "Wasl engine benchmarks ({}, {}, {profile} profile)",
        std::env::consts::OS,
        std::env::consts::ARCH
    );
    println!();
    println!("| Benchmark | Workload | Latency / Wire | Throughput / Detail |");
    println!("|---|---|---|---|");
    bench_three_way_planner();
    bench_nway_planner();
    bench_chunking()?;
    bench_vault_v21()?;
    bench_quic_sync().await?;
    Ok(())
}
