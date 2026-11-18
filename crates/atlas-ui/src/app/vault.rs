//! Session-only private-folder controller. Secrets never enter AppState/config.
use super::App;
use atlas_core::vault::{self, Control, Session};
use gpui::{AppContext as _, Context, Entity, FocusHandle, Focusable, Window};
use gpui_component::input::InputState;
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    sync::atomic::Ordering,
    time::{Duration, Instant},
};
use zeroize::Zeroizing;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Phase {
    Create,
    Unlock,
    Browse,
    Manage,
    Restore,
}

pub(crate) struct Dialog {
    pub path: PathBuf,
    pub name: String,
    pub phase: Phase,
    pub inputs: Vec<Entity<InputState>>,
    pub session: Option<Session>,
    pub profiles: HashSet<String>,
    pub profiles_open: bool,
    pub reveal: bool,
    pub busy: bool,
    pub error: Option<String>,
    pub notice: Option<String>,
    pub parents: Vec<String>,
    pub selected: Option<String>,
    pub preview: Option<PrivatePreview>,
    pub control: Control,
    pub last_activity: Instant,
    pub generation: Instant,
    rx: Option<flume::Receiver<Outcome>>,
    progress: u64,
}
impl Drop for Dialog {
    fn drop(&mut self) {
        self.control.cancel();
    }
}

pub(crate) enum PrivatePreview {
    Text(Zeroizing<String>),
    Image(std::sync::Arc<gpui::RenderImage>),
}

enum Outcome {
    #[cfg(any(windows, target_os = "linux"))]
    Mounted(
        atlas_core::vault::mount::Mounted,
        Option<PathBuf>,
        Option<String>,
    ),
    MountUnavailable(Session, Option<PathBuf>, String),
    Ready(Session, Option<PathBuf>),
    Updated(Session, Option<String>),
    Preview(Session, PrivatePreview),
    Restored(PathBuf, bool),
    Failed(Option<Session>, String),
}

impl App {
    pub(crate) fn request_vault(&mut self, path: PathBuf, phase: Phase) {
        if self.vault.as_ref().is_some_and(|d| d.busy) {
            return;
        }
        #[cfg(any(windows, target_os = "linux"))]
        if phase == Phase::Unlock {
            if let Some(mounted) = &self.vault_mount {
                if mounted.root() == path {
                    let location = mounted.path().to_path_buf();
                    self.navigate(self.state.focused_tile, location);
                    return;
                }
            }
        }
        if !self.close_native_vault(false) {
            return;
        }
        let now = Instant::now();
        self.state.preview.clear_all();
        let name = if phase == Phase::Create {
            path.file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned()
        } else {
            vault::display_name(&path, &self.state.profiles.active().id)
                .unwrap_or_else(|| rust_i18n::t!("vault_locked_folder").to_string())
        };
        self.vault = Some(Dialog {
            path,
            name,
            phase,
            inputs: Vec::new(),
            session: None,
            profiles: HashSet::from([self.state.profiles.active().id.clone()]),
            profiles_open: false,
            reveal: false,
            busy: false,
            error: None,
            notice: None,
            parents: Vec::new(),
            selected: None,
            preview: None,
            control: Control::default(),
            last_activity: now,
            generation: now,
            rx: None,
            progress: 0,
        });
    }

    pub(crate) fn try_open_vault(&mut self, path: &Path) -> bool {
        let Some(root) = vault::storage_root(path) else {
            return false;
        };
        if vault::visible_to(&root, &self.state.profiles.active().id) {
            self.request_vault(root, Phase::Unlock);
        } else {
            self.toast_error(rust_i18n::t!("vault_access_denied").to_string());
        }
        true
    }

    pub(crate) fn private_folder_entries(
        &self,
        entries: Vec<atlas_model::file_entry::FileEntry>,
    ) -> Vec<atlas_model::file_entry::FileEntry> {
        vault::filter_entries(
            entries
                .into_iter()
                .filter(|entry| {
                    !vault::live::is_private(&entry.path())
                        || self.owns_native_vault_path(&entry.path())
                })
                .collect(),
            &self.state.profiles.active().id,
            &rust_i18n::t!("vault_locked_folder"),
        )
    }

    pub(crate) fn ensure_vault_inputs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(d) = self.vault.as_ref() else {
            return;
        };
        let count = match d.phase {
            Phase::Create => 2,
            Phase::Unlock => 1,
            Phase::Manage => 3,
            _ => 0,
        };
        if d.inputs.len() == count {
            return;
        }
        let inputs = (0..count)
            .map(|_| cx.new(|cx| InputState::new(window, cx).masked(true)))
            .collect();
        if let Some(d) = &mut self.vault {
            d.inputs = inputs;
            d.reveal = false;
        }
    }

    pub(crate) fn vault_focus(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> FocusHandle {
        self.ensure_vault_inputs(window, cx);
        // Preserve whichever field the user chose; never snap back from confirmation.
        if let Some(d) = &self.vault {
            if !d.busy {
                for input in &d.inputs {
                    let focus = input.read(cx).focus_handle(cx);
                    if focus.is_focused(window) {
                        return focus;
                    }
                }
            }
        }
        let panel = self.overlay_focus(cx);
        if panel.contains_focused(window, cx) {
            return panel;
        }
        self.vault
            .as_ref()
            .filter(|d| !d.busy)
            .and_then(|d| d.inputs.first())
            .map(|i| i.read(cx).focus_handle(cx))
            .unwrap_or(panel)
    }

    pub(crate) fn clear_vault_inputs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(d) = &mut self.vault {
            for input in &d.inputs {
                input.update(cx, |i, cx| i.set_value("", window, cx));
            }
            d.inputs.clear();
            d.reveal = false;
        }
    }

    pub(crate) fn close_vault(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(d) = &self.vault {
            if d.busy {
                d.control.cancel();
                return;
            }
        }
        self.clear_vault_inputs(window, cx);
        self.vault = None;
        self.state.preview.clear_all();
        cx.notify();
    }

    pub(crate) fn vault_phase(
        &mut self,
        phase: Phase,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.clear_vault_inputs(window, cx);
        if phase == Phase::Browse {
            if let Some(session) = self.vault.as_mut().and_then(|d| d.session.take()) {
                self.run_vault_job(move || Outcome::Ready(session, None));
                cx.notify();
                return;
            }
        }
        if let Some(d) = &mut self.vault {
            if d.busy {
                return;
            }
            d.phase = phase;
            d.error = None;
            d.notice = None;
            d.preview = None;
            d.profiles_open = false;
            if let Some(session) = &d.session {
                d.profiles = session.profiles().iter().cloned().collect();
            }
        }
        self.ensure_vault_inputs(window, cx);
        cx.notify();
    }

    fn run_vault_job(&mut self, job: impl FnOnce() -> Outcome + Send + 'static) {
        let Some(d) = &mut self.vault else {
            return;
        };
        d.busy = true;
        d.error = None;
        d.notice = None;
        d.preview = None;
        let (tx, rx) = flume::bounded(1);
        d.rx = Some(rx);
        let wake = self.tx.clone();
        self.rt.spawn_blocking(move || {
            let result = match job() {
                Outcome::Ready(session, source) => mount_ready(session, source, None),
                Outcome::Updated(session, notice) => mount_ready(session, None, notice),
                other => other,
            };
            let _ = tx.send(result);
            let _ = wake.send(atlas_model::app_state::BgResult::VaultWake);
        });
    }

    pub(crate) fn submit_vault(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(d) = self.vault.as_ref().filter(|d| !d.busy) else {
            return;
        };
        let phase = d.phase;
        let values: Vec<_> = d
            .inputs
            .iter()
            .map(|i| Zeroizing::new(i.read(cx).value().to_string()))
            .collect();
        if matches!(phase, Phase::Create | Phase::Manage) {
            let (password, confirmation) = if phase == Phase::Manage {
                (&values[1], &values[2])
            } else {
                (&values[0], &values[1])
            };
            if (!password.is_empty() || phase == Phase::Create) && password.chars().count() < 12 {
                self.vault.as_mut().unwrap().error =
                    Some(rust_i18n::t!("vault_password_length").to_string());
                cx.notify();
                return;
            }
            if password.as_str() != confirmation.as_str() {
                self.vault.as_mut().unwrap().error =
                    Some(rust_i18n::t!("vault_password_mismatch").to_string());
                cx.notify();
                return;
            }
        }
        let active = self.state.profiles.active().id.clone();
        let profiles: Vec<_> = self
            .state
            .profiles
            .profiles
            .iter()
            .filter(|p| d.profiles.contains(&p.id))
            .map(|p| p.id.clone())
            .collect();
        let path = d.path.clone();
        self.clear_vault_inputs(window, cx);
        let d = self.vault.as_mut().unwrap();
        d.control = Control::default();
        let control = d.control.clone();
        match phase {
            Phase::Create => self.run_vault_job(move || {
                match vault::protect(&path, &values[0], profiles, &active, &control) {
                    Ok(s) => Outcome::Ready(s, Some(path)),
                    Err(e) => Outcome::Failed(None, format!("{e:#}")),
                }
            }),
            Phase::Unlock => {
                self.run_vault_job(move || match vault::unlock(&path, &values[0], &active) {
                    Ok(s) => Outcome::Ready(s, None),
                    Err(e) => Outcome::Failed(None, format!("{e:#}")),
                })
            }
            Phase::Manage => {
                let Some(mut s) = d.session.take() else {
                    return;
                };
                self.run_vault_job(move || {
                    let new = (!values[1].is_empty()).then_some(values[1].as_str());
                    match s.change_access(&values[0], new, profiles, &active) {
                        Ok(()) => Outcome::Updated(
                            s,
                            Some(rust_i18n::t!("vault_access_saved").to_string()),
                        ),
                        Err(e) => Outcome::Failed(Some(s), format!("{e:#}")),
                    }
                });
            }
            Phase::Restore => {
                let Some(s) = d.session.take() else {
                    return;
                };
                self.run_vault_job(move || {
                    // Export errors retain the session and all encrypted originals.
                    let destination = s.root().parent().unwrap_or(Path::new(".")).join(s.name());
                    if let Err(e) = s.export(None, &destination, &control) {
                        return Outcome::Failed(Some(s), format!("{e:#}"));
                    }
                    let root = s.root().to_path_buf();
                    drop(s);
                    let removed = std::fs::remove_dir_all(root).is_ok();
                    Outcome::Restored(destination, removed)
                });
            }
            Phase::Browse => {}
        }
        cx.notify();
    }

    pub(crate) fn poll_vault(&mut self) -> bool {
        #[cfg(any(windows, target_os = "linux"))]
        if self.vault_mount.as_ref().is_some_and(|mount| {
            mount.idle_for() >= Duration::from_secs(300) && !mount.has_open_files()
        }) {
            self.close_native_vault(false);
            self.toast_info(rust_i18n::t!("vault_auto_locked").to_string());
            return true;
        }
        let Some(d) = &mut self.vault else {
            return false;
        };
        if !d.busy && d.last_activity.elapsed() >= Duration::from_secs(300) {
            self.vault = None;
            self.toast_info(rust_i18n::t!("vault_auto_locked").to_string());
            return true;
        }
        let progress = d.control.completed.load(Ordering::Relaxed);
        let changed = progress != d.progress;
        d.progress = progress;
        let result = match d.rx.as_ref().map(flume::Receiver::try_recv) {
            Some(Ok(result)) => result,
            Some(Err(flume::TryRecvError::Disconnected)) => {
                Outcome::Failed(None, "vault_worker_failed".into())
            }
            _ => return changed,
        };
        d.rx = None;
        d.busy = false;
        d.last_activity = Instant::now();
        match result {
            #[cfg(any(windows, target_os = "linux"))]
            Outcome::Mounted(mounted, source, notice) => {
                if let Some(source) = source {
                    self.purge_private_history(&source);
                }
                let location = mounted.path().to_path_buf();
                self.vault = None;
                self.vault_mount = Some(mounted);
                self.navigate(self.state.focused_tile, location);
                self.refresh_vault_parent();
                if let Some(notice) = notice {
                    self.toast_info(notice);
                }
            }
            Outcome::MountUnavailable(session, source, error) => {
                d.path = session.root().to_path_buf();
                d.profiles = session.profiles().iter().cloned().collect();
                d.session = Some(session);
                d.phase = Phase::Browse;
                d.error = Some(vault_error(&error));
                if let Some(source) = source {
                    self.purge_private_history(&source);
                    self.refresh_vault_parent();
                }
            }
            Outcome::Ready(session, source) => {
                d.path = session.root().to_path_buf();
                d.profiles = session.profiles().iter().cloned().collect();
                d.session = Some(session);
                d.phase = Phase::Browse;
                if let Some(source) = source {
                    self.purge_private_history(&source);
                    self.refresh_vault_parent();
                }
            }
            Outcome::Updated(s, notice) => {
                d.session = Some(s);
                d.phase = Phase::Browse;
                d.notice = notice;
            }
            Outcome::Preview(s, text) => {
                d.session = Some(s);
                d.preview = Some(text);
            }
            Outcome::Restored(path, removed) => {
                self.vault = None;
                self.refresh_all_vault_listings();
                if removed {
                    self.toast_info(rust_i18n::t!("vault_restored").to_string());
                } else {
                    self.toast_info(rust_i18n::t!("vault_restored_copy_retained").to_string());
                }
                let _ = path;
            }
            Outcome::Failed(s, error) => {
                d.session = s;
                if d.session.is_none() && !matches!(d.phase, Phase::Create | Phase::Unlock) {
                    d.phase = Phase::Unlock;
                }
                d.error = Some(vault_error(&error));
            }
        }
        true
    }

    fn refresh_vault_parent(&mut self) {
        self.refresh_all_vault_listings();
    }

    /// Revoke the OS mount before profile snapshots, then discard all private UI state.
    pub(crate) fn close_native_vault(&mut self, force: bool) -> bool {
        #[cfg(any(windows, target_os = "linux"))]
        {
            if self.vault_mount.is_some() && !force && !self.state.ops.active_ops.is_empty() {
                self.toast_error(rust_i18n::t!("vault_files_open").to_string());
                return false;
            }
            if self
                .vault_mount
                .as_ref()
                .is_some_and(|mount| !force && !mount.prepare_lock())
            {
                self.toast_error(rust_i18n::t!("vault_files_open").to_string());
                return false;
            }
            if let Some(mounted) = self.vault_mount.take() {
                let path = mounted.path().to_path_buf();
                let parent = mounted
                    .root()
                    .parent()
                    .unwrap_or_else(|| Path::new(if cfg!(windows) { "C:\\" } else { "/" }))
                    .to_path_buf();
                self.watchers
                    .retain(|_, watcher| !watcher.watched_path.starts_with(&path));
                for (_, tile) in self.state.pane_tree.tiles.iter_mut() {
                    if let atlas_model::layout_tree::Tile::Pane(tab) = tile {
                        if tab.location.display_path().starts_with(&path) {
                            tab.location =
                                atlas_model::location::Location::Directory(parent.clone());
                        }
                    }
                }
                drop(mounted);
                self.purge_private_history(&path);
                self.preview_textures.clear();
                self.multi_textures.clear();
                self.preview_editor = None;
                self.refresh_all_vault_listings();
            }
        }
        #[cfg(not(any(windows, target_os = "linux")))]
        let _ = force;
        true
    }

    pub(crate) fn owns_native_vault_path(&self, path: &Path) -> bool {
        #[cfg(any(windows, target_os = "linux"))]
        {
            self.vault_mount
                .as_ref()
                .is_some_and(|mount| path.starts_with(mount.path()))
        }
        #[cfg(not(any(windows, target_os = "linux")))]
        {
            let _ = path;
            false
        }
    }

    pub(crate) fn manage_native_vault(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        #[cfg(any(windows, target_os = "linux"))]
        {
            if self
                .vault_mount
                .as_ref()
                .is_some_and(|mount| !mount.prepare_lock())
            {
                self.toast_error(rust_i18n::t!("vault_files_open").to_string());
                return;
            }
            if let Some(mount) = self.vault_mount.take() {
                let root = mount.root().to_path_buf();
                let path = mount.path().to_path_buf();
                let session = mount.into_session();
                let parent = root
                    .parent()
                    .unwrap_or_else(|| Path::new(if cfg!(windows) { "C:\\" } else { "/" }))
                    .to_path_buf();
                for (_, tile) in self.state.pane_tree.tiles.iter_mut() {
                    if let atlas_model::layout_tree::Tile::Pane(tab) = tile {
                        if tab.location.display_path().starts_with(&path) {
                            tab.location =
                                atlas_model::location::Location::Directory(parent.clone());
                        }
                    }
                }
                self.watchers
                    .retain(|_, watcher| !watcher.watched_path.starts_with(&path));
                self.purge_private_history(&path);
                self.request_vault(root, Phase::Manage);
                self.vault.as_mut().unwrap().session = Some(session);
                self.vault_phase(Phase::Manage, window, cx);
            }
        }
        #[cfg(not(any(windows, target_os = "linux")))]
        let _ = (window, cx);
    }

    pub(crate) fn install_vault_driver(&mut self, cx: &mut Context<Self>) {
        #[cfg(windows)]
        {
            let Some(dialog) = self.vault.as_mut().filter(|d| !d.busy) else {
                return;
            };
            let Some(session) = dialog.session.take() else {
                return;
            };
            let rt = self.rt.clone();
            self.run_vault_job(move || {
                match rt.block_on(atlas_core::vault::mount::install_driver()) {
                    Ok(()) => Outcome::Ready(session, None),
                    Err(error) => Outcome::Failed(Some(session), format!("WinFsp: {error:#}")),
                }
            });
            cx.notify();
        }
        #[cfg(not(windows))]
        let _ = cx;
    }
    fn refresh_all_vault_listings(&mut self) {
        self.nucleo.reset();
        self.goto_warm_nucleo.reset();
        self.goto_index_root = None;
        self.goto_service_cache = None;
        self.goto_snapshot = Default::default();
        self.goto_service_generation.fetch_add(1, Ordering::SeqCst);
        self.content_gen = self.content_gen.wrapping_add(1);
        self.content_results.clear();
        self.state.preview.clear_all();
        self.thumb_textures.clear();
        self.file_filter_projections.clear();
        let panes: Vec<_> = self
            .state
            .all_pane_tile_ids()
            .into_iter()
            .filter_map(|id| self.tab_data_for(id).map(|t| (id, t.location.clone())))
            .collect();
        for (id, location) in panes {
            if let Some(path) = location.fs_dir() {
                if !self.watchers.contains_key(&id) {
                    if let Ok(watcher) = atlas_core::fs::watcher::watch_path(path, self.tx.clone())
                    {
                        self.watchers.insert(id, watcher);
                    }
                }
            }
            self.spawn_list(id, location);
        }
        self.dirty_all_panes = true;
    }

    fn purge_private_history(&mut self, source: &Path) {
        self.state.overlay.op_errors.clear();
        let public_undo = |entry: &atlas_model::undo::UndoEntry| {
            use atlas_model::undo::UndoEntry;
            match entry {
                UndoEntry::Move { pairs } | UndoEntry::Copy { pairs } => pairs
                    .iter()
                    .all(|(a, b)| !a.starts_with(source) && !b.starts_with(source)),
                UndoEntry::Rename { from, to } => {
                    !from.starts_with(source) && !to.starts_with(source)
                }
                UndoEntry::Trash { paths } => paths.iter().all(|path| !path.starts_with(source)),
            }
        };
        self.state.ops.undo_stack.undo.retain(public_undo);
        self.state.ops.undo_stack.redo.retain(public_undo);
        if self
            .state
            .ops
            .clipboard
            .as_ref()
            .is_some_and(|clipboard| clipboard.paths.iter().any(|path| path.starts_with(source)))
        {
            self.state.ops.clipboard = None;
            #[cfg(windows)]
            if crate::dnd::windows::ole_clipboard::get_clipboard_files()
                .is_some_and(|files| files.paths.iter().any(|path| path.starts_with(source)))
            {
                crate::dnd::windows::ole_clipboard::clear();
            }
        }
        let _ = self
            .tag_db_tx
            .send(atlas_core::fs::tag_db::TagDbCmd::ForgetSubtree(
                source.to_path_buf(),
            ));
        use atlas_model::{
            layout_tree::Tile,
            location::Location,
            profiles::{FolderViewFeature, RecentFoldersFeature},
        };
        let parent = source.parent().unwrap_or(Path::new("/")).to_path_buf();
        let scrub = |tab: &mut atlas_model::pane::TabData| {
            if tab.location.fs_dir().is_some_and(|p| p.starts_with(source)) {
                tab.location = Location::Directory(parent.clone());
            }
            tab.history
                .retain(|l| !l.display_path().starts_with(source));
            tab.history_forward
                .retain(|l| !l.display_path().starts_with(source));
            tab.breadcrumb_trail = None;
        };
        for profile in &mut self.state.profiles.profiles {
            profile.bookmarks.retain(|b| !b.path.starts_with(source));
            let mut recents = profile.scoped::<RecentFoldersFeature>();
            recents.retain(|p| !p.starts_with(source));
            profile.set_scoped::<RecentFoldersFeature>(&recents);
            // Folder view preferences include native paths; drop the affected ones.
            let mut views = profile.scoped::<FolderViewFeature>();
            views.remove_subtree(source);
            profile.set_scoped::<FolderViewFeature>(&views);
            profile.prefs.remove("recent_folder_volumes");
            if let Some(tree) = &mut profile.tree {
                for (_, tile) in tree.tiles.iter_mut() {
                    if let Tile::Pane(tab) = tile {
                        scrub(tab);
                    }
                }
            }
        }
        for (_, tile) in self.state.pane_tree.tiles.iter_mut() {
            if let Tile::Pane(tab) = tile {
                scrub(tab);
            }
        }
        self.state.tab_states.clear();
        self.watchers
            .retain(|_, watcher| !watcher.watched_path.starts_with(source));
        self.layout_revision += 1;
        atlas_core::util::config::save_profiles(&self.state.profiles);
        atlas_core::util::config::save_session(&self.state.pane_tree);
    }

    pub(crate) fn vault_preview_selected(&mut self, cx: &mut Context<Self>) {
        let Some(d) = self.vault.as_mut().filter(|d| !d.busy) else {
            return;
        };
        let Some(id) = d.selected.clone() else {
            return;
        };
        if d.session
            .as_ref()
            .and_then(|s| s.entry(&id))
            .is_some_and(|e| e.directory)
        {
            d.parents.push(id);
            d.selected = None;
            d.preview = None;
            d.error = None;
            d.last_activity = Instant::now();
            cx.notify();
            return;
        }
        let Some(s) = d.session.take() else {
            return;
        };
        let is_image =
            s.entry(&id)
                .and_then(|entry| {
                    Path::new(&entry.name).extension()?.to_str().map(|ext| {
                        atlas_model::preview_cache::is_image_ext(&ext.to_ascii_lowercase())
                    })
                })
                .unwrap_or(false);
        d.control = Control::default();
        self.run_vault_job(move || {
            match s.read(
                &id,
                if is_image {
                    32 * 1024 * 1024
                } else {
                    1024 * 1024
                },
            ) {
                Ok(bytes) if is_image => match decode_private_image(&bytes) {
                    Ok(preview) => Outcome::Preview(s, preview),
                    Err(_) => Outcome::Failed(Some(s), "vault_preview_unsupported".into()),
                },
                Ok(bytes) => match std::str::from_utf8(&bytes) {
                    Ok(text) if !text.contains('\0') => {
                        Outcome::Preview(s, PrivatePreview::Text(Zeroizing::new(text.to_owned())))
                    }
                    _ => Outcome::Failed(Some(s), "vault_preview_unsupported".into()),
                },
                Err(e) => Outcome::Failed(Some(s), format!("{e:#}")),
            }
        });
        cx.notify();
    }

    pub(crate) fn vault_import_picker(&mut self, directories: bool, cx: &mut Context<Self>) {
        let Some(d) = self.vault.as_ref().filter(|d| !d.busy) else {
            return;
        };
        let generation = d.generation;
        let picker = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: !directories,
            directories,
            multiple: true,
            prompt: Some(rust_i18n::t!("vault_import").to_string().into()),
        });
        cx.spawn(async move |app, cx| {
            let Ok(Ok(Some(paths))) = picker.await else {
                return;
            };
            let _ = app.update(cx, |this, cx| {
                let Some(d) = this
                    .vault
                    .as_mut()
                    .filter(|d| d.generation == generation && !d.busy)
                else {
                    return;
                };
                let Some(mut s) = d.session.take() else {
                    return;
                };
                let parent = d.parents.last().cloned();
                d.control = Control::default();
                let control = d.control.clone();
                this.run_vault_job(
                    move || match s.import(&paths, parent.as_deref(), &control) {
                        Ok(()) => {
                            Outcome::Updated(s, Some(rust_i18n::t!("vault_imported").to_string()))
                        }
                        Err(e) => Outcome::Failed(Some(s), format!("{e:#}")),
                    },
                );
                cx.notify();
            });
        })
        .detach();
    }

    pub(crate) fn vault_export_picker(&mut self, cx: &mut Context<Self>) {
        let Some(d) = self.vault.as_ref().filter(|d| !d.busy) else {
            return;
        };
        let Some(s) = &d.session else {
            return;
        };
        let id = d.selected.clone();
        let name = id
            .as_ref()
            .and_then(|id| s.entry(id))
            .map_or(s.name(), |e| e.name.as_str());
        let generation = d.generation;
        let picker =
            cx.prompt_for_new_path(s.root().parent().unwrap_or(Path::new(".")), Some(name));
        cx.spawn(async move |app, cx| {
            let Ok(Ok(Some(destination))) = picker.await else {
                return;
            };
            let _ = app.update(cx, |this, cx| {
                let Some(d) = this
                    .vault
                    .as_mut()
                    .filter(|d| d.generation == generation && !d.busy)
                else {
                    return;
                };
                let Some(s) = d.session.take() else {
                    return;
                };
                d.control = Control::default();
                let control = d.control.clone();
                this.run_vault_job(
                    move || match s.export(id.as_deref(), &destination, &control) {
                        Ok(()) => {
                            Outcome::Updated(s, Some(rust_i18n::t!("vault_exported").to_string()))
                        }
                        Err(e) => Outcome::Failed(Some(s), format!("{e:#}")),
                    },
                );
                cx.notify();
            });
        })
        .detach();
    }
}

/// Decode in memory with bounded dimensions/allocation; no paths, temp files, or thumbnail cache.
fn mount_ready(session: Session, source: Option<PathBuf>, notice: Option<String>) -> Outcome {
    #[cfg(any(windows, target_os = "linux"))]
    {
        match atlas_core::vault::mount::Mounted::start(session) {
            Ok(mount) => Outcome::Mounted(mount, source, notice),
            Err((session, error)) => Outcome::MountUnavailable(session, source, error),
        }
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        let _ = notice;
        Outcome::MountUnavailable(session, source, "vault_mount_unavailable".into())
    }
}

fn decode_private_image(bytes: &[u8]) -> anyhow::Result<PrivatePreview> {
    let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes)).with_guessed_format()?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(8192);
    limits.max_image_height = Some(8192);
    limits.max_alloc = Some(64 * 1024 * 1024);
    reader.limits(limits);
    let image = reader.decode()?.thumbnail(1600, 1600).into_rgba8();
    let (width, height) = image.dimensions();
    let pixels = Zeroizing::new(image.into_raw());
    let rendered =
        crate::ui::inspector::rgba_to_render_image(&pixels, width as usize, height as usize)
            .ok_or_else(|| anyhow::anyhow!("invalid image dimensions"))?;
    Ok(PrivatePreview::Image(rendered))
}

fn vault_error(error: &str) -> String {
    for key in [
        "vault_driver_missing",
        "vault_mount_failed",
        "vault_mount_unavailable",
        "vault_driver_install_failed",
        "vault_wrong_password",
        "vault_access_denied",
        "vault_password_length",
        "vault_invalid_profiles",
        "vault_in_use",
        "vault_unsafe_link",
        "vault_invalid_name",
        "vault_nested",
        "vault_original_cleanup_failed",
        "vault_source_changed",
        "vault_name_exists",
        "vault_export_exists",
        "vault_preview_too_large",
        "vault_preview_unsupported",
        "vault_cancelled",
        "vault_damaged",
        "vault_invalid_manifest",
        "vault_too_many_entries",
    ] {
        if error.contains(key) {
            return rust_i18n::t!(key).to_string();
        }
    }
    rust_i18n::t!("vault_operation_failed").to_string()
}

#[cfg(feature = "ui-lab")]
pub(crate) mod diagnostic;
