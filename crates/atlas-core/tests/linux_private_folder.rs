//! A private folder opened as a real FUSE mount on Linux.
//! Needs /dev/fuse and fusermount3 (package fuse3), so it runs on request:
//! `cargo test -p atlas-core --test linux_private_folder -- --ignored`
#![cfg(target_os = "linux")]

use std::{path::Path, process::Command};

#[test]
#[ignore = "needs /dev/fuse and fusermount3"]
fn private_folder_mounts_through_fuse_without_plaintext_on_disk() {
    use atlas_core::vault::{self, mount::Mounted, Control};
    let dir = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
    let folder = dir.path().join("Private records");
    std::fs::create_dir_all(folder.join("Nested")).unwrap();
    std::fs::write(folder.join("personal.txt"), b"confidential-marker-5521").unwrap();
    let profile = uuid::Uuid::new_v4().to_string();
    let password = "a long test passphrase";
    let session = vault::protect(
        &folder,
        password,
        vec![profile.clone()],
        &profile,
        &Control::default(),
    )
    .unwrap();
    let root = session.root().to_path_buf();
    let mount = Mounted::start(session).map_err(|(_, e)| e).unwrap();
    let path = mount.path().to_path_buf();
    assert!(path.starts_with(std::env::var("XDG_RUNTIME_DIR").unwrap()));

    assert_eq!(
        std::fs::read(path.join("personal.txt")).unwrap(),
        b"confidential-marker-5521"
    );
    std::fs::write(path.join("Nested/new.txt"), b"second-marker-7788").unwrap();
    // The modification time is kept (`touch -d`, `cp -p`).
    let old = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_500_000_000);
    std::fs::File::options()
        .write(true)
        .open(path.join("Nested/new.txt"))
        .unwrap()
        .set_modified(old)
        .unwrap();
    assert_eq!(
        std::fs::metadata(path.join("Nested/new.txt"))
            .unwrap()
            .modified()
            .unwrap(),
        old
    );
    std::fs::create_dir(path.join("Made here")).unwrap();
    std::fs::rename(
        path.join("Nested/new.txt"),
        path.join("Made here/moved.txt"),
    )
    .unwrap();
    let names: Vec<_> = std::fs::read_dir(&path)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    assert!(
        names.contains(&"Made here".to_string()) && names.contains(&"personal.txt".to_string())
    );
    std::fs::remove_dir(path.join("Nested")).unwrap();
    // The kernel sends RELEASE after close returns, so the last handle may still
    // count for a moment.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while !mount.prepare_lock() {
        assert!(
            std::time::Instant::now() < deadline,
            "a closed file still counts as open"
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    drop(mount);
    assert!(!path.exists() || std::fs::read_dir(&path).unwrap().next().is_none());
    let mounts = std::fs::read_to_string("/proc/self/mounts").unwrap();
    assert!(!mounts.contains(path.to_str().unwrap()), "still mounted");

    // Only ciphertext and opaque names remain on disk.
    let leaked = Command::new("grep")
        .args([
            "-rlaE",
            "confidential-marker-5521|second-marker-7788|Made here|moved.txt",
        ])
        .arg(dir.path())
        .output()
        .unwrap();
    // grep exits 1 when nothing matched; 2 would be an error, not a clean result.
    assert!(
        leaked.status.code() == Some(1),
        "plaintext found in {}",
        String::from_utf8_lossy(&leaked.stdout)
    );

    let session = vault::unlock(&root, password, &profile).unwrap();
    let dir_id = session
        .resolve(Path::new("Made here/moved.txt"))
        .unwrap()
        .expect("moved file");
    assert_eq!(
        &*session.read(&dir_id, 1024).unwrap(),
        b"second-marker-7788"
    );
}

fn stored_files(root: &Path) -> usize {
    std::fs::read_dir(root)
        .unwrap()
        .map(|e| e.unwrap().path())
        .map(|p| if p.is_dir() { stored_files(&p) } else { 1 })
        .sum()
}

#[test]
#[ignore = "needs /dev/fuse and fusermount3"]
fn deleted_and_replaced_files_leave_no_ciphertext_while_unlocked() {
    use atlas_core::vault::{self, mount::Mounted, Control};
    use std::io::Read;
    let dir = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
    let folder = dir.path().join("Private");
    std::fs::create_dir_all(&folder).unwrap();
    // Files from before the mount: this session never opens them, so no
    // release callback can free their blocks by accident. FUSE sends a
    // release asynchronously after close, which would race with the rm.
    for (name, byte) in [("gone.bin", 1u8), ("a.bin", 2), ("b.bin", 3), ("č.bin", 4)] {
        std::fs::write(folder.join(name), vec![byte; 50_000]).unwrap();
    }
    let profile = uuid::Uuid::new_v4().to_string();
    let session = vault::protect(
        &folder,
        "a long test passphrase",
        vec![profile.clone()],
        &profile,
        &Control::default(),
    )
    .unwrap();
    let root = session.root().to_path_buf();
    let mount = Mounted::start(session).map_err(|(_, e)| e).unwrap();
    let path = mount.path().to_path_buf();
    let before = stored_files(&root);

    std::fs::remove_file(path.join("gone.bin")).unwrap();
    let after_delete = stored_files(&root);
    assert!(after_delete < before, "deleted file kept its ciphertext");

    // Look the target up under another spelling first: after the replace, that
    // spelling must lead to the new file, not to the replaced one.
    assert!(path.join("A.BIN").exists());
    std::fs::rename(path.join("b.bin"), path.join("a.bin")).unwrap();
    let after_replace = stored_files(&root);
    assert!(
        after_replace < after_delete,
        "replaced file kept its ciphertext"
    );
    for name in ["a.bin", "A.BIN"] {
        assert_eq!(std::fs::read(path.join(name)).unwrap(), vec![3u8; 50_000]);
    }

    // Names match without regard to case, also beyond ASCII, as the vault itself
    // compares them: the stored `č.bin` is found as `Č.bin` before it is replaced.
    assert!(path.join("Č.bin").exists());
    std::fs::rename(path.join("a.bin"), path.join("Č.bin")).unwrap();
    assert!(
        stored_files(&root) < after_replace,
        "file replaced by a case-different name kept its ciphertext"
    );

    // A file deleted while open keeps its blocks for that handle.
    let data = vec![7u8; 50_000];
    std::fs::write(path.join("open.bin"), &data).unwrap();
    let with_open = stored_files(&root);
    let mut open = std::fs::File::open(path.join("open.bin")).unwrap();
    std::fs::remove_file(path.join("open.bin")).unwrap();
    assert_eq!(stored_files(&root), with_open);
    let mut back = Vec::new();
    open.read_to_end(&mut back).unwrap();
    assert_eq!(back, data);
    drop(open);
    drop(mount);
}

#[test]
#[ignore = "needs /dev/fuse and fusermount3"]
fn locking_a_folder_still_in_use_does_not_hang() {
    use atlas_core::vault::{self, mount::Mounted, Control};
    let dir = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
    let folder = dir.path().join("Private");
    std::fs::create_dir_all(folder.join("Inside")).unwrap();
    let profile = uuid::Uuid::new_v4().to_string();
    let session = vault::protect(
        &folder,
        "a long test passphrase",
        vec![profile.clone()],
        &profile,
        &Control::default(),
    )
    .unwrap();
    let mount = Mounted::start(session).map_err(|(_, e)| e).unwrap();
    let path = mount.path().to_path_buf();
    // A shell whose working directory is inside the folder keeps the mount busy
    // without any open file, so the lock check lets it through.
    let mut shell = Command::new("sh")
        .args(["-c", "read line"])
        .current_dir(path.join("Inside"))
        .stdin(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    assert!(mount.prepare_lock());
    // Before the fix, this waited for the shell to leave and never returned.
    let (done, unmounted) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        drop(mount);
        let _ = done.send(());
    });
    assert!(
        unmounted
            .recv_timeout(std::time::Duration::from_secs(30))
            .is_ok(),
        "unmount hung"
    );
    let mounts = std::fs::read_to_string("/proc/self/mounts").unwrap();
    assert!(!mounts.contains(path.to_str().unwrap()));
    drop(shell.stdin.take());
    shell.wait().unwrap();
}
