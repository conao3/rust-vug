use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::fs;
use std::io::ErrorKind;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::Command;

use serde::Deserialize;

use super::output::io;
use crate::build::{nix_store_dir, valid_store_id};
use crate::{gc, nix_closure};
use vug::treehash;

const SYSTEM_STORE: &str = "/vug/store";

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Manifest {
    packages: BTreeMap<String, String>,
    closure: BTreeSet<String>,
}

pub(crate) fn run(output: &Path, program: &Path, arguments: &[OsString]) -> Result<(), String> {
    if unsafe { libc::geteuid() } == 0 {
        return Err("run home programs as the owner of the user environment".to_owned());
    }
    let resolved = io(output, output.canonicalize())?;
    let store = resolved.parent().ok_or("home output has no parent store")?;
    let data = store.parent().ok_or("home store has no data directory")?;
    let _transaction = gc::transaction(data, false)?;
    let output = io(output, output.canonicalize())?;
    let generation = output
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|id| valid_store_id(id) && output == store.join(id))
        .ok_or("expected a home generation directory")?;
    let manifest_path = output.join("manifest/home.json");
    let manifest: Manifest = serde_json::from_slice(&io(&manifest_path, fs::read(&manifest_path))?)
        .map_err(|error| format!("{}: {error}", manifest_path.display()))?;
    let path = program
        .strip_prefix(SYSTEM_STORE)
        .map_err(|_| format!("{}: expected a home package path", program.display()))?;
    let mut parts = path.components();
    let Some(std::path::Component::Normal(id)) = parts.next() else {
        return Err(format!("{}: missing package ID", program.display()));
    };
    let id = id.to_str().ok_or("invalid package ID")?;
    if !manifest.closure.contains(id)
        || !manifest.packages.values().any(|package| package == id)
        || parts.as_path().as_os_str().is_empty()
        || parts.any(|part| !matches!(part, std::path::Component::Normal(_)))
    {
        return Err(format!(
            "{}: expected a file in a selected package",
            program.display()
        ));
    }

    let bwrap = if Path::new("/bin/bwrap").is_file() {
        "/bin/bwrap"
    } else {
        "bwrap"
    };
    let mut command = Command::new(bwrap);
    command.args(["--unshare-user", "--die-with-parent"]);
    for entry in io(Path::new("/"), fs::read_dir("/"))? {
        let path = entry.map_err(|error| error.to_string())?.path();
        let flag = if path == Path::new("/dev") {
            "--dev-bind"
        } else {
            "--bind"
        };
        command.arg(flag).arg(&path).arg(&path);
    }

    let system = Path::new(SYSTEM_STORE);
    let mut system_outputs = BTreeSet::new();
    match fs::read_dir(system) {
        Ok(entries) => {
            for entry in entries {
                let entry = io(system, entry)?;
                if entry.file_name().to_str().is_some_and(valid_store_id) {
                    system_outputs.insert(entry.file_name());
                }
            }
        }
        Err(error) if error.kind() == ErrorKind::NotFound => (),
        Err(error) => return io(system, Err(error)),
    }

    command.args(["--tmpfs", SYSTEM_STORE]);
    for id in &system_outputs {
        let path = system.join(id);
        command.arg("--ro-bind").arg(&path).arg(&path);
    }
    for id in &manifest.closure {
        if !valid_store_id(id) {
            return Err(format!(
                "{}: invalid closure ID {id:?}",
                manifest_path.display()
            ));
        }
        let source = store.join(id);
        let destination = system.join(id);
        if !io(&source, fs::symlink_metadata(&source))?.is_dir() {
            return Err(format!(
                "{}: expected a package directory",
                source.display()
            ));
        }
        if system_outputs.contains(std::ffi::OsStr::new(id)) {
            let user_hash = io(&source, treehash::hash_tree(&source))?;
            let system_hash = io(&destination, treehash::hash_tree(&destination))?;
            if user_hash != system_hash {
                return Err(format!("{id}: conflicting home and system store outputs"));
            }
        } else {
            command.arg("--ro-bind").arg(source).arg(destination);
        }
    }

    let mut live = manifest.closure;
    live.insert(generation.to_owned());
    let nix_paths = nix_closure::paths(&nix_closure::for_outputs(store, &live)?);
    nix_closure::bind(&mut command, &nix_store_dir(data), &nix_paths)?;
    let _lease = gc::lease(&mut command, data, &live, !nix_paths.is_empty())?;
    command
        .args(["--remount-ro", SYSTEM_STORE, "--"])
        .arg(program)
        .args(arguments);
    Err(format!("bwrap: {}", command.exec()))
}
