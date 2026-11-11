//! Explorer's "Always keep on this device" and "Free up space" only set a pin state on the file
//! (the PINNED / UNPINNED attributes); the provider is expected to act on it. A thread watches the
//! folder's attribute changes and hands each changed path to a worker, which pins and hydrates,
//! or unpins and dehydrates (see [`Shared::apply_pin_state`]). Acting is idempotent, so the
//! changes the worker itself causes settle after one round.

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use super::{Shared, cf};

pub struct Watcher {
    stop: Arc<AtomicBool>,
    reader: Option<JoinHandle<()>>,
    worker: Option<JoinHandle<()>>,
    tx: Option<mpsc::Sender<PathBuf>>,
}

impl Watcher {
    pub fn start(shared: Arc<Shared>) -> std::io::Result<Self> {
        let dir = cf::open_dir_for_watch(&shared.root)?;
        let stop = Arc::new(AtomicBool::new(false));
        let (tx, rx) = mpsc::channel::<PathBuf>();
        let worker = {
            let shared = shared.clone();
            thread::Builder::new()
                .name("wasl-cf-pins".into())
                .spawn(move || {
                    while let Ok(first) = rx.recv() {
                        // Take everything queued, once each, in path order (folders first).
                        let mut batch: BTreeSet<PathBuf> = BTreeSet::from([first]);
                        batch.extend(rx.try_iter());
                        for p in batch {
                            shared.apply_pin_state(&p);
                        }
                    }
                })?
        };
        let reader = {
            let (stop, tx, root) = (stop.clone(), tx.clone(), shared.root.clone());
            thread::Builder::new()
                .name("wasl-cf-watch".into())
                .spawn(move || {
                    let mut buf = vec![0u32; 16 * 1024];
                    while !stop.load(Ordering::Acquire) {
                        match cf::read_changes(&dir, &mut buf) {
                            Ok(0) => {
                                // Too many changes at once: look at everything.
                                let _ = tx.send(root.clone());
                            }
                            Ok(n) => {
                                for rel in parse(&buf, n) {
                                    let _ = tx.send(root.join(rel));
                                }
                            }
                            Err(e) => {
                                if stop.load(Ordering::Acquire) {
                                    break;
                                }
                                shared.problem(format!("watching pin states: {e}"));
                                thread::sleep(Duration::from_millis(200));
                            }
                        }
                    }
                })?
        };
        Ok(Self {
            stop,
            reader: Some(reader),
            worker: Some(worker),
            tx: Some(tx),
        })
    }

    /// Queues a path as if its attributes had changed (a placeholder created in a pinned folder).
    pub fn poke(&self, path: PathBuf) {
        if let Some(tx) = &self.tx {
            let _ = tx.send(path);
        }
    }

    /// Stops both threads; waits for the worker to finish what it is doing.
    pub fn stop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(reader) = self.reader.take() {
            // The reader sits in a synchronous ReadDirectoryChangesW; cancel it until it notices.
            let deadline = Instant::now() + Duration::from_secs(5);
            while !reader.is_finished() && Instant::now() < deadline {
                cf::cancel_io(&reader);
                thread::sleep(Duration::from_millis(5));
            }
            if reader.is_finished() {
                let _ = reader.join();
            }
        }
        self.tx = None;
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

impl Drop for Watcher {
    fn drop(&mut self) {
        self.stop();
    }
}

/// The relative paths in a buffer of FILE_NOTIFY_INFORMATION records.
fn parse(buf: &[u32], len: usize) -> Vec<PathBuf> {
    let bytes: Vec<u8> = buf.iter().flat_map(|w| w.to_ne_bytes()).take(len).collect();
    let u32_at = |o: usize| {
        bytes
            .get(o..o + 4)
            .map(|b| u32::from_ne_bytes(b.try_into().expect("4")) as usize)
    };
    let mut out = Vec::new();
    let mut at = 0usize;
    while let (Some(next), Some(name_len)) = (u32_at(at), u32_at(at + 8)) {
        if let Some(name) = bytes.get(at + 12..at + 12 + name_len) {
            let units: Vec<u16> = name
                .as_chunks::<2>()
                .0
                .iter()
                .map(|c| u16::from_ne_bytes([c[0], c[1]]))
                .collect();
            out.push(PathBuf::from(String::from_utf16_lossy(&units)));
        }
        if next == 0 {
            break;
        }
        at += next;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_notify_records() {
        let mut bytes = Vec::new();
        for (i, name) in ["a.txt", r"dir\b.bin"].iter().enumerate() {
            let units: Vec<u16> = name.encode_utf16().collect();
            let rec_len = (12 + units.len() * 2).div_ceil(4) * 4;
            let next = if i == 0 { rec_len as u32 } else { 0 };
            bytes.extend_from_slice(&next.to_ne_bytes());
            bytes.extend_from_slice(&3u32.to_ne_bytes());
            bytes.extend_from_slice(&((units.len() * 2) as u32).to_ne_bytes());
            for u in &units {
                bytes.extend_from_slice(&u.to_ne_bytes());
            }
            bytes.resize(bytes.len().div_ceil(4) * 4, 0);
        }
        let words: Vec<u32> = bytes
            .as_chunks::<4>()
            .0
            .iter()
            .map(|c| u32::from_ne_bytes(*c))
            .collect();
        assert_eq!(
            vec![PathBuf::from("a.txt"), PathBuf::from(r"dir\b.bin")],
            parse(&words, bytes.len())
        );
    }
}
