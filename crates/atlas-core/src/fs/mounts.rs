#![allow(dead_code)] // reason: retained API surface not exercised by every GPUI flow yet
use std::path::PathBuf;
use std::time::{Duration, Instant};

use std::sync::{Arc, Mutex, OnceLock};

thread_local! {
    static MOUNT_CACHE: std::cell::RefCell<Option<(Instant, Vec<PathBuf>)>> = const { std::cell::RefCell::new(None) };
}

static DRIVE_CACHE: OnceLock<Arc<Mutex<Option<(Instant, Vec<DriveInfo>, bool)>>>> = OnceLock::new();

/// A mounted volume with capacity info, used to render a fullness bar in the sidebar.
#[derive(Debug, Clone)]
pub struct DriveInfo {
    /// Human label (volume name, or the mount point basename / drive root as fallback).
    pub label: String,
    pub mount_point: PathBuf,
    pub total_bytes: u64,
    pub available_bytes: u64,
    pub is_removable: bool,
    pub is_system: bool,
}

impl DriveInfo {
    /// Fraction of the volume in use, `0.0..=1.0`. Zero when total is unknown.
    pub fn used_fraction(&self) -> f32 {
        if self.total_bytes == 0 {
            return 0.0;
        }
        let used = self.total_bytes.saturating_sub(self.available_bytes);
        (used as f64 / self.total_bytes as f64).clamp(0.0, 1.0) as f32
    }
}

/// Enumerate mounted volumes with capacity, cached for 5s (sysinfo refresh is not free).
/// Cross-platform via `sysinfo`: on Windows these are drive roots like `C:\`, on
/// macOS `/` plus `/Volumes/*`, on Linux the real mounted filesystems.
pub fn list_drives() -> Vec<DriveInfo> {
    let cache_arc = DRIVE_CACHE
        .get_or_init(|| Arc::new(Mutex::new(None)))
        .clone();
    let mut cache = cache_arc
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);

    if let Some((last_update, drives, is_updating)) = &mut *cache {
        if last_update.elapsed() < Duration::from_secs(5) {
            return drives.clone();
        }
        if !*is_updating {
            *is_updating = true;
            let cache_clone = cache_arc.clone();
            std::thread::spawn(move || {
                let new_drives = list_drives_internal();
                if let Some(mut c) = cache_clone.lock().or_else(atlas_model::sync::recover).ok() {
                    *c = Some((Instant::now(), new_drives, false));
                }
            });
        }
        return drives.clone();
    }

    let drives = list_drives_internal();
    *cache = Some((Instant::now(), drives.clone(), false));
    drives
}

fn list_drives_internal() -> Vec<DriveInfo> {
    let disks = sysinfo::Disks::new_with_refreshed_list();
    let mut drives: Vec<(DriveInfo, String)> = disks
        .list()
        .iter()
        .map(|disk| {
            let mount_point = disk.mount_point().to_path_buf();
            let name = disk.name().to_string_lossy();
            let label = if name.trim().is_empty() {
                mount_point.file_name().map_or_else(
                    || mount_point.to_string_lossy().into_owned(),
                    |s| s.to_string_lossy().into_owned(),
                )
            } else {
                name.into_owned()
            };
            let is_system = is_system_mount(&mount_point);
            let is_removable = disk.is_removable();
            let drive = DriveInfo {
                label,
                mount_point,
                total_bytes: disk.total_space(),
                available_bytes: disk.available_space(),
                is_removable,
                is_system,
            };
            (drive, disk.name().to_string_lossy().into_owned())
        })
        .collect();
    drives.sort_by(|a, b| a.0.mount_point.cmp(&b.0.mount_point));
    drives.dedup_by(|a, b| a.0.mount_point == b.0.mount_point);
    if cfg!(target_os = "linux") {
        drives = user_visible_linux_mounts(drives);
    }
    drives.into_iter().map(|(drive, _)| drive).collect()
}

/// Mount points under these hold the operating system, not user files; GIO counts
/// most of them as system mounts too. `/etc` and `/sysroot` are bind mounts of the
/// root disk on Fedora Atomic.
const LINUX_SYSTEM_MOUNT_ROOTS: &[&str] = &[
    "/boot", "/efi", "/etc", "/sysroot", "/usr", "/var", "/tmp", "/snap",
];

/// Fedora Atomic keeps `/home`, `/mnt`, `/opt` and `/srv` here, so a home partition or
/// a second disk is mounted under `/var`.
const LINUX_USER_MOUNT_ROOTS_IN_VAR: &[&str] = &["/var/home", "/var/mnt", "/var/opt", "/var/srv"];

fn is_linux_system_mount(path: &std::path::Path) -> bool {
    LINUX_SYSTEM_MOUNT_ROOTS
        .iter()
        .any(|root| path.starts_with(root))
        && !LINUX_USER_MOUNT_ROOTS_IN_VAR
            .iter()
            .any(|root| path.starts_with(root))
}

/// Linux lists every mount of a disk: Fedora's btrfs layout mounts one partition at `/`
/// and `/home` (its cloud image also at `/var` and `/boot`), plus the EFI partition at
/// `/boot/efi`. Drop system mounts, then keep one entry per block device, the mount
/// point nearest the root (fewest components, then shortest, so `/` wins over `/home`),
/// so the sidebar shows disks the way Windows shows drive letters.
/// Fedora Atomic's `/` is composefs, which reports the image's size, so its root disk
/// shows as `/var/home`, the one user mount it has.
/// `drives` must be sorted by mount point; the second field is the mount source.
fn user_visible_linux_mounts(drives: Vec<(DriveInfo, String)>) -> Vec<(DriveInfo, String)> {
    let drives: Vec<_> = drives
        .into_iter()
        .filter(|(drive, _)| !is_linux_system_mount(&drive.mount_point))
        .collect();
    let nearness = |path: &std::path::Path| (path.components().count(), path.as_os_str().len());
    let mut nearest = std::collections::HashMap::new();
    for (drive, device) in drives
        .iter()
        .filter(|(_, device)| device.starts_with("/dev/"))
    {
        let best = nearest.entry(device.as_str()).or_insert(&drive.mount_point);
        if nearness(&drive.mount_point) < nearness(best) {
            *best = &drive.mount_point;
        }
    }
    let keep: Vec<bool> = drives
        .iter()
        .map(|(drive, device)| {
            !device.starts_with("/dev/")
                || nearest.get(device.as_str()) == Some(&&drive.mount_point)
        })
        .collect();
    drives
        .into_iter()
        .zip(keep)
        .filter_map(|(drive, keep)| keep.then_some(drive))
        .collect()
}

#[inline]
fn is_system_mount(mount_point: &std::path::Path) -> bool {
    #[cfg(target_os = "windows")]
    {
        if let Ok(sys_drive) = std::env::var("SystemDrive") {
            let p_str = mount_point.to_string_lossy();
            if p_str.starts_with(&sys_drive) {
                return true;
            }
        }
        let s = mount_point.to_string_lossy();
        s.starts_with("C:") || s.starts_with("c:")
    }
    #[cfg(not(target_os = "windows"))]
    {
        mount_point == std::path::Path::new("/")
    }
}

pub fn read_mounts() -> Vec<PathBuf> {
    MOUNT_CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        if let Some((last_update, mounts)) = &*cache {
            if last_update.elapsed() < Duration::from_secs(5) {
                return mounts.clone();
            }
        }

        let new_mounts = read_mounts_internal();
        *cache = Some((Instant::now(), new_mounts.clone()));
        new_mounts
    })
}

/// Linux: parse `/proc/mounts` and keep only real on-disk filesystems (skipping the
/// many virtual/pseudo mounts the kernel exposes).
#[cfg(target_os = "linux")]
fn read_mounts_internal() -> Vec<PathBuf> {
    let content = match std::fs::read_to_string("/proc/mounts") {
        Ok(c) => c,
        Err(_) => return vec![],
    };
    let real_fstypes = [
        "ext2", "ext3", "ext4", "btrfs", "xfs", "f2fs", "jfs", "reiserfs", "ntfs", "ntfs-3g",
        "vfat", "exfat", "fuseblk", "zfs",
    ];
    content
        .lines()
        .filter_map(|line| {
            let mut parts = line.split_whitespace();
            let _dev = parts.next()?;
            let mountpoint = parts.next()?;
            let fstype = parts.next()?;
            real_fstypes
                .contains(&fstype)
                .then(|| PathBuf::from(mountpoint))
        })
        .collect()
}

/// Windows / macOS: enumerate mounted volumes via `sysinfo` (already a dependency and
/// cross-platform). On Windows these are drive roots like `C:\`; on macOS they are the
/// root `/` plus `/Volumes/*` for external disks.
#[cfg(not(target_os = "linux"))]
fn read_mounts_internal() -> Vec<PathBuf> {
    let disks = sysinfo::Disks::new_with_refreshed_list();
    let mut mounts: Vec<PathBuf> = disks
        .list()
        .iter()
        .map(|disk| disk.mount_point().to_path_buf())
        .collect();
    mounts.sort();
    mounts.dedup();
    mounts
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mount(path: &str, device: &str) -> (DriveInfo, String) {
        let drive = DriveInfo {
            label: device.to_string(),
            mount_point: PathBuf::from(path),
            total_bytes: 1,
            available_bytes: 1,
            is_removable: false,
            is_system: path == "/",
        };
        (drive, device.to_string())
    }

    fn visible(mounts: &[(&str, &str)]) -> Vec<String> {
        let mut drives: Vec<_> = mounts.iter().map(|(p, d)| mount(p, d)).collect();
        drives.sort_by(|a, b| a.0.mount_point.cmp(&b.0.mount_point));
        user_visible_linux_mounts(drives)
            .into_iter()
            .map(|(drive, _)| drive.mount_point.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn fedora_btrfs_subvolumes_show_one_drive() {
        let fedora = [
            ("/", "/dev/vda3"),
            ("/home", "/dev/vda3"),
            ("/var", "/dev/vda3"),
            ("/boot", "/dev/vda3"),
            ("/boot/efi", "/dev/vda2"),
        ];
        assert_eq!(visible(&fedora), ["/"]);
    }

    #[test]
    fn separate_disks_and_home_partition_stay() {
        let mounts = [
            ("/", "/dev/nvme0n1p2"),
            ("/boot/efi", "/dev/nvme0n1p1"),
            ("/home", "/dev/nvme0n1p3"),
            ("/mnt/data", "/dev/sdb1"),
            ("/var/lib/docker/overlay2/x/merged", "overlay"),
        ];
        assert_eq!(visible(&mounts), ["/", "/home", "/mnt/data"]);
    }

    #[test]
    fn the_mount_nearest_the_root_stands_for_its_disk() {
        let mounts = [
            ("/", "/dev/sda1"),
            ("/home/me/shared", "/dev/sdb1"),
            ("/mnt/z", "/dev/sdb1"),
            ("/media/usb", "/dev/sdc1"),
            ("/mnt/usb-bind", "/dev/sdc1"),
        ];
        assert_eq!(visible(&mounts), ["/", "/media/usb", "/mnt/z"]);
    }

    #[test]
    fn non_device_mounts_are_not_merged() {
        let mounts = [
            ("/", "/dev/sda1"),
            ("/mnt/a", "server:/a"),
            ("/mnt/b", "server:/a"),
        ];
        assert_eq!(visible(&mounts), ["/", "/mnt/a", "/mnt/b"]);
    }

    #[test]
    fn a_disk_with_a_system_mount_keeps_its_user_mount() {
        let mounts = [
            ("/", "/dev/sda1"),
            ("/home", "/dev/sdb1"),
            ("/var", "/dev/sdb1"),
            ("/home/me/Games", "/dev/sdc1"),
            ("/var/lib/docker", "/dev/sdc1"),
            ("/run/media/me/USB", "/dev/sdd1"),
            ("/tmp/x", "/dev/sdd1"),
        ];
        assert_eq!(
            visible(&mounts),
            ["/", "/home", "/home/me/Games", "/run/media/me/USB"]
        );
    }

    #[test]
    fn fedora_atomic_default_layout_shows_its_disk_as_home() {
        let mounts = [
            ("/", "composefs"),
            ("/boot", "/dev/nvme0n1p2"),
            ("/boot/efi", "/dev/nvme0n1p1"),
            ("/etc", "/dev/nvme0n1p3"),
            ("/var", "/dev/nvme0n1p3"),
            ("/var/home", "/dev/nvme0n1p3"),
        ];
        assert_eq!(visible(&mounts), ["/", "/var/home"]);
    }

    #[test]
    fn fedora_atomic_home_and_mnt_under_var_stay() {
        let mounts = [
            ("/", "composefs"),
            ("/boot", "/dev/nvme0n1p2"),
            ("/boot/efi", "/dev/nvme0n1p1"),
            ("/etc", "/dev/nvme0n1p3"),
            ("/sysroot", "/dev/nvme0n1p3"),
            ("/var", "/dev/nvme0n1p3"),
            ("/var/home", "/dev/nvme0n1p4"),
            ("/var/mnt/data", "/dev/sdb1"),
            ("/run/media/me/USB", "/dev/sdc1"),
        ];
        assert_eq!(
            visible(&mounts),
            ["/", "/run/media/me/USB", "/var/home", "/var/mnt/data"]
        );
    }
}
