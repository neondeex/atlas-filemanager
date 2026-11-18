//! Linux FUSE (fuser) virtual folder mirroring the `WinFsp` callbacks of `mount.rs`.
//! Only ciphertext is persisted by filesystem callbacks.
#![cfg(target_os = "linux")]
// reason: FUSE callbacks and mount setup run on their own threads, never on the UI thread.
#![allow(clippy::disallowed_methods)]

use super::{live, Entry, Session};
use anyhow::Result;
use fuser::{
    FileAttr, FileType, Filesystem, KernelConfig, ReplyAttr, ReplyCreate, ReplyData,
    ReplyDirectory, ReplyEmpty, ReplyEntry, ReplyOpen, ReplyStatfs, ReplyWrite, Request,
};
use std::{
    collections::HashMap,
    ffi::OsStr,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
        Arc, Mutex, MutexGuard,
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const TTL: Duration = Duration::from_secs(1);
/// Names are not cached by the kernel. They match without regard to case, so
/// `A.bin` and `a.bin` are one file, but the kernel drops only the spelling a
/// rename or delete used; a cached other spelling would still lead to the old file.
const ENTRY_TTL: Duration = Duration::ZERO;
const BLOCK_SIZE: u32 = 4096;
const ROOT_INO: u64 = 1;

struct Shared {
    session: Mutex<Option<Session>>,
    files: AtomicUsize,
    /// Open handles per inode. A deleted or replaced file keeps its blocks
    /// until the last one closes.
    handles: Mutex<HashMap<u64, usize>>,
    closing: AtomicBool,
    by_inode: Mutex<HashMap<u64, Option<String>>>,
    by_id: Mutex<HashMap<String, u64>>,
    next_inode: AtomicU64,
    activity: Mutex<Instant>,
    uid: u32,
    gid: u32,
}

struct PrivateFs(Arc<Shared>);

pub struct Mounted {
    session: Option<fuser::BackgroundSession>,
    shared: Arc<Shared>,
    /// Device number of the mount, which names its FUSE connection in fusectl.
    device: Option<u64>,
    path: PathBuf,
    root: PathBuf,
    name: String,
}

fn process_uid_gid() -> (u32, u32) {
    // Safe alternative to libc::getuid/getgid; avoids the workspace's
    // `-D unsafe-code` lint and works on any Linux.
    let status = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
    let mut uid = 0u32;
    let mut gid = 0u32;
    for line in status.lines() {
        if let Some(rest) = line.strip_prefix("Uid:\t") {
            if let Some(v) = rest.split_whitespace().next() {
                uid = v.parse().unwrap_or(0);
            }
        } else if let Some(rest) = line.strip_prefix("Gid:\t") {
            if let Some(v) = rest.split_whitespace().next() {
                gid = v.parse().unwrap_or(0);
            }
        }
    }
    (uid, gid)
}

fn unix_to_systemtime(secs: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(secs)
}

fn errno(error: &anyhow::Error) -> i32 {
    let msg = format!("{error:#}");
    if msg.contains("vault_missing_entry") {
        libc::ENOENT
    } else if msg.contains("vault_name_exists") {
        libc::EEXIST
    } else if msg.contains("vault_directory_not_empty") {
        libc::ENOTEMPTY
    } else if msg.contains("vault_damaged") {
        libc::EIO
    } else if msg.contains("vault_invalid") || msg.contains("vault_not_file") {
        libc::EINVAL
    } else if msg.contains("vault_in_use") {
        libc::EBUSY
    } else if msg.contains("vault_file_too_large") || msg.contains("vault_too_many_entries") {
        libc::EFBIG
    } else if msg.contains("vault_unsafe_link") || msg.contains("vault_access_denied") {
        libc::EACCES
    } else if let Some(code) = error
        .chain()
        .find_map(|e| e.downcast_ref::<std::io::Error>())
        .and_then(std::io::Error::raw_os_error)
    {
        // A full disk, a quota or a read error under the vault reaches apps as itself
        // (ENOSPC, EDQUOT, EIO), not as a permission problem.
        code
    } else {
        libc::EIO
    }
}

fn attr_from(shared: &Shared, ino: u64, entry: Option<&Entry>) -> FileAttr {
    let (kind, size, mtime) = match entry {
        Some(e) if e.directory => (FileType::Directory, 0, e.modified()),
        Some(e) => (FileType::RegularFile, e.size, e.modified()),
        None => (FileType::Directory, 0, 0),
    };
    let perm: u16 = match kind {
        FileType::Directory => 0o755,
        _ => 0o644,
    };
    let time = unix_to_systemtime(mtime);
    FileAttr {
        ino,
        size,
        blocks: size.div_ceil(512),
        atime: time,
        mtime: time,
        ctime: time,
        crtime: time,
        kind,
        perm,
        nlink: if kind == FileType::Directory { 2 } else { 1 },
        uid: shared.uid,
        gid: shared.gid,
        rdev: 0,
        flags: 0,
        blksize: BLOCK_SIZE,
    }
}

impl Shared {
    fn session(&self) -> anyhow::Result<MutexGuard<'_, Option<Session>>> {
        let guard = self
            .session
            .lock()
            .map_err(|_| anyhow::anyhow!("vault_internal"))?;
        if guard.is_none() || self.closing.load(Ordering::Acquire) {
            anyhow::bail!("vault_device_not_ready");
        }
        Ok(guard)
    }

    fn session_mut(&self) -> anyhow::Result<MutexGuard<'_, Option<Session>>> {
        self.session()
    }

    fn inode_for(&self, id: &str) -> u64 {
        let mut by_id = self
            .by_id
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(&ino) = by_id.get(id) {
            return ino;
        }
        let ino = self.next_inode.fetch_add(1, Ordering::Relaxed);
        by_id.insert(id.to_owned(), ino);
        drop(by_id);
        self.by_inode
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(ino, Some(id.to_owned()));
        ino
    }

    fn opened(&self, ino: u64) {
        self.files.fetch_add(1, Ordering::AcqRel);
        *self
            .handles
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .entry(ino)
            .or_default() += 1;
    }

    /// Returns true when this was the inode's last open handle.
    fn closed(&self, ino: u64) -> bool {
        self.files.fetch_sub(1, Ordering::AcqRel);
        let mut handles = self
            .handles
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(count) = handles.get_mut(&ino) else {
            return true;
        };
        *count -= 1;
        if *count > 0 {
            return false;
        }
        handles.remove(&ino);
        true
    }

    fn is_open(&self, id: &str) -> bool {
        let Some(&ino) = self
            .by_id
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(id)
        else {
            return false;
        };
        self.handles
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .contains_key(&ino)
    }

    /// `None` for an unknown inode, `Some(None)` for the root.
    #[allow(clippy::option_option)] // reason: the root is the one inode without an entry id
    fn id_for(&self, ino: u64) -> Option<Option<String>> {
        self.by_inode
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&ino)
            .cloned()
    }

    fn child_id(&self, parent: u64, name: &OsStr) -> anyhow::Result<Option<String>> {
        let parent_id = self
            .id_for(parent)
            .ok_or_else(|| anyhow::anyhow!("vault_missing_entry"))?;
        let name_str = name
            .to_str()
            .ok_or_else(|| anyhow::anyhow!("vault_invalid_name"))?;
        Ok(self
            .session()?
            .as_ref()
            .unwrap()
            .children(parent_id.as_deref())
            .into_iter()
            .find(|e| super::same_name(&e.name, name_str))
            .map(|e| e.id.clone()))
    }

    fn touch(&self) {
        *self
            .activity
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Instant::now();
    }
}

impl Mounted {
    #[allow(clippy::result_large_err)] // reason: hands the session back, as `mount.rs` does
    pub fn start(session: Session) -> std::result::Result<Self, (Session, String)> {
        let root = session.root().to_path_buf();
        let name = session.name().to_owned();
        // Only the per-user runtime directory: a shared fallback such as /tmp could be
        // taken over by another user. Without it the folder opens in the encrypted browser.
        let Some(runtime) = std::env::var_os("XDG_RUNTIME_DIR").filter(|d| !d.is_empty()) else {
            return Err((
                session,
                "vault_mount_failed: XDG_RUNTIME_DIR is not set".into(),
            ));
        };
        let path = PathBuf::from(runtime)
            .join(live::NAMESPACE)
            .join(uuid::Uuid::new_v4().simple().to_string());
        if let Err(e) = session.collect_orphans() {
            return Err((session, format!("vault_mount_failed: {e:#}")));
        }
        let (uid, gid) = process_uid_gid();
        let shared = Arc::new(Shared {
            session: Mutex::new(Some(session)),
            files: AtomicUsize::new(0),
            handles: Mutex::new(HashMap::new()),
            closing: AtomicBool::new(false),
            by_inode: Mutex::new(HashMap::from([(ROOT_INO, None)])),
            by_id: Mutex::new(HashMap::new()),
            next_inode: AtomicU64::new(ROOT_INO + 1),
            activity: Mutex::new(Instant::now()),
            uid,
            gid,
        });
        let result = (|| -> anyhow::Result<fuser::BackgroundSession> {
            let parent = path
                .parent()
                .ok_or_else(|| anyhow::anyhow!("mount path has no parent"))?;
            std::fs::create_dir_all(parent)?;
            remove_stale_mounts(parent);
            // No AutoUnmount: it needs `allow_other`, which would expose plaintext to
            // other users and fails unless /etc/fuse.conf sets user_allow_other.
            // Drop unmounts; mounts orphaned by a crash are removed on the next start.
            let options = vec![
                fuser::MountOption::FSName("atlas-private".into()),
                fuser::MountOption::Subtype("atlas".into()),
                fuser::MountOption::DefaultPermissions,
                fuser::MountOption::NoDev,
                fuser::MountOption::NoSuid,
            ];
            // Another Atlas mount's `remove_stale_mounts` removes empty folders here, so
            // the new, still empty mount point can vanish before it is mounted: retry.
            let mut attempt = 0;
            loop {
                std::fs::create_dir_all(&path)?;
                match fuser::spawn_mount2(PrivateFs(shared.clone()), &path, &options) {
                    Err(_) if attempt < 3 && !path.exists() => attempt += 1,
                    result => return Ok(result?),
                }
            }
        })();
        match result {
            Ok(bg) => {
                live::register(path.clone(), name.clone());
                let device = std::fs::metadata(&path).ok().map(|m| m.dev());
                Ok(Self {
                    session: Some(bg),
                    shared,
                    device,
                    path,
                    root,
                    name,
                })
            }
            Err(e) => Err((
                shared
                    .session
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .take()
                    .expect("session present on failure"),
                format!("vault_mount_failed: {e:#}"),
            )),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
    pub fn root(&self) -> &Path {
        &self.root
    }
    pub fn name(&self) -> &str {
        &self.name
    }
    pub fn has_open_files(&self) -> bool {
        self.shared.files.load(Ordering::Acquire) != 0
    }
    pub fn prepare_lock(&self) -> bool {
        let _guard = self
            .shared
            .session
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.shared.closing.store(true, Ordering::Release);
        if self.has_open_files() {
            self.shared.closing.store(false, Ordering::Release);
            false
        } else {
            true
        }
    }
    pub fn idle_for(&self) -> Duration {
        self.shared
            .activity
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .elapsed()
    }
    pub fn touch(&self) {
        self.shared.touch();
    }
    pub fn into_session(mut self) -> Session {
        self.detach();
        self.shared
            .session
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
            .expect("session present until into_session")
    }
    fn detach(&mut self) {
        self.shared.closing.store(true, Ordering::Release);
        if let Some(bg) = self.session.take() {
            // fuser unmounts lazily (`fusermount3 -u -z`). A mount that is still in use,
            // such as a shell's working directory, leaves the file tree at once but keeps
            // its FUSE connection, and the session thread serves it until its last user
            // is gone. So the join runs on its own thread: wait for it briefly, then abort
            // the connection (as WinFsp unmounts by force), and never block the caller,
            // the UI thread, any longer.
            let (done_tx, done) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                bg.join();
                let _ = done_tx.send(());
            });
            let busy = |done: &std::sync::mpsc::Receiver<()>| {
                matches!(
                    done.recv_timeout(Duration::from_secs(2)),
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout)
                )
            };
            // Checked again right before the abort: once the session has ended, its
            // connection number can be reused by another FUSE mount.
            if busy(&done) && matches!(done.try_recv(), Err(std::sync::mpsc::TryRecvError::Empty)) {
                if let Some(device) = self.device {
                    abort_connection(device);
                }
                let _ = busy(&done);
            }
        }
        live::unregister(&self.path);
        let _ = std::fs::remove_dir(&self.path);
    }
}

impl Drop for Mounted {
    fn drop(&mut self) {
        self.detach();
        self.shared
            .session
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
    }
}

/// Ends the FUSE connection of the mount with this device number, so its session
/// thread stops even while the lazily unmounted folder is still in use. The mounting
/// user owns the connection's `abort` file in fusectl.
fn abort_connection(device: u64) {
    let minor = (device & 0xff) | ((device >> 12) & 0xffff_ff00);
    let _ = std::fs::write(format!("/sys/fs/fuse/connections/{minor}/abort"), "1");
}

/// Lazily unmount mount points whose FUSE process is gone (they report ENOTCONN)
/// and remove empty leftovers. Live mounts of other Atlas processes are untouched.
fn remove_stale_mounts(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let stale =
            matches!(std::fs::metadata(&path), Err(e) if e.raw_os_error() == Some(libc::ENOTCONN));
        if stale {
            let _ = std::process::Command::new("fusermount3")
                .args(["-u", "-z"])
                .arg(&path)
                .status();
        }
        // Succeeds only for an empty, unmounted directory.
        let _ = std::fs::remove_dir(&path);
    }
}

#[allow(clippy::unused_async)] // reason: same signature as the WinFsp `install_driver`
pub async fn install_driver() -> Result<()> {
    anyhow::bail!(
        "vault_driver_already_installed: FUSE is provided by the system (pacman -S fuse3)"
    )
}

impl Filesystem for PrivateFs {
    fn init(&mut self, _req: &Request, _config: &mut KernelConfig) -> Result<(), libc::c_int> {
        Ok(())
    }

    fn lookup(&mut self, _req: &Request, parent: u64, name: &OsStr, reply: ReplyEntry) {
        self.0.touch();
        match self.0.child_id(parent, name) {
            Ok(Some(id)) => {
                let ino = self.0.inode_for(&id);
                let attr = {
                    let Ok(guard) = self.0.session() else {
                        reply.error(libc::EIO);
                        return;
                    };
                    let s = guard.as_ref().unwrap();
                    // Removed between the name lookup and here.
                    let Some(entry) = s.entry(&id) else {
                        reply.error(libc::ENOENT);
                        return;
                    };
                    attr_from(&self.0, ino, Some(entry))
                };
                reply.entry(&ENTRY_TTL, &attr, 0);
            }
            Ok(None) if parent == ROOT_INO => {
                // No child by that name — unless `name` is a stale inode the
                // kernel still remembers. That case is handled by the kernel.
                reply.error(libc::ENOENT);
            }
            Ok(None) => reply.error(libc::ENOENT),
            Err(e) => reply.error(errno(&e)),
        }
    }

    fn getattr(&mut self, _req: &Request, ino: u64, _fh: Option<u64>, reply: ReplyAttr) {
        self.0.touch();
        let Some(id) = self.0.id_for(ino) else {
            reply.error(libc::ENOENT);
            return;
        };
        let Ok(guard) = self.0.session() else {
            reply.error(libc::EIO);
            return;
        };
        let s = guard.as_ref().unwrap();
        // Only the root has no entry; any other inode without one was deleted.
        let entry = match id.as_deref() {
            None => None,
            Some(id) => {
                let Some(entry) = s.entry(id) else {
                    reply.error(libc::ENOENT);
                    return;
                };
                Some(entry)
            }
        };
        reply.attr(&TTL, &attr_from(&self.0, ino, entry));
    }

    fn setattr(
        &mut self,
        _req: &Request,
        ino: u64,
        _mode: Option<u32>,
        _uid: Option<u32>,
        _gid: Option<u32>,
        size: Option<u64>,
        _atime: Option<fuser::TimeOrNow>,
        mtime: Option<fuser::TimeOrNow>,
        _ctime: Option<SystemTime>,
        _fh: Option<u64>,
        _crtime: Option<SystemTime>,
        _chgtime: Option<SystemTime>,
        _bkuptime: Option<SystemTime>,
        _flags: Option<u32>,
        reply: ReplyAttr,
    ) {
        self.0.touch();
        if ino == ROOT_INO {
            // `touch` on the mount point: the root has no stored metadata to change.
            reply.attr(&TTL, &attr_from(&self.0, ino, None));
            return;
        }
        let Some(id) = self.0.id_for(ino).flatten() else {
            reply.error(libc::ENOENT);
            return;
        };
        if let Some(size) = size {
            let Ok(mut guard) = self.0.session_mut() else {
                reply.error(libc::EIO);
                return;
            };
            let s = guard.as_mut().unwrap();
            if let Err(e) = s.resize(&id, size) {
                reply.error(errno(&e));
                return;
            }
        }
        // The modification time is kept, as on Windows (`touch -d`, `cp -p`). Mode,
        // owner and access time are not stored and are ignored, as on a FAT disk.
        if let Some(time) = mtime {
            let time = match time {
                fuser::TimeOrNow::SpecificTime(time) => time,
                fuser::TimeOrNow::Now => SystemTime::now(),
            };
            let secs = time.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
            let Ok(mut guard) = self.0.session_mut() else {
                reply.error(libc::EIO);
                return;
            };
            let s = guard.as_mut().unwrap();
            let attributes = s.entry(&id).map_or(0, super::Entry::attributes);
            if let Err(e) = s.set_metadata(&id, attributes, Some(secs)) {
                reply.error(errno(&e));
                return;
            }
        }
        let Ok(guard) = self.0.session() else {
            reply.error(libc::EIO);
            return;
        };
        let s = guard.as_ref().unwrap();
        let Some(entry) = s.entry(&id) else {
            reply.error(libc::ENOENT);
            return;
        };
        reply.attr(&TTL, &attr_from(&self.0, ino, Some(entry)));
    }

    fn open(&mut self, _req: &Request, ino: u64, _flags: i32, reply: ReplyOpen) {
        self.0.touch();
        let Some(Some(id)) = self.0.id_for(ino) else {
            reply.error(libc::ENOENT);
            return;
        };
        // Count the handle under the session lock, so `prepare_lock` either sees it
        // or this open fails once Lock has started.
        let Ok(guard) = self.0.session() else {
            reply.error(libc::EIO);
            return;
        };
        // A deleted or replaced file is not opened again.
        if guard.as_ref().unwrap().entry(&id).is_none() {
            reply.error(libc::ENOENT);
            return;
        }
        self.0.opened(ino);
        reply.opened(ino, 0);
    }

    fn read(
        &mut self,
        _req: &Request,
        ino: u64,
        _fh: u64,
        offset: i64,
        size: u32,
        _flags: i32,
        _lock: Option<u64>,
        reply: ReplyData,
    ) {
        self.0.touch();
        let Some(id) = self.0.id_for(ino).flatten() else {
            reply.error(libc::ENOENT);
            return;
        };
        let Ok(guard) = self.0.session() else {
            reply.error(libc::EIO);
            return;
        };
        let s = guard.as_ref().unwrap();
        let mut buffer = vec![0u8; size as usize];
        match s.read_range(&id, offset as u64, &mut buffer) {
            Ok(n) => reply.data(&buffer[..n]),
            Err(e) => reply.error(errno(&e)),
        }
    }

    fn write(
        &mut self,
        _req: &Request,
        ino: u64,
        _fh: u64,
        offset: i64,
        data: &[u8],
        _write_flags: u32,
        _flags: i32,
        _lock_owner: Option<u64>,
        reply: ReplyWrite,
    ) {
        self.0.touch();
        let Some(id) = self.0.id_for(ino).flatten() else {
            reply.error(libc::ENOENT);
            return;
        };
        let Ok(mut guard) = self.0.session_mut() else {
            reply.error(libc::EIO);
            return;
        };
        let s = guard.as_mut().unwrap();
        match s.write_at(&id, offset as u64, data) {
            Ok(n) => reply.written(n as u32),
            Err(e) => reply.error(errno(&e)),
        }
    }

    fn flush(&mut self, _req: &Request, _ino: u64, _fh: u64, _lock_owner: u64, reply: ReplyEmpty) {
        // Every write acknowledges only after encrypted objects and manifest are
        // synced. Nothing to do here — the ciphertext is already durable.
        reply.ok();
    }

    fn release(
        &mut self,
        _req: &Request,
        ino: u64,
        _fh: u64,
        _flags: i32,
        _lock_owner: Option<u64>,
        _flush: bool,
        reply: ReplyEmpty,
    ) {
        self.0.touch();
        if let Some(Some(id)) = self.0.id_for(ino) {
            if self.0.closed(ino) {
                // Last handle on this inode: drop any retired block set.
                if let Ok(mut guard) = self.0.session_mut() {
                    if let Some(s) = guard.as_mut() {
                        s.release_entry(&id);
                    }
                }
            }
        }
        reply.ok();
    }

    fn readdir(
        &mut self,
        _req: &Request,
        ino: u64,
        _fh: u64,
        offset: i64,
        mut reply: ReplyDirectory,
    ) {
        self.0.touch();
        let Some(parent_id) = self.0.id_for(ino) else {
            reply.error(libc::ENOENT);
            return;
        };
        let Ok(guard) = self.0.session() else {
            reply.error(libc::EIO);
            return;
        };
        let s = guard.as_ref().unwrap();

        let mut entries: Vec<(u64, FileType, String)> = Vec::new();

        // ".." — parent inode (or self for root).
        let dotdot_ino = match &parent_id {
            Some(id) => s
                .entry(id)
                .and_then(|e| e.parent.clone())
                .map_or(ROOT_INO, |pid| self.0.inode_for(&pid)),
            None => ROOT_INO,
        };
        // "." — current inode.
        entries.push((ino, FileType::Directory, ".".into()));
        entries.push((dotdot_ino, FileType::Directory, "..".into()));

        for child in s.children(parent_id.as_deref()) {
            let child_ino = self.0.inode_for(&child.id);
            let kind = if child.directory {
                FileType::Directory
            } else {
                FileType::RegularFile
            };
            entries.push((child_ino, kind, child.name.clone()));
        }

        for (i, (child_ino, kind, name)) in
            entries.into_iter().enumerate().skip(offset.max(0) as usize)
        {
            if reply.add(child_ino, (i + 1) as i64, kind, name) {
                break;
            }
        }
        reply.ok();
    }

    fn create(
        &mut self,
        _req: &Request,
        parent: u64,
        name: &OsStr,
        _mode: u32,
        _umask: u32,
        _flags: i32,
        reply: ReplyCreate,
    ) {
        self.0.touch();
        let Some(parent_id) = self.0.id_for(parent) else {
            reply.error(libc::ENOENT);
            return;
        };
        let Ok(name_str) = name
            .to_str()
            .ok_or_else(|| anyhow::anyhow!("vault_invalid_name"))
        else {
            reply.error(libc::EINVAL);
            return;
        };
        let Ok(mut guard) = self.0.session_mut() else {
            reply.error(libc::EIO);
            return;
        };
        let s = guard.as_mut().unwrap();
        match s.create_entry(parent_id.as_deref(), name_str, false) {
            Ok(id) => {
                let ino = self.0.inode_for(&id);
                self.0.opened(ino);
                let entry = s.entry(&id);
                reply.created(&ENTRY_TTL, &attr_from(&self.0, ino, entry), 0, ino, 0);
            }
            Err(e) => reply.error(errno(&e)),
        }
    }

    fn mkdir(
        &mut self,
        _req: &Request,
        parent: u64,
        name: &OsStr,
        _mode: u32,
        _umask: u32,
        reply: ReplyEntry,
    ) {
        self.0.touch();
        let Some(parent_id) = self.0.id_for(parent) else {
            reply.error(libc::ENOENT);
            return;
        };
        let Ok(name_str) = name
            .to_str()
            .ok_or_else(|| anyhow::anyhow!("vault_invalid_name"))
        else {
            reply.error(libc::EINVAL);
            return;
        };
        let Ok(mut guard) = self.0.session_mut() else {
            reply.error(libc::EIO);
            return;
        };
        let s = guard.as_mut().unwrap();
        match s.create_entry(parent_id.as_deref(), name_str, true) {
            Ok(id) => {
                let ino = self.0.inode_for(&id);
                let entry = s.entry(&id);
                reply.entry(&ENTRY_TTL, &attr_from(&self.0, ino, entry), 0);
            }
            Err(e) => reply.error(errno(&e)),
        }
    }

    fn unlink(&mut self, _req: &Request, parent: u64, name: &OsStr, reply: ReplyEmpty) {
        self.0.touch();
        let child = self.0.child_id(parent, name);
        let child_id = match child {
            Ok(Some(id)) => id,
            Ok(None) => {
                reply.error(libc::ENOENT);
                return;
            }
            Err(e) => {
                reply.error(errno(&e));
                return;
            }
        };
        let Ok(mut guard) = self.0.session_mut() else {
            reply.error(libc::EIO);
            return;
        };
        let s = guard.as_mut().unwrap();
        match s.delete_entry(&child_id) {
            Ok(()) => {
                // Nothing has it open: its blocks go now, not at the next unlock.
                if !self.0.is_open(&child_id) {
                    s.release_entry(&child_id);
                }
                reply.ok();
            }
            Err(e) => reply.error(errno(&e)),
        }
    }

    fn rmdir(&mut self, req: &Request, parent: u64, name: &OsStr, reply: ReplyEmpty) {
        // Same code path as unlink: `delete_entry` enforces "directory empty".
        self.unlink(req, parent, name, reply);
    }

    fn rename(
        &mut self,
        _req: &Request,
        parent: u64,
        name: &OsStr,
        newparent: u64,
        newname: &OsStr,
        flags: u32,
        reply: ReplyEmpty,
    ) {
        self.0.touch();
        let child = self.0.child_id(parent, name);
        let child_id = match child {
            Ok(Some(id)) => id,
            Ok(None) => {
                reply.error(libc::ENOENT);
                return;
            }
            Err(e) => {
                reply.error(errno(&e));
                return;
            }
        };
        let Some(newparent_id) = self.0.id_for(newparent) else {
            reply.error(libc::ENOENT);
            return;
        };
        let Ok(new_name_str) = newname
            .to_str()
            .ok_or_else(|| anyhow::anyhow!("vault_invalid_name"))
        else {
            reply.error(libc::EINVAL);
            return;
        };
        // RENAME_NOREPLACE (Linux 3.15+) forbids overwriting an existing name.
        let replace = flags & libc::RENAME_NOREPLACE == 0;
        let Ok(mut guard) = self.0.session_mut() else {
            reply.error(libc::EIO);
            return;
        };
        let s = guard.as_mut().unwrap();
        // The file this rename replaces, matched the way `rename_entry` matches it.
        let replaced = s
            .children(newparent_id.as_deref())
            .into_iter()
            .find(|e| e.id != child_id && super::same_name(&e.name, new_name_str))
            .map(|e| e.id.clone());
        match s.rename_entry(&child_id, newparent_id.as_deref(), new_name_str, replace) {
            Ok(()) => {
                if let Some(old) = replaced.filter(|id| !self.0.is_open(id)) {
                    s.release_entry(&old);
                }
                reply.ok();
            }
            Err(e) => reply.error(errno(&e)),
        }
    }

    fn statfs(&mut self, _req: &Request, _ino: u64, reply: ReplyStatfs) {
        let Ok(guard) = self.0.session() else {
            reply.error(libc::EIO);
            return;
        };
        let s = guard.as_ref().unwrap();
        let (blocks, bfree, bavail) =
            match (fs2::total_space(s.root()), fs2::available_space(s.root())) {
                (Ok(total), Ok(free)) => (
                    total.div_ceil(u64::from(BLOCK_SIZE)),
                    free.div_ceil(u64::from(BLOCK_SIZE)),
                    free.div_ceil(u64::from(BLOCK_SIZE)),
                ),
                _ => (0, 0, 0),
            };
        let files_used = s.children(None).len() as u64;
        // Conservative hard cap; matches MAX_ENTRIES in vault.rs.
        let files_total = 100_000u64;
        reply.statfs(
            blocks,
            bfree,
            bavail,
            files_used,
            files_total.saturating_sub(files_used),
            BLOCK_SIZE,
            255, // max filename length
            BLOCK_SIZE,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn errors_reach_apps_as_their_errno() {
        let full = anyhow::Error::from(std::io::Error::from_raw_os_error(libc::ENOSPC))
            .context("writing a block");
        assert_eq!(errno(&full), libc::ENOSPC);
        assert_eq!(errno(&anyhow::anyhow!("vault_file_too_large")), libc::EFBIG);
        assert_eq!(errno(&anyhow::anyhow!("vault_name_exists")), libc::EEXIST);
        assert_eq!(errno(&anyhow::anyhow!("vault_missing_entry")), libc::ENOENT);
        assert_eq!(errno(&anyhow::anyhow!("unexpected")), libc::EIO);
    }
}
