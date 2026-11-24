//! Removing generations and store paths that nothing refers to
//! (`docs/system.md`, "Cleaning up").

use std::collections::BTreeSet;
use std::fs::{self, File, OpenOptions, TryLockError};
use std::io::ErrorKind;
use std::os::fd::AsRawFd;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::{build, nix_closure};

const STORE: &str = "/vug/store";

fn io<T>(what: &str, path: &Path, result: std::io::Result<T>) -> Result<T, String> {
    result.map_err(|e| format!("{what} {}: {e}", path.display()))
}

fn lock_file(path: &Path, exclusive: bool) -> Result<File, String> {
    let file = io(
        "open",
        path,
        OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(path),
    )?;
    if !io("stat", path, file.metadata())?.is_file() {
        return Err(format!("{}: expected a lock file", path.display()));
    }
    io(
        "lock",
        path,
        if exclusive {
            file.lock()
        } else {
            file.lock_shared()
        },
    )?;
    Ok(file)
}

pub(crate) fn transaction(data: &Path, exclusive: bool) -> Result<File, String> {
    io("create", data, fs::create_dir_all(data))?;
    lock_file(&data.join(".gc-lock"), exclusive)
}

pub(crate) fn inherit(file: &File) -> Result<(), String> {
    use rustix::io::{FdFlags, fcntl_getfd, fcntl_setfd};
    let flags = fcntl_getfd(file).map_err(|error| error.to_string())?;
    fcntl_setfd(file, flags - FdFlags::CLOEXEC).map_err(|error| error.to_string())
}

fn lease_directory(data: &Path, create: bool) -> Result<Option<PathBuf>, String> {
    let dir = data.join(".gc-leases");
    if create {
        match fs::DirBuilder::new().mode(0o700).create(&dir) {
            Ok(()) => (),
            Err(error) if error.kind() == ErrorKind::AlreadyExists => (),
            Err(error) => return io("create", &dir, Err(error)),
        }
    }
    let meta = match fs::symlink_metadata(&dir) {
        Ok(meta) => meta,
        Err(error) if !create && error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => return io("stat", &dir, Err(error)),
    };
    if !meta.is_dir() || meta.uid() != unsafe { libc::geteuid() } || meta.mode() & 0o022 != 0 {
        return Err(format!(
            "{}: expected a private lease directory",
            dir.display()
        ));
    }
    Ok(Some(dir))
}

pub(crate) fn lease(
    command: &mut Command,
    data: &Path,
    live: &BTreeSet<String>,
    nix_namespace: bool,
) -> Result<File, String> {
    let dir = lease_directory(data, true)?.unwrap();
    let mut file = io(
        "create lease",
        &dir,
        tempfile::Builder::new().prefix("run-").tempfile_in(&dir),
    )?;
    io("lock", file.path(), file.as_file().lock())?;
    serde_json::to_writer(&mut file, live).map_err(|error| error.to_string())?;
    let (file, _) = file
        .keep()
        .map_err(|error| format!("{}: {error}", dir.display()))?;
    inherit(&file)?;
    if nix_namespace {
        command.args(["--unshare-pid", "--proc", "/proc"]);
    }
    command.args(["--sync-fd", &file.as_raw_fd().to_string()]);
    Ok(file)
}

fn lease_roots(data: &Path, dry_run: bool) -> Result<BTreeSet<String>, String> {
    let mut live = BTreeSet::new();
    let Some(dir) = lease_directory(data, false)? else {
        return Ok(live);
    };
    for entry in io("read", &dir, fs::read_dir(&dir))? {
        let entry = io("read", &dir, entry)?;
        if !entry.file_name().as_encoded_bytes().starts_with(b"run-") {
            continue;
        }
        let path = entry.path();
        let file = io(
            "open lease",
            &path,
            OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
                .open(&path),
        )?;
        if !io("stat", &path, file.metadata())?.is_file() {
            return Err(format!("{}: expected a lease file", path.display()));
        }
        match file.try_lock() {
            Ok(()) => {
                if !dry_run {
                    io("remove lease", &path, fs::remove_file(&path))?;
                }
            }
            Err(TryLockError::WouldBlock) => {
                let ids: BTreeSet<String> = serde_json::from_reader(file)
                    .map_err(|error| format!("{}: {error}", path.display()))?;
                live.extend(ids);
            }
            Err(TryLockError::Error(error)) => return io("lock", &path, Err(error)),
        }
    }
    Ok(live)
}

pub(crate) fn sweep_closures(
    data: &Path,
    mut live: BTreeSet<String>,
    dry_run: bool,
) -> Result<(usize, Vec<String>), String> {
    live.extend(lease_roots(data, dry_run)?);
    let store = data.join("store");
    let nix_paths = nix_closure::paths(&nix_closure::for_outputs(&store, &live)?);
    let (kept, mut removed) = sweep(&store, &live, dry_run)?;
    let (nix_kept, nix_removed) =
        vug::nix_cache::collect_garbage(&build::nix_store_dir(data), &nix_paths, dry_run)?;
    removed.extend(nix_removed);
    Ok((kept + nix_kept, removed))
}

fn id_of(path: &Path) -> Option<String> {
    let id = path.strip_prefix(STORE).ok()?.components().next()?;
    id.as_os_str().to_str().map(str::to_owned)
}

/// (number, link) of every `system-<N>` in `profiles`, ascending.
fn generations(profiles: &Path) -> Result<Vec<(u64, PathBuf)>, String> {
    let mut found = Vec::new();
    for entry in io("read", profiles, fs::read_dir(profiles))? {
        let entry = io("read", profiles, entry)?;
        let name = entry.file_name();
        if let Some(number) = name
            .to_str()
            .and_then(|name| name.strip_prefix("system-"))
            .and_then(|digits| digits.parse::<u64>().ok())
        {
            found.push((number, entry.path()));
        }
    }
    found.sort();
    Ok(found)
}

fn target_id(link: &Path) -> Option<String> {
    id_of(&fs::read_link(link).ok()?)
}

/// Removes every generation except the newest `keep`, the default one, and
/// the one that is running. Returns the numbers removed.
pub fn delete_generations(profiles: &Path, keep: usize) -> Result<Vec<u64>, String> {
    let _profiles = lock_file(&profiles.join(".lock"), true)?;
    let all = generations(profiles)?;
    let default = fs::read_link(profiles.join("system"))
        .ok()
        .and_then(|target| target.to_str().map(str::to_owned));
    let running = target_id(Path::new("/run/current-system"));
    let newest: BTreeSet<u64> = all.iter().rev().take(keep).map(|(n, _)| *n).collect();
    let mut removed = Vec::new();
    for (number, link) in &all {
        let name = format!("system-{number}");
        if newest.contains(number)
            || default.as_deref() == Some(name.as_str())
            || (running.is_some() && target_id(link) == running)
        {
            continue;
        }
        io("remove", link, fs::remove_file(link))?;
        removed.push(*number);
    }
    Ok(removed)
}

pub(crate) fn make_writable(path: &Path) -> std::io::Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.is_dir() {
        fs::set_permissions(path, fs::Permissions::from_mode(0o755))?;
        for entry in fs::read_dir(path)? {
            make_writable(&entry?.path())?;
        }
    }
    Ok(())
}

/// Removes every store path of `data_dir` that no generation in `profiles`
/// (and not the running system) lists in its closure. With `dry_run`
/// nothing is removed. Returns (kept, removed ids).
pub fn collect_garbage(
    data_dir: &Path,
    profiles: &Path,
    dry_run: bool,
) -> Result<(usize, Vec<String>), String> {
    let _transaction = transaction(data_dir, true)?;
    if data_dir.join("home-state").is_symlink() {
        return Err("collect a home store with home collect-garbage".to_owned());
    }
    let _profiles = lock_file(&profiles.join(".lock"), false)?;
    let store = data_dir.join("store");
    let mut roots = BTreeSet::new();
    for (_, link) in generations(profiles)? {
        roots.insert(
            target_id(&link)
                .ok_or_else(|| format!("{}: invalid generation target", link.display()))?,
        );
    }
    if data_dir
        .canonicalize()
        .is_ok_and(|path| path == Path::new("/vug"))
    {
        roots.extend(target_id(Path::new("/run/current-system")));
        let pins = Path::new("/run/vug-switch-pins");
        match fs::read_dir(pins) {
            Ok(entries) => {
                for entry in entries {
                    let pin = io("read", pins, entry)?.path();
                    roots.insert(
                        target_id(&pin)
                            .ok_or_else(|| format!("{}: invalid switch pin", pin.display()))?,
                    );
                }
            }
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(error) => return io("read", pins, Err(error)),
        }
    }
    if roots.is_empty() {
        return Err(format!(
            "{}: no generation; refusing to remove every store path",
            profiles.display()
        ));
    }
    let mut live = BTreeSet::new();
    for root in roots {
        let list = store.join(&root).join("manifest/closure");
        for line in io("read", &list, fs::read_to_string(&list))?.lines() {
            live.extend(id_of(Path::new(line)));
        }
        live.insert(root);
    }
    sweep_closures(data_dir, live, dry_run)
}

fn sweep(
    store: &Path,
    live: &BTreeSet<String>,
    dry_run: bool,
) -> Result<(usize, Vec<String>), String> {
    let mut removed = Vec::new();
    let mut kept = 0;
    let mut names: Vec<String> = io("read", store, fs::read_dir(store))?
        .filter_map(|entry| entry.ok()?.file_name().into_string().ok())
        .collect();
    names.sort();
    for id in names {
        if live.contains(&id) {
            kept += 1;
            continue;
        }
        if !dry_run {
            let path = store.join(&id);
            io("chmod", &path, make_writable(&path))?;
            io("remove", &path, fs::remove_dir_all(&path))?;
        }
        removed.push(id);
    }
    Ok((kept, removed))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn garbage_collection_protects_a_running_lease() {
        let home = tempfile::tempdir().unwrap();
        let data = home.path();
        let store = data.join("store");
        let nix_store = build::nix_store_dir(data);
        let path = format!("{}-running", "0".repeat(32));
        let id = "a".repeat(32);
        fs::create_dir_all(store.join(&id).join(".vug")).unwrap();
        fs::create_dir_all(nix_store.join(".vug-owned")).unwrap();
        fs::write(nix_store.join(".vug-owned").join(&path), []).unwrap();
        fs::write(nix_store.join(&path), "running program").unwrap();
        let absolute = format!("/nix/store/{path}");
        let manifest = serde_json::json!({"format": 1, "paths": [{
            "store-path": absolute,
            "nar-hash": format!("sha256:{}", "0".repeat(52)),
            "nar-size": 1,
            "references": [],
        }]});
        fs::write(
            store.join(&id).join(build::NIX_CLOSURE_MANIFEST),
            manifest.to_string(),
        )
        .unwrap();

        let transaction = transaction(data, false).unwrap();
        let lease = lease(
            &mut Command::new("bwrap"),
            data,
            &BTreeSet::from([id.clone()]),
            true,
        )
        .unwrap();
        drop(transaction);

        assert_eq!(
            sweep_closures(data, BTreeSet::new(), false).unwrap(),
            (2, Vec::new())
        );
        assert!(nix_store.join(&path).exists());
        drop(lease);
        assert_eq!(
            sweep_closures(data, BTreeSet::new(), false).unwrap(),
            (0, vec![id, absolute])
        );
        assert!(!nix_store.join(&path).exists());
    }
}
