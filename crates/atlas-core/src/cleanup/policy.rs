use std::path::{Component, Path, PathBuf};

/// Deliberately excludes network/device paths and ambiguous drive-relative paths.
#[cfg(windows)]
pub fn is_drive_root(path: &Path) -> bool {
    let value = path.as_os_str().to_string_lossy();
    let bytes = value.as_bytes();
    bytes.len() == 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && matches!(bytes[2], b'\\' | b'/')
}

/// `/` or another mount point, given as an absolute, normalized path.
#[cfg(not(windows))]
pub fn is_drive_root(path: &Path) -> bool {
    if !path.is_absolute()
        || path
            .components()
            .any(|c| !matches!(c, Component::RootDir | Component::Normal(_)))
    {
        return false;
    }
    is_mount_point(path)
}

/// Looks the path up in the mount table instead of calling stat on it, so a
/// right-click never waits on a hung network mount.
#[cfg(target_os = "linux")]
fn is_mount_point(path: &Path) -> bool {
    let Some(table) = mount_table() else {
        return path == Path::new("/");
    };
    let path = path.as_os_str().as_encoded_bytes();
    let path = if path.len() > 1 {
        path.strip_suffix(b"/").unwrap_or(path)
    } else {
        path
    };
    table.iter().any(|mount| mount == path)
}

/// Every mount point in the mount table, read without touching the mounts
/// themselves (a hung network mount would block a stat).
#[cfg(target_os = "linux")]
pub(super) fn mount_points() -> std::collections::HashSet<PathBuf> {
    use std::os::unix::ffi::OsStringExt;
    mount_table()
        .unwrap_or_default()
        .into_iter()
        .map(|mount| PathBuf::from(std::ffi::OsString::from_vec(mount)))
        .collect()
}

/// The mount points in /proc/self/mountinfo, as raw bytes: a path need not be
/// UTF-8, and one such mount must not hide all the others.
#[cfg(target_os = "linux")]
fn mount_table() -> Option<Vec<Vec<u8>>> {
    #[allow(clippy::disallowed_methods)] // reason: procfs is in memory and never blocks
    let table = std::fs::read("/proc/self/mountinfo").ok()?;
    Some(parse_mountinfo(&table))
}

/// The mount point is the fifth field of each mountinfo line.
#[cfg(target_os = "linux")]
fn parse_mountinfo(table: &[u8]) -> Vec<Vec<u8>> {
    table
        .split(|&b| b == b'\n')
        .filter_map(|line| line.split(|&b| b == b' ').nth(4))
        .map(unescape_mountinfo)
        .collect()
}

/// Whether `path` is on the filesystem mounted at `volume`. On Linux a path prefix
/// is not enough: /home is often its own mount below /, and a symlinked cache
/// folder can lead to another disk.
pub fn on_volume(path: &Path, volume: &Path) -> bool {
    #[cfg(target_os = "linux")]
    {
        let Ok(real) = path.canonicalize() else {
            return false;
        };
        containing_mount(&real, &mount_points()).is_some_and(|mount| mount == volume)
    }
    #[cfg(not(target_os = "linux"))]
    {
        path.starts_with(volume)
    }
}

/// The deepest mount point that holds `path`.
#[cfg(target_os = "linux")]
fn containing_mount<'m>(
    path: &Path,
    mounts: &'m std::collections::HashSet<PathBuf>,
) -> Option<&'m Path> {
    mounts
        .iter()
        .filter(|mount| path.starts_with(mount))
        .max_by_key(|mount| mount.components().count())
        .map(PathBuf::as_path)
}

/// mountinfo writes space, tab, newline and backslash as `\ooo` octal escapes.
#[cfg(target_os = "linux")]
fn unescape_mountinfo(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        // A byte needs at most \377, so the first digit is 0-3.
        let octal = bytes.get(i + 1..i + 4).filter(|d| {
            (b'0'..=b'3').contains(&d[0]) && d[1..].iter().all(|b| (b'0'..=b'7').contains(b))
        });
        match (bytes[i], octal) {
            (b'\\', Some(d)) => {
                out.push((d[0] - b'0') * 64 + (d[1] - b'0') * 8 + (d[2] - b'0'));
                i += 4;
            }
            (b, _) => {
                out.push(b);
                i += 1;
            }
        }
    }
    out
}

/// Other Unix systems: a directory on a different filesystem than its parent.
#[cfg(all(unix, not(target_os = "linux")))]
#[allow(clippy::disallowed_methods)] // reason: no mount table file to read instead
fn is_mount_point(path: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    let Some(parent) = path.parent() else {
        return path == Path::new("/");
    };
    match (std::fs::symlink_metadata(path), std::fs::metadata(parent)) {
        (Ok(dir), Ok(parent)) => dir.is_dir() && dir.dev() != parent.dev(),
        _ => false,
    }
}

pub fn user_temp_root() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        dirs::data_local_dir().map(|local| local.join("Temp"))
    }
    // Linux has no per-user Temp folder: /tmp is shared, and ~/.cache also holds
    // virtual environments, downloaded browsers and models that are costly to
    // rebuild, so only the known caches in `categories` are offered.
    #[cfg(not(windows))]
    {
        None
    }
}

/// A user-facing group of regenerable data. Every root is a directory whose
/// old files may be offered for cleanup; nothing outside these roots ever is.
pub struct Category {
    pub key: &'static str,
    pub roots: Vec<PathBuf>,
}

pub fn categories() -> Vec<Category> {
    #[cfg(windows)]
    {
        windows_categories()
    }
    #[cfg(not(windows))]
    {
        linux_categories()
    }
}

#[cfg(windows)]
fn windows_categories() -> Vec<Category> {
    let local = dirs::data_local_dir();
    let home = dirs::home_dir();
    let join = |base: &Option<PathBuf>, rel: &str| base.as_ref().map(|b| b.join(rel));
    let mut browsers = Vec::new();
    for base in [
        "Google/Chrome",
        "Microsoft/Edge",
        "BraveSoftware/Brave-Browser",
        "Vivaldi",
    ] {
        for cache in ["Cache", "Code Cache", "GPUCache"] {
            browsers.extend(join(&local, &format!("{base}/User Data/Default/{cache}")));
        }
    }
    if let Some(profiles) = join(&local, "Mozilla/Firefox/Profiles") {
        if let Ok(entries) = std::fs::read_dir(profiles) {
            browsers.extend(entries.flatten().map(|e| e.path().join("cache2")));
        }
    }
    let list = [
        ("cleanup_cat_temp", user_temp_root().into_iter().collect()),
        ("cleanup_cat_browser", browsers),
        (
            "cleanup_cat_crash",
            [
                join(&local, "CrashDumps"),
                join(&local, "Microsoft/Windows/WER/ReportArchive"),
                join(&local, "Microsoft/Windows/WER/ReportQueue"),
            ]
            .into_iter()
            .flatten()
            .collect(),
        ),
        (
            "cleanup_cat_dev",
            [
                join(&local, "npm-cache"),
                join(&local, "pip/Cache"),
                join(&local, "Yarn/Cache"),
                join(&home, ".cargo/registry/cache"),
            ]
            .into_iter()
            .flatten()
            .collect(),
        ),
    ];
    list.into_iter()
        .map(|(key, roots): (&'static str, Vec<PathBuf>)| Category {
            key,
            roots: roots.into_iter().filter(|r| r.is_dir()).collect(),
        })
        .filter(|c| !c.roots.is_empty())
        .collect()
}

#[cfg(not(windows))]
fn linux_categories() -> Vec<Category> {
    linux_categories_in(dirs::cache_dir(), dirs::home_dir())
}

/// `cache` is `$XDG_CACHE_HOME` (`~/.cache`).
#[cfg(not(windows))]
fn linux_categories_in(cache: Option<PathBuf>, home: Option<PathBuf>) -> Vec<Category> {
    // Canonical bases, so a symlinked ~/.cache gives the same paths that the volume
    // check and the no-follow delete walk see.
    let (cache, home) = (
        cache.and_then(|b| b.canonicalize().ok()),
        home.and_then(|b| b.canonicalize().ok()),
    );
    let join = |base: &Option<PathBuf>, rel: &str| base.as_ref().map(|b| b.join(rel));

    let mut browsers = Vec::new();
    for base in [
        "google-chrome",
        "chromium",
        "microsoft-edge",
        "BraveSoftware/Brave-Browser",
        "vivaldi",
    ] {
        for cache_dir in ["Default/Cache", "Default/Code Cache", "Default/GPUCache"] {
            browsers.extend(join(&cache, &format!("{base}/{cache_dir}")));
        }
    }
    if let Some(profiles) = join(&cache, "mozilla/firefox") {
        #[allow(clippy::disallowed_methods)] // reason: one small local folder
        let entries = std::fs::read_dir(profiles);
        if let Ok(entries) = entries {
            browsers.extend(
                entries
                    .flatten()
                    .filter(|e| {
                        let name = e.file_name();
                        let name = name.to_string_lossy();
                        name.ends_with(".default") || name.ends_with(".default-release")
                    })
                    .map(|e| e.path().join("cache2")),
            );
        }
    }

    let list = [
        ("cleanup_cat_browser", browsers),
        (
            "cleanup_cat_crash",
            // Thumbnail cache; regenerable. The Trash is deliberately not offered:
            // its contents are user files waiting to be restored or emptied.
            std::iter::once(join(&cache, "thumbnails"))
                .flatten()
                .collect(),
        ),
        (
            "cleanup_cat_dev",
            [
                join(&home, ".npm/_cacache"),
                join(&cache, "pip"),
                join(&home, ".cargo/registry/cache"),
                join(&cache, "yarn"),
                join(&cache, "pnpm"),
            ]
            .into_iter()
            .flatten()
            .collect(),
        ),
    ];
    list.into_iter()
        .map(|(key, roots): (&'static str, Vec<PathBuf>)| Category {
            key,
            // A root reached through a symlink below the base could point at any
            // folder, such as ~/Documents, so it is not offered as a cache.
            roots: roots
                .into_iter()
                .filter(|r| r.is_dir() && r.canonicalize().is_ok_and(|c| &c == r))
                .collect(),
        })
        .filter(|c| !c.roots.is_empty())
        .collect()
}

pub fn is_cleanup_root(path: &Path) -> bool {
    categories()
        .iter()
        .any(|c| c.roots.iter().any(|r| r == path))
}

/// Conservative component policy shared by discovery and execution. This is an
/// extra exclusion; filesystem identity checks remain the mutation authority.
pub fn protected(path: &Path) -> bool {
    path.components().any(|part| match part {
        Component::Normal(name) => {
            let name = name.to_string_lossy().to_ascii_lowercase();
            matches!(
                name.as_str(),
                ".ssh"
                    | ".gnupg"
                    | ".aws"
                    | ".azure"
                    | ".kube"
                    | ".docker"
                    | ".git"
                    | ".bb"
                    | ".agents"
                    | ".gemini"
                    | ".env"
                    | ".npmrc"
                    | ".pypirc"
                    | ".netrc"
                    | ".claude"
                    | ".codex"
                    | ".cursor"
                    | "credentials"
                    | "protect"
                    | "crypto"
                    | "vault"
                    | "systemcertificates"
                    | "login data"
                    | "cookies"
                    | "web data"
                    | "sessions"
                    | "start menu"
                    | "$recycle.bin"
                    | "system volume information"
            )
        }
        _ => false,
    })
}

pub(super) fn valid_temp_candidate(path: &Path, temp: &Path) -> bool {
    path.strip_prefix(temp).is_ok_and(|relative| {
        relative
            .components()
            .all(|c| matches!(c, Component::Normal(_)))
            && relative.components().next().is_some()
    }) && !protected(path)
}

#[cfg(test)]
#[allow(clippy::disallowed_methods)] // reason: test fixtures
mod tests {
    use super::*;
    #[test]
    #[cfg(windows)]
    fn drive_scope_rejects_unc_relative_and_nested_paths() {
        for value in ["C:\\", "d:/"] {
            assert!(is_drive_root(Path::new(value)));
        }
        for value in ["C:", "C:\\Users", "\\\\server\\share", "\\\\?\\C:\\", "/"] {
            assert!(!is_drive_root(Path::new(value)), "{value}");
        }
    }
    #[cfg(target_os = "linux")]
    #[test]
    fn mountinfo_escapes_are_decoded() {
        assert_eq!(unescape_mountinfo(br"/media/My\040Disk"), b"/media/My Disk");
        assert_eq!(unescape_mountinfo(br"/a\134b\0"), b"/a\\b\\0");
        assert_eq!(unescape_mountinfo(b"/mnt/\xff"), b"/mnt/\xff");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn mount_points_are_volume_roots() {
        assert!(is_drive_root(Path::new("/")));
        assert!(is_drive_root(Path::new("/proc")));
        assert!(is_drive_root(Path::new("/proc/")));
        for value in ["/usr/bin", "relative", "/tmp/../", "C:\\"] {
            assert!(!is_drive_root(Path::new(value)), "{value}");
        }
    }
    #[test]
    fn temp_scope_is_component_bounded_and_protected() {
        let root = Path::new("local/Temp");
        assert!(valid_temp_candidate(Path::new("local/Temp/a/b.tmp"), root));
        for path in [
            "local/Temporary/file",
            "local/Temp",
            "local/Temp/../a",
            "local/Temp/.ssh/id",
            "local/Temp/.codex/sessions/a",
            "local/Temp/.env",
            "local/Temp/.bb/state",
        ] {
            assert!(!valid_temp_candidate(Path::new(path), root), "{path}");
        }
    }
    #[cfg(not(windows))]
    #[test]
    fn linux_categories_are_known_caches_only() {
        let fixture = tempfile::tempdir().unwrap();
        let base = fixture.path().canonicalize().unwrap();
        let (cache, home) = (base.join("cache"), base.join("home"));
        for dir in [
            "cache/chromium/Default/Cache",
            "cache/mozilla/firefox/ab12.default-release/cache2",
            "cache/thumbnails",
            "cache/pip",
            "cache/pypoetry/virtualenvs",
            "home/.cargo/registry/cache",
        ] {
            std::fs::create_dir_all(base.join(dir)).unwrap();
        }
        let roots: Vec<_> = linux_categories_in(Some(cache.clone()), Some(home.clone()))
            .into_iter()
            .map(|c| (c.key, c.roots))
            .collect();
        assert_eq!(
            roots,
            [
                (
                    "cleanup_cat_browser",
                    vec![
                        cache.join("chromium/Default/Cache"),
                        cache.join("mozilla/firefox/ab12.default-release/cache2"),
                    ]
                ),
                ("cleanup_cat_crash", vec![cache.join("thumbnails")]),
                (
                    "cleanup_cat_dev",
                    vec![cache.join("pip"), home.join(".cargo/registry/cache")]
                ),
            ]
        );
        // A symlinked ~/.cache gives the real paths, which the delete walk accepts.
        let link = base.join("cache-link");
        std::os::unix::fs::symlink(&cache, &link).unwrap();
        let through_link: Vec<_> = linux_categories_in(Some(link), Some(home.clone()))
            .into_iter()
            .map(|c| (c.key, c.roots))
            .collect();
        assert_eq!(through_link, roots);
        // A cache folder that is itself a symlink, here to user data, is not a root.
        let documents = home.join("Documents");
        std::fs::create_dir_all(&documents).unwrap();
        std::fs::remove_dir(cache.join("pip")).unwrap();
        std::os::unix::fs::symlink(&documents, cache.join("pip")).unwrap();
        let dev = linux_categories_in(Some(cache), Some(home.clone()))
            .into_iter()
            .find(|c| c.key == "cleanup_cat_dev")
            .unwrap();
        assert_eq!(dev.roots, vec![home.join(".cargo/registry/cache")]);
        // ~/.cache itself is never a category root, and there is no Temp folder.
        assert!(user_temp_root().is_none());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn mountinfo_lines_give_mount_points() {
        let table = b"22 1 259:2 / / rw,relatime shared:1 - ext4 /dev/nvme0n1p2 rw\n\
            23 22 259:3 / /home rw shared:2 - ext4 /dev/nvme0n1p3 rw\n\
            24 22 8:1 / /media/My\\040Disk rw - vfat /dev/sda1 rw\n";
        assert_eq!(
            parse_mountinfo(table),
            [b"/".to_vec(), b"/home".to_vec(), b"/media/My Disk".to_vec()]
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_separate_home_mount_is_not_part_of_root() {
        let mounts = ["/", "/home", "/home/u/.cache/remote"]
            .map(PathBuf::from)
            .into_iter()
            .collect();
        let at = |p: &str| containing_mount(Path::new(p), &mounts).map(Path::to_owned);
        assert_eq!(at("/home/u/.cache/pip"), Some(PathBuf::from("/home")));
        assert_eq!(
            at("/home/u/.cache/remote/x"),
            Some(PathBuf::from("/home/u/.cache/remote"))
        );
        assert_eq!(at("/homework"), Some(PathBuf::from("/")));
        assert_eq!(at("/usr"), Some(PathBuf::from("/")));
    }
}
