use std::collections::{BTreeMap, BTreeSet, btree_map};
use std::fs;
use std::io::ErrorKind;
use std::path::Path;
use std::process::Command;

use serde::Deserialize;
use vug::nix_cache::StorePath;

use crate::build::{NIX_CLOSURE_MANIFEST, nix_store_dir, valid_store_id};

pub(crate) const NIX_STORE: &str = "/nix/store";

#[derive(Deserialize)]
struct Manifest {
    format: u32,
    paths: Vec<Entry>,
}

#[derive(Deserialize)]
#[serde(rename_all = "kebab-case")]
struct Entry {
    store_path: String,
    nar_hash: String,
}

pub(crate) fn paths(store: &Path, ids: &BTreeSet<String>) -> Result<BTreeSet<StorePath>, String> {
    let mut paths = BTreeMap::new();
    for id in ids {
        if !valid_store_id(id) {
            return Err(format!("{}: invalid closure ID {id:?}", store.display()));
        }
        let file = store.join(id).join(NIX_CLOSURE_MANIFEST);
        let at = |error| format!("{}: {error}", file.display());
        match fs::symlink_metadata(&file) {
            Ok(metadata) if metadata.is_file() => {}
            Ok(_) => return Err(at("expected a regular Nix closure manifest".to_owned())),
            Err(error) if error.kind() == ErrorKind::NotFound => continue,
            Err(error) => return Err(at(error.to_string())),
        }
        let bytes = fs::read(&file).map_err(|error| at(error.to_string()))?;
        let manifest: Manifest =
            serde_json::from_slice(&bytes).map_err(|error| at(error.to_string()))?;
        if manifest.format != 1 || manifest.paths.is_empty() {
            return Err(at("invalid Nix closure manifest format or paths".to_owned()));
        }
        for entry in manifest.paths {
            let path = entry.store_path.parse::<StorePath>().map_err(at)?;
            match paths.entry(path) {
                btree_map::Entry::Vacant(slot) => {
                    slot.insert(entry.nar_hash);
                }
                btree_map::Entry::Occupied(slot) if *slot.get() != entry.nar_hash => {
                    return Err(at(format!("{}: conflicting NarHash values", slot.key())));
                }
                btree_map::Entry::Occupied(_) => {}
            }
        }
    }
    Ok(paths.into_keys().collect())
}

fn io<T>(path: &Path, result: std::io::Result<T>) -> Result<T, String> {
    result.map_err(|error| format!("{}: {error}", path.display()))
}

pub(crate) fn command(
    store: &Path,
    closure: &BTreeSet<String>,
    fhs: Option<&crate::fhs::Runtime>,
) -> Result<Command, String> {
    let bwrap = if Path::new("/bin/bwrap").is_file() {
        "/bin/bwrap"
    } else {
        "bwrap"
    };
    let mut command = Command::new(bwrap);
    command.args(["--unshare-user", "--die-with-parent"]);
    let nix_paths = paths(store, closure)?;
    for entry in io(Path::new("/"), fs::read_dir("/"))? {
        let path = entry.map_err(|error| error.to_string())?.path();
        if (!nix_paths.is_empty() || fhs.is_some()) && path == Path::new("/nix")
            || fhs.is_some_and(|fhs| fhs.replaces(&path))
        {
            continue;
        }
        let flag = if path == Path::new("/dev") {
            "--dev-bind"
        } else {
            "--bind"
        };
        command.arg(flag).arg(&path).arg(&path);
    }

    if !nix_paths.is_empty() {
        let data = store.parent().ok_or("home store has no data directory")?;
        let nix_store = nix_store_dir(data);
        if !io(&nix_store, fs::symlink_metadata(&nix_store))?.is_dir() {
            return Err(format!(
                "{}: expected a Nix store directory",
                nix_store.display()
            ));
        }
        command.args(["--tmpfs", NIX_STORE]);
        for path in nix_paths {
            let source = nix_store.join(path.basename());
            let metadata = io(&source, fs::symlink_metadata(&source))?;
            if metadata.is_symlink() {
                let target = io(&source, fs::read_link(&source))?;
                command.arg("--symlink").arg(target).arg(path.as_str());
            } else if metadata.is_dir() || metadata.is_file() {
                command.arg("--ro-bind").arg(source).arg(path.as_str());
            } else {
                return Err(format!("{}: invalid Nix store entry", source.display()));
            }
        }
        command.args(["--remount-ro", NIX_STORE]);
    }

    if let Some(fhs) = fhs {
        fhs.configure(&mut command)?;
    }
    Ok(command)
}
