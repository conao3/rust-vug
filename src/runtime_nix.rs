use std::collections::{BTreeMap, BTreeSet, btree_map};
use std::fs;
use std::io::ErrorKind;
use std::path::Path;

use serde::Deserialize;
use vug::nix_cache::StorePath;

use crate::build::{NIX_CLOSURE_MANIFEST, valid_store_id};

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
