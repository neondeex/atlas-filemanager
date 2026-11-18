use std::collections::HashMap;
use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

use crate::undo::UndoStack;

pub struct ClipboardState {
    pub paths: Vec<PathBuf>,
    /// Optional destination names, parallel to `paths`.
    /// Used when the source path's real filename is not the user-visible name
    /// (Windows recycle bin `$R...` payloads).
    pub target_names: Option<Vec<OsString>>,
    pub is_cut: bool,
    /// Set when the paths come from a remote tab (remote-relative paths, not local).
    /// `None` = local filesystem paths.
    pub remote_id: Option<String>,
    /// Atlas offered these paths on the system clipboard itself and keeps
    /// serving them until another app takes the clipboard over (Wayland).
    pub on_system_clipboard: bool,
}

/// How to handle a destination file that already exists when pasting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resolution {
    /// Replace the existing file/folder.
    Overwrite,
    /// Leave the existing file untouched; don't paste the source.
    Skip,
    /// Paste under an auto-generated unique name ("name (1)").
    Rename,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransferKind {
    Copy,
    Move,
}

/// Where a transfer was initiated from. Drives toast wording and per-origin
/// behavior (e.g. cleaning up temp files extracted for an external drop).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransferOrigin {
    Paste,
    /// Paste of files *cut in another app* (Explorer): on success,
    /// `Performed DropEffect = MOVE` is written back so the source clears its
    /// cut state.
    PasteExternalCut,
    DndInternal,
    DndExternal,
    Menu,
    Undo,
}

/// Conflict handling for an entire transfer. `Ask` holds the transfer and opens
/// the conflict dialog; the rest execute immediately.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConflictPolicy {
    Ask,
    OverwriteAll,
    SkipAll,
    RenameAll,
    /// One [`Resolution`] per conflict, parallel to
    /// [`TransferConflictState::conflicts`].
    PerItem(Vec<Resolution>),
}

/// A single source→destination transfer of one or more files, with all the
/// context needed to validate, resolve conflicts, and execute it.
#[derive(Debug, Clone)]
pub struct TransferRequest {
    pub sources: Vec<PathBuf>,
    /// Optional destination names, parallel to `sources`.
    pub target_names: Option<Vec<OsString>>,
    pub dest: PathBuf,
    pub kind: TransferKind,
    pub policy: ConflictPolicy,
    pub origin: TransferOrigin,
}

/// One name collision at the destination, with enough detail for the dialog
/// (sizes/dates are stat'ed once at detection time, not in the render loop).
#[derive(Debug, Clone)]
pub struct ConflictItem {
    pub source: PathBuf,
    /// Name the incoming item should have under the destination.
    pub target_name: OsString,
    /// The pre-existing entry at the destination that `source` collides with.
    pub existing: PathBuf,
    pub is_dir: bool,
    pub src_size: Option<u64>,
    pub src_modified: Option<std::time::SystemTime>,
    pub dst_size: Option<u64>,
    pub dst_modified: Option<std::time::SystemTime>,
}

/// Pending transfer that collided with existing names at the destination. While
/// this is `Some`, the conflict dialog is shown and the transfer is held until
/// the user picks a policy (or cancels).
pub struct TransferConflictState {
    pub request: TransferRequest,
    pub conflicts: Vec<ConflictItem>,
    /// Sources with no collision plus their destination names.
    pub clean: Vec<(PathBuf, OsString)>,
    /// `Some` while the dialog is in "decide one by one" mode: the pending
    /// choice for each conflict, parallel to `conflicts`.
    pub per_item: Option<Vec<Resolution>>,
    /// Keyboard-highlighted footer action. `0` is the safe default action.
    pub selected_action: usize,
}

/// Typed cause of a per-item failure. The UI translates each variant
/// (`op_error_text`); core never bakes user-facing prose into errors.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpErrorKind {
    /// Source or destination no longer exists.
    Gone,
    /// The target filesystem does not expose an OS trash/recycle bin.
    TrashUnsupported,
    PermissionDenied,
    /// Locked by another process (sharing/lock violation).
    InUse,
    DiskFull,
    PathTooLong,
    NetworkUnreachable,
    /// The destination already exists (e.g. restoring over a recreated file).
    AlreadyExists,
    /// Copying/moving a folder into itself.
    SelfNested,
    /// Overwriting a file with itself.
    SelfOverwrite,
    /// The existing destination could not be recycled before replacing it.
    ReplaceFailed(String),
    /// A move copied the file but could not remove the source.
    SourceNotRemoved(String),
    /// Pre-flight free-space check failed (values pre-formatted, e.g. "1.2 GB").
    NotEnoughSpace {
        needed: String,
        free: String,
    },
    /// The engine aborted after too many identical consecutive failures.
    RepeatedAbort,
    /// The whole operation was cancelled by the user.
    Cancelled,
    /// Fallback: OS detail or an already-localized message.
    Msg(String),
}

/// A per-item failure surfaced to the user as its own toast.
#[derive(Debug, Clone)]
pub struct OpError {
    pub path: PathBuf,
    pub kind: OpErrorKind,
}

// Log-only formatting (tracing); user-facing text comes from the UI translator.
impl std::fmt::Display for OpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let name = self.path.file_name().map_or_else(
            || self.path.display().to_string(),
            |n| n.to_string_lossy().into_owned(),
        );
        write!(f, "{name}: {:?}", self.kind)
    }
}

/// Outcome of an executed transfer. `completed` holds the *real* src→dst pairs
/// (post conflict-resolution), which is what undo records.
#[derive(Debug, Clone, Default)]
pub struct TransferReport {
    pub completed: Vec<(PathBuf, PathBuf)>,
    pub skipped: Vec<PathBuf>,
    pub errors: Vec<OpError>,
    pub cancelled: bool,
}

pub struct OpState {
    pub done: u64,
    pub total: u64,
    pub bytes_done: u64,
    pub bytes_total: u64,
    pub current_file: PathBuf,
    /// Short verb for the status bar ("Copiando", "Moviendo", "Eliminando"…).
    pub label: String,
    /// Set to request cancellation; the worker checks it per file and per chunk.
    pub cancel: Arc<AtomicBool>,
    /// Wall-clock start used by the transfer UI to derive a stable average
    /// throughput and ETA without coupling presentation state to the engine.
    pub started_at: std::time::Instant,
}

pub struct OperationManager {
    pub undo_stack: UndoStack,
    pub clipboard: Option<ClipboardState>,
    pub active_ops: HashMap<u64, OpState>,
    op_id_counter: Arc<AtomicU64>,
}

impl Default for OperationManager {
    fn default() -> Self {
        Self {
            undo_stack: UndoStack::default(),
            clipboard: None,
            active_ops: HashMap::new(),
            op_id_counter: Arc::new(AtomicU64::new(1)),
        }
    }
}

impl OperationManager {
    pub fn next_op_id(&self) -> u64 {
        self.op_id_counter.fetch_add(1, Ordering::Relaxed)
    }

    /// Register a new background operation with a status-bar label; returns the
    /// op ID and the cancellation flag to hand to the worker.
    pub fn start_op_labeled(&mut self, count: usize, label: String) -> (u64, Arc<AtomicBool>) {
        let op_id = self.next_op_id();
        let cancel = Arc::new(AtomicBool::new(false));
        self.active_ops.insert(
            op_id,
            OpState {
                done: 0,
                total: count as u64,
                bytes_done: 0,
                bytes_total: 0,
                current_file: PathBuf::new(),
                label,
                cancel: cancel.clone(),
                started_at: std::time::Instant::now(),
            },
        );
        (op_id, cancel)
    }
}
