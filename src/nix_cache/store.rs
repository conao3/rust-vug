use std::collections::BTreeSet;
use std::fs::{self, File, OpenOptions};
use std::io::{self, ErrorKind, Read, Write};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use rustix::fs::{CWD, RenameFlags, renameat_with};
use sha2::{Digest, Sha256};

use super::{Closure, NarInfo, STORE, StorePath, fetch_entries, nar, nix_hash};

struct LockedStore {
    path: PathBuf,
    _lock: File,
}

fn lock_store(store: &Path, create: bool) -> Result<Option<LockedStore>, String> {
    let at = |error: io::Error| format!("{}: {error}", store.display());
    let store = std::path::absolute(store).map_err(at)?;
    if store == Path::new(STORE) && Path::new("/nix/var/nix/db").exists() {
        return Err(format!("{}: managed by Nix", store.display()));
    }
    let parent = store.parent().ok_or("Nix store needs a parent directory")?;
    let name = store
        .file_name()
        .ok_or("Nix store needs a directory name")?;
    let mut ancestor = parent;
    while let Err(error) = fs::symlink_metadata(ancestor) {
        if error.kind() != ErrorKind::NotFound {
            return Err(at(error));
        }
        ancestor = ancestor.parent().ok_or_else(|| at(error))?;
    }
    if ancestor.canonicalize().map_err(at)?.starts_with(STORE) {
        return Err(format!("{}: invalid Nix store location", store.display()));
    }
    if !create {
        match fs::symlink_metadata(&store) {
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
            result => {
                result.map_err(at)?;
            }
        }
    }
    fs::create_dir_all(parent).map_err(at)?;
    let parent = parent.canonicalize().map_err(at)?;
    let resolved = parent.join(name);
    if (resolved.starts_with(STORE) && store != Path::new(STORE))
        || (store == Path::new(STORE) && resolved != store)
    {
        return Err(format!("{}: invalid Nix store location", store.display()));
    }
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .mode(0o600)
        .open(parent.join(".vug-store.lock"))
        .map_err(at)?;
    if !lock.metadata().map_err(at)?.is_file() {
        return Err(format!(
            "{}: expected a regular store lock file",
            parent.display()
        ));
    }
    lock.lock().map_err(at)?;
    if create {
        match fs::create_dir(&resolved) {
            Ok(()) if store == Path::new(STORE) => {
                fs::set_permissions(&resolved, fs::Permissions::from_mode(0o755)).map_err(at)?;
            }
            Ok(()) => {}
            Err(error) if error.kind() == ErrorKind::AlreadyExists => {}
            Err(error) => return Err(at(error)),
        }
    }
    let metadata = fs::symlink_metadata(&resolved).map_err(at)?;
    if !metadata.is_dir()
        || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.mode() & 0o022 != 0
        || (store == Path::new(STORE) && (metadata.uid() != 0 || metadata.mode() & 0o7777 != 0o755))
    {
        return Err(format!(
            "{}: expected a vug-owned Nix store directory",
            store.display()
        ));
    }

    Ok(Some(LockedStore {
        path: resolved,
        _lock: lock,
    }))
}

fn remove(path: &Path) -> io::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() => {
            nar::make_dirs_writable(path)?;
            fs::remove_dir_all(path)
        }
        Ok(_) => fs::remove_file(path),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn directory(path: &Path) -> io::Result<()> {
    match fs::DirBuilder::new().mode(0o755).create(path) {
        Ok(()) => Ok(()),
        Err(error)
            if error.kind() == ErrorKind::AlreadyExists && fs::symlink_metadata(path)?.is_dir() =>
        {
            Ok(())
        }
        Err(error) => Err(error),
    }
}

pub(super) fn realize<R: Read>(
    closure: &Closure,
    store: &Path,
    get: impl Fn(&str) -> Result<R, String>,
) -> Result<(), String> {
    let store = lock_store(store, true)?.unwrap();
    let owned = store.path.join(".vug-owned");
    let staging = store.path.join(".vug-staging");
    let slot = store.path.join(".vug-path");
    let at = |error: io::Error| format!("{}: {error}", store.path.display());
    remove(&staging).map_err(at)?;
    remove(&slot).map_err(at)?;

    let mut missing = Vec::new();
    for entry in closure.entries() {
        let path = store.path.join(entry.store_path().basename());
        match fs::symlink_metadata(&path) {
            Ok(_) => verify(&path, entry)?,
            Err(error) if error.kind() == ErrorKind::NotFound => missing.push(entry),
            Err(error) => return Err(format!("{}: {error}", path.display())),
        }
    }
    if missing.is_empty() {
        return Ok(());
    }

    directory(&owned).map_err(at)?;
    directory(&staging).map_err(at)?;
    let result = (|| {
        let unpacked = fetch_entries(missing.iter().copied(), closure.limits, &staging, get)?;
        for entry in missing {
            let name = entry.store_path().basename();
            let source = unpacked.path().join(name);
            let dest = store.path.join(name);
            let marker = owned.join(name);
            fs::rename(&source, &slot).map_err(at)?;
            nar::freeze(&slot).map_err(at)?;
            match fs::remove_file(&marker) {
                Ok(()) => (),
                Err(error) if error.kind() == ErrorKind::NotFound => (),
                Err(error) => return Err(at(error)),
            }
            File::create_new(&marker).map_err(at)?;
            if let Err(error) = renameat_with(CWD, &slot, CWD, &dest, RenameFlags::NOREPLACE) {
                fs::remove_file(&marker).map_err(at)?;
                if error != rustix::io::Errno::EXIST {
                    return Err(format!("{}: publish NAR: {error}", dest.display()));
                }
                verify(&dest, entry)?;
                remove(&slot).map_err(at)?;
            }
        }
        Ok(())
    })();
    let cleaned_stage = remove(&staging).map_err(at);
    let cleaned_slot = remove(&slot).map_err(at);
    result.and(cleaned_stage).and(cleaned_slot)
}

pub fn collect(
    store: &Path,
    live: &BTreeSet<StorePath>,
    dry_run: bool,
) -> Result<(usize, Vec<String>), String> {
    let Some(store) = lock_store(store, false)? else {
        return Ok((0, Vec::new()));
    };
    let staging = store.path.join(".vug-staging");
    let slot = store.path.join(".vug-path");
    let owned = store.path.join(".vug-owned");
    let at = |error: io::Error| format!("{}: {error}", store.path.display());
    if !dry_run {
        remove(&staging).map_err(at)?;
        remove(&slot).map_err(at)?;
    }
    let entries = match fs::symlink_metadata(&owned) {
        Ok(metadata) if metadata.is_dir() => fs::read_dir(&owned).map_err(at)?,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok((0, Vec::new())),
        _ => {
            return Err(format!(
                "{}: expected an ownership directory",
                owned.display()
            ));
        }
    };
    let mut kept = 0;
    let mut removed = Vec::new();
    for marker in entries {
        let marker = marker.map_err(at)?;
        let name = marker.file_name();
        let path: StorePath = format!("{STORE}/{}", name.to_string_lossy()).parse()?;
        if !marker.file_type().map_err(at)?.is_file() {
            return Err(format!(
                "{}: expected an ownership file",
                marker.path().display()
            ));
        }
        let target = store.path.join(&name);
        match fs::symlink_metadata(&target) {
            Err(error) if error.kind() == ErrorKind::NotFound => {
                if !dry_run {
                    fs::remove_file(marker.path()).map_err(at)?;
                }
                continue;
            }
            result => {
                result.map_err(at)?;
            }
        }
        if live.contains(&path) {
            kept += 1;
        } else {
            if !dry_run {
                fs::rename(&target, &slot).map_err(at)?;
                fs::remove_file(marker.path()).map_err(at)?;
                remove(&slot).map_err(at)?;
            }
            removed.push(path.as_str().to_owned());
        }
    }
    removed.sort();
    Ok((kept, removed))
}

fn verify(path: &Path, entry: &NarInfo) -> Result<(), String> {
    let mut output = NarHash {
        hasher: Sha256::new(),
        remaining: entry.nar_size(),
    };
    nar::dump(path, &mut output)
        .map_err(|error| format!("{}: verify installed NAR: {error}", path.display()))?;
    if output.remaining != 0 || nix_hash(&output.hasher.finalize().into()) != entry.nar_hash() {
        return Err(format!(
            "{}: installed NAR does not match its signed NarHash and NarSize",
            path.display()
        ));
    }
    Ok(())
}

struct NarHash {
    hasher: Sha256,
    remaining: u64,
}

impl Write for NarHash {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.remaining = self
            .remaining
            .checked_sub(bytes.len() as u64)
            .ok_or_else(|| {
                io::Error::new(ErrorKind::InvalidData, "installed NAR exceeds NarSize")
            })?;
        self.hasher.update(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
