//! Handle-bound retirement of individually reviewed temporary files. No Shell
//! fallback, recursive deletion, elevation, ownership changes or link following.
use atlas_model::cleanup::{AncestorIdentity, CleanupCandidate, FileIdentity};
use std::{
    io,
    mem::{size_of, zeroed},
    os::windows::ffi::OsStrExt,
    path::{Component, Path},
};
use windows_sys::Win32::{
    Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE},
    Storage::FileSystem::{
        CreateFileW, FileDispositionInfo, FileIdInfo, GetDiskFreeSpaceExW, GetDriveTypeW,
        GetFileInformationByHandle, GetFileInformationByHandleEx, SetFileInformationByHandle,
        BY_HANDLE_FILE_INFORMATION, FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_REPARSE_POINT,
        FILE_DISPOSITION_INFO, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
        FILE_ID_INFO, FILE_READ_ATTRIBUTES, FILE_READ_DATA, FILE_SHARE_READ, OPEN_EXISTING,
    },
};

struct Handle(HANDLE);
impl Drop for Handle {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0);
        }
    }
}

/// Keeps a directory from being replaced or changed into a reparse point while
/// a caller enumerates it. Parent leases must outlive child leases.
pub struct DirectoryLease {
    _handles: Vec<Handle>,
}

pub fn pin_directory(path: &Path, chain: bool) -> io::Result<DirectoryLease> {
    let paths = if chain {
        path.ancestors()
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect()
    } else {
        vec![path]
    };
    let mut handles = Vec::new();
    let mut volume = None;
    for path in paths {
        let handle = open(path, false)?;
        let (id, attrs) = identity(&handle)?;
        if attrs & FILE_ATTRIBUTE_DIRECTORY == 0 || volume.is_some_and(|v| v != id.volume) {
            return Err(io::Error::other("cleanup_scope_changed"));
        }
        volume = Some(id.volume);
        handles.push(handle);
    }
    Ok(DirectoryLease { _handles: handles })
}

fn wide(path: &Path) -> Vec<u16> {
    path.as_os_str().encode_wide().chain(Some(0)).collect()
}

fn open(path: &Path, delete: bool) -> io::Result<Handle> {
    let encoded = wide(path);
    // Attribute-only handles do not participate fully in Windows share-access
    // enforcement. READ_DATA (LIST_DIRECTORY for directories) is essential:
    // without it a concurrent rename can succeed despite omitting SHARE_DELETE.
    let h = unsafe {
        CreateFileW(
            encoded.as_ptr(),
            FILE_READ_ATTRIBUTES | FILE_READ_DATA | if delete { 0x10000 } else { 0 },
            // Deny writes, renames and removal while the identity chain is held.
            FILE_SHARE_READ,
            std::ptr::null(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
            0,
        )
    };
    if h == INVALID_HANDLE_VALUE {
        Err(io::Error::last_os_error())
    } else {
        Ok(Handle(h))
    }
}

fn identity(handle: &Handle) -> io::Result<(FileIdentity, u32)> {
    let mut id: FILE_ID_INFO = unsafe { zeroed() };
    if unsafe {
        GetFileInformationByHandleEx(
            handle.0,
            FileIdInfo,
            (&mut id as *mut FILE_ID_INFO).cast(),
            size_of::<FILE_ID_INFO>() as u32,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { zeroed() };
    if unsafe { GetFileInformationByHandle(handle.0, &mut info) } == 0 {
        return Err(io::Error::last_os_error());
    }
    if info.dwFileAttributes & (FILE_ATTRIBUTE_REPARSE_POINT | 0x1000 | 0x40000 | 0x400000) != 0 {
        return Err(io::Error::other("cleanup_unsafe_link"));
    }
    Ok((
        FileIdentity {
            volume: id.VolumeSerialNumber,
            id: id.FileId.Identifier,
            bytes: (u64::from(info.nFileSizeHigh) << 32) | u64::from(info.nFileSizeLow),
            modified: (u64::from(info.ftLastWriteTime.dwHighDateTime) << 32)
                | u64::from(info.ftLastWriteTime.dwLowDateTime),
            links: info.nNumberOfLinks,
        },
        info.dwFileAttributes,
    ))
}

struct Pinned {
    // Ancestors remain pinned until AFTER the file handle has been retired.
    file: Handle,
    _ancestors: Vec<Handle>,
    candidate: CleanupCandidate,
}

fn pin(path: &Path, delete: bool) -> io::Result<Pinned> {
    if !path.is_absolute()
        || path.components().any(|c| match c {
            Component::ParentDir | Component::CurDir => true,
            Component::Normal(s) => s.to_string_lossy().contains(':'),
            Component::Prefix(prefix) => !matches!(prefix.kind(), std::path::Prefix::Disk(_)),
            _ => false,
        })
    {
        return Err(io::Error::other("cleanup_scope_changed"));
    }
    let mut ancestors = Vec::new();
    let mut handles = Vec::new();
    for parent in path
        .ancestors()
        .skip(1)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
    {
        let handle = open(parent, false)?;
        let (id, attrs) = identity(&handle)?;
        if attrs & FILE_ATTRIBUTE_DIRECTORY == 0 {
            return Err(io::Error::other("cleanup_scope_changed"));
        }
        if ancestors
            .last()
            .is_some_and(|a: &AncestorIdentity| a.volume != id.volume)
        {
            return Err(io::Error::other("cleanup_scope_changed"));
        }
        ancestors.push(AncestorIdentity {
            path: parent.to_owned(),
            volume: id.volume,
            id: id.id,
        });
        handles.push(handle);
    }
    let file = open(path, delete)?;
    let (id, attrs) = identity(&file)?;
    if attrs & FILE_ATTRIBUTE_DIRECTORY != 0
        || id.links != 1
        || ancestors.last().is_none_or(|a| a.volume != id.volume)
    {
        return Err(io::Error::other("cleanup_unsafe_link"));
    }
    Ok(Pinned {
        file,
        _ancestors: handles,
        candidate: CleanupCandidate {
            path: path.to_owned(),
            identity: id,
            ancestors,
        },
    })
}

pub fn inspect(path: &Path) -> io::Result<CleanupCandidate> {
    Ok(pin(path, false)?.candidate.clone())
}

/// Fails if an ancestor or the leaf changed, acquired another hardlink, or has a
/// writer/deleter. Only marks this exact file handle; never resolves its name to
/// perform the deletion. Empty directories are intentionally retained.
pub fn delete_verified(expected: &CleanupCandidate) -> io::Result<u64> {
    let pinned = pin(&expected.path, true)?;
    if pinned.candidate.identity != expected.identity
        || pinned.candidate.ancestors != expected.ancestors
    {
        return Err(io::Error::other("cleanup_scope_changed"));
    }
    let disposition = FILE_DISPOSITION_INFO { DeleteFile: 1 };
    if unsafe {
        SetFileInformationByHandle(
            pinned.file.0,
            FileDispositionInfo,
            (&disposition as *const FILE_DISPOSITION_INFO).cast(),
            size_of::<FILE_DISPOSITION_INFO>() as u32,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    let bytes = expected.identity.bytes;
    drop(pinned);
    Ok(bytes)
}

pub fn capacity(path: &Path) -> Option<(u64, u64)> {
    let path = wide(path);
    let (mut free, mut total, mut all_free) = (0, 0, 0);
    if unsafe { GetDiskFreeSpaceExW(path.as_ptr(), &mut free, &mut total, &mut all_free) } == 0 {
        None
    } else {
        Some((total, free))
    }
}

pub fn local_drive(path: &Path) -> bool {
    // Fixed or removable local filesystem only, never network/optical/RAM.
    matches!(unsafe { GetDriveTypeW(wide(path).as_ptr()) }, 2 | 3)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> std::path::PathBuf {
        // The Windows clock is coarse: parallel tests can read the same instant.
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "atlas-cleanup-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).unwrap();
        path
    }
    #[test]
    fn changed_leaf_and_ancestor_are_rejected() {
        let root = fixture();
        let dir = root.join("old");
        std::fs::create_dir(&dir).unwrap();
        let file = dir.join("file");
        std::fs::write(&file, b"original").unwrap();
        let expected = inspect(&file).unwrap();
        std::fs::rename(&dir, root.join("saved")).unwrap();
        std::fs::create_dir(&dir).unwrap();
        std::fs::write(&file, b"sentinel").unwrap();
        assert!(delete_verified(&expected).is_err());
        assert_eq!(std::fs::read(&file).unwrap(), b"sentinel");
        std::fs::remove_file(&file).unwrap();
        std::fs::remove_file(root.join("saved/file")).unwrap();
        std::fs::remove_dir(&dir).unwrap();
        std::fs::remove_dir(root.join("saved")).unwrap();
        std::fs::remove_dir(root).unwrap();
    }
    #[test]
    fn hardlinks_are_refused_and_only_reviewed_leaf_is_deleted() {
        let root = fixture();
        let file = root.join("file");
        let other = root.join("other");
        std::fs::write(&file, b"temporary").unwrap();
        let expected = inspect(&file).unwrap();
        std::fs::hard_link(&file, &other).unwrap();
        assert!(delete_verified(&expected).is_err());
        std::fs::remove_file(&other).unwrap();
        assert_eq!(delete_verified(&expected).unwrap(), 9);
        assert!(!file.exists());
        std::fs::remove_dir(root).unwrap();
    }
    #[test]
    fn an_open_writer_prevents_retirement() {
        let root = fixture();
        let file = root.join("file");
        std::fs::write(&file, b"data").unwrap();
        let expected = inspect(&file).unwrap();
        let writer = std::fs::OpenOptions::new().write(true).open(&file).unwrap();
        assert!(delete_verified(&expected).is_err());
        assert!(file.exists());
        drop(writer);
        delete_verified(&expected).unwrap();
        std::fs::remove_dir(root).unwrap();
    }
    #[test]
    fn directory_lease_blocks_replacement_until_released() {
        let root = fixture();
        let dir = root.join("parent");
        std::fs::create_dir(&dir).unwrap();
        let lease = pin_directory(&dir, true).unwrap();
        assert!(std::fs::rename(&dir, root.join("replacement")).is_err());
        drop(lease);
        std::fs::rename(&dir, root.join("replacement")).unwrap();
        std::fs::remove_dir(root.join("replacement")).unwrap();
        std::fs::remove_dir(root).unwrap();
    }
    #[test]
    fn modified_file_and_substituted_leaf_survive() {
        let root = fixture();
        let file = root.join("file");
        std::fs::write(&file, b"old").unwrap();
        let expected = inspect(&file).unwrap();
        std::fs::write(&file, b"new data").unwrap();
        assert!(delete_verified(&expected).is_err());
        assert_eq!(std::fs::read(&file).unwrap(), b"new data");
        std::fs::remove_file(&file).unwrap();
        std::fs::write(&file, b"old").unwrap();
        assert!(delete_verified(&expected).is_err());
        std::fs::remove_file(file).unwrap();
        std::fs::remove_dir(root).unwrap();
    }
    #[test]
    fn junction_ancestor_is_rejected_and_external_sentinel_survives() {
        let root = fixture();
        let target = root.join("target");
        let link = root.join("link");
        std::fs::create_dir(&target).unwrap();
        std::fs::write(target.join("sentinel"), b"keep").unwrap();
        let status = std::process::Command::new("cmd.exe")
            .args(["/c", "mklink", "/J"])
            .arg(&link)
            .arg(&target)
            .output()
            .unwrap();
        assert!(status.status.success(), "junction fixture setup failed");
        assert!(inspect(&link.join("sentinel")).is_err());
        assert!(pin_directory(&link, true).is_err());
        assert_eq!(std::fs::read(target.join("sentinel")).unwrap(), b"keep");
        std::fs::remove_dir(link).unwrap();
        std::fs::remove_file(target.join("sentinel")).unwrap();
        std::fs::remove_dir(target).unwrap();
        std::fs::remove_dir(root).unwrap();
    }
}
