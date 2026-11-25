use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::fs;
use std::io::ErrorKind;
use std::os::unix::process::CommandExt;
use std::path::Path;

use serde::Deserialize;

use super::output::io;
use crate::build::valid_store_id;
use crate::runtime_nix;
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
    let output = io(output, output.canonicalize())?;
    let store = output.parent().ok_or("home output has no parent store")?;
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

    let package = store.join(id);
    let fhs = crate::fhs::load(&package)?;
    let fhs_program = fhs
        .as_ref()
        .map(|fhs| fhs.program(&package, program, SYSTEM_STORE, id))
        .transpose()?;
    let mut command = runtime_nix::command(store, &manifest.closure, fhs.as_ref())?;

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

    command
        .args(["--remount-ro", SYSTEM_STORE, "--"])
        .arg(fhs_program.as_deref().unwrap_or(program))
        .args(arguments);
    Err(format!("bwrap: {}", command.exec()))
}
