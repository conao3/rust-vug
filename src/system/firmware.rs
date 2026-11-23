use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Component, Path, PathBuf};

use super::{STORE, io, package_selector, strings, symlink};

pub(super) type Files = BTreeMap<PathBuf, PathBuf>;

pub(super) fn packages(table: &toml::Table) -> Result<Vec<String>, String> {
    if !table.contains_key("firmware-packages") {
        return Ok(Vec::new());
    }
    strings(table, "firmware-packages", "")?
        .iter()
        .map(|selector| package_selector(selector).map(str::to_owned))
        .collect()
}

pub(super) fn selection(table: &toml::Table) -> Result<Vec<PathBuf>, String> {
    if !table.contains_key("firmware-files") {
        return Ok(Vec::new());
    }
    strings(table, "firmware-files", "boot.initrd.")?
        .into_iter()
        .map(|name| {
            let path = PathBuf::from(&name);
            if !name.is_empty()
                && path
                    .components()
                    .all(|part| matches!(part, Component::Normal(_)))
            {
                Ok(path.components().collect())
            } else {
                Err(format!("boot.initrd.firmware-files: invalid path {name:?}"))
            }
        })
        .collect()
}

pub(super) fn collect(root: &Path, relative: &Path, files: &mut Files) -> Result<(), String> {
    let directory = root.join(relative);
    for entry in io(
        "read firmware directory",
        &directory,
        fs::read_dir(&directory),
    )? {
        let entry = io("read firmware entry", &directory, entry)?;
        let path = entry.path();
        let name = relative.join(entry.file_name());
        let kind = io("stat firmware", &path, entry.file_type())?;
        if kind.is_dir() {
            collect(root, &name, files)?;
        } else if let Some(previous) = files.get(&name) {
            let equal = if kind.is_symlink() && previous.is_symlink() {
                io("readlink", &path, fs::read_link(&path))?
                    == io("readlink", previous, fs::read_link(previous))?
            } else if kind.is_file() && !previous.is_symlink() {
                io("read", &path, fs::read(&path))? == io("read", previous, fs::read(previous))?
            } else {
                false
            };
            if !equal {
                return Err(format!("firmware: conflicting files at {}", name.display()));
            }
        } else if kind.is_file() || kind.is_symlink() {
            files.insert(name, path);
        } else {
            return Err(format!("firmware: unsupported file {}", path.display()));
        }
    }
    Ok(())
}

fn link_target(name: &Path, source: &Path) -> Result<PathBuf, String> {
    let target = io("readlink", source, fs::read_link(source))?;
    let mut resolved = name.parent().unwrap().to_path_buf();
    for part in target.components() {
        match part {
            Component::Normal(part) => resolved.push(part),
            Component::CurDir => (),
            Component::ParentDir if resolved.pop() => (),
            _ => {
                return Err(format!(
                    "firmware: link escapes firmware directory: {}",
                    name.display()
                ));
            }
        }
    }
    Ok(resolved)
}

pub(super) fn install(files: &Files, store: &Path, root: &Path) -> Result<(), String> {
    for name in files.keys() {
        if name
            .ancestors()
            .skip(1)
            .any(|parent| files.contains_key(parent))
        {
            return Err(format!(
                "firmware: conflicting directory at {}",
                name.display()
            ));
        }
    }
    for (name, source) in files {
        let destination = root.join(name);
        if source.is_symlink() {
            let target = link_target(name, source)?;
            if !files.contains_key(&target) {
                return Err(format!(
                    "firmware: missing link target {}",
                    target.display()
                ));
            }
            symlink(io("readlink", source, fs::read_link(source))?, &destination)?;
        } else {
            let relative = source
                .strip_prefix(store)
                .map_err(|error| error.to_string())?;
            symlink(Path::new(STORE).join(relative), &destination)?;
        }
    }
    Ok(())
}

pub(super) fn copy_selected(files: &Files, names: &[PathBuf], root: &Path) -> Result<(), String> {
    let mut selected = BTreeSet::new();
    for name in names {
        let mut chain = BTreeSet::new();
        let mut name = name.clone();
        loop {
            if !chain.insert(name.clone()) {
                return Err(format!("firmware: cyclic link at {}", name.display()));
            }
            let source = files
                .get(&name)
                .ok_or_else(|| format!("boot.initrd.firmware-files: missing {}", name.display()))?;
            selected.insert(name.clone());
            if !source.is_symlink() {
                break;
            }
            name = link_target(&name, source)?;
        }
    }
    for name in selected {
        let source = &files[&name];
        let destination = root.join(name);
        if source.is_symlink() {
            symlink(io("readlink", source, fs::read_link(source))?, &destination)?;
        } else {
            let parent = destination.parent().unwrap();
            io("create", parent, fs::create_dir_all(parent))?;
            io("copy", source, fs::copy(source, &destination))?;
        }
    }
    Ok(())
}
