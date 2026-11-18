//! Immutable in-memory size tree. One scan produces one `SpaceTree`; navigating
//! to any folder is then a lookup, never another disk read.
use atlas_model::cleanup::{DiskEntry, DiskSnapshot, ScanBackend, ScanStatus};
use std::{
    cmp::Reverse,
    collections::BinaryHeap,
    path::{Component, Path, PathBuf},
    sync::{Arc, Mutex},
};

/// Files smaller than this are folded into their folder's `loose` bytes so a
/// multi-million file volume only keeps directories and meaningful files.
pub const FILE_KEEP: u64 = 256 * 1024;
const MAX_ROWS: usize = 512;
const MAX_FILES: usize = 256;

struct TNode {
    parent: u32,
    name: Box<str>,
    /// Subtree bytes (own loose bytes + children).
    size: u64,
    files: u64,
    dir: bool,
    kids: (u32, u32),
}

pub struct SpaceTree {
    root: PathBuf,
    nodes: Vec<TNode>,
    kids: Vec<u32>,
    pub backend: ScanBackend,
    pub skipped: u64,
    pub errors: u64,
    pub limited: bool,
}

pub struct TreeBuilder {
    nodes: Vec<TNode>,
    keep: u64,
    pub skipped: u64,
    pub errors: u64,
    pub limited: bool,
}

impl TreeBuilder {
    pub fn new(keep: u64) -> Self {
        Self {
            nodes: vec![TNode {
                parent: u32::MAX,
                name: "".into(),
                size: 0,
                files: 0,
                dir: true,
                kids: (0, 0),
            }],
            keep,
            skipped: 0,
            errors: 0,
            limited: false,
        }
    }
    pub fn add_dir(&mut self, parent: u32, name: &str) -> u32 {
        self.nodes.push(TNode {
            parent,
            name: name.into(),
            size: 0,
            files: 0,
            dir: true,
            kids: (0, 0),
        });
        (self.nodes.len() - 1) as u32
    }
    pub fn add_file(&mut self, parent: u32, name: &str, size: u64) {
        if size >= self.keep && size > 0 {
            self.nodes.push(TNode {
                parent,
                name: name.into(),
                size,
                files: 1,
                dir: false,
                kids: (0, 0),
            });
        } else {
            self.fold(parent, size, 1);
        }
    }
    /// Account for files that are not individually retained.
    pub fn fold(&mut self, parent: u32, bytes: u64, files: u64) {
        let node = &mut self.nodes[parent as usize];
        node.size = node.size.saturating_add(bytes);
        node.files += files;
    }
    pub fn finish(mut self, root: PathBuf, backend: ScanBackend) -> SpaceTree {
        let n = self.nodes.len();
        // Children always have a larger id than their parent.
        for i in (1..n).rev() {
            let (size, files, parent) = {
                let node = &self.nodes[i];
                (node.size, node.files, node.parent as usize)
            };
            let p = &mut self.nodes[parent];
            p.size = p.size.saturating_add(size);
            p.files += files;
        }
        let mut counts = vec![0u32; n + 1];
        for node in &self.nodes[1..] {
            counts[node.parent as usize] += 1;
        }
        let mut start = 0u32;
        for (i, count) in counts.iter().take(n).enumerate() {
            self.nodes[i].kids = (start, *count);
            start += count;
        }
        let mut cursor: Vec<u32> = self.nodes.iter().map(|n| n.kids.0).collect();
        let mut kids = vec![0u32; start as usize];
        for i in 1..n {
            let p = self.nodes[i].parent as usize;
            kids[cursor[p] as usize] = i as u32;
            cursor[p] += 1;
        }
        for i in 0..n {
            let (s, c) = self.nodes[i].kids;
            let nodes = &self.nodes;
            kids[s as usize..(s + c) as usize].sort_unstable_by(|a, b| {
                let (a, b) = (&nodes[*a as usize], &nodes[*b as usize]);
                b.size.cmp(&a.size).then_with(|| a.name.cmp(&b.name))
            });
        }
        SpaceTree {
            root,
            nodes: self.nodes,
            kids,
            backend,
            skipped: self.skipped,
            errors: self.errors,
            limited: self.limited,
        }
    }
}

impl SpaceTree {
    fn kids_of(&self, id: u32) -> &[u32] {
        let (s, c) = self.nodes[id as usize].kids;
        &self.kids[s as usize..(s + c) as usize]
    }
    fn find(&self, path: &Path) -> Option<u32> {
        self.locate(path, true)
    }
    /// `dir_only` restricts every component to directories; otherwise the last may be a file.
    fn locate(&self, path: &Path, dir_only: bool) -> Option<u32> {
        let rel = path.strip_prefix(&self.root).ok()?;
        let mut current = 0u32;
        let count = rel.components().count();
        for (i, part) in rel.components().enumerate() {
            let Component::Normal(name) = part else {
                return None;
            };
            let name = name.to_string_lossy();
            current = *self.kids_of(current).iter().find(|k| {
                let n = &self.nodes[**k as usize];
                (n.dir || (!dir_only && i + 1 == count)) && n.name.eq_ignore_ascii_case(&name)
            })?;
        }
        Some(current)
    }
    fn path_of(&self, id: u32, base: &Path, base_id: u32) -> PathBuf {
        let mut names = Vec::new();
        let mut at = id;
        while at != base_id && at != u32::MAX {
            names.push(&*self.nodes[at as usize].name);
            at = self.nodes[at as usize].parent;
        }
        let mut path = base.to_owned();
        for name in names.into_iter().rev() {
            path.push(name);
        }
        path
    }
    fn entry(&self, id: u32, base: &Path, base_id: u32) -> DiskEntry {
        let node = &self.nodes[id as usize];
        DiskEntry {
            path: self.path_of(id, base, base_id),
            bytes: node.size,
            directory: node.dir,
        }
    }

    /// Forget an entry that was deleted, keeping every ancestor total consistent.
    fn remove_node(&mut self, path: &Path) -> bool {
        let Some(id) = self.locate(path, false).filter(|id| *id != 0) else {
            return false;
        };
        let (size, files) = {
            let n = &mut self.nodes[id as usize];
            (std::mem::take(&mut n.size), std::mem::take(&mut n.files))
        };
        let mut at = self.nodes[id as usize].parent;
        while at != u32::MAX {
            let n = &mut self.nodes[at as usize];
            n.size = n.size.saturating_sub(size);
            n.files = n.files.saturating_sub(files);
            let (s, c) = n.kids;
            let nodes = &self.nodes;
            let mut range = self.kids[s as usize..(s + c) as usize].to_vec();
            range.sort_unstable_by(|a, b| {
                let (a, b) = (&nodes[*a as usize], &nodes[*b as usize]);
                b.size.cmp(&a.size).then_with(|| a.name.cmp(&b.name))
            });
            self.kids[s as usize..(s + c) as usize].copy_from_slice(&range);
            at = self.nodes[at as usize].parent;
        }
        true
    }

    /// Snapshot of any folder inside this tree; no I/O besides the capacity query.
    pub fn snapshot(&self, path: &Path, generation: u64) -> Option<DiskSnapshot> {
        let id = self.find(path)?;
        let node = &self.nodes[id as usize];
        let kids = self.kids_of(id);
        let entries: Vec<_> = kids
            .iter()
            .filter(|k| self.nodes[**k as usize].size > 0)
            .take(MAX_ROWS)
            .map(|k| self.entry(*k, path, id))
            .collect();
        let shown: u64 = entries.iter().fold(0, |n, e| n.saturating_add(e.bytes));
        let mut heap: BinaryHeap<Reverse<(u64, u32)>> = BinaryHeap::with_capacity(MAX_FILES + 1);
        let mut stack = vec![id];
        while let Some(at) = stack.pop() {
            for k in self.kids_of(at) {
                let n = &self.nodes[*k as usize];
                if heap.len() >= MAX_FILES && heap.peek().is_some_and(|m| n.size <= m.0 .0) {
                    // Kids are sorted by size: the rest cannot qualify either.
                    break;
                }
                if n.dir {
                    stack.push(*k);
                } else {
                    heap.push(Reverse((n.size, *k)));
                    if heap.len() > MAX_FILES {
                        heap.pop();
                    }
                }
            }
        }
        let mut large: Vec<_> = heap.into_iter().map(|Reverse((_, k))| k).collect();
        large.sort_unstable_by(|a, b| {
            let (a, b) = (&self.nodes[*a as usize], &self.nodes[*b as usize]);
            b.size.cmp(&a.size)
        });
        let limited = kids.len() > MAX_ROWS || self.limited;
        let mut snapshot = DiskSnapshot {
            generation,
            root: path.to_owned(),
            backend: self.backend,
            bytes: node.size,
            entry_count: node.files,
            skipped: self.skipped,
            errors: self.errors,
            limited,
            other_bytes: node.size.saturating_sub(shown),
            large_files: large.into_iter().map(|k| self.entry(k, path, id)).collect(),
            entries,
            ..Default::default()
        };
        #[cfg(target_os = "linux")]
        {
            snapshot.capacity = super::verified::capacity(path);
        }
        #[cfg(windows)]
        {
            snapshot.capacity = super::verified::capacity(path);
            snapshot.fast_scan_available = self
                .root
                .ancestors()
                .last()
                .is_some_and(atlas_win_space::mft_space::supported);
        }
        snapshot.status = if snapshot.skipped > 0 || snapshot.errors > 0 || limited {
            ScanStatus::Partial
        } else {
            ScanStatus::Complete
        };
        Some(snapshot)
    }
}

static TREE: Mutex<Option<Arc<SpaceTree>>> = Mutex::new(None);

pub fn store(tree: Arc<SpaceTree>) {
    *TREE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(tree);
}
pub fn cached(path: &Path) -> Option<Arc<SpaceTree>> {
    TREE.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .as_ref()
        .filter(|t| t.find(path).is_some())
        .cloned()
}
/// Update the cached tree after `path` was deleted. False if nothing changed.
pub fn remove(path: &Path) -> bool {
    let mut guard = TREE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    guard
        .as_mut()
        .and_then(Arc::get_mut)
        .is_some_and(|tree| tree.remove_node(path))
}
pub fn invalidate() {
    if let Some(old) = TREE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .take()
    {
        std::thread::spawn(move || drop(old));
    }
}
/// Instant folder view from the last completed scan, if it covers `path`.
pub fn cached_snapshot(path: &Path, generation: u64) -> Option<DiskSnapshot> {
    cached(path)?.snapshot(path, generation)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn totals_navigation_and_large_files_come_from_one_tree() {
        let mut b = TreeBuilder::new(0);
        let a = b.add_dir(0, "A");
        let bb = b.add_dir(a, "B");
        b.add_file(a, "x", 3);
        b.add_file(bb, "y", 7);
        b.add_file(0, "z", 2);
        let root = PathBuf::from("C:\\");
        let t = b.finish(root.clone(), ScanBackend::NativeDirectory);
        let whole = t.snapshot(&root, 1).unwrap();
        assert_eq!((whole.bytes, whole.entry_count), (12, 3));
        assert_eq!(
            whole
                .large_files
                .iter()
                .map(|f| f.bytes)
                .collect::<Vec<_>>(),
            vec![7, 3, 2]
        );
        let child = t.snapshot(&root.join("a"), 2).unwrap();
        assert_eq!((child.bytes, child.entry_count), (10, 2));
        assert_eq!(
            child.entries.iter().map(|f| f.bytes).collect::<Vec<_>>(),
            vec![7, 3]
        );
        assert!(t.snapshot(&root.join("missing"), 3).is_none());
    }
    #[test]
    fn removing_an_entry_updates_every_ancestor() {
        let mut b = TreeBuilder::new(0);
        let a = b.add_dir(0, "A");
        b.add_file(a, "x", 3);
        b.add_file(a, "y", 7);
        let root = PathBuf::from("C:\\");
        let mut t = b.finish(root.clone(), ScanBackend::NativeDirectory);
        assert!(t.remove_node(&root.join("A").join("y")));
        let s = t.snapshot(&root, 1).unwrap();
        assert_eq!((s.bytes, s.entry_count), (3, 1));
        let child = t.snapshot(&root.join("A"), 1).unwrap();
        assert_eq!(child.entries.len(), 1);
        assert!(!t.remove_node(&root.join("missing")));
    }
    #[test]
    fn small_files_fold_into_the_folder_total() {
        let mut b = TreeBuilder::new(100);
        b.add_file(0, "tiny", 10);
        b.add_file(0, "big", 500);
        let t = b.finish(PathBuf::from("C:\\"), ScanBackend::NativeDirectory);
        let s = t.snapshot(Path::new("C:\\"), 1).unwrap();
        assert_eq!((s.bytes, s.entry_count, s.entries.len()), (510, 2, 1));
        assert_eq!(s.other_bytes, 10);
    }
}
