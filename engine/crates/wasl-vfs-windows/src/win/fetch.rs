//! FETCH_DATA and CANCEL_FETCH_DATA: Windows asks for a byte range of a placeholder; the range is
//! read through the [`Hydrator`](wasl_vfs::Hydrator) on Wasl's runtime and handed back with
//! `CfExecute`, a block at a time, so the callback itself returns at once. A failed read is
//! reported as a failed transfer, so the reader (Explorer, an app) gets an error instead of
//! waiting for the platform's timeout.

use std::ffi::c_void;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use wasl_vfs::BLOCK;
use windows::Win32::Storage::CloudFilters::{
    CF_CALLBACK_TYPE_CANCEL_FETCH_DATA, CF_CALLBACK_TYPE_FETCH_DATA, CF_CALLBACK_TYPE_NONE,
};

use super::Shared;
use super::cf::{
    self, CF_CALLBACK_INFO, CF_CALLBACK_PARAMETERS, CF_CALLBACK_REGISTRATION, CF_CONNECTION_KEY, Transfer,
    UNIT,
};
use crate::identity::{self, Identity};

/// The callbacks Wasl registers. FETCH_PLACEHOLDERS is not needed: every folder is created fully
/// populated ("always full" population).
pub static CALLBACKS: [CF_CALLBACK_REGISTRATION; 3] = [
    CF_CALLBACK_REGISTRATION {
        Type: CF_CALLBACK_TYPE_FETCH_DATA,
        Callback: Some(on_fetch_data),
    },
    CF_CALLBACK_REGISTRATION {
        Type: CF_CALLBACK_TYPE_CANCEL_FETCH_DATA,
        Callback: Some(on_cancel_fetch_data),
    },
    CF_CALLBACK_REGISTRATION {
        Type: CF_CALLBACK_TYPE_NONE,
        Callback: None,
    },
];

/// One FETCH_DATA request, copied out of the callback's arguments.
pub struct Fetch {
    pub connection: CF_CONNECTION_KEY,
    pub transfer_key: i64,
    pub request_key: i64,
    pub identity: Vec<u8>,
    pub offset: u64,
    pub len: u64,
    pub path: String,
    /// The program whose read caused this fetch, and its id.
    pub process: String,
    pub process_id: u32,
}

/// A fetch in progress, so a cancel can stop it.
pub struct Inflight {
    pub start: u64,
    pub end: u64,
    pub cancelled: Arc<AtomicBool>,
}

/// # Safety
///
/// Called by the Cloud Filter platform only, with valid `info` and `params` for the duration of
/// the call, and `info.CallbackContext` the `Box<Arc<Shared>>` pointer given to `cf::connect`.
unsafe extern "system" fn on_fetch_data(
    info: *const CF_CALLBACK_INFO,
    params: *const CF_CALLBACK_PARAMETERS,
) {
    // SAFETY: the platform passes pointers valid for the duration of this call.
    let (info, params) = unsafe { (&*info, &*params) };
    // SAFETY: this is a FETCH_DATA callback, so `FetchData` is the union's live member.
    let p = unsafe { params.Anonymous.FetchData };
    // SAFETY: the identity is the blob given at creation, `FileIdentityLength` bytes, valid for
    // the call (it is copied before returning).
    let identity = unsafe { bytes(info.FileIdentity, info.FileIdentityLength) };
    // SAFETY: with CF_CONNECT_FLAG_REQUIRE_FULL_FILE_PATH, NormalizedPath is a NUL-terminated
    // string valid for the call (or null).
    let path = unsafe { wide(info.NormalizedPath.0) };
    // SAFETY: with CF_CONNECT_FLAG_REQUEST_PROCESS_INFO, ProcessInfo is null or points at a
    // CF_PROCESS_INFO valid for the call, whose ImagePath is null or a NUL-terminated string.
    let (process, process_id) = unsafe { process(info.ProcessInfo) };
    let fetch = Fetch {
        connection: info.ConnectionKey,
        transfer_key: info.TransferKey,
        request_key: info.RequestKey,
        identity,
        offset: p.RequiredFileOffset.max(0) as u64,
        len: p.RequiredLength.max(0) as u64,
        path,
        process,
        process_id,
    };
    // SAFETY: the context is the `Box<Arc<Shared>>` pointer kept alive by the OnDemandRoot until
    // after `CfDisconnectSyncRoot` returns, and no callback runs after that.
    let shared = unsafe { shared(info.CallbackContext) };
    // A panic must not unwind into Windows; the request then times out on its own.
    let _ = catch_unwind(AssertUnwindSafe(|| start(shared, fetch)));
}

/// # Safety
///
/// As [`on_fetch_data`].
unsafe extern "system" fn on_cancel_fetch_data(
    info: *const CF_CALLBACK_INFO,
    params: *const CF_CALLBACK_PARAMETERS,
) {
    // SAFETY: the platform passes pointers valid for the duration of this call.
    let (info, params) = unsafe { (&*info, &*params) };
    // SAFETY: this is a CANCEL_FETCH_DATA callback: `Cancel` is the live member and, for a data
    // fetch, its `FetchData` range.
    let range = unsafe { params.Anonymous.Cancel.Anonymous.FetchData };
    // SAFETY: as in `on_fetch_data`.
    let shared = unsafe { shared(info.CallbackContext) };
    let (start, end) = (
        range.FileOffset.max(0) as u64,
        range.FileOffset.max(0) as u64 + range.Length.max(0) as u64,
    );
    let _ = catch_unwind(AssertUnwindSafe(|| shared.cancel(info.TransferKey, start, end)));
}

/// # Safety
///
/// `ctx` must be a live `Box<Arc<Shared>>` raw pointer for the returned lifetime.
unsafe fn shared<'a>(ctx: *mut c_void) -> &'a Arc<Shared> {
    // SAFETY: `ctx` came from `Box::into_raw(Box::new(Arc<Shared>))` (see `OnDemandRoot`) and is
    // freed only after disconnecting, so it is live during every callback.
    unsafe { &*ctx.cast::<Arc<Shared>>() }
}

/// # Safety
///
/// `p` must be null or point at a `CF_PROCESS_INFO` whose `ImagePath` is null or a NUL-terminated
/// UTF-16 string, valid for the call.
unsafe fn process(p: *const cf::CF_PROCESS_INFO) -> (String, u32) {
    if p.is_null() {
        return ("unknown".into(), 0);
    }
    // SAFETY: the caller's contract.
    let p = unsafe { &*p };
    // SAFETY: the caller's contract (ImagePath is null or NUL-terminated).
    let image = unsafe { wide(p.ImagePath.0) };
    let name = image.rsplit(['\\', '/']).next().unwrap_or_default().to_owned();
    if name.is_empty() {
        (format!("pid {}", p.ProcessId), p.ProcessId)
    } else {
        (name, p.ProcessId)
    }
}

/// # Safety
///
/// `p` must be null or point at `len` readable bytes.
unsafe fn bytes(p: *const c_void, len: u32) -> Vec<u8> {
    if p.is_null() || len == 0 {
        return Vec::new();
    }
    // SAFETY: the caller's contract.
    unsafe { std::slice::from_raw_parts(p.cast::<u8>(), len as usize) }.to_vec()
}

/// # Safety
///
/// `p` must be null or a NUL-terminated UTF-16 string.
unsafe fn wide(p: *const u16) -> String {
    if p.is_null() {
        return String::new();
    }
    // SAFETY: the caller's contract; the length is found by scanning to the NUL.
    unsafe { windows::core::PCWSTR(p).to_string() }.unwrap_or_default()
}

/// Records the fetch and serves it on the runtime; returns at once.
fn start(shared: &Arc<Shared>, fetch: Fetch) {
    shared.fetched(crate::FetchRecord {
        process: fetch.process.clone(),
        process_id: fetch.process_id,
        path: fetch.path.clone(),
        offset: fetch.offset,
        len: fetch.len,
    });
    let cancelled = Arc::new(AtomicBool::new(false));
    shared.track(
        fetch.transfer_key,
        Inflight {
            start: fetch.offset,
            end: fetch.offset + fetch.len,
            cancelled: cancelled.clone(),
        },
    );
    let s = shared.clone();
    shared.rt.spawn(async move {
        let key = fetch.transfer_key;
        serve(&s, &fetch, &cancelled).await;
        s.untrack(key, &cancelled);
    });
}

/// Reads the requested range block by block and transfers each piece as it arrives.
async fn serve(shared: &Shared, fetch: &Fetch, cancelled: &AtomicBool) {
    let fail = |at: u64, end: u64, why: String| {
        shared.problem(format!("reading {}: {why}", fetch.path));
        let _ = cf::transfer(
            fetch.connection,
            fetch.transfer_key,
            fetch.request_key,
            at,
            Transfer::Failed(end - at),
        );
    };
    let Some(Identity::File(file)) = identity::decode(&fetch.identity) else {
        let end = fetch.offset + fetch.len.div_ceil(UNIT) * UNIT;
        fail(fetch.offset, end, "not a Wasl placeholder".into());
        return;
    };
    // Only files of the tree this folder shows: the identity comes from the placeholder, which
    // another program may have made (security review 2026-10).
    if !shared.knows(&file.id) {
        let end = fetch.offset + fetch.len.div_ceil(UNIT) * UNIT;
        fail(fetch.offset, end, "not a file of this folder's listing".into());
        return;
    }
    // The platform asks in 4 KiB units; the last one may run past the end of the file.
    let end = (fetch.offset + fetch.len)
        .div_ceil(UNIT)
        .saturating_mul(UNIT)
        .min(file.size);
    let mut at = fetch.offset;
    while at < end {
        if cancelled.load(Ordering::Acquire) {
            return;
        }
        let piece_end = ((at / BLOCK + 1) * BLOCK).min(end);
        match shared.hydrator.read(&file, at, piece_end - at).await {
            Ok(bytes) if bytes.len() as u64 == piece_end - at => {
                if cancelled.load(Ordering::Acquire) {
                    return;
                }
                if let Err(e) = cf::transfer(
                    fetch.connection,
                    fetch.transfer_key,
                    fetch.request_key,
                    at,
                    Transfer::Data(&bytes),
                ) {
                    // Cancelled by the platform (the reader went away) or the file changed.
                    shared.problem(format!("handing over {}: {e}", fetch.path));
                    return;
                }
            }
            Ok(bytes) => {
                fail(
                    at,
                    end,
                    format!("{} bytes where {} were expected", bytes.len(), piece_end - at),
                );
                return;
            }
            Err(e) => {
                fail(at, end, e.to_string());
                return;
            }
        }
        at = piece_end;
    }
}
