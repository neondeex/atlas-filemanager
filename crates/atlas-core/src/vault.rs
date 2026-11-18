//! Private folders. Only ciphertext and opaque object identifiers reach storage.
//!
//! Format v1: Argon2id (64 MiB, 3 passes) wraps a random 256-bit key with
//! XChaCha20-Poly1305. The authenticated public envelope binds profile grants.
//! Names/tree/metadata are in an encrypted manifest; file objects use bounded,
//! independently authenticated 1 MiB chunks bound to vault + object + position.
//! No automatic extraction, plaintext staging, persisted key, or password hash.

use anyhow::{ensure, Context, Result};
use argon2::{Algorithm, Argon2, Params, Version};
use chacha20poly1305::{
    aead::{Aead, KeyInit, Payload},
    XChaCha20Poly1305, XNonce,
};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Component, Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};
use zeroize::{Zeroize, ZeroizeOnDrop, Zeroizing};

pub const PREFIX: &str = ".atlas-vault-";
pub mod live;
#[cfg(windows)]
pub mod mount;
#[cfg(target_os = "linux")]
pub mod mount_linux;
#[cfg(target_os = "linux")]
pub use mount_linux as mount;
mod mutable;
mod names;
const HEADER: &str = "vault.json";
const CHUNK: usize = 1024 * 1024;
const MAX_HEADER: u64 = 32 * 1024 * 1024;
const MAX_ENTRIES: usize = 100_000;
const MAGIC: &[u8; 8] = b"ATVOBJ01";

#[derive(Clone, Default)]
pub struct Control {
    pub cancelled: Arc<AtomicBool>,
    pub completed: Arc<std::sync::atomic::AtomicU64>,
    pub total: Arc<std::sync::atomic::AtomicU64>,
}
impl Control {
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }
    fn check(&self) -> Result<()> {
        ensure!(!self.cancelled.load(Ordering::Acquire), "vault_cancelled");
        Ok(())
    }
}

#[derive(Serialize, Deserialize)]
struct Envelope {
    version: u32,
    id: String,
    salt: [u8; 16],
    profiles: Vec<String>,
    wrapped_key: Vec<u8>,
    manifest: Vec<u8>,
    // Optional, Windows-account-encrypted label. Never a file or password key.
    // Excluded from password AAD so old v1/v2 containers remain compatible.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    sealed_name: Vec<u8>,
}
impl Envelope {
    fn aad(&self) -> Result<Vec<u8>> {
        Ok(serde_json::to_vec(&(
            "atlas-vault",
            self.version,
            &self.id,
            self.salt,
            &self.profiles,
        ))?)
    }
}

#[derive(Clone, Serialize, Deserialize, Zeroize, ZeroizeOnDrop)]
pub struct Entry {
    pub id: String,
    pub parent: Option<String>,
    pub name: String,
    pub directory: bool,
    pub size: u64,
    modified: u64,
    digest: [u8; 32],
    #[serde(default, skip_serializing_if = "Option::is_none")]
    blocks: Option<Vec<Option<String>>>,
    #[serde(default)]
    attributes: u32,
}

impl Entry {
    pub const fn modified(&self) -> u64 {
        self.modified
    }
    pub const fn attributes(&self) -> u32 {
        self.attributes
    }
}

#[derive(Serialize, Deserialize, Zeroize, ZeroizeOnDrop)]
struct Manifest {
    name: String,
    entries: Vec<Entry>,
}

/// Intentionally neither Debug nor Serialize. Dropping the session erases its
/// master key and decrypted manifest; the OS releases its exclusive writer lock.
pub struct Session {
    root: PathBuf,
    envelope: Envelope,
    key: Zeroizing<[u8; 32]>,
    manifest: Manifest,
    retired: Vec<Entry>,
    _lock: File,
}
impl Session {
    pub fn root(&self) -> &Path {
        &self.root
    }
    pub fn name(&self) -> &str {
        &self.manifest.name
    }
    pub fn profiles(&self) -> &[String] {
        &self.envelope.profiles
    }
    pub fn children(&self, parent: Option<&str>) -> Vec<Entry> {
        let mut entries: Vec<_> = self
            .manifest
            .entries
            .iter()
            .filter(|e| e.parent.as_deref() == parent)
            .cloned()
            .collect();
        entries.sort_by(|a, b| {
            b.directory
                .cmp(&a.directory)
                .then_with(|| crate::fs::list::natural_cmp(&a.name, &b.name))
        });
        entries
    }
    pub fn entry(&self, id: &str) -> Option<&Entry> {
        self.manifest
            .entries
            .iter()
            .chain(&self.retired)
            .find(|e| e.id == id)
    }

    /// Small in-app previews only. Returned bytes are erased on drop, never cached to disk.
    pub fn read(&self, id: &str, limit: u64) -> Result<Zeroizing<Vec<u8>>> {
        let entry = self.entry(id).context("vault_missing_entry")?;
        ensure!(
            !entry.directory && entry.size <= limit,
            "vault_preview_too_large"
        );
        let mut out = Zeroizing::new(Vec::with_capacity(entry.size as usize));
        self.decrypt_object(entry, &mut *out, &Control::default())?;
        Ok(out)
    }

    pub fn change_access(
        &mut self,
        current: &str,
        new_password: Option<&str>,
        profiles: Vec<String>,
        active: &str,
    ) -> Result<()> {
        let derived = derive(current, &self.envelope.salt)?;
        let verified = open(&derived, &self.envelope.wrapped_key, &self.envelope.aad()?)
            .map_err(|_| anyhow::anyhow!("vault_wrong_password"))?;
        ensure!(
            verified.as_slice() == self.key.as_slice(),
            "vault_wrong_password"
        );
        validate_grants(&profiles, active)?;
        let password = new_password.unwrap_or(current);
        validate_password(password)?;
        let mut replacement = Envelope {
            version: 2,
            id: self.envelope.id.clone(),
            salt: random()?,
            profiles,
            wrapped_key: Vec::new(),
            manifest: Vec::new(),
            sealed_name: Vec::new(),
        };
        replacement.sealed_name = names::seal(&replacement, &self.manifest.name);
        let derived = derive(password, &replacement.salt)?;
        replacement.wrapped_key = seal(&derived, self.key.as_slice(), &replacement.aad()?)?;
        replacement.manifest = seal(
            &self.key,
            &serde_json_secret(&self.manifest)?,
            replacement.id.as_bytes(),
        )?;
        write_envelope(&self.root, &replacement)?;
        self.envelope = replacement;
        Ok(())
    }

    /// Imports are copy-in: originals are retained. The manifest is committed
    /// atomically after every object exists; failure removes only new objects.
    pub fn import(
        &mut self,
        paths: &[PathBuf],
        parent: Option<&str>,
        control: &Control,
    ) -> Result<()> {
        if let Some(id) = parent {
            ensure!(
                self.entry(id).is_some_and(|e| e.directory),
                "vault_missing_entry"
            );
        }
        let mut added = Vec::new();
        let result = (|| {
            for path in paths {
                ensure!(!is_private_path(path), "vault_nested");
                let name = file_name(path)?;
                ensure!(!self.manifest.entries.iter().chain(added.iter()).any(|e: &Entry| e.parent.as_deref() == parent && same_name(&e.name, &name)), "vault_name_exists");
                self.import_one(path, parent, &mut added, control, 0)?;
            }
            ensure!(
                self.manifest.entries.len() + added.len() <= MAX_ENTRIES,
                "vault_too_many_entries"
            );
            control.check()?;
            // Build a new manifest so errors cannot leave the live session ahead of disk.
            let mut next = Manifest {
                name: self.manifest.name.clone(),
                entries: self.manifest.entries.clone(),
            };
            next.entries.extend(added.iter().cloned());
            validate_manifest(&next)?;
            self.save_manifest(&next)?;
            self.manifest = next;
            Ok(())
        })();
        if result.is_err() {
            for entry in &added {
                if !entry.directory {
                    let _ = fs::remove_file(self.root.join(&entry.id));
                }
            }
        }
        result
    }

    fn import_one(
        &self,
        path: &Path,
        parent: Option<&str>,
        added: &mut Vec<Entry>,
        control: &Control,
        depth: usize,
    ) -> Result<()> {
        control.check()?;
        ensure!(depth <= 128, "vault_too_many_entries");
        ensure!(
            added.len() + self.manifest.entries.len() < MAX_ENTRIES,
            "vault_too_many_entries"
        );
        // Reject junctions, symlinks, devices and hardlinks. Following them could
        // encrypt data outside the selected folder or leave another plaintext link.
        let metadata = plain_metadata(path)?;
        let directory = metadata.is_dir();
        let mut entry = Entry {
            id: uuid::Uuid::new_v4().simple().to_string(),
            parent: parent.map(str::to_owned),
            name: file_name(path)?,
            directory,
            size: if directory { 0 } else { metadata.len() },
            modified: metadata
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map_or(0, |d| d.as_secs()),
            digest: [0; 32],
            blocks: None,
            attributes: 0,
        };
        if directory {
            let id = entry.id.clone();
            added.push(entry);
            for child in fs::read_dir(path)? {
                self.import_one(&child?.path(), Some(&id), added, control, depth + 1)?;
            }
        } else {
            let object = self.root.join(&entry.id);
            match self.encrypt_object(path, &object, &entry, control) {
                Ok(digest) => {
                    entry.digest = digest;
                    added.push(entry);
                }
                Err(e) => {
                    let _ = fs::remove_file(object);
                    return Err(e);
                }
            }
        }
        Ok(())
    }

    fn save_manifest(&self, manifest: &Manifest) -> Result<()> {
        let mut replacement = Envelope {
            version: self.envelope.version,
            id: self.envelope.id.clone(),
            salt: self.envelope.salt,
            profiles: self.envelope.profiles.clone(),
            wrapped_key: self.envelope.wrapped_key.clone(),
            sealed_name: self.envelope.sealed_name.clone(),
            manifest: seal(
                &self.key,
                &serde_json_secret(manifest)?,
                self.envelope.id.as_bytes(),
            )?,
        };
        let result = write_envelope(&self.root, &replacement);
        replacement.manifest.zeroize();
        result
    }

    /// Explicit export to a NEW path. No overwrite or merge with existing trees.
    pub fn export(&self, id: Option<&str>, destination: &Path, control: &Control) -> Result<()> {
        ensure!(
            !destination.exists() && !is_private_path(destination),
            "vault_export_exists"
        );
        let parent = destination.parent().context("vault_invalid_path")?;
        ensure!(
            parent.is_dir() && fs::canonicalize(parent)?.is_dir(),
            "vault_invalid_path"
        );
        if let Some(entry) = id.and_then(|id| self.entry(id)) {
            if !entry.directory {
                let mut tmp = tempfile::NamedTempFile::new_in(parent)?;
                self.decrypt_object(entry, tmp.as_file_mut(), control)?;
                tmp.as_file().sync_all()?;
                tmp.persist_noclobber(destination).map_err(|e| e.error)?;
                return Ok(());
            }
        } else {
            ensure!(id.is_none(), "vault_missing_entry");
        }
        let temp = tempfile::Builder::new()
            .prefix(".atlas-export-")
            .tempdir_in(parent)?;
        self.export_children(id, temp.path(), control)?;
        control.check()?;
        // The destination must not appear during export; never merge/overwrite.
        ensure!(!destination.exists(), "vault_export_exists");
        fs::rename(temp.path(), destination)?;
        Ok(())
    }

    fn export_children(
        &self,
        parent: Option<&str>,
        destination: &Path,
        control: &Control,
    ) -> Result<()> {
        for entry in self.children(parent) {
            control.check()?;
            validate_name(&entry.name)?;
            let path = destination.join(&entry.name);
            if entry.directory {
                fs::create_dir(&path)?;
                self.export_children(Some(&entry.id), &path, control)?;
            } else {
                let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
                self.decrypt_object(&entry, &mut file, control)?;
                file.sync_all()?;
            }
        }
        Ok(())
    }

    /// Restore plaintext only on explicit "Remove protection". Ciphertext stays
    /// recoverable if export fails. A successful export is authoritative even if
    /// encrypted-container cleanup is denied by the OS.
    pub fn restore(self, control: &Control) -> Result<(PathBuf, bool)> {
        let destination = self
            .root
            .parent()
            .context("vault_invalid_path")?
            .join(&self.manifest.name);
        self.export(None, &destination, control)?;
        let root = self.root.clone();
        drop(self);
        let removed = fs::remove_dir_all(root).is_ok();
        Ok((destination, removed))
    }

    fn object_aad(&self, entry: &Entry, index: u64) -> Vec<u8> {
        let mut aad = Vec::with_capacity(96);
        aad.extend_from_slice(self.envelope.id.as_bytes());
        aad.extend_from_slice(entry.id.as_bytes());
        aad.extend_from_slice(&entry.size.to_le_bytes());
        aad.extend_from_slice(&index.to_le_bytes());
        aad
    }

    fn encrypt_object(
        &self,
        source: &Path,
        destination: &Path,
        entry: &Entry,
        control: &Control,
    ) -> Result<[u8; 32]> {
        let mut source = open_plain(source)?;
        ensure!(
            source.metadata()?.len() == entry.size,
            "vault_source_changed"
        );
        let mut target = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(destination)?;
        let prefix: [u8; 16] = random()?;
        target.write_all(MAGIC)?;
        target.write_all(&prefix)?;
        target.write_all(&entry.size.to_le_bytes())?;
        let cipher = XChaCha20Poly1305::new_from_slice(self.key.as_slice())
            .map_err(|_| anyhow::anyhow!("vault_invalid_key"))?;
        let mut remaining = entry.size;
        let mut index = 0u64;
        let mut hash = Sha256::new();
        let mut bytes = Zeroizing::new(vec![0u8; CHUNK]);
        loop {
            control.check()?;
            let size = remaining.min(CHUNK as u64) as usize;
            source.read_exact(&mut bytes[..size])?;
            hash.update(&bytes[..size]);
            let mut nonce = [0; 24];
            nonce[..16].copy_from_slice(&prefix);
            nonce[16..].copy_from_slice(&index.to_le_bytes());
            let encrypted = cipher
                .encrypt(
                    XNonce::from_slice(&nonce),
                    Payload {
                        msg: &bytes[..size],
                        aad: &self.object_aad(entry, index),
                    },
                )
                .map_err(|_| anyhow::anyhow!("vault_encrypt_failed"))?;
            target.write_all(&encrypted)?;
            control.completed.fetch_add(size as u64, Ordering::Relaxed);
            remaining -= size as u64;
            if remaining == 0 {
                break;
            }
            index += 1;
        }
        ensure!(source.read(&mut bytes[..1])? == 0, "vault_source_changed");
        target.sync_all()?;
        Ok(hash.finalize().into())
    }

    fn decrypt_object(
        &self,
        entry: &Entry,
        target: &mut impl Write,
        control: &Control,
    ) -> Result<()> {
        if entry.blocks.is_some() {
            let mut offset = 0;
            let mut bytes = Zeroizing::new(vec![0; CHUNK]);
            while offset < entry.size {
                control.check()?;
                let n = self.read_range(&entry.id, offset, &mut bytes)?;
                ensure!(n > 0, "vault_damaged");
                target.write_all(&bytes[..n])?;
                offset += n as u64;
            }
            return Ok(());
        }
        ensure!(valid_id(&entry.id), "vault_invalid_manifest");
        let path = self.root.join(&entry.id);
        plain_metadata(&path)?;
        let mut source = File::open(path)?;
        let mut header = [0; 32];
        source.read_exact(&mut header)?;
        ensure!(
            &header[..8] == MAGIC && header[24..] == entry.size.to_le_bytes(),
            "vault_damaged"
        );
        let cipher = XChaCha20Poly1305::new_from_slice(self.key.as_slice())
            .map_err(|_| anyhow::anyhow!("vault_invalid_key"))?;
        let mut remaining = entry.size;
        let mut index = 0u64;
        let mut hash = Sha256::new();
        let mut buffer = vec![0; CHUNK + 16];
        loop {
            control.check()?;
            let size = remaining.min(CHUNK as u64) as usize;
            source.read_exact(&mut buffer[..size + 16])?;
            let mut nonce = [0; 24];
            nonce[..16].copy_from_slice(&header[8..24]);
            nonce[16..].copy_from_slice(&index.to_le_bytes());
            let bytes = Zeroizing::new(
                cipher
                    .decrypt(
                        XNonce::from_slice(&nonce),
                        Payload {
                            msg: &buffer[..size + 16],
                            aad: &self.object_aad(entry, index),
                        },
                    )
                    .map_err(|_| anyhow::anyhow!("vault_damaged"))?,
            );
            hash.update(&*bytes);
            target.write_all(&bytes)?;
            remaining -= size as u64;
            if remaining == 0 {
                break;
            }
            index += 1;
        }
        ensure!(source.read(&mut buffer[..1])? == 0, "vault_damaged");
        ensure!(
            <[u8; 32]>::from(hash.finalize()) == entry.digest,
            "vault_damaged"
        );
        Ok(())
    }

    pub fn verify(&self, control: &Control) -> Result<()> {
        for entry in &self.manifest.entries {
            if !entry.directory {
                self.decrypt_object(entry, &mut std::io::sink(), control)?;
            }
        }
        Ok(())
    }
}

pub fn protect(
    source: &Path,
    password: &str,
    profiles: Vec<String>,
    active: &str,
    control: &Control,
) -> Result<Session> {
    validate_password(password)?;
    validate_grants(&profiles, active)?;
    ensure!(
        !is_private_path(source) && !live::is_private(source),
        "vault_nested"
    );
    ensure!(plain_metadata(source)?.is_dir(), "vault_not_directory");
    let name = file_name(source)?;
    let parent = source.parent().context("vault_invalid_path")?;
    let id = uuid::Uuid::new_v4().simple().to_string();
    let stage = tempfile::Builder::new()
        .prefix(&format!("{PREFIX}{id}-"))
        .tempdir_in(parent)?;
    let key = Zeroizing::new(random()?);
    let mut envelope = Envelope {
        version: 2,
        id,
        salt: random()?,
        profiles,
        wrapped_key: Vec::new(),
        manifest: Vec::new(),
        sealed_name: Vec::new(),
    };
    envelope.sealed_name = names::seal(&envelope, &name);
    let derived = derive(password, &envelope.salt)?;
    envelope.wrapped_key = seal(&derived, key.as_slice(), &envelope.aad()?)?;
    let mut session = Session {
        root: stage.path().to_path_buf(),
        envelope,
        key,
        manifest: Manifest {
            name,
            entries: Vec::new(),
        },
        retired: Vec::new(),
        _lock: lock(stage.path())?,
    };
    // Enumerate before writing so progress is meaningful and unsafe links fail early.
    let total = source_bytes(source)?;
    control.total.store(total, Ordering::Relaxed);
    let paths: Vec<_> = fs::read_dir(source)?
        .map(|r| r.map(|e| e.path()))
        .collect::<std::io::Result<_>>()?;
    session
        .import(&paths, None, control)
        .context("vault_import_stage")?;
    session.verify(control).context("vault_verify_stage")?;
    verify_source(&session, source, None, control)?;
    control.check()?;
    // Commit ciphertext first. From this point it is a recoverable vault, including
    // after a crash. Never use the OS trash for the plaintext originals.
    // The atomic manifest replacement is the commit. Keep the opaque directory
    // in place: Windows cannot rename a directory with a locked child handle.
    // Maintaining the exclusive OS lock throughout is more important than a
    // cosmetic directory rename. A crash leaves only ciphertext here.
    let _committed_root = stage.keep();
    // A changed original is never deleted: leave it intact and report failure.
    // The fully verified encrypted copy remains in its opaque destination.
    verify_source(&session, source, None, control)?;
    retire_originals(&session, source, None).context("vault_original_cleanup_failed")?;
    Ok(session)
}

pub fn unlock(root: &Path, password: &str, profile: &str) -> Result<Session> {
    ensure!(
        storage_root(root).as_deref() == Some(root),
        "vault_invalid_path"
    );
    let envelope = read_envelope(root)?;
    ensure!(
        envelope.profiles.iter().any(|id| id == profile),
        "vault_access_denied"
    );
    let guard = lock(root)?;
    // Read again under the lock: another session may have written the envelope since,
    // and a stale manifest would let `collect_orphans` delete its new blocks.
    let envelope = read_envelope(root)?;
    ensure!(
        envelope.profiles.iter().any(|id| id == profile),
        "vault_access_denied"
    );
    let derived = derive(password, &envelope.salt)?;
    let unwrapped = open(&derived, &envelope.wrapped_key, &envelope.aad()?)
        .map_err(|_| anyhow::anyhow!("vault_wrong_password"))?;
    ensure!(unwrapped.len() == 32, "vault_damaged");
    let mut key = Zeroizing::new([0; 32]);
    key.copy_from_slice(&unwrapped);
    let bytes = open(&key, &envelope.manifest, envelope.id.as_bytes())
        .map_err(|_| anyhow::anyhow!("vault_damaged"))?;
    let manifest: Manifest = serde_json::from_slice(&bytes).context("vault_damaged")?;
    validate_manifest(&manifest)?;
    let mut session = Session {
        root: root.to_path_buf(),
        envelope,
        key,
        manifest,
        retired: Vec::new(),
        _lock: guard,
    };
    // Upgrade the authenticated envelope before enabling v2 mutable blocks.
    // Legacy objects remain readable and are migrated only on their first edit.
    if session.envelope.version == 1 {
        session.change_access(password, None, session.profiles().to_vec(), profile)?;
    }
    // Learn names from existing containers only after authenticating the password
    // and decrypting the real manifest. A moved vault can refresh this account's
    // label without changing the password or any encrypted file objects.
    if names::open(&session.envelope).as_deref() != Some(session.name()) {
        let sealed = names::seal(&session.envelope, session.name());
        if !sealed.is_empty() {
            session.envelope.sealed_name = sealed;
            if let Err(error) = write_envelope(&session.root, &session.envelope) {
                // Optional presentation metadata must never prevent content access.
                tracing::warn!(%error, "Private folder display name could not be saved");
            }
        }
    }
    Ok(session)
}

/// Public grants are a discovery hint, not authentication. Editing them cannot
/// grant decryption: the same bytes are authenticated when unwrapping the key.
pub fn visible_to(root: &Path, profile: &str) -> bool {
    read_envelope(root).is_ok_and(|e| e.profiles.iter().any(|id| id == profile))
}

/// Display metadata only. This never unlocks a session or reveals child names.
pub fn display_name(root: &Path, profile: &str) -> Option<String> {
    let envelope = read_envelope(root).ok()?;
    if !envelope.profiles.iter().any(|id| id == profile) {
        return None;
    }
    names::open(&envelope)
}

pub fn is_private_path(path: &Path) -> bool {
    storage_root(path).is_some()
}
pub fn storage_root(path: &Path) -> Option<PathBuf> {
    let mut root = PathBuf::new();
    for component in path.components() {
        root.push(component.as_os_str());
        if component
            .as_os_str()
            .to_string_lossy()
            .to_ascii_lowercase()
            .starts_with(PREFIX)
        {
            return Some(root);
        }
    }
    None
}

/// Apply on every listing delivery (including previews/tree/Miller/watchers).
/// Never change a row's name without retaining its exact physical path.
pub fn filter_entries(
    entries: Vec<atlas_model::file_entry::FileEntry>,
    profile: &str,
    label: &str,
) -> Vec<atlas_model::file_entry::FileEntry> {
    use atlas_model::file_entry::{FileEntryData, FileKind};
    entries
        .into_iter()
        .filter_map(|entry| {
            let path = entry.path();
            let Some(root) = storage_root(&path) else {
                return Some(entry);
            };
            if root != path {
                return None;
            }
            let envelope = read_envelope(&root).ok()?;
            if !envelope.profiles.iter().any(|id| id == profile) {
                return None;
            }
            let name = names::open(&envelope).unwrap_or_else(|| label.to_owned());
            Some(
                FileEntryData {
                    name: name.into(),
                    path,
                    kind: FileKind::Dir,
                    size: None,
                    modified: None,
                    _is_hidden: false,
                    _is_symlink: false,
                    archive_root: None,
                    recycle_info: None,
                }
                .into(),
            )
        })
        .collect()
}

fn validate_password(password: &str) -> Result<()> {
    ensure!(
        (12..=1024).contains(&password.chars().count()),
        "vault_password_length"
    );
    Ok(())
}
fn validate_grants(profiles: &[String], active: &str) -> Result<()> {
    ensure!(
        !profiles.is_empty() && profiles.len() <= 256 && profiles.iter().any(|id| id == active),
        "vault_invalid_profiles"
    );
    ensure!(
        profiles.iter().all(|id| uuid::Uuid::parse_str(id).is_ok()),
        "vault_invalid_profiles"
    );
    ensure!(
        profiles.iter().collect::<HashSet<_>>().len() == profiles.len(),
        "vault_invalid_profiles"
    );
    Ok(())
}
fn file_name(path: &Path) -> Result<String> {
    let name = path
        .file_name()
        .and_then(|s| s.to_str())
        .context("vault_invalid_name")?;
    validate_name(name)?;
    Ok(name.to_owned())
}
fn validate_name(name: &str) -> Result<()> {
    let mut components = Path::new(name).components();
    ensure!(
        !name.is_empty()
            && name.len() <= 1024
            && !name.contains(['/', '\\', ':', '\0'])
            && !name.ends_with(['.', ' '])
            && matches!(components.next(), Some(Component::Normal(_)))
            && components.next().is_none(),
        "vault_invalid_name"
    );
    let stem = name
        .split('.')
        .next()
        .unwrap_or_default()
        .to_ascii_uppercase();
    ensure!(
        !matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
            && !((stem.starts_with("COM") || stem.starts_with("LPT"))
                && stem.len() == 4
                && matches!(stem.as_bytes()[3], b'1'..=b'9')),
        "vault_invalid_name"
    );
    Ok(())
}
fn same_name(a: &str, b: &str) -> bool {
    a.to_lowercase() == b.to_lowercase()
}
fn valid_id(id: &str) -> bool {
    id.len() == 32 && id.bytes().all(|c| c.is_ascii_hexdigit())
}
fn validate_manifest(manifest: &Manifest) -> Result<()> {
    validate_name(&manifest.name)?;
    ensure!(
        manifest.entries.len() <= MAX_ENTRIES,
        "vault_invalid_manifest"
    );
    let ids: HashSet<_> = manifest.entries.iter().map(|e| e.id.as_str()).collect();
    ensure!(
        ids.len() == manifest.entries.len(),
        "vault_invalid_manifest"
    );
    let mut names = HashSet::new();
    for entry in &manifest.entries {
        ensure!(valid_id(&entry.id), "vault_invalid_manifest");
        if let Some(blocks) = &entry.blocks {
            ensure!(
                !entry.directory
                    && blocks.len() as u64 == entry.size.div_ceil(CHUNK as u64)
                    && blocks.len() <= 1_048_576,
                "vault_invalid_manifest"
            );
            ensure!(
                blocks
                    .iter()
                    .flatten()
                    .all(|id| valid_id(id) && !ids.contains(id.as_str())),
                "vault_invalid_manifest"
            );
            let mut unique = HashSet::new();
            ensure!(
                blocks.iter().flatten().all(|id| unique.insert(id)),
                "vault_invalid_manifest"
            );
        }
        validate_name(&entry.name)?;
        ensure!(
            names.insert((entry.parent.as_deref(), entry.name.to_lowercase())),
            "vault_invalid_manifest"
        );
        let mut ancestor = entry.parent.as_deref();
        let mut seen = HashSet::new();
        while let Some(id) = ancestor {
            ensure!(
                id != entry.id && seen.insert(id) && seen.len() <= 256,
                "vault_invalid_manifest"
            );
            let parent = manifest
                .entries
                .iter()
                .find(|e| e.id == id && e.directory)
                .context("vault_invalid_manifest")?;
            ancestor = parent.parent.as_deref();
        }
    }
    Ok(())
}
fn random<const N: usize>() -> Result<[u8; N]> {
    let mut bytes = [0; N];
    getrandom::getrandom(&mut bytes).map_err(|_| anyhow::anyhow!("vault_random_failed"))?;
    Ok(bytes)
}
fn derive(password: &str, salt: &[u8; 16]) -> Result<Zeroizing<[u8; 32]>> {
    ensure!(password.len() <= 4096, "vault_password_length");
    let params =
        Params::new(65536, 3, 1, Some(32)).map_err(|_| anyhow::anyhow!("vault_kdf_failed"))?;
    let argon = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
    let mut key = Zeroizing::new([0; 32]);
    argon
        .hash_password_into(password.as_bytes(), salt, &mut *key)
        .map_err(|_| anyhow::anyhow!("vault_kdf_failed"))?;
    Ok(key)
}
fn seal(key: &[u8; 32], bytes: &[u8], aad: &[u8]) -> Result<Vec<u8>> {
    let nonce: [u8; 24] = random()?;
    let cipher =
        XChaCha20Poly1305::new_from_slice(key).map_err(|_| anyhow::anyhow!("vault_invalid_key"))?;
    let mut out = nonce.to_vec();
    out.extend(
        cipher
            .encrypt(XNonce::from_slice(&nonce), Payload { msg: bytes, aad })
            .map_err(|_| anyhow::anyhow!("vault_encrypt_failed"))?,
    );
    Ok(out)
}
fn open(key: &[u8; 32], bytes: &[u8], aad: &[u8]) -> Result<Zeroizing<Vec<u8>>> {
    ensure!(bytes.len() >= 40, "vault_damaged");
    let cipher =
        XChaCha20Poly1305::new_from_slice(key).map_err(|_| anyhow::anyhow!("vault_invalid_key"))?;
    Ok(Zeroizing::new(
        cipher
            .decrypt(
                XNonce::from_slice(&bytes[..24]),
                Payload {
                    msg: &bytes[24..],
                    aad,
                },
            )
            .map_err(|_| anyhow::anyhow!("vault_damaged"))?,
    ))
}
fn serde_json_secret(value: &impl Serialize) -> Result<Zeroizing<Vec<u8>>> {
    Ok(Zeroizing::new(serde_json::to_vec(value)?))
}
fn read_envelope(root: &Path) -> Result<Envelope> {
    plain_metadata(root)?;
    let path = root.join(HEADER);
    let meta = plain_metadata(&path)?;
    ensure!(meta.len() <= MAX_HEADER && meta.is_file(), "vault_damaged");
    let mut bytes = Vec::new();
    File::open(path)?
        .take(MAX_HEADER + 1)
        .read_to_end(&mut bytes)?;
    ensure!(bytes.len() as u64 <= MAX_HEADER, "vault_damaged");
    let envelope: Envelope = serde_json::from_slice(&bytes).context("vault_damaged")?;
    ensure!(
        matches!(envelope.version, 1 | 2)
            && valid_id(&envelope.id)
            && envelope.profiles.len() <= 256
            && envelope.wrapped_key.len() == 72,
        "vault_damaged"
    );
    Ok(envelope)
}
fn write_envelope(root: &Path, envelope: &Envelope) -> Result<()> {
    let bytes = serde_json::to_vec(envelope)?;
    ensure!(bytes.len() as u64 <= MAX_HEADER, "vault_too_many_entries");
    let mut file = tempfile::NamedTempFile::new_in(root)?;
    file.write_all(&bytes)?;
    file.as_file().sync_all()?;
    file.persist(root.join(HEADER)).map_err(|e| e.error)?;
    Ok(())
}
fn lock(root: &Path) -> Result<File> {
    let path = root.join("lock");
    if path.exists() {
        plain_metadata(&path)?;
    }
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(path)?;
    file.try_lock_exclusive().context("vault_in_use")?;
    Ok(file)
}
fn plain_metadata(path: &Path) -> Result<fs::Metadata> {
    let meta = fs::symlink_metadata(path)?;
    ensure!(
        !meta.file_type().is_symlink() && (meta.is_dir() || meta.is_file()),
        "vault_unsafe_link"
    );
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        ensure!(meta.file_attributes() & 0x400 == 0, "vault_unsafe_link");
        atlas_win_space::vault_safety::validate_source(path, meta.is_dir())?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        ensure!(meta.is_dir() || meta.nlink() == 1, "vault_unsafe_link");
    }
    Ok(meta)
}
fn open_plain(path: &Path) -> Result<File> {
    plain_metadata(path)?;
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.share_mode(1);
    }
    Ok(options.open(path)?)
}
fn source_bytes(root: &Path) -> Result<u64> {
    let mut total = 0u64;
    for entry in walkdir::WalkDir::new(root)
        .max_depth(256)
        .follow_links(false)
    {
        let entry = entry?;
        ensure!(!is_private_path(entry.path()), "vault_nested");
        let meta = plain_metadata(entry.path())?;
        if meta.is_file() {
            total = total.checked_add(meta.len()).context("vault_too_large")?;
        }
    }
    Ok(total)
}

fn retire_originals(session: &Session, root: &Path, parent: Option<&str>) -> Result<()> {
    // Retire only files whose CURRENT bytes match the committed ciphertext.
    // Never recursively delete an unverified tree: newly added or edited files
    // survive a failed migration, together with the valid encrypted copy.
    let entries = session.children(parent);
    for entry in entries {
        let path = root.join(&entry.name);
        plain_metadata(&path)?;
        if entry.directory {
            retire_originals(session, &path, Some(&entry.id))?;
        } else {
            let mut options = OpenOptions::new();
            options.read(true);
            #[cfg(windows)]
            {
                use std::os::windows::fs::OpenOptionsExt;
                options
                    .access_mode(0x8001_0000)
                    .share_mode(1)
                    .custom_flags(0x0020_0000);
            }
            let mut file = options.open(&path)?;
            ensure!(file.metadata()?.len() == entry.size, "vault_source_changed");
            let mut hash = Sha256::new();
            let mut bytes = Zeroizing::new(vec![0; CHUNK]);
            loop {
                let n = file.read(&mut bytes)?;
                if n == 0 {
                    break;
                }
                hash.update(&bytes[..n]);
            }
            ensure!(
                <[u8; 32]>::from(hash.finalize()) == entry.digest,
                "vault_source_changed"
            );
            #[cfg(windows)]
            atlas_win_space::vault_safety::delete_on_close(&file)?;
            #[cfg(not(windows))]
            fs::remove_file(&path)?;
        }
    }
    fs::remove_dir(root)?;
    Ok(())
}
fn verify_source(
    session: &Session,
    root: &Path,
    parent: Option<&str>,
    control: &Control,
) -> Result<()> {
    control.check()?;
    let entries = session.children(parent);
    ensure!(
        fs::read_dir(root)?.count() == entries.len(),
        "vault_source_changed"
    );
    for entry in entries {
        control.check()?;
        let path = root.join(&entry.name);
        let meta = plain_metadata(&path)?;
        ensure!(meta.is_dir() == entry.directory, "vault_source_changed");
        if entry.directory {
            verify_source(session, &path, Some(&entry.id), control)?;
        } else {
            ensure!(meta.len() == entry.size, "vault_source_changed");
            let mut file = open_plain(&path)?;
            let mut hash = Sha256::new();
            let mut bytes = Zeroizing::new(vec![0; CHUNK]);
            loop {
                control.check()?;
                let n = file.read(&mut bytes)?;
                if n == 0 {
                    break;
                }
                hash.update(&bytes[..n]);
            }
            ensure!(
                <[u8; 32]>::from(hash.finalize()) == entry.digest,
                "vault_source_changed"
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
