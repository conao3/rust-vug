use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::ErrorKind;
use std::path::Path;
use std::process::Command;

use serde::{Deserialize, Serialize};
use vug::nix_cache::{self, NarInfo, StorePath};

pub(crate) const GENERATION_MANIFEST: &str = "manifest/nix-closure.json";

#[derive(Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub(crate) struct Entry {
    store_path: String,
    nar_hash: String,
    nar_size: u64,
    references: Vec<String>,
}

impl From<&NarInfo> for Entry {
    fn from(info: &NarInfo) -> Self {
        Self {
            store_path: info.store_path().to_string(),
            nar_hash: info.nar_hash().to_owned(),
            nar_size: info.nar_size(),
            references: info.references().iter().map(ToString::to_string).collect(),
        }
    }
}

#[derive(Deserialize, Serialize)]
pub(crate) struct Manifest {
    format: u32,
    paths: Vec<Entry>,
}

fn read(path: &Path) -> Result<Option<Manifest>, String> {
    let contents = match fs::read(path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("{}: {error}", path.display())),
    };
    let manifest: Manifest = serde_json::from_slice(&contents)
        .map_err(|error| format!("{}: {error}", path.display()))?;
    if manifest.format != 1 {
        return Err(format!(
            "{}: unsupported Nix closure format",
            path.display()
        ));
    }
    Ok(Some(manifest))
}

pub(crate) fn paths(entries: &BTreeMap<StorePath, Entry>) -> BTreeSet<StorePath> {
    entries.keys().cloned().collect()
}

pub(crate) fn for_outputs(
    store: &Path,
    ids: &BTreeSet<String>,
) -> Result<BTreeMap<StorePath, Entry>, String> {
    let mut entries = BTreeMap::new();
    for id in ids {
        if !crate::build::valid_store_id(id) {
            return Err(format!("{id:?}: invalid store ID"));
        }
        let output = store.join(id);
        let manifest = match read(&output.join(crate::build::NIX_CLOSURE_MANIFEST))? {
            None if !output.join(".vug/record.toml").is_file() => {
                read(&output.join(GENERATION_MANIFEST))?
            }
            manifest => manifest,
        };
        let Some(manifest) = manifest else {
            continue;
        };
        for entry in manifest.paths {
            let path: StorePath = entry.store_path.parse()?;
            nix_cache::parse_hash(&entry.nar_hash)?;
            match entries.entry(path) {
                std::collections::btree_map::Entry::Vacant(slot) => {
                    slot.insert(entry);
                }
                std::collections::btree_map::Entry::Occupied(slot) if slot.get() != &entry => {
                    return Err(format!("{}: conflicting Nix closure entries", slot.key()));
                }
                _ => {}
            }
        }
    }
    for entry in entries.values() {
        for reference in &entry.references {
            if !entries.contains_key(&reference.parse::<StorePath>()?) {
                return Err(format!(
                    "{}: missing Nix reference {reference}",
                    entry.store_path
                ));
            }
        }
    }
    Ok(entries)
}

pub(crate) fn write(output: &Path, entries: BTreeMap<StorePath, Entry>) -> Result<(), String> {
    if entries.is_empty() {
        return Ok(());
    }
    let manifest = Manifest {
        format: 1,
        paths: entries.into_values().collect(),
    };
    let path = output.join(GENERATION_MANIFEST);
    let mut contents = serde_json::to_vec(&manifest).map_err(|error| error.to_string())?;
    contents.push(b'\n');
    fs::write(&path, contents).map_err(|error| format!("{}: {error}", path.display()))
}

pub(crate) fn bind(
    command: &mut Command,
    store: &Path,
    paths: &BTreeSet<StorePath>,
) -> Result<(), String> {
    if paths.is_empty() || store == Path::new("/nix/store") {
        return Ok(());
    }
    command.args(["--tmpfs", "/nix", "--dir", "/nix/store"]);
    for path in paths {
        let source = store.join(path.basename());
        let metadata = fs::symlink_metadata(&source)
            .map_err(|error| format!("{}: {error}", source.display()))?;
        if metadata.is_symlink() {
            let target =
                fs::read_link(&source).map_err(|error| format!("{}: {error}", source.display()))?;
            command.arg("--symlink").arg(target).arg(path.as_str());
        } else {
            command.arg("--ro-bind").arg(source).arg(path.as_str());
        }
    }
    command.args(["--remount-ro", "/nix"]);
    Ok(())
}
