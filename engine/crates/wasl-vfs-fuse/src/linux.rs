//! The FUSE file system over a [`View`] (Linux), with `fuser`'s safe API.
//!
//! The kernel's requests arrive on one fuser thread. Everything but `read` answers from memory
//! right there; `read` may need the network, so it moves its reply object into a task on the
//! daemon's tokio runtime and returns at once: the fuser thread is never blocked on a fetch, and
//! the kernel (FUSE_ASYNC_READ) keeps several reads in flight.

use std::ffi::OsStr;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, mpsc};
use std::thread;
use std::time::{Duration, SystemTime};

use fuser::{
    BackgroundSession, Config, Errno, FileAttr, FileHandle, FileType, Filesystem, FopenFlags, Generation,
    INodeNo, LockOwner, MountOption, Notifier, OpenAccMode, OpenFlags, RenameFlags, ReplyAttr, ReplyCreate,
    ReplyData, ReplyDirectory, ReplyEmpty, ReplyEntry, ReplyOpen, ReplyWrite, ReplyXattr, Request, TimeOrNow,
    WriteFlags,
};
use tokio::runtime::Handle;
use wasl_vfs::{Hydrator, Tree};

use crate::{Attr, Change, FsError, Kind, MountOptions, View, XattrReply};

fn errno(e: FsError) -> Errno {
    match e {
        FsError::NotFound => Errno::ENOENT,
        FsError::NotDir => Errno::ENOTDIR,
        FsError::IsDir => Errno::EISDIR,
        FsError::ReadOnly => Errno::EROFS,
        FsError::NoAttr => Errno::NO_XATTR,
        FsError::Range => Errno::ERANGE,
        FsError::BadHandle => Errno::EBADF,
        FsError::Io => Errno::EIO,
    }
}

struct Fs {
    view: Arc<View>,
    runtime: Handle,
    ttl: Duration,
    /// The mounting user: every file and folder is theirs.
    uid: u32,
    gid: u32,
}

impl Fs {
    fn attr(&self, a: Attr) -> FileAttr {
        FileAttr {
            ino: INodeNo(a.ino),
            size: a.size,
            blocks: a.blocks,
            atime: a.mtime,
            mtime: a.mtime,
            ctime: a.mtime,
            crtime: a.mtime,
            kind: kind(a.kind),
            perm: a.perm,
            nlink: a.nlink,
            uid: self.uid,
            gid: self.gid,
            rdev: 0,
            blksize: 4096,
            flags: 0,
        }
    }
}

fn kind(k: Kind) -> FileType {
    match k {
        Kind::Dir => FileType::Directory,
        Kind::File => FileType::RegularFile,
    }
}

fn xattr(reply: ReplyXattr, size: u32, value: Result<Vec<u8>, FsError>) {
    match value.and_then(|v| XattrReply::for_buffer(size, v)) {
        Ok(XattrReply::Size(n)) => reply.size(n),
        Ok(XattrReply::Data(d)) => reply.data(&d),
        Err(e) => reply.error(errno(e)),
    }
}

impl Filesystem for Fs {
    fn lookup(&self, _req: &Request, parent: INodeNo, name: &OsStr, reply: ReplyEntry) {
        let Some(name) = name.to_str() else {
            return reply.error(Errno::ENOENT);
        };
        match self.view.lookup(parent.0, name) {
            Ok(a) => reply.entry(&self.ttl, &self.attr(a), Generation(0)),
            Err(e) => reply.error(errno(e)),
        }
    }

    fn getattr(&self, _req: &Request, ino: INodeNo, _fh: Option<FileHandle>, reply: ReplyAttr) {
        match self.view.getattr(ino.0) {
            Ok(a) => reply.attr(&self.ttl, &self.attr(a)),
            Err(e) => reply.error(errno(e)),
        }
    }

    fn open(&self, _req: &Request, ino: INodeNo, flags: OpenFlags, reply: ReplyOpen) {
        let write = flags.acc_mode() != OpenAccMode::O_RDONLY;
        match self.view.open(ino.0, write) {
            // No FOPEN_KEEP_CACHE: the kernel drops cached pages on every open, so a file opened
            // after a refresh never shows the previous version's pages.
            Ok(fh) => reply.opened(FileHandle(fh), FopenFlags::empty()),
            Err(e) => reply.error(errno(e)),
        }
    }

    fn read(
        &self,
        _req: &Request,
        _ino: INodeNo,
        fh: FileHandle,
        offset: u64,
        size: u32,
        _flags: OpenFlags,
        _lock_owner: Option<LockOwner>,
        reply: ReplyData,
    ) {
        let view = self.view.clone();
        // The reply goes with the task; a task that never runs (the runtime is shutting down)
        // drops it, and fuser then answers EIO.
        self.runtime.spawn(async move {
            match view.read(fh.0, offset, u64::from(size)).await {
                Ok(bytes) => reply.data(&bytes),
                Err(e) => reply.error(errno(e)),
            }
        });
    }

    fn release(
        &self,
        _req: &Request,
        _ino: INodeNo,
        fh: FileHandle,
        _flags: OpenFlags,
        _lock_owner: Option<LockOwner>,
        _flush: bool,
        reply: ReplyEmpty,
    ) {
        self.view.release(fh.0);
        reply.ok();
    }

    fn readdir(&self, _req: &Request, ino: INodeNo, _fh: FileHandle, offset: u64, mut reply: ReplyDirectory) {
        match self.view.readdir(ino.0, offset) {
            Ok(entries) => {
                for e in entries {
                    if reply.add(INodeNo(e.ino), e.offset, kind(e.kind), &e.name) {
                        break; // The buffer is full; the kernel asks again from e's offset.
                    }
                }
                reply.ok();
            }
            Err(e) => reply.error(errno(e)),
        }
    }

    fn getxattr(&self, _req: &Request, ino: INodeNo, name: &OsStr, size: u32, reply: ReplyXattr) {
        let value = match name.to_str() {
            Some(n) => self.view.getxattr(ino.0, n),
            None => Err(FsError::NoAttr),
        };
        xattr(reply, size, value);
    }

    fn listxattr(&self, _req: &Request, ino: INodeNo, size: u32, reply: ReplyXattr) {
        xattr(reply, size, self.view.listxattr(ino.0));
    }

    // Everything that would change the folder. The mount is `ro`, so the kernel refuses most of
    // these before they arrive; they are answered here too so the folder stays read-only
    // whatever the mount options. Local edits on on-demand pairs come in a later step.

    fn setattr(
        &self,
        _req: &Request,
        _ino: INodeNo,
        _mode: Option<u32>,
        _uid: Option<u32>,
        _gid: Option<u32>,
        _size: Option<u64>,
        _atime: Option<TimeOrNow>,
        _mtime: Option<TimeOrNow>,
        _ctime: Option<SystemTime>,
        _fh: Option<FileHandle>,
        _crtime: Option<SystemTime>,
        _chgtime: Option<SystemTime>,
        _bkuptime: Option<SystemTime>,
        _flags: Option<fuser::BsdFileFlags>,
        reply: ReplyAttr,
    ) {
        reply.error(Errno::EROFS);
    }

    fn write(
        &self,
        _req: &Request,
        _ino: INodeNo,
        _fh: FileHandle,
        _offset: u64,
        _data: &[u8],
        _write_flags: WriteFlags,
        _flags: OpenFlags,
        _lock_owner: Option<LockOwner>,
        reply: ReplyWrite,
    ) {
        reply.error(Errno::EROFS);
    }

    fn create(
        &self,
        _req: &Request,
        _parent: INodeNo,
        _name: &OsStr,
        _mode: u32,
        _umask: u32,
        _flags: i32,
        reply: ReplyCreate,
    ) {
        reply.error(Errno::EROFS);
    }

    fn mknod(
        &self,
        _req: &Request,
        _parent: INodeNo,
        _name: &OsStr,
        _mode: u32,
        _umask: u32,
        _rdev: u32,
        reply: ReplyEntry,
    ) {
        reply.error(Errno::EROFS);
    }

    fn mkdir(
        &self,
        _req: &Request,
        _parent: INodeNo,
        _name: &OsStr,
        _mode: u32,
        _umask: u32,
        reply: ReplyEntry,
    ) {
        reply.error(Errno::EROFS);
    }

    fn unlink(&self, _req: &Request, _parent: INodeNo, _name: &OsStr, reply: ReplyEmpty) {
        reply.error(Errno::EROFS);
    }

    fn rmdir(&self, _req: &Request, _parent: INodeNo, _name: &OsStr, reply: ReplyEmpty) {
        reply.error(Errno::EROFS);
    }

    fn symlink(
        &self,
        _req: &Request,
        _parent: INodeNo,
        _link_name: &OsStr,
        _target: &Path,
        reply: ReplyEntry,
    ) {
        reply.error(Errno::EROFS);
    }

    fn rename(
        &self,
        _req: &Request,
        _parent: INodeNo,
        _name: &OsStr,
        _newparent: INodeNo,
        _newname: &OsStr,
        _flags: RenameFlags,
        reply: ReplyEmpty,
    ) {
        reply.error(Errno::EROFS);
    }

    fn link(&self, _req: &Request, _ino: INodeNo, _newparent: INodeNo, _newname: &OsStr, reply: ReplyEntry) {
        reply.error(Errno::EROFS);
    }

    fn setxattr(
        &self,
        _req: &Request,
        _ino: INodeNo,
        _name: &OsStr,
        _value: &[u8],
        _flags: i32,
        _position: u32,
        reply: ReplyEmpty,
    ) {
        // Pins change through the daemon (`OnDemandMount::pin`), not through the attribute.
        reply.error(Errno::EROFS);
    }

    fn removexattr(&self, _req: &Request, _ino: INodeNo, _name: &OsStr, reply: ReplyEmpty) {
        reply.error(Errno::EROFS);
    }
}

/// A pair's on-demand view mounted at a folder. Unmounts when dropped.
pub struct OnDemandMount {
    view: Arc<View>,
    session: Option<BackgroundSession>,
    notifier: Notifier,
    at: PathBuf,
    unmount_wait: Duration,
}

impl OnDemandMount {
    /// Mounts `tree` at the folder `at` (which must exist; ideally empty), reading through
    /// `hydrator` on `runtime`. Needs `/dev/fuse` and the `fusermount3` helper (the `fuse3`
    /// package) unless running as root.
    pub fn mount(
        at: impl AsRef<Path>,
        tree: Tree,
        hydrator: Arc<Hydrator>,
        runtime: Handle,
        options: &MountOptions,
    ) -> io::Result<Self> {
        let at = at.as_ref().to_path_buf();
        let view = Arc::new(View::new(tree, hydrator));
        let fs = Fs {
            view: view.clone(),
            runtime,
            ttl: options.ttl,
            uid: nix::unistd::getuid().as_raw(),
            gid: nix::unistd::getgid().as_raw(),
        };
        let mut config = Config::default();
        config.mount_options = vec![
            MountOption::FSName(options.name.clone()),
            MountOption::Subtype("wasl".into()),
            MountOption::RO,
            MountOption::NoDev,
            MountOption::NoSuid,
            MountOption::NoAtime,
            // The kernel checks permissions against the modes above, so access() works without
            // a round trip and only the mounting user can enter the folder.
            MountOption::DefaultPermissions,
        ];
        let session = fuser::spawn_mount(fs, &at, &config)?;
        let notifier = session.notifier();
        Ok(Self {
            view,
            session: Some(session),
            notifier,
            at,
            unmount_wait: options.unmount_wait,
        })
    }

    /// Where the folder is mounted.
    pub fn mountpoint(&self) -> &Path {
        &self.at
    }

    /// The view the mount serves (for lookups by path, open-file counts, tests).
    pub fn view(&self) -> &Arc<View> {
        &self.view
    }

    /// Swaps in a new listing in one step and tells the kernel what changed. Files already open
    /// keep reading the version they opened; the next open sees the new one.
    pub fn refresh(&self, tree: Tree) {
        for change in self.view.refresh(tree) {
            // The kernel answers ENOENT for what it never cached; nothing to do then.
            let _ = match change {
                Change::Inode(ino) => self.notifier.inval_inode(INodeNo(ino), 0, 0),
                Change::Entry { parent, name } => {
                    self.notifier.inval_entry(INodeNo(parent), OsStr::new(&name))
                }
            };
        }
    }

    /// Pins a file or every file in a folder (`""`: all of it): fetched now, kept on the device.
    pub async fn pin(&self, path: &str) -> wasl_vfs::Result<()> {
        self.view.pin(path).await
    }

    /// Lets a file's (or a folder's files') blocks be evicted again.
    pub fn unpin(&self, path: &str) -> wasl_vfs::Result<()> {
        self.view.unpin(path)
    }

    pub fn is_pinned(&self, path: &str) -> bool {
        self.view.is_pinned(path)
    }

    /// Unmounts and waits (up to [`MountOptions::unmount_wait`]) for the FUSE thread to end. A
    /// folder still in use is detached lazily: it disappears now and ends when its files close.
    pub fn unmount(mut self) -> io::Result<()> {
        self.shutdown()
    }

    fn shutdown(&mut self) -> io::Result<()> {
        let Some(session) = self.session.take() else {
            return Ok(());
        };
        let (tx, rx) = mpsc::channel();
        thread::Builder::new()
            .name("wasl-fuse-unmount".into())
            .spawn(move || {
                let _ = tx.send(session.umount_and_join());
            })?;
        match rx.recv_timeout(self.unmount_wait) {
            Ok(r) => r,
            Err(_) => Err(io::Error::new(
                io::ErrorKind::TimedOut,
                format!(
                    "{} is still in use; it was detached and ends when its files close",
                    self.at.display()
                ),
            )),
        }
    }
}

impl Drop for OnDemandMount {
    fn drop(&mut self) {
        let _ = self.shutdown();
    }
}
