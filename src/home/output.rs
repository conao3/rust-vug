use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::{ErrorKind, Read};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt, symlink};
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::build::{Builder, Settings, valid_store_id};
use crate::{gc, nix_closure, system};
use vug::treehash;

use super::{HomeRecord, load};

const STORE: &str = "/vug/store";
const STORE_REFERENCE: &[u8] = b"/vug/store/";
const REFERENCE_SIZE: usize = STORE_REFERENCE.len() + 32;

pub(crate) struct BuiltHome {
    pub id: String,
    pub path: PathBuf,
    pub generation: u64,
}

#[derive(Serialize)]
#[serde(rename_all = "PascalCase")]
struct Manifest<'a> {
    name: &'a str,
    packages: &'a BTreeMap<String, String>,
    closure: &'a BTreeSet<String>,
    files: Vec<&'a str>,
    units: BTreeMap<&'a str, ManifestUnit<'a>>,
    secrets: Vec<&'a str>,
}

#[derive(Serialize)]
#[serde(rename_all = "PascalCase")]
struct ManifestUnit<'a> {
    enable: &'a BTreeSet<String>,
    secrets: &'a BTreeSet<String>,
}

pub(super) struct State {
    pub(super) root: PathBuf,
    pub(super) store: PathBuf,
    _state_lock: File,
    _data_lock: File,
}

pub(super) fn io<T>(path: &Path, result: std::io::Result<T>) -> Result<T, String> {
    result.map_err(|error| format!("{}: {error}", path.display()))
}

fn owned_directory(path: &Path) -> Result<PathBuf, String> {
    let absolute = io(path, std::path::absolute(path))?;
    let mut missing = Vec::new();
    let mut ancestor = absolute.as_path();
    let mut canonical = loop {
        match fs::canonicalize(ancestor) {
            Ok(resolved) => break resolved,
            Err(error) if error.kind() == ErrorKind::NotFound => {
                let (Some(name), Some(parent)) = (ancestor.file_name(), ancestor.parent()) else {
                    return io(ancestor, Err(error));
                };
                missing.push(name);
                ancestor = parent;
            }
            Err(error) => return io(ancestor, Err(error)),
        }
    };
    canonical.extend(missing.into_iter().rev());
    if canonical.starts_with("/vug") {
        return Err(format!("{}: reserved for the system store", path.display()));
    }
    io(&canonical, fs::create_dir_all(&canonical))?;
    check_owner(&canonical, &io(&canonical, fs::metadata(&canonical))?)?;
    Ok(canonical)
}

pub(super) fn check_owner(path: &Path, metadata: &fs::Metadata) -> Result<(), String> {
    if metadata.uid() != unsafe { libc::geteuid() } || metadata.mode() & 0o022 != 0 {
        return Err(format!(
            "{}: expected a path owned by the current user and writable only by its owner",
            path.display()
        ));
    }
    Ok(())
}

fn local_directory(path: &Path) -> Result<(), String> {
    match fs::create_dir(path) {
        Ok(()) => {}
        Err(error) if error.kind() == ErrorKind::AlreadyExists => {}
        Err(error) => return io(path, Err(error)),
    }
    let metadata = io(path, fs::symlink_metadata(path))?;
    if !metadata.is_dir() {
        return Err(format!("{}: expected a directory", path.display()));
    }
    check_owner(path, &metadata)
}

pub(super) fn lock(path: &Path) -> Result<File, String> {
    let file = io(
        path,
        OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(path),
    )?;
    let metadata = io(path, file.metadata())?;
    if !metadata.is_file() {
        return Err(format!("{}: expected a lock file", path.display()));
    }
    check_owner(path, &metadata)?;
    io(path, file.lock())?;
    Ok(file)
}

pub(super) fn read_link(path: &Path) -> Result<Option<PathBuf>, String> {
    match fs::read_link(path) {
        Ok(target) => Ok(Some(target)),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
        Err(error) => io(path, Err(error)),
    }
}

impl State {
    pub(super) fn open(state: &Path, data: &Path) -> Result<Self, String> {
        let root = owned_directory(state)?;
        let data = owned_directory(data)?;
        if root.starts_with(&data) || data.starts_with(&root) {
            return Err("home state and data directories must be separate".to_owned());
        }
        let state_lock = lock(&root.join("lock"))?;
        let data_lock = lock(&data.join("home-lock"))?;
        let store = data.join("store");
        let links = [
            (root.join("store"), &store),
            (data.join("home-state"), &root),
        ];
        for (link, expected) in &links {
            if let Some(target) = read_link(link)?
                && target != **expected
            {
                return Err(format!("{} points to {}", link.display(), target.display()));
            }
        }
        for path in [
            store.clone(),
            data.join("index"),
            data.join("tmp"),
            root.join("generations"),
        ] {
            local_directory(&path)?;
        }
        for (link, expected) in links {
            if read_link(&link)?.is_none() {
                io(&link, symlink(expected, &link))?;
            }
        }
        Ok(Self {
            root,
            store,
            _state_lock: state_lock,
            _data_lock: data_lock,
        })
    }

    pub(super) fn generations(&self) -> Result<BTreeMap<u64, String>, String> {
        let dir = self.root.join("generations");
        let mut generations = BTreeMap::new();
        for entry in io(&dir, fs::read_dir(&dir))? {
            let entry = io(&dir, entry)?;
            let path = entry.path();
            let name = entry.file_name();
            let number = name
                .to_str()
                .and_then(|name| name.parse::<u64>().ok())
                .filter(|number| *number > 0 && name == number.to_string().as_str())
                .ok_or_else(|| format!("{}: invalid generation number", path.display()))?;
            let target = io(&path, fs::read_link(&path))?;
            let id = target
                .strip_prefix(&self.store)
                .ok()
                .and_then(Path::to_str)
                .filter(|id| valid_store_id(id) && target == self.store.join(id))
                .ok_or_else(|| format!("{}: invalid generation target", path.display()))?;
            require_directory(&target)?;
            generations.insert(number, id.to_owned());
        }
        Ok(generations)
    }

    pub(super) fn current(
        &self,
        generations: &BTreeMap<u64, String>,
    ) -> Result<Option<u64>, String> {
        let path = self.root.join("current");
        let Some(target) = read_link(&path)? else {
            return Ok(None);
        };
        generations
            .keys()
            .find(|number| {
                target.as_os_str()
                    == Path::new("generations")
                        .join(number.to_string())
                        .as_os_str()
            })
            .copied()
            .map(Some)
            .ok_or_else(|| format!("{}: invalid current generation", path.display()))
    }

    fn record_generation(
        &self,
        id: &str,
        generations: &BTreeMap<u64, String>,
    ) -> Result<u64, String> {
        let number = match generations.last_key_value() {
            Some((&number, latest)) if latest == id => return Ok(number),
            Some((&number, _)) => number.checked_add(1).ok_or("generation number overflow")?,
            None => 1,
        };
        let link = self.root.join("generations").join(number.to_string());
        io(&link, symlink(self.store.join(id), &link))?;
        Ok(number)
    }
}

fn require_directory(path: &Path) -> Result<(), String> {
    if !io(path, fs::symlink_metadata(path))?.is_dir() {
        return Err(format!("{}: expected a directory", path.display()));
    }
    Ok(())
}

fn write(path: &Path, contents: impl AsRef<[u8]>) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        io(parent, fs::create_dir_all(parent))?;
    }
    io(path, fs::write(path, contents))
}

fn write_json(path: &Path, contents: &impl Serialize) -> Result<(), String> {
    let mut bytes = serde_json::to_vec_pretty(contents).map_err(|e| e.to_string())?;
    bytes.push(b'\n');
    write(path, bytes)
}

fn make_read_only(path: &Path) -> std::io::Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.is_dir() {
        for entry in fs::read_dir(path)? {
            make_read_only(&entry?.path())?;
        }
        fs::set_permissions(path, fs::Permissions::from_mode(0o555))?;
    } else if metadata.is_file() {
        let mode = 0o444 | (metadata.mode() & 0o111);
        fs::set_permissions(path, fs::Permissions::from_mode(mode))?;
    }
    Ok(())
}

fn merge_package(from: &Path, to: &Path, relative: &Path) -> Result<(), String> {
    let file_type = io(from, fs::symlink_metadata(from))?.file_type();
    if file_type.is_dir() {
        match fs::create_dir(to) {
            Ok(()) => {}
            Err(error) if error.kind() == ErrorKind::AlreadyExists => {
                require_directory(to)
                    .map_err(|_| format!("{}: package path collision", to.display()))?;
            }
            Err(error) => return io(to, Err(error)),
        }
        let mut names = io(from, fs::read_dir(from))?
            .map(|entry| entry.map(|entry| entry.file_name()))
            .collect::<std::io::Result<Vec<_>>>()
            .map_err(|error| format!("{}: {error}", from.display()))?;
        names.sort();
        for name in names {
            merge_package(
                &from.join(&name),
                &to.join(&name),
                &Path::new("..").join(relative).join(&name),
            )?;
        }
    } else {
        match symlink(relative, to) {
            Ok(()) => {}
            Err(error) if error.kind() == ErrorKind::AlreadyExists => {
                if io(to, fs::read_link(to)).as_deref() != Ok(relative) {
                    return Err(format!("{}: package path collision", to.display()));
                }
            }
            Err(error) => return io(to, Err(error)),
        }
    }
    Ok(())
}

fn package_file(output: &Path, id: &str, source: &Path) -> Result<PathBuf, String> {
    let invalid = || {
        format!(
            "{}: expected a regular file inside the package output",
            output.join(source).display()
        )
    };
    let mut remaining: VecDeque<OsString> = source.iter().map(OsString::from).collect();
    let mut path = output.to_path_buf();
    let inside = Path::new(STORE).join(id);
    let mut links = 0;
    while let Some(component) = remaining.pop_front() {
        if component == ".." {
            if path == output {
                return Err(invalid());
            }
            path.pop();
            continue;
        }
        path.push(component);
        if io(&path, fs::symlink_metadata(&path))?.is_symlink() {
            links += 1;
            if links > 40 {
                return Err(format!(
                    "{}: too many symlinks",
                    output.join(source).display()
                ));
            }
            let target = io(&path, fs::read_link(&path))?;
            path.pop();
            let relative = if target.is_absolute() {
                path = output.to_path_buf();
                target.strip_prefix(&inside).map_err(|_| invalid())?
            } else {
                &target
            };
            for component in relative.iter().rev() {
                remaining.push_front(component.to_owned());
            }
        }
    }
    if !io(&path, fs::metadata(&path))?.is_file() {
        return Err(invalid());
    }
    Ok(path)
}

fn scan_references(bytes: &[u8], references: &mut BTreeSet<String>) {
    for window in bytes.windows(REFERENCE_SIZE) {
        if let Some(id) = window.strip_prefix(STORE_REFERENCE)
            && let Ok(id) = std::str::from_utf8(id)
            && valid_store_id(id)
        {
            references.insert(id.to_owned());
        }
    }
}

fn store_references(path: &Path, references: &mut BTreeSet<String>) -> Result<(), String> {
    let kind = io(path, fs::symlink_metadata(path))?.file_type();
    if kind.is_dir() {
        for entry in io(path, fs::read_dir(path))? {
            store_references(&io(path, entry)?.path(), references)?;
        }
    } else if kind.is_symlink() {
        let target = io(path, fs::read_link(path))?;
        scan_references(target.as_os_str().as_bytes(), references);
    } else if kind.is_file() {
        let mut file = io(path, File::open(path))?;
        let mut bytes = vec![0; 64 * 1024 + REFERENCE_SIZE - 1];
        let mut overlap = 0;
        loop {
            let read = io(path, file.read(&mut bytes[overlap..]))?;
            if read == 0 {
                break;
            }
            let end = overlap + read;
            scan_references(&bytes[..end], references);
            overlap = end.min(REFERENCE_SIZE - 1);
            bytes.copy_within(end - overlap..end, 0);
        }
    }
    Ok(())
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn render(
    record: &HomeRecord,
    out: &Path,
    store: &Path,
    packages: &BTreeMap<String, String>,
    closure: &BTreeSet<String>,
) -> Result<(), String> {
    for dir in [
        "sw/bin",
        "commands",
        "files",
        "units",
        "environment",
        "manifest",
    ] {
        let path = out.join(dir);
        io(&path, fs::create_dir_all(&path))?;
    }
    for id in packages.values().collect::<BTreeSet<_>>() {
        let package = store.join(id);
        for entry in io(&package, fs::read_dir(&package))? {
            let entry = io(&package, entry)?;
            let name = entry.file_name();
            if name != ".vug" {
                merge_package(
                    &entry.path(),
                    &out.join("sw").join(&name),
                    &Path::new("../..").join(id).join(&name),
                )?;
            }
        }
    }
    for file in &record.file {
        let path = out.join("files").join(&file.path);
        if let (Some(package), Some(source)) = (&file.package, &file.source) {
            let id = &packages[package];
            let source = package_file(&store.join(id), id, Path::new(source))?;
            if let Some(parent) = path.parent() {
                io(parent, fs::create_dir_all(parent))?;
            }
            let mode = io(&source, fs::metadata(&source))?.mode();
            io(&path, fs::copy(source, &path))?;
            io(
                &path,
                fs::set_permissions(&path, fs::Permissions::from_mode(0o644 | (mode & 0o100))),
            )?;
        } else if let Some(content) = &file.content {
            write(&path, content)?;
        }
    }
    for entry in io(out, fs::read_dir(out.join("sw/bin")))? {
        let entry = io(out, entry)?;
        if !io(out, entry.file_type())?.is_symlink() {
            continue;
        }
        let relative = io(&entry.path(), fs::read_link(entry.path()))?;
        let target = relative
            .strip_prefix("../../..")
            .map_err(|_| format!("{}: unexpected package link", entry.path().display()))?;
        let source = store.join(target);
        let metadata = io(&source, fs::symlink_metadata(&source))?;
        if !metadata.is_symlink() && (!metadata.is_file() || metadata.mode() & 0o111 == 0) {
            continue;
        }
        let target = Path::new(STORE).join(target);
        let target = target.to_str().ok_or("package command path is not UTF-8")?;
        let launcher = out.join("commands").join(entry.file_name());
        write(
            &launcher,
            format!(
                "#!/bin/sh\nlauncher=$(/bin/readlink -f -- \"$0\") || exit 1\nexec /bin/vug home run-program --home-output \"${{launcher%/commands/*}}\" --program {} -- \"$@\"\n",
                shell_quote(target)
            ),
        )?;
        io(
            &launcher,
            fs::set_permissions(&launcher, fs::Permissions::from_mode(0o755)),
        )?;
    }
    let package_paths = packages
        .iter()
        .map(|(name, id)| (name.clone(), format!("{STORE}/{id}")))
        .collect();
    if !record.secret.is_empty() {
        write(
            &out.join("units").join(super::secrets::UNIT),
            "[Unit]\nDescription=Prepare Vug home credentials\nStartLimitIntervalSec=0\n[Service]\nType=oneshot\nExecStart=/bin/vug home prepare-secrets\n",
        )?;
        let wants = out.join("units/default.target.wants");
        io(&wants, fs::create_dir_all(&wants))?;
        let link = wants.join(super::secrets::UNIT);
        io(
            &link,
            symlink(Path::new("..").join(super::secrets::UNIT), &link),
        )?;
    }
    for unit in &record.unit {
        let content = system::expand(
            &unit.content,
            "pkg",
            &package_paths,
            "is not a package of this home",
        )?;
        let mut rendered = String::new();
        for line in unit
            .content
            .split_inclusive('\n')
            .zip(content.split_inclusive('\n'))
        {
            let (source, expanded) = line;
            let Some((directive, command)) = source.trim_start().split_once('=') else {
                rendered.push_str(expanded);
                continue;
            };
            if directive.trim_end() != "ExecStart" || !command.trim_start().starts_with("{pkg:") {
                rendered.push_str(expanded);
                continue;
            }
            let (_, expanded) = expanded.split_once('=').ok_or("missing ExecStart value")?;
            let expanded = expanded.trim_start_matches([' ', '\t']);
            let end = expanded.find(char::is_whitespace).unwrap_or(expanded.len());
            let program = &expanded[..end];
            rendered.push_str(&format!(
                "ExecStart=/bin/vug home run-program --home-output \"%h/.config/vug/home/profile\" --program {program} --{}",
                &expanded[end..],
            ));
        }
        if unit.name.ends_with(".service") && !record.environment.is_empty() {
            rendered.push_str("\n[Service]\nEnvironmentFile=%h/.config/vug/home/profile/environment/systemd.env\n");
        }
        if !unit.secrets.is_empty() {
            let gate = super::secrets::UNIT;
            rendered.push_str(&format!(
                "\n[Unit]\nRequires={gate}\nAfter={gate}\n[Service]\n"
            ));
            for name in &unit.secrets {
                rendered.push_str(&format!("LoadCredential={name}:%t/vug/secrets/{name}\n"));
            }
        }
        write(&out.join("units").join(&unit.name), rendered)?;
        for target in &unit.enable {
            let wants = out.join("units").join(format!("{target}.wants"));
            io(&wants, fs::create_dir_all(&wants))?;
            let link = wants.join(&unit.name);
            io(&link, symlink(Path::new("..").join(&unit.name), &link))?;
        }
    }
    write_json(&out.join("environment/variables.json"), &record.environment)?;
    write(&out.join("environment/path"), "commands\n")?;
    let mut profile = String::from("export PATH=\"$HOME/.config/vug/home/profile/commands\"");
    match record.environment.get("PATH") {
        None => profile.push_str("\"${PATH:+:$PATH}\""),
        Some(path) if !path.is_empty() => profile.push_str(&shell_quote(&format!(":{path}"))),
        Some(_) => (),
    }
    profile.push('\n');
    let mut systemd = String::new();
    for (name, value) in &record.environment {
        use std::fmt::Write as _;
        if name != "PATH" {
            writeln!(profile, "export {name}={}", shell_quote(value)).unwrap();
        }
        let escaped = value.replace('\\', "\\\\").replace('"', "\\\"");
        writeln!(systemd, "{name}=\"{escaped}\"").unwrap();
    }
    write(&out.join("environment/profile.sh"), profile)?;
    write(&out.join("environment/systemd.env"), systemd)?;

    write_json(
        &out.join("manifest/home.json"),
        &Manifest {
            name: &record.name,
            packages,
            closure,
            files: record.file.iter().map(|file| file.path.as_str()).collect(),
            units: record
                .unit
                .iter()
                .map(|unit| {
                    (
                        unit.name.as_str(),
                        ManifestUnit {
                            enable: &unit.enable,
                            secrets: &unit.secrets,
                        },
                    )
                })
                .collect(),
            secrets: record
                .secret
                .iter()
                .map(|secret| secret.name.as_str())
                .collect(),
        },
    )?;
    Ok(())
}

pub(crate) fn build(settings: &Settings, state: &Path, name: &str) -> Result<BuiltHome, String> {
    let record =
        load(&settings.root, name).map_err(|error| format!("{}: {}", error.path, error.message))?;
    let state = State::open(state, &settings.data_dir)?;
    let _transaction = gc::transaction(&settings.data_dir, false)?;
    let generations = state.generations()?;
    let mut closure = BTreeSet::new();
    let mut packages = BTreeMap::new();
    if !record.packages.is_empty() {
        let mut builder = Builder::cached(settings)?;
        for package in &record.packages {
            let built = builder.build(package, None)?;
            closure.extend(built.closure().iter().cloned());
            packages.insert(package.clone(), built.id);
        }
        let mut pending = closure.clone();
        while let Some(id) = pending.pop_first() {
            builder.require_output(&id)?;
            let mut references = BTreeSet::new();
            store_references(&state.store.join(id), &mut references)?;
            for reference in references {
                if closure.insert(reference.clone()) {
                    pending.insert(reference);
                }
            }
        }
    }

    let work = io(
        &state.store,
        tempfile::Builder::new()
            .prefix(".home-")
            .tempdir_in(&state.store),
    )?;
    render(&record, work.path(), &state.store, &packages, &closure)?;
    nix_closure::write(
        work.path(),
        nix_closure::for_outputs(&state.store, &closure)?,
    )?;
    let id = treehash::encode_base32(&io(work.path(), treehash::hash_tree(work.path()))?);
    let path = state.store.join(&id);
    match fs::symlink_metadata(&path) {
        Ok(metadata) if metadata.is_dir() => {}
        Ok(_) => return Err(format!("{}: expected a directory", path.display())),
        Err(error) if error.kind() == ErrorKind::NotFound => {
            let publish = io(work.path(), make_read_only(work.path()))
                .and_then(|()| io(&path, fs::rename(work.path(), &path)));
            if publish.is_err() {
                let _ = gc::make_writable(work.path());
            }
            publish?;
        }
        Err(error) => return io(&path, Err(error)),
    }
    let generation = state.record_generation(&id, &generations)?;
    Ok(BuiltHome {
        id,
        path,
        generation,
    })
}

pub(crate) fn delete_generations(
    state: &Path,
    data: &Path,
    keep: usize,
) -> Result<Vec<u64>, String> {
    let state = State::open(state, data)?;
    let generations = state.generations()?;
    let current = state.current(&generations)?;
    let activated = super::activate::activation_generations(&state.root)?;
    let mut removed = Vec::new();
    for &number in generations
        .keys()
        .take(generations.len().saturating_sub(keep.max(1)))
    {
        if Some(number) != current && !activated.contains(&number) {
            let link = state.root.join("generations").join(number.to_string());
            io(&link, fs::remove_file(&link))?;
            removed.push(number);
        }
    }
    Ok(removed)
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Closure {
    closure: BTreeSet<String>,
}

pub(crate) fn collect_garbage(
    state: &Path,
    data: &Path,
    dry_run: bool,
) -> Result<(usize, Vec<String>), String> {
    let state = State::open(state, data)?;
    let _transaction = gc::transaction(data, true)?;
    let generations = state.generations()?;
    state.current(&generations)?;
    if generations.is_empty() {
        return Err(format!(
            "{}: no home generations; refusing to remove every store path",
            state.root.display()
        ));
    }
    let mut live = BTreeSet::new();
    for id in generations.values().collect::<BTreeSet<_>>() {
        let path = state.store.join(id).join("manifest/home.json");
        let contents = io(&path, fs::read(&path))?;
        let manifest: Closure = serde_json::from_slice(&contents)
            .map_err(|error| format!("{}: {error}", path.display()))?;
        for id in &manifest.closure {
            if !valid_store_id(id) {
                return Err(format!("{}: invalid closure id {id:?}", path.display()));
            }
            require_directory(&state.store.join(id))?;
        }
        live.extend(manifest.closure);
        live.insert(id.clone());
    }
    gc::sweep_closures(data, live, dry_run)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn package_directory_union_rejects_a_second_provider_for_a_leaf() {
        let work = tempfile::tempdir().unwrap();
        let store = work.path().join("store");
        let first = store.join("first");
        let second = store.join("second");
        let sw = store.join("profile/sw");
        write(&first.join("bin/tool"), "first").unwrap();
        write(&second.join("bin/other"), "second").unwrap();
        fs::create_dir_all(&sw).unwrap();
        merge_package(
            &first.join("bin"),
            &sw.join("bin"),
            Path::new("../../first/bin"),
        )
        .unwrap();
        merge_package(
            &second.join("bin"),
            &sw.join("bin"),
            Path::new("../../second/bin"),
        )
        .unwrap();
        assert_eq!(fs::read(sw.join("bin/other")).unwrap(), b"second");
        write(&second.join("bin/tool"), "first").unwrap();
        assert!(
            merge_package(
                &second.join("bin"),
                &sw.join("bin"),
                Path::new("../../second/bin")
            )
            .unwrap_err()
            .contains("package path collision")
        );
    }

    #[test]
    fn store_references_cover_symlinks_and_cross_buffer_matches() {
        let work = tempfile::tempdir().unwrap();
        let id = "a".repeat(32);
        let other_id = "b".repeat(32);
        let reference = format!("{STORE}/{id}/lib/loader");
        let mut bytes = vec![0; 64 * 1024 + REFERENCE_SIZE - 6];
        bytes.extend(reference.as_bytes());
        write(&work.path().join("binary"), bytes).unwrap();
        symlink(
            format!("{STORE}/{other_id}/bin/tool"),
            work.path().join("link"),
        )
        .unwrap();
        let mut references = BTreeSet::new();
        store_references(work.path(), &mut references).unwrap();
        assert_eq!(references, BTreeSet::from([id, other_id]));
    }

    #[test]
    fn package_file_accepts_canonical_self_links_and_rejects_escapes() {
        let work = tempfile::tempdir().unwrap();
        let id = "a".repeat(32);
        let package = work.path().join(&id);
        write(&package.join("share/settings"), "public").unwrap();
        symlink(format!("{STORE}/{id}/share"), package.join("data")).unwrap();
        assert_eq!(
            package_file(&package, &id, Path::new("data/settings")).unwrap(),
            package.join("share/settings")
        );
        write(&work.path().join("private"), "private").unwrap();
        symlink("../private", package.join("outside")).unwrap();
        assert!(package_file(&package, &id, Path::new("outside")).is_err());
    }
}
