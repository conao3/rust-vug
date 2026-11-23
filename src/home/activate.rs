use std::collections::{BTreeMap, BTreeSet};
use std::ffi::{CStr, CString, OsStr, OsString};
use std::fs::{self, File, OpenOptions};
use std::io::{ErrorKind, Write};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::fs::{OpenOptionsExt, symlink};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::output::{State, check_owner, io};
use super::{manager, unit_suffix, validate_file_path};

const JOURNAL: &str = "activation-pending.json";
const ACTIVE: &str = "activation-home.json";

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Manifest {
    files: Vec<String>,
    units: BTreeMap<String, ManifestUnit>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct ManifestUnit {
    enable: BTreeSet<String>,
}

#[derive(Deserialize, Serialize)]
struct Activation {
    home: Vec<u8>,
    generation: u64,
}

#[derive(Deserialize, Serialize)]
struct Pending {
    home: Vec<u8>,
    from: Option<u64>,
    to: u64,
    #[serde(default)]
    manager: Option<manager::Changes>,
}

fn read_state<T: serde::de::DeserializeOwned>(path: &Path) -> Result<Option<T>, String> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => return io(path, Err(error)),
    };
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|error| format!("{}: {error}", path.display()))
}

fn write_state(
    root: &Path,
    name: &str,
    value: &impl Serialize,
    replace: bool,
) -> Result<(), String> {
    let path = root.join(name);
    let mut file = io(&path, tempfile::NamedTempFile::new_in(root))?;
    serde_json::to_writer(&mut file, value)
        .map_err(|error| format!("{}: {error}", path.display()))?;
    io(&path, file.flush())?;
    io(&path, file.as_file().sync_all())?;
    let result = if replace {
        file.persist(&path)
    } else {
        file.persist_noclobber(&path)
    };
    io(&path, result.map_err(|error| error.error))?;
    sync_directory(root)
}

fn sync_directory(path: &Path) -> Result<(), String> {
    io(path, File::open(path).and_then(|dir| dir.sync_all()))
}

pub(super) fn activation_generations(root: &Path) -> Result<BTreeSet<u64>, String> {
    let active = read_state::<Activation>(&root.join(ACTIVE))?;
    let pending = read_state::<Pending>(&root.join(JOURNAL))?;
    Ok(active
        .into_iter()
        .map(|active| active.generation)
        .chain(
            pending
                .into_iter()
                .flat_map(|pending| pending.from.into_iter().chain([pending.to])),
        )
        .collect())
}

fn link_plan(
    state: &State,
    home: &Home,
    generations: &BTreeMap<u64, String>,
    number: Option<u64>,
) -> Result<BTreeMap<PathBuf, PathBuf>, String> {
    let mut links = BTreeMap::new();
    let Some(number) = number else {
        return Ok(links);
    };
    let id = generations
        .get(&number)
        .ok_or_else(|| format!("home generation {number} does not exist"))?;
    let output = state.store.join(id);
    let manifest_path = output.join("manifest/home.json");
    let manifest: Manifest = read_state(&manifest_path)?
        .ok_or_else(|| format!("{}: missing home manifest", manifest_path.display()))?;
    let mut add = |destination: PathBuf, source: PathBuf| {
        let absolute = home.path.join(&destination);
        for reserved in [&state.root, state.store.parent().unwrap()] {
            if absolute.starts_with(reserved) || reserved.starts_with(&absolute) {
                return Err(format!(
                    "{}: destination overlaps {}",
                    absolute.display(),
                    reserved.display()
                ));
            }
        }
        let target = state.root.join("current").join(source);
        if links.insert(destination, target).is_some() {
            return Err(format!("{}: duplicate destination", absolute.display()));
        }
        Ok(())
    };
    if output.join("environment/profile.sh").is_file() {
        add(PathBuf::from(".config/vug/home/profile"), PathBuf::new())?;
    }
    for path in manifest.files {
        validate_file_path(&path)?;
        let source = Path::new("files").join(&path);
        let file = output.join(&source);
        if !io(&file, fs::symlink_metadata(&file))?.is_file() {
            return Err(format!("{}: expected a regular file", file.display()));
        }
        add(path.into(), source)?;
    }
    let unit_root = Path::new(".config/systemd/user");
    for (name, unit) in manifest.units {
        if unit_suffix(&name).is_none() {
            return Err(format!(
                "{}: invalid unit name {name:?}",
                manifest_path.display()
            ));
        }
        let source = Path::new("units").join(&name);
        let file = output.join(&source);
        if !io(&file, fs::symlink_metadata(&file))?.is_file() {
            return Err(format!("{}: expected a regular file", file.display()));
        }
        add(unit_root.join(&name), source)?;
        for target in unit.enable {
            if unit_suffix(&target) != Some("target") {
                return Err(format!(
                    "{}: invalid unit target {target:?}",
                    manifest_path.display()
                ));
            }
            let entry = PathBuf::from(format!("{target}.wants")).join(&name);
            let source = Path::new("units").join(&entry);
            let file = output.join(&source);
            if io(&file, fs::read_link(&file))? != Path::new("..").join(&name) {
                return Err(format!("{}: unexpected unit link target", file.display()));
            }
            add(unit_root.join(entry), source)?;
        }
    }
    for path in links.keys() {
        if path
            .ancestors()
            .skip(1)
            .any(|parent| links.contains_key(parent))
        {
            return Err(format!(
                "{}: destination has a managed parent",
                path.display()
            ));
        }
    }
    Ok(links)
}

struct Home {
    path: PathBuf,
    dir: File,
}

fn c_path(path: &OsStr) -> std::io::Result<CString> {
    CString::new(path.as_bytes())
        .map_err(|error| std::io::Error::new(ErrorKind::InvalidInput, error))
}

fn syscall(result: libc::c_int) -> std::io::Result<()> {
    if result == -1 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

fn open_directory(parent: &File, name: &CStr) -> std::io::Result<File> {
    let fd = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            name.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    syscall(fd)?;
    Ok(unsafe { File::from_raw_fd(fd) })
}

fn read_link(parent: &File, name: &CStr) -> std::io::Result<Option<PathBuf>> {
    let mut bytes = vec![0; libc::PATH_MAX as usize];
    let size = unsafe {
        libc::readlinkat(
            parent.as_raw_fd(),
            name.as_ptr(),
            bytes.as_mut_ptr().cast(),
            bytes.len(),
        )
    };
    if size < 0 {
        let error = std::io::Error::last_os_error();
        return match error.kind() {
            ErrorKind::NotFound => Ok(None),
            ErrorKind::InvalidInput => Err(std::io::Error::new(
                ErrorKind::AlreadyExists,
                "home path collision with an existing file",
            )),
            _ => Err(error),
        };
    }
    if size as usize == bytes.len() {
        return Err(std::io::Error::new(
            ErrorKind::InvalidData,
            "link target is too long",
        ));
    }
    bytes.truncate(size as usize);
    Ok(Some(PathBuf::from(OsString::from_vec(bytes))))
}

impl Home {
    fn open(path: &Path) -> Result<Self, String> {
        if !path.is_absolute() {
            return Err("HOME must be an absolute path".to_owned());
        }
        let path = io(path, fs::canonicalize(path))?;
        let dir = io(
            &path,
            OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
                .open(&path),
        )?;
        let metadata = io(&path, dir.metadata())?;
        check_owner(&path, &metadata)?;
        Ok(Self { path, dir })
    }

    fn identity(&self) -> &[u8] {
        self.path.as_os_str().as_bytes()
    }

    fn parent(&self, path: &Path, create: bool) -> Result<Option<File>, String> {
        let mut parent = io(&self.path, self.dir.try_clone())?;
        let mut location = self.path.clone();
        for component in path.parent().unwrap() {
            location.push(component);
            let name = io(&location, c_path(component))?;
            parent = match open_directory(&parent, &name) {
                Ok(dir) => dir,
                Err(error) if error.kind() == ErrorKind::NotFound && !create => return Ok(None),
                Err(error) if error.kind() == ErrorKind::NotFound => {
                    let result = unsafe { libc::mkdirat(parent.as_raw_fd(), name.as_ptr(), 0o777) };
                    match syscall(result) {
                        Ok(()) => io(&location, parent.sync_all())?,
                        Err(error) if error.kind() == ErrorKind::AlreadyExists => {}
                        Err(error) => return io(&location, Err(error)),
                    }
                    io(&location, open_directory(&parent, &name))?
                }
                Err(error) => {
                    return Err(format!(
                        "{}: home path collision or inaccessible parent directory: {error}",
                        location.display()
                    ));
                }
            };
        }
        Ok(Some(parent))
    }

    fn check(&self, path: &Path, expected: Option<&Path>) -> Result<(), String> {
        let absolute = self.path.join(path);
        let actual = match self.parent(path, false)? {
            Some(parent) => {
                let name = io(&absolute, c_path(path.file_name().unwrap()))?;
                io(&absolute, read_link(&parent, &name))?
            }
            None => None,
        };
        check_target(&absolute, actual.as_deref(), expected)
    }

    fn set_link(
        &self,
        path: &Path,
        target: &Path,
        present: bool,
        recovering: bool,
    ) -> Result<(), String> {
        let absolute = self.path.join(path);
        let Some(parent) = self.parent(path, present)? else {
            return if recovering {
                Ok(())
            } else {
                check_target(&absolute, None, Some(target))
            };
        };
        let name = io(&absolute, c_path(path.file_name().unwrap()))?;
        let actual = io(&absolute, read_link(&parent, &name))?;
        if recovering {
            if actual.is_some() {
                check_target(&absolute, actual.as_deref(), Some(target))?;
            }
            if actual.is_some() == present {
                return io(&absolute, parent.sync_all());
            }
        } else {
            check_target(&absolute, actual.as_deref(), (!present).then_some(target))?;
        }
        if present {
            let target = io(&absolute, c_path(target.as_os_str()))?;
            io(
                &absolute,
                syscall(unsafe {
                    libc::symlinkat(target.as_ptr(), parent.as_raw_fd(), name.as_ptr())
                }),
            )?;
        } else {
            io(
                &absolute,
                syscall(unsafe { libc::unlinkat(parent.as_raw_fd(), name.as_ptr(), 0) }),
            )?;
        }
        io(&absolute, parent.sync_all())
    }
}

fn check_target(path: &Path, actual: Option<&Path>, expected: Option<&Path>) -> Result<(), String> {
    if actual.map(Path::as_os_str) != expected.map(Path::as_os_str) {
        return Err(format!(
            "{}: home path collision (expected {expected:?}, found {actual:?})",
            path.display()
        ));
    }
    Ok(())
}

fn set_current(state: &State, number: Option<u64>) -> Result<(), String> {
    let path = state.root.join("current");
    if let Some(number) = number {
        let temp = io(&path, tempfile::tempdir_in(&state.root))?;
        let link = temp.path().join("current");
        io(
            &link,
            symlink(Path::new("generations").join(number.to_string()), &link),
        )?;
        io(&path, fs::rename(&link, &path))?;
    } else {
        io(&path, fs::remove_file(&path))?;
    }
    sync_directory(&state.root)
}

fn finish(state: &State) -> Result<(), String> {
    let journal = state.root.join(JOURNAL);
    io(&journal, fs::remove_file(&journal))?;
    sync_directory(&state.root)
}

fn active_generation(state: &State, home: &Home) -> Result<Option<u64>, String> {
    let path = state.root.join(ACTIVE);
    let active = read_state::<Activation>(&path)?;
    if active
        .as_ref()
        .is_some_and(|active| active.home != home.identity())
    {
        return Err(format!(
            "{}: profile belongs to a different HOME directory",
            path.display()
        ));
    }
    Ok(active.map(|active| active.generation))
}

fn recover(
    state: &State,
    home: &Home,
    generations: &BTreeMap<u64, String>,
    pending: &Pending,
) -> Result<(), String> {
    if pending.home != home.identity() {
        return Err("unfinished activation belongs to a different HOME directory".to_owned());
    }
    let active = active_generation(state, home)?;
    let current = state.current(generations)?;
    if active == Some(pending.to) && current == active {
        return finish(state);
    }
    if active != pending.from || (current != pending.from && current != Some(pending.to)) {
        return Err(
            "current or recorded generation was changed during an unfinished activation".to_owned(),
        );
    }
    let old = link_plan(state, home, generations, pending.from)?;
    let new = link_plan(state, home, generations, Some(pending.to))?;
    for (path, target) in &new {
        if !old.contains_key(path) {
            home.set_link(path, target, false, true)?;
        }
    }
    for (path, target) in &old {
        if !new.contains_key(path) {
            home.set_link(path, target, true, true)?;
        }
    }
    if current != pending.from {
        set_current(state, pending.from)?;
    }
    if manager::available(&home.path)?
        && let Some(changes) = &pending.manager
    {
        changes.restore()?;
    }
    finish(state)
}

pub(crate) enum Selection {
    Generation(u64),
    Previous,
}

pub(crate) fn activate(
    state: &Path,
    data: &Path,
    home: &Path,
    selection: Selection,
) -> Result<(u64, bool), String> {
    let home = Home::open(home)?;
    let state = State::open(state, data)?;
    let generations = state.generations()?;
    if let Some(pending) = read_state(&state.root.join(JOURNAL))? {
        recover(&state, &home, &generations, &pending)
            .map_err(|error| format!("could not recover unfinished home activation: {error}"))?;
    }
    let current = state.current(&generations)?;
    if current != active_generation(&state, &home)? {
        return Err("current home generation differs from the last recorded activation".to_owned());
    }
    let number = match selection {
        Selection::Generation(number) => number,
        Selection::Previous => {
            let active = current.ok_or("no active home generation")?;
            *generations
                .range(..active)
                .next_back()
                .ok_or("no earlier retained home generation")?
                .0
        }
    };
    let old = link_plan(&state, &home, &generations, current)?;
    let new = link_plan(&state, &home, &generations, Some(number))?;
    for (path, target) in &old {
        home.check(path, Some(target))?;
    }
    for path in new.keys().filter(|path| !old.contains_key(*path)) {
        home.check(path, None)?;
    }
    let manager_available = manager::available(&home.path)?;
    if current == Some(number) {
        return Ok((number, !manager_available));
    }
    let unit_state = |generation: Option<u64>| {
        let mut definitions = BTreeMap::new();
        let Some(id) = generation.and_then(|number| generations.get(&number)) else {
            return Ok::<_, String>((definitions, Vec::new()));
        };
        let output = state.store.join(id);
        let manifest_path = output.join("manifest/home.json");
        let manifest: Manifest = read_state(&manifest_path)?
            .ok_or_else(|| format!("{}: missing home manifest", manifest_path.display()))?;
        for (name, unit) in manifest.units {
            let file = output.join("units").join(&name);
            definitions.insert(name, (io(&file, fs::read(&file))?, unit.enable));
        }
        let variables = output.join("environment/variables.json");
        Ok((definitions, io(&variables, fs::read(&variables))?))
    };
    let manager = if manager_available {
        let (old_units, old_environment) = unit_state(current)?;
        let (new_units, new_environment) = unit_state(Some(number))?;
        Some(manager::Changes::plan(
            &old_units,
            &new_units,
            old_environment != new_environment,
        )?)
    } else {
        None
    };
    let pending = Pending {
        home: home.identity().to_vec(),
        from: current,
        to: number,
        manager,
    };
    write_state(&state.root, JOURNAL, &pending, false)?;
    let apply = || {
        if let Some(changes) = &pending.manager {
            changes.before()?;
        }
        for (path, target) in &old {
            if !new.contains_key(path) {
                home.set_link(path, target, false, false)?;
            }
        }
        for (path, target) in &new {
            if !old.contains_key(path) {
                home.set_link(path, target, true, false)?;
            }
        }
        for (path, target) in &new {
            home.check(path, Some(target))?;
        }
        if state.current(&generations)? != current {
            return Err("current home generation changed during activation".to_owned());
        }
        set_current(&state, Some(number))?;
        if let Some(changes) = &pending.manager {
            changes.after()?;
        }
        write_state(
            &state.root,
            ACTIVE,
            &Activation {
                home: home.identity().to_vec(),
                generation: number,
            },
            true,
        )
    };
    if let Err(error) = apply() {
        return match recover(&state, &home, &generations, &pending) {
            Ok(()) => Err(error),
            Err(recovery) => Err(format!(
                "{error}; home activation recovery is pending: {recovery}"
            )),
        };
    }
    finish(&state)?;
    Ok((number, !manager_available))
}
