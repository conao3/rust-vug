//! Removing generations and store paths that nothing refers to
//! (`docs/system.md`, "Cleaning up").

use std::collections::BTreeSet;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

const STORE: &str = "/vug/store";

fn io<T>(what: &str, path: &Path, result: std::io::Result<T>) -> Result<T, String> {
    result.map_err(|e| format!("{what} {}: {e}", path.display()))
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
    let store = data_dir.join("store");
    let mut roots: Vec<String> = generations(profiles)?
        .iter()
        .filter_map(|(_, link)| target_id(link))
        .collect();
    roots.extend(target_id(Path::new("/run/current-system")));
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
    sweep(&store, &live, dry_run)
}

pub fn sweep(
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
