//! Atlas-owned disk analysis. No external cleaner or shell scripts are executed.
mod execution;
#[cfg(target_os = "linux")]
pub(crate) mod linux_verified;
#[cfg(windows)]
mod mft;
mod policy;
mod scan;
mod tree;
mod walk;

pub use execution::{execute, prepare_plan, prepare_plan_in, PLAN_LIMIT};
pub use policy::{
    categories, is_cleanup_root, is_drive_root, on_volume, protected, user_temp_root, Category,
};
pub use scan::{ScanJob, ScanMode};
use std::path::PathBuf;
pub use tree::{cached_snapshot, invalidate as invalidate_tree};

#[cfg(windows)]
pub(crate) use atlas_win_space::cleanup as verified;
/// Identity-verified inspection and deletion of cleanup candidates.
#[cfg(target_os = "linux")]
pub(crate) use linux_verified as verified;

/// Open and identify the given files in parallel; files that vanished, are
/// locked or are otherwise unsafe are simply left out.
pub fn inspect_candidates(paths: &[PathBuf]) -> Vec<atlas_model::cleanup::CleanupCandidate> {
    #[cfg(any(windows, target_os = "linux"))]
    {
        use rayon::prelude::*;
        paths
            .par_iter()
            .filter_map(|p| verified::inspect(p).ok())
            .collect()
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        let _ = paths;
        Vec::new()
    }
}

/// Move one analyzed entry to the Recycle Bin and update the cached totals.
pub fn move_to_trash(path: &std::path::Path) -> std::io::Result<()> {
    let blocked = policy::is_drive_root(path)
        || policy::protected(path)
        || crate::vault::is_private_path(path)
        || crate::vault::live::is_private(path)
        || system_entry(path);
    if blocked {
        return Err(std::io::Error::other("cleanup_scope_changed"));
    }
    std::fs::symlink_metadata(path)?;
    trash::delete(path).map_err(std::io::Error::other)?;
    tree::remove(path);
    Ok(())
}

/// Operating-system folders (and the Users root itself) are never offered.
#[cfg(windows)]
fn system_entry(path: &std::path::Path) -> bool {
    use std::path::Component;
    let mut parts = path.components().filter_map(|c| match c {
        Component::Normal(n) => Some(n.to_string_lossy().to_ascii_lowercase()),
        _ => None,
    });
    match parts.next().as_deref() {
        Some(
            "windows"
            | "program files"
            | "program files (x86)"
            | "programdata"
            | "$recycle.bin"
            | "system volume information"
            | "recovery"
            | "boot",
        ) => true,
        Some("users") => parts.next().is_none(),
        _ => false,
    }
}

/// The top-level system folders of a Linux root and /home itself, and anything
/// inside the system folders, are never offered. The Map of / reaches them.
/// Removable disks under /run/media/<user>/<disk> and the /var/home of
/// image-based systems (Silverblue) hold user files and stay allowed.
#[cfg(not(windows))]
// reason: the allowed arm must come before the "run" and "var" arm, so it is not merged into `_`
#[allow(clippy::match_same_arms)]
fn system_entry(path: &std::path::Path) -> bool {
    use std::path::Component;
    let parts: Vec<_> = path
        .components()
        .filter_map(|c| match c {
            Component::Normal(n) => Some(n.to_str().unwrap_or("")),
            _ => None,
        })
        .collect();
    match parts.as_slice() {
        ["run", "media", _, _, _, ..] | ["var", "home", _, ..] => false,
        ["bin" | "boot" | "dev" | "efi" | "etc" | "lib" | "lib32" | "lib64" | "libx32"
        | "lost+found" | "nix" | "opt" | "proc" | "root" | "run" | "sbin" | "snap" | "srv"
        | "sys" | "usr" | "var", ..] => true,
        ["home"] => true,
        _ => false,
    }
}

/// Large read-only indices are released away from the GUI thread.
pub fn release_analysis_cache() {
    tree::invalidate();
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn trash_moves_a_real_file_and_refuses_system_roots() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("atlas-trash-test.bin");
        std::fs::write(&file, [1u8; 64]).unwrap();
        move_to_trash(&file).unwrap();
        assert!(!file.exists());
        #[cfg(windows)]
        for path in [r"C:\", r"C:\Windows\System32", r"C:\Users"] {
            assert!(move_to_trash(std::path::Path::new(path)).is_err(), "{path}");
        }
        #[cfg(not(windows))]
        {
            // Checked on the guard itself: move_to_trash on these would fail for a
            // normal user anyway, and run as root it must never be tried.
            for path in [
                "/usr",
                "/usr/share/doc",
                "/etc/fstab",
                "/var/log",
                "/var/home",
                "/home",
                "/run/user/1000",
                "/run/media/user",
                "/run/media/user/USB",
            ] {
                assert!(system_entry(std::path::Path::new(path)), "{path}");
            }
            for path in [
                "/home/user",
                "/tmp/x",
                "/mnt/data/usr",
                "/media/disk/etc",
                "/run/media/user/USB/photos",
                "/var/home/user/big.iso",
            ] {
                assert!(!system_entry(std::path::Path::new(path)), "{path}");
            }
        }
        assert!(move_to_trash(&dir.path().join("missing")).is_err());
    }
}
