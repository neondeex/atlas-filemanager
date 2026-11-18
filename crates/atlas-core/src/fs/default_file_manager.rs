//! Atlas as the default file manager on Linux, the counterpart of the Win+E
//! setting on Windows: the `inode/directory` handler in the user's
//! `~/.config/mimeapps.list`, which GNOME, KDE, xdg-open and browsers ("Show in
//! folder") follow. On puts Atlas first in that handler list and keeps the
//! rest; off removes only Atlas from it, so the previous choice comes back
//! when `GLib` could read it (a malformed entry it ignored becomes valid).
// reason: one small config file read at startup and written on a settings change,
// like util/config.rs; not the render hot path the sync-fs ban targets.
#![allow(clippy::disallowed_methods)]

use std::io;
use std::path::PathBuf;

/// Installed by the deb, rpm, Arch and Nix packages as `atlas.desktop`.
pub const DESKTOP_ID: &str = "atlas.desktop";
const SECTION: &str = "[Default Applications]";
const KEY: &str = "inode/directory";

fn mimeapps_path() -> Option<PathBuf> {
    dirs::config_dir().map(|dir| dir.join("mimeapps.list"))
}

/// Whether folders opened by other apps open in Atlas.
pub fn is_default() -> bool {
    mimeapps_path()
        .and_then(|path| std::fs::read_to_string(path).ok())
        .is_some_and(|content| default_handler(&content) == Some(DESKTOP_ID))
}

pub fn set_default(on: bool) -> io::Result<()> {
    let path = mimeapps_path()
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no config directory"))?;
    // A symlinked file (dotfile managers) is edited at its target, so the link stays.
    let path = std::fs::canonicalize(&path).unwrap_or(path);
    let content = match std::fs::read_to_string(&path) {
        Ok(content) => content,
        Err(err) if err.kind() == io::ErrorKind::NotFound => String::new(),
        Err(err) => return Err(err),
    };
    let updated = with_default(&content, on);
    if updated == content {
        return Ok(());
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    // Write next to it and rename, so a crash never leaves a half-written file;
    // the pid keeps two Atlas windows from sharing the temporary file.
    let tmp = path.with_extension(format!("list.atlas-tmp-{}", std::process::id()));
    std::fs::write(&tmp, updated)?;
    std::fs::rename(tmp, path)
}

/// The desktop id that decides for folders: the first one listed in the last
/// `inode/directory=` key of `[Default Applications]`, since `GLib`, like other
/// key-file readers, lets a repeated key override an earlier one.
fn default_handler(content: &str) -> Option<&str> {
    let mut in_section = false;
    let mut handler = None;
    for line in content.lines().map(str::trim) {
        if line.starts_with('[') {
            in_section = line == SECTION;
        } else if let Some(handlers) = in_section.then(|| folder_handlers(line)).flatten() {
            handler = handlers.first().copied();
        }
    }
    handler
}

/// The handler list of a `inode/directory=` line, or `None` for any other line.
fn folder_handlers(line: &str) -> Option<Vec<&str>> {
    let (key, value) = line.split_once('=')?;
    (key.trim() == KEY).then(|| {
        value
            .split(';')
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .collect()
    })
}

fn folder_line(handlers: &[&str]) -> String {
    let mut line = format!("{KEY}=");
    for handler in handlers {
        line.push_str(handler);
        line.push(';');
    }
    line
}

/// `content` with Atlas first in (on) or removed from (off) the folder handler
/// lists of `[Default Applications]`. On changes only the last folder key, the one
/// that decides; off changes only the keys that list Atlas. A key that off leaves
/// empty is dropped, unless an earlier folder key would then take over. Every
/// other line is kept as it was.
fn with_default(content: &str, on: bool) -> String {
    let mut in_section = false;
    let mut last_key = None;
    let mut any_ours = false;
    for (n, line) in content.lines().map(str::trim).enumerate() {
        if line.starts_with('[') {
            in_section = line == SECTION;
        } else if let Some(handlers) = in_section.then(|| folder_handlers(line)).flatten() {
            last_key = Some(n);
            any_ours |= handlers.contains(&DESKTOP_ID);
        }
    }
    let unchanged = if on {
        default_handler(content) == Some(DESKTOP_ID)
    } else {
        !any_ours
    };
    if unchanged {
        return content.to_owned();
    }
    let mut out = Vec::new();
    let mut in_section = false;
    let mut keys_before = 0;
    let mut header_at = None;
    for (n, line) in content.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            in_section = trimmed == SECTION;
            if in_section && header_at.is_none() {
                header_at = Some(out.len());
            }
            out.push(line.to_owned());
            continue;
        }
        let Some(handlers) = in_section.then(|| folder_handlers(trimmed)).flatten() else {
            out.push(line.to_owned());
            continue;
        };
        let earlier_keys = keys_before > 0;
        keys_before += 1;
        let edit = if on {
            Some(n) == last_key
        } else {
            handlers.contains(&DESKTOP_ID)
        };
        if !edit {
            out.push(line.to_owned());
            continue;
        }
        let mut rest: Vec<&str> = handlers.into_iter().filter(|h| *h != DESKTOP_ID).collect();
        if on {
            rest.insert(0, DESKTOP_ID);
        }
        if !rest.is_empty() || earlier_keys {
            out.push(folder_line(&rest));
        }
    }
    if on && last_key.is_none() {
        let line = folder_line(&[DESKTOP_ID]);
        if let Some(at) = header_at {
            out.insert(at + 1, line);
        } else {
            if out.last().is_some_and(|l| !l.trim().is_empty()) {
                out.push(String::new());
            }
            out.push(SECTION.to_owned());
            out.push(line);
        }
    }
    let mut text = out.join("\n");
    text.push('\n');
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn turning_it_on_puts_atlas_first_and_keeps_the_rest() {
        let before = "[Default Applications]\ntext/plain=org.gnome.TextEditor.desktop;\ninode/directory=org.gnome.Nautilus.desktop;\n\n[Added Associations]\ninode/directory=org.gnome.Nautilus.desktop;\n";
        let after = with_default(before, true);
        assert_eq!(default_handler(&after), Some(DESKTOP_ID));
        assert!(after.contains("text/plain=org.gnome.TextEditor.desktop;"));
        assert!(after.contains("[Default Applications]\ntext/plain=org.gnome.TextEditor.desktop;\ninode/directory=atlas.desktop;org.gnome.Nautilus.desktop;\n"));
        assert!(after.contains("[Added Associations]\ninode/directory=org.gnome.Nautilus.desktop;"));
        // Off gives back exactly the user's own choice.
        assert_eq!(with_default(&after, false), before);
    }

    #[test]
    fn turning_it_off_removes_atlas_wherever_it_is_listed() {
        let second =
            "[Default Applications]\ninode/directory=org.kde.dolphin.desktop;atlas.desktop;\n";
        assert_eq!(
            with_default(second, false),
            "[Default Applications]\ninode/directory=org.kde.dolphin.desktop;\n"
        );
        let section_without_line = "[Default Applications]\ntext/plain=gedit.desktop;\n";
        let on = with_default(section_without_line, true);
        assert_eq!(
            on,
            "[Default Applications]\ninode/directory=atlas.desktop;\ntext/plain=gedit.desktop;\n"
        );
        assert_eq!(with_default(&on, false), section_without_line);
    }

    #[test]
    fn turning_it_off_brings_back_the_desktop_default() {
        let on = with_default("[Added Associations]\nimage/png=eog.desktop;\n", true);
        assert_eq!(default_handler(&on), Some(DESKTOP_ID));
        let off = with_default(&on, false);
        assert_eq!(default_handler(&off), None);
        assert!(off.contains("image/png=eog.desktop;"));
        // Off never removes another app's choice.
        let other = "[Default Applications]\ninode/directory=org.kde.dolphin.desktop\n";
        assert_eq!(with_default(other, false), other);
    }

    #[test]
    fn a_repeated_folder_key_cannot_override_atlas() {
        // The last key decides in GLib, so On edits that one and leaves the rest.
        let before =
            "[Default Applications]\ninode/directory=one.desktop;\ninode/directory=two.desktop;\n";
        let on = with_default(before, true);
        assert_eq!(
            on,
            "[Default Applications]\ninode/directory=one.desktop;\ninode/directory=atlas.desktop;two.desktop;\n"
        );
        assert_eq!(default_handler(&on), Some(DESKTOP_ID));
        assert_eq!(with_default(&on, false), before);
        // An empty last key hides the earlier one; after on and off it still does.
        let empty_last = "[Default Applications]\ninode/directory=one.desktop;\ninode/directory=\n";
        assert_eq!(default_handler(empty_last), None);
        let round_trip = with_default(&with_default(empty_last, true), false);
        assert_eq!(round_trip, empty_last);
        assert_eq!(default_handler(&round_trip), None);
        // Atlas named only outside the folder keys: off changes nothing.
        let elsewhere =
            "# atlas.desktop\r\n[Added Associations]\ninode/directory=atlas.desktop;\r\n";
        assert_eq!(with_default(elsewhere, false), elsewhere);
    }

    #[test]
    fn an_empty_or_missing_file_gets_a_section() {
        assert_eq!(
            with_default("", true),
            "[Default Applications]\ninode/directory=atlas.desktop;\n"
        );
        assert_eq!(with_default("", false), "");
    }
}
