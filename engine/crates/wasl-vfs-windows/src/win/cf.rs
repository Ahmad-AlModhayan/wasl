//! Thin wrappers over the Cloud Filter API and the few other Win32 calls this crate makes. Every
//! `unsafe` block of the crate except the callbacks' (in `fetch.rs`) is here, each with the
//! reason it is sound. Handles are always `std::fs::File`s, so they are valid for the call and
//! closed exactly once.

use std::ffi::c_void;
use std::fs::{File, OpenOptions};
use std::io;
use std::mem::{offset_of, size_of};
use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::io::AsRawHandle;
use std::path::Path;

use windows::Win32::Foundation::{
    HANDLE, HLOCAL, LocalFree, NTSTATUS, STATUS_CLOUD_FILE_UNSUCCESSFUL, STATUS_SUCCESS,
};
use windows::Win32::Security::Authorization::ConvertSidToStringSidW;
use windows::Win32::Security::{GetTokenInformation, TOKEN_QUERY, TOKEN_USER, TokenUser};
use windows::Win32::Storage::CloudFilters::*;
use windows::Win32::Storage::FileSystem::{
    FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_NORMAL, FILE_BASIC_INFO, FILE_NOTIFY_CHANGE_ATTRIBUTES,
    ReadDirectoryChangesW,
};
use windows::Win32::System::IO::CancelSynchronousIo;
use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
use windows::core::{HSTRING, PCWSTR, PWSTR};

use crate::identity;

const FILE_READ_DATA: u32 = 0x1;
const FILE_LIST_DIRECTORY: u32 = 0x1;
const FILE_WRITE_DATA: u32 = 0x2;
const FILE_READ_ATTRIBUTES: u32 = 0x80;
const FILE_WRITE_ATTRIBUTES: u32 = 0x100;
const WRITE_DAC: u32 = 0x0004_0000;
const FILE_ATTRIBUTE_READONLY: u32 = 0x1;
const FILE_SHARE_ALL: u32 = 0x7;
const FILE_SHARE_READ: u32 = 0x1;
const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
/// `HRESULT_FROM_WIN32(ERROR_NOT_A_CLOUD_FILE)`.
const NOT_A_CLOUD_FILE: i32 = 0x8007_0178_u32 as i32;

/// Cloud Files transfers in 4 KiB units (except at the end of the file).
pub const UNIT: u64 = 4096;

pub use windows::Win32::Storage::CloudFilters::{
    CF_CALLBACK_INFO, CF_CALLBACK_PARAMETERS, CF_CALLBACK_REGISTRATION, CF_CONNECTION_KEY, CF_PIN_STATE,
    CF_PIN_STATE_PINNED, CF_PIN_STATE_UNSPECIFIED, CF_PROCESS_INFO,
};

fn os(e: windows::core::Error) -> io::Error {
    e.into()
}

fn handle(f: &File) -> HANDLE {
    HANDLE(f.as_raw_handle())
}

/// Opens a file or folder for the placeholder calls: attributes always, data and attributes
/// writable when `write` (pin state, update, hydrate, dehydrate, revert need it). Opening a
/// placeholder does not hydrate it; only reading its data does.
///
/// A read-only file cannot be opened for writing its data; every placeholder call that wants
/// write access also accepts `WRITE_DAC` instead (which the file's owner always has), so a
/// read-only placeholder is opened that way.
pub fn open(path: &Path, write: bool) -> io::Result<File> {
    if !write {
        return open_with(path, FILE_READ_ATTRIBUTES);
    }
    match open_with(
        path,
        FILE_READ_DATA | FILE_READ_ATTRIBUTES | FILE_WRITE_DATA | FILE_WRITE_ATTRIBUTES,
    ) {
        Err(e) if e.kind() == io::ErrorKind::PermissionDenied => open_with(
            path,
            FILE_READ_DATA | FILE_READ_ATTRIBUTES | FILE_WRITE_ATTRIBUTES | WRITE_DAC,
        ),
        r => r,
    }
}

fn open_with(path: &Path, access: u32) -> io::Result<File> {
    OpenOptions::new()
        .access_mode(access)
        .share_mode(FILE_SHARE_ALL)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
        .open(path)
}

/// Opens a placeholder to read its content and update it through one handle that shares only
/// reading: it cannot be opened while another program has the file open for writing (or for
/// deleting), and while it is open nobody can start writing. What is read through it is
/// therefore exactly what an update through it replaces. Read-only files are opened with
/// `WRITE_DAC` instead of write access, as in [`open`].
pub fn open_alone(path: &Path) -> io::Result<File> {
    let open = |access: u32| {
        OpenOptions::new()
            .access_mode(access)
            .share_mode(FILE_SHARE_READ)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
            .open(path)
    };
    match open(FILE_READ_DATA | FILE_READ_ATTRIBUTES | FILE_WRITE_DATA | FILE_WRITE_ATTRIBUTES) {
        Err(e) if e.kind() == io::ErrorKind::PermissionDenied => {
            open(FILE_READ_DATA | FILE_READ_ATTRIBUTES | FILE_WRITE_ATTRIBUTES | WRITE_DAC)
        }
        r => r,
    }
}

/// Opens a folder for [`read_changes`].
pub fn open_dir_for_watch(path: &Path) -> io::Result<File> {
    OpenOptions::new()
        .access_mode(FILE_LIST_DIRECTORY)
        .share_mode(FILE_SHARE_ALL)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
        .open(path)
}

/// Unix milliseconds as a Windows FILETIME (100 ns since 1601).
pub fn filetime(ms: i64) -> i64 {
    ms.saturating_mul(10_000).saturating_add(116_444_736_000_000_000)
}

/// A placeholder's state, as `CfGetPlaceholderInfo` reports it.
#[derive(Debug, Clone)]
pub struct Info {
    pub pin: CF_PIN_STATE,
    pub in_sync: bool,
    /// Bytes of content on disk (0 when dehydrated).
    pub on_disk: u64,
    pub identity: Vec<u8>,
}

/// The placeholder's state; `None` when the file is not a placeholder.
pub fn info(f: &File) -> io::Result<Option<Info>> {
    // u64s so the buffer is aligned for the struct the API writes into it.
    let mut buf = vec![0u64; (size_of::<CF_PLACEHOLDER_STANDARD_INFO>() + identity::MAX).div_ceil(8)];
    let len = (buf.len() * 8) as u32;
    let mut returned = 0u32;
    // SAFETY: the handle is open for the call (borrowed from `f`); the buffer is writable for
    // `len` bytes and 8-aligned, and the API writes at most `len` bytes and reports how many.
    let r = unsafe {
        CfGetPlaceholderInfo(
            handle(f),
            CF_PLACEHOLDER_INFO_STANDARD,
            buf.as_mut_ptr().cast::<c_void>(),
            len,
            Some(&mut returned),
        )
    };
    match r {
        Err(e) if e.code().0 == NOT_A_CLOUD_FILE => return Ok(None),
        Err(e) => return Err(os(e)),
        Ok(()) => {}
    }
    let bytes: Vec<u8> = buf.iter().flat_map(|w| w.to_ne_bytes()).collect();
    let bytes = &bytes[..(returned as usize).min(bytes.len())];
    let i64_at = |o: usize| {
        bytes
            .get(o..o + 8)
            .map_or(0, |b| i64::from_ne_bytes(b.try_into().expect("8")))
    };
    let i32_at = |o: usize| {
        bytes
            .get(o..o + 4)
            .map_or(0, |b| i32::from_ne_bytes(b.try_into().expect("4")))
    };
    let id_len = i32_at(offset_of!(CF_PLACEHOLDER_STANDARD_INFO, FileIdentityLength)) as u32 as usize;
    let id_at = offset_of!(CF_PLACEHOLDER_STANDARD_INFO, FileIdentity);
    Ok(Some(Info {
        pin: CF_PIN_STATE(i32_at(offset_of!(CF_PLACEHOLDER_STANDARD_INFO, PinState))),
        in_sync: i32_at(offset_of!(CF_PLACEHOLDER_STANDARD_INFO, InSyncState)) == CF_IN_SYNC_STATE_IN_SYNC.0,
        on_disk: i64_at(offset_of!(CF_PLACEHOLDER_STANDARD_INFO, OnDiskDataSize)).max(0) as u64,
        identity: bytes.get(id_at..id_at + id_len).unwrap_or_default().to_vec(),
    }))
}

/// One placeholder to create in a folder.
pub struct NewPlaceholder {
    /// The name in the folder (one component).
    pub name: String,
    pub is_dir: bool,
    pub size: u64,
    pub mtime_ms: i64,
    pub identity: Vec<u8>,
    /// A file with the read-only attribute (folders ignore it).
    pub read_only: bool,
}

fn metadata(is_dir: bool, size: u64, mtime_ms: i64, read_only: bool) -> CF_FS_METADATA {
    let t = filetime(mtime_ms);
    CF_FS_METADATA {
        BasicInfo: FILE_BASIC_INFO {
            CreationTime: t,
            LastAccessTime: t,
            LastWriteTime: t,
            ChangeTime: t,
            FileAttributes: if is_dir {
                FILE_ATTRIBUTE_DIRECTORY.0
            } else if read_only {
                FILE_ATTRIBUTE_READONLY
            } else {
                FILE_ATTRIBUTE_NORMAL.0
            },
        },
        FileSize: if is_dir { 0 } else { size as i64 },
    }
}

/// Creates placeholders in `dir` (in sync; folders fully populated, since Wasl creates every
/// placeholder itself). One result per entry.
pub fn create(dir: &Path, entries: &[NewPlaceholder]) -> Vec<io::Result<()>> {
    if entries.is_empty() {
        return Vec::new();
    }
    let names: Vec<HSTRING> = entries.iter().map(|e| HSTRING::from(e.name.as_str())).collect();
    let mut infos: Vec<CF_PLACEHOLDER_CREATE_INFO> = entries
        .iter()
        .zip(&names)
        .map(|(e, name)| CF_PLACEHOLDER_CREATE_INFO {
            RelativeFileName: PCWSTR(name.as_ptr()),
            FsMetadata: metadata(e.is_dir, e.size, e.mtime_ms, e.read_only),
            FileIdentity: e.identity.as_ptr().cast(),
            FileIdentityLength: e.identity.len() as u32,
            Flags: if e.is_dir {
                CF_PLACEHOLDER_CREATE_FLAG_MARK_IN_SYNC
                    | CF_PLACEHOLDER_CREATE_FLAG_DISABLE_ON_DEMAND_POPULATION
            } else {
                CF_PLACEHOLDER_CREATE_FLAG_MARK_IN_SYNC
            },
            ..Default::default()
        })
        .collect();
    let base = HSTRING::from(dir);
    let mut processed = 0u32;
    // SAFETY: every pointer in `infos` (names, identities) points into `names` / `entries`,
    // which outlive the call; the API writes only each entry's `Result` / `CreateUsn` and the
    // count of entries it processed.
    let all = unsafe { CfCreatePlaceholders(&base, &mut infos, CF_CREATE_FLAG_NONE, Some(&mut processed)) };
    infos
        .iter()
        .enumerate()
        .map(|(n, i)| match &all {
            _ if i.Result.is_err() => Err(os(i.Result.into())),
            // Not reached: the whole call failed before this entry.
            Err(e) if n as u32 >= processed => Err(os(e.clone())),
            _ => Ok(()),
        })
        .collect()
}

/// New size, time and identity for a placeholder, dropping its content when `dehydrate`, and
/// marking it in sync (read-only when `read_only`).
pub fn update(
    f: &File,
    size: u64,
    mtime_ms: i64,
    identity: &[u8],
    dehydrate: bool,
    read_only: bool,
) -> io::Result<()> {
    let meta = metadata(false, size, mtime_ms, read_only);
    let mut flags = CF_UPDATE_FLAG_MARK_IN_SYNC;
    if dehydrate {
        flags |= CF_UPDATE_FLAG_DEHYDRATE;
    }
    // SAFETY: the handle is open with write access for the call; `meta` and `identity` are live
    // for the call and `identity.len()` is its length; no USN or overlapped I/O is asked for.
    unsafe {
        CfUpdatePlaceholder(
            handle(f),
            Some(&meta),
            Some(identity.as_ptr().cast()),
            identity.len() as u32,
            None,
            flags,
            None,
            None,
        )
    }
    .map_err(os)
}

/// Marks a placeholder in sync with a new identity, its content kept.
pub fn mark_in_sync(f: &File, identity: &[u8]) -> io::Result<()> {
    // SAFETY: as in `update`; no metadata is passed, so the file's own is kept.
    unsafe {
        CfUpdatePlaceholder(
            handle(f),
            None,
            Some(identity.as_ptr().cast()),
            identity.len() as u32,
            None,
            CF_UPDATE_FLAG_MARK_IN_SYNC,
            None,
            None,
        )
    }
    .map_err(os)
}

/// Makes a regular file a (full, in sync) placeholder.
pub fn convert(f: &File, identity: &[u8]) -> io::Result<()> {
    // SAFETY: the handle is open with write access for the call; `identity` is live for it.
    unsafe {
        CfConvertToPlaceholder(
            handle(f),
            Some(identity.as_ptr().cast()),
            identity.len() as u32,
            CF_CONVERT_FLAG_MARK_IN_SYNC,
            None,
            None,
        )
    }
    .map_err(os)
}

/// Sets the pin state (Explorer's "Always keep on this device" is pinned, "Free up space"
/// unpinned), on a folder's descendants too when `recurse`.
pub fn set_pin(f: &File, state: CF_PIN_STATE, recurse: bool) -> io::Result<()> {
    let flags = if recurse {
        CF_SET_PIN_FLAG_RECURSE
    } else {
        CF_SET_PIN_FLAG_NONE
    };
    // SAFETY: the handle is open with write access for the call; synchronous (no overlapped).
    unsafe { CfSetPinState(handle(f), state, flags, None) }.map_err(os)
}

/// Fetches the whole file (through this process's own fetch callbacks); blocks until done.
pub fn hydrate(f: &File) -> io::Result<()> {
    // SAFETY: the handle is open for the call; offset 0 and length -1 mean the whole file;
    // synchronous (no overlapped).
    unsafe { CfHydratePlaceholder(handle(f), 0, -1, CF_HYDRATE_FLAG_NONE, None) }.map_err(os)
}

/// Drops the file's content from the disk (it stays a placeholder at full size).
pub fn dehydrate(f: &File) -> io::Result<()> {
    // SAFETY: the handle is open with write access for the call; the whole file; synchronous.
    unsafe { CfDehydratePlaceholder(handle(f), 0, -1, CF_DEHYDRATE_FLAG_NONE, None) }.map_err(os)
}

/// Makes a fully hydrated placeholder a regular file again.
pub fn revert(f: &File) -> io::Result<()> {
    // SAFETY: the handle is open with write access for the call; synchronous.
    unsafe { CfRevertPlaceholder(handle(f), CF_REVERT_FLAG_NONE, None) }.map_err(os)
}

/// Connects this process as the folder's provider. `context` is handed to every callback.
///
/// # Safety
///
/// `callbacks` must end with a `CF_CALLBACK_TYPE_NONE` entry and, like `context`, stay valid
/// until [`disconnect`] returns; the callbacks must be sound for any `context` given here.
pub unsafe fn connect(
    root: &Path,
    callbacks: &'static [CF_CALLBACK_REGISTRATION],
    context: *const c_void,
    block_self_hydration: bool,
) -> io::Result<CF_CONNECTION_KEY> {
    let path = HSTRING::from(root);
    // Process info: each fetch says which program's read caused it (FetchRecord).
    let mut flags = CF_CONNECT_FLAG_REQUIRE_FULL_FILE_PATH | CF_CONNECT_FLAG_REQUIRE_PROCESS_INFO;
    if block_self_hydration {
        flags |= CF_CONNECT_FLAG_BLOCK_SELF_IMPLICIT_HYDRATION;
    }
    // SAFETY: the table is 'static and terminated (the caller's contract), and the caller keeps
    // `context` alive until `disconnect`.
    unsafe { CfConnectSyncRoot(&path, callbacks.as_ptr(), Some(context), flags) }.map_err(os)
}

/// Disconnects; no callback runs after this returns.
pub fn disconnect(key: CF_CONNECTION_KEY) -> io::Result<()> {
    // SAFETY: a connection key is a plain value; a stale one makes the call fail, not misbehave.
    unsafe { CfDisconnectSyncRoot(key) }.map_err(os)
}

/// What a fetch answers for one range.
pub enum Transfer<'a> {
    /// The bytes at the offset (a multiple of [`UNIT`] long, or ending at the end of the file).
    Data(&'a [u8]),
    /// The range (offset, length) cannot be read: the reader sees an error instead of waiting.
    Failed(u64),
}

/// Answers part of a FETCH_DATA request (`CfExecute` with `TRANSFER_DATA`); callable from any
/// thread, after the callback has returned, until the request is cancelled.
pub fn transfer(
    connection: CF_CONNECTION_KEY,
    transfer_key: i64,
    request_key: i64,
    offset: u64,
    what: Transfer<'_>,
) -> io::Result<()> {
    let (status, buffer, length): (NTSTATUS, *const c_void, u64) = match what {
        Transfer::Data(b) => (STATUS_SUCCESS, b.as_ptr().cast(), b.len() as u64),
        Transfer::Failed(len) => (STATUS_CLOUD_FILE_UNSUCCESSFUL, std::ptr::null(), len),
    };
    let info = CF_OPERATION_INFO {
        StructSize: size_of::<CF_OPERATION_INFO>() as u32,
        Type: CF_OPERATION_TYPE_TRANSFER_DATA,
        ConnectionKey: connection,
        TransferKey: transfer_key,
        RequestKey: request_key,
        ..Default::default()
    };
    let mut params = CF_OPERATION_PARAMETERS {
        // The size up to the end of the member used (the header's CF_SIZE_OF_OP_PARAM).
        ParamSize: (offset_of!(CF_OPERATION_PARAMETERS, Anonymous) + size_of::<CF_OPERATION_PARAMETERS_0_0>())
            as u32,
        Anonymous: CF_OPERATION_PARAMETERS_0 {
            TransferData: CF_OPERATION_PARAMETERS_0_0 {
                Flags: CF_OPERATION_TRANSFER_DATA_FLAG_NONE,
                CompletionStatus: status,
                Buffer: buffer,
                Offset: offset as i64,
                Length: length as i64,
            },
        },
    };
    // SAFETY: `info` and `params` are fully initialised and live for the call; `buffer` is null
    // only with a failure status, otherwise it points at `length` readable bytes borrowed for the
    // call; the platform copies the data before returning.
    unsafe { CfExecute(&info, &mut params) }.map_err(os)
}

/// Whether `path` is in a registered sync root.
pub fn in_sync_root(path: &Path) -> bool {
    let p = HSTRING::from(path);
    let mut buf = [0u64; 64];
    let mut returned = 0u32;
    // SAFETY: the buffer is writable for its byte length and aligned; the API writes at most
    // that many bytes (a basic info is a 64-bit file id).
    unsafe {
        CfGetSyncRootInfoByPath(
            &p,
            CF_SYNC_ROOT_INFO_BASIC,
            buf.as_mut_ptr().cast(),
            (buf.len() * 8) as u32,
            Some(&mut returned),
        )
    }
    .is_ok()
}

/// Waits for attribute changes under `dir` (recursively). Returns the bytes of
/// FILE_NOTIFY_INFORMATION records written to `buf` (0: too many changes, rescan).
pub fn read_changes(dir: &File, buf: &mut [u32]) -> io::Result<usize> {
    let mut returned = 0u32;
    // SAFETY: the handle is a folder opened for listing, live for the call; the buffer is
    // DWORD-aligned and writable for its byte length; synchronous (no overlapped, no routine).
    unsafe {
        ReadDirectoryChangesW(
            handle(dir),
            buf.as_mut_ptr().cast(),
            (buf.len() * 4) as u32,
            true,
            FILE_NOTIFY_CHANGE_ATTRIBUTES,
            Some(&mut returned),
            None,
            None,
        )
    }
    .map_err(os)?;
    Ok(returned as usize)
}

/// Makes a thread's pending synchronous I/O (its [`read_changes`]) return.
pub fn cancel_io(thread: &std::thread::JoinHandle<()>) {
    // SAFETY: the thread handle is owned by the live JoinHandle, so it is valid for the call;
    // cancelling when no I/O is pending is harmless (the call fails with ERROR_NOT_FOUND).
    let _ = unsafe { CancelSynchronousIo(HANDLE(thread.as_raw_handle())) };
}

/// The current user's SID (`S-1-5-21-…`), the middle part of a sync root id.
pub fn user_sid() -> io::Result<String> {
    let mut token = HANDLE::default();
    // SAFETY: GetCurrentProcess returns a pseudo handle that needs no closing; `token` receives a
    // new handle, closed below by wrapping it in an OwnedHandle.
    unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) }.map_err(os)?;
    // SAFETY: `token` is a handle we own from now on; OwnedHandle closes it once.
    let token = unsafe {
        <std::os::windows::io::OwnedHandle as std::os::windows::io::FromRawHandle>::from_raw_handle(token.0)
    };
    let mut buf = vec![0u64; 64];
    let mut needed = 0u32;
    // SAFETY: the token handle is open with query access; the buffer is aligned and writable for
    // its byte length, and the API writes at most that much.
    unsafe {
        GetTokenInformation(
            HANDLE(token.as_raw_handle()),
            TokenUser,
            Some(buf.as_mut_ptr().cast()),
            (buf.len() * 8) as u32,
            &mut needed,
        )
    }
    .map_err(os)?;
    // SAFETY: the call succeeded, so the buffer starts with a TOKEN_USER whose SID pointer points
    // into the same buffer, which is still alive.
    let sid = unsafe { (*buf.as_ptr().cast::<TOKEN_USER>()).User.Sid };
    let mut s = PWSTR::null();
    // SAFETY: `sid` is a valid SID (above); the API allocates `s` with LocalAlloc.
    unsafe { ConvertSidToStringSidW(sid, &mut s) }.map_err(os)?;
    // SAFETY: `s` is a NUL-terminated string from the call above, read before it is freed.
    let out = unsafe { s.to_string() }.map_err(io::Error::other);
    // SAFETY: `s` was allocated by LocalAlloc in ConvertSidToStringSidW and is freed once.
    unsafe { LocalFree(Some(HLOCAL(s.0.cast()))) };
    out
}
