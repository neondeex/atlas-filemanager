# Changelog

Write user-facing release notes here. Release headings use
`## [VERSION] - YYYY-MM-DD`; the workflow requires notes for the version it builds.

## [Unreleased]

## [0.1.6-beta.1] - 2026-11-12

### Fixed

- Dragging files to other applications copies the originals and never starts a source-deletion operation.
- Markdown previews scroll in the same direction with smooth and precise scrolling.
- Windows PDF previews use the native renderer even with an older Document Previews extension installed.
- Theme color fields keep keyboard focus, accept pasted hex and HSL/HSLA values, and preserve slider precision.
- Conversion format labels use opaque sectors and contrasting text; the selected format is highlighted immediately.
- Windows 10 glass uses the legacy blur material instead of acrylic to avoid the native window-drag penalty.
- Store packages declare the Microsoft Desktop VCLibs runtime dependency; the Inno installer bundles its desktop runtime.

### Changed

- Quick Look is listed in Commands and the shortcut editor. Space can be changed, removed or restored per profile.

### Validation still required

- The new Discord report of a freeze followed by repeated startup crashes needs the affected user's version and crash report.
- Kaspersky's report (#14) requires vendor review of the exact distributed binaries. This beta does not claim an allowlist approval.

## [0.1.5] - 2026-11-05

### New

- Settings redesigned as a searchable palette, with French and more languages selectable.
- Atlas Ember theme, theme editor improvements and new interface icons.
- Local file conversion with a radial menu, including FLAC, H.264/AAC video and optional Opus.
- Quick actions ring for converting and acting on selected files.
- Atlas Free and Atlas Premium. Premium unlocks with a license key, or in-app through the Microsoft Store in regional-price markets.

### Fixed

- Crash on launch caused by some shell extensions, such as Google Drive for desktop (#1, #3).
- Opening sub-folders on SMB and other cloud connections (#2).
- Typing into the fields of the add-connection dialog.

### Stability

- Crash reports with symbolized stacks that can be sent from the app.
- Recovery from worker failures and poisoned locks instead of closing the app.

## [0.1.0] - 2026-10-11

First public alpha.

### Highlights

- Native, GPU-accelerated file manager for Windows built with Rust and GPUI.
- Grid, List, Details, Miller columns and Tree views; tabs and split panes.
- Command palette, fuzzy go-to, indexed search and previews.
- Local file conversion, archives browsed as folders, tags and profiles.
- Optional first-party extensions (icon themes, thumbnails, Pandoc, Opus, cloud drives, document previews).
- Experimental Linux packaging (Debian/Ubuntu, Fedora/openSUSE, Arch, portable); not yet a supported release.
