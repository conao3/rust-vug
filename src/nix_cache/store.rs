use std::fs::{self, OpenOptions};
use std::io::{self, ErrorKind, Read, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::Path;

use rustix::fs::{CWD, RenameFlags, renameat_with};
use sha2::{Digest, Sha256};

use super::{Closure, NarInfo, STORE, fetch_entries, nar, nix_hash};

pub(super) fn realize<R: Read>(
    closure: &Closure,
    store: &Path,
    get: impl Fn(&str) -> Result<R, String>,
) -> Result<(), String> {
    let at = |error: io::Error| format!("{}: {error}", store.display());
    let store = std::path::absolute(store).map_err(at)?;
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
    match fs::create_dir(&resolved) {
        Ok(()) if store == Path::new(STORE) => {
            fs::set_permissions(&resolved, fs::Permissions::from_mode(0o755)).map_err(at)?;
        }
        Ok(()) => {}
        Err(error) if error.kind() == ErrorKind::AlreadyExists => {}
        Err(error) => return Err(at(error)),
    }
    let metadata = fs::symlink_metadata(&resolved).map_err(at)?;
    if !metadata.is_dir()
        || (store == Path::new(STORE) && (metadata.uid() != 0 || metadata.mode() & 0o7777 != 0o755))
    {
        return Err(format!(
            "{}: expected a vug-owned Nix store directory",
            store.display()
        ));
    }

    let mut missing = Vec::new();
    for entry in closure.entries() {
        let path = resolved.join(entry.store_path().basename());
        match fs::symlink_metadata(&path) {
            Ok(_) => verify(&path, entry)?,
            Err(error) if error.kind() == ErrorKind::NotFound => missing.push(entry),
            Err(error) => return Err(format!("{}: {error}", path.display())),
        }
    }
    if missing.is_empty() {
        return Ok(());
    }
    let staging = fetch_entries(missing.iter().copied(), closure.limits, &resolved, get)?;
    let stage_name = staging.path().file_name().unwrap().to_string_lossy();
    for (index, entry) in missing.into_iter().enumerate() {
        let name = entry.store_path().basename();
        let source = staging.path().join(name);
        let temporary = resolved.join(format!(".{stage_name}-{index}"));
        let dest = resolved.join(name);
        renameat_with(CWD, &source, CWD, &temporary, RenameFlags::NOREPLACE)
            .map_err(|error| format!("{}: stage NAR: {error}", dest.display()))?;
        if let Err(error) = nar::freeze(&temporary).and_then(|()| {
            renameat_with(CWD, &temporary, CWD, &dest, RenameFlags::NOREPLACE)
                .map_err(io::Error::from)
        }) {
            if fs::symlink_metadata(&temporary).map_err(at)?.is_dir() {
                nar::make_dirs_writable(&temporary).map_err(at)?;
                fs::remove_dir_all(&temporary).map_err(at)?;
            } else {
                fs::remove_file(&temporary).map_err(at)?;
            }
            if error.kind() != ErrorKind::AlreadyExists {
                return Err(format!("{}: publish NAR: {error}", dest.display()));
            }
            verify(&dest, entry)?;
        }
    }
    Ok(())
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
