use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::ffi::{OsStr, OsString};
use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Component, Path, PathBuf};

use rustix::fs::{CWD, RenameFlags, renameat_with};
use serde::Serialize;
use vug::nix_cache::{Cache, Closure, Limits, NarInfo, StorePath};
use vug::treehash;

use super::{Built, NIX_CLOSURE_MANIFEST, Settings, io};
use crate::nix_closure::Entry;
use crate::record::NixOutput;

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct Manifest<'a> {
    format: u32,
    build_key: &'a str,
    output: &'a str,
    paths: Vec<Entry>,
}

pub fn nix_store_dir(data_dir: &Path) -> PathBuf {
    if data_dir == Path::new("/vug")
        && unsafe { libc::geteuid() } == 0
        && fs::read_link("/run/current-system").is_ok_and(|path| {
            path.parent() == Some(Path::new("/vug/store"))
                && path
                    .file_name()
                    .and_then(OsStr::to_str)
                    .is_some_and(super::valid_store_id)
        })
        && !Path::new("/nix/var/nix/db").exists()
    {
        PathBuf::from("/nix/store")
    } else {
        data_dir.join("nix/store")
    }
}

fn build_key(canonical: &str, paths: &BTreeMap<&StorePath, &NarInfo>) -> String {
    let mut hasher = blake3::Hasher::new_derive_key("vug nix-output build key v1");
    let mut field = |value: &str| {
        hasher.update(&(value.len() as u64).to_le_bytes());
        hasher.update(value.as_bytes());
    };
    field(canonical);
    for (path, info) in paths {
        field(path.as_str());
        field(info.nar_hash());
    }
    treehash::encode_base32(hasher.finalize().as_bytes().first_chunk().unwrap())
}

fn checked_paths<'a>(
    record: &NixOutput,
    closure: &'a Closure,
) -> Result<BTreeMap<&'a StorePath, &'a NarInfo>, String> {
    let paths: BTreeMap<_, _> = closure
        .entries()
        .iter()
        .map(|info| (info.store_path(), info))
        .collect();
    for output in record.outputs.values() {
        if paths
            .get(&output.path)
            .is_none_or(|info| info.nar_hash() != output.nar_hash)
        {
            return Err(format!(
                "{}: signed NarHash does not match the record",
                output.path
            ));
        }
    }
    Ok(paths)
}

pub(super) fn build(
    settings: &Settings,
    name: &str,
    version: String,
    output: &str,
    record: &NixOutput,
    canonical: &str,
) -> Result<Built, String> {
    let cache = Cache::default();
    let roots: Vec<_> = record
        .outputs
        .values()
        .map(|entry| entry.path.clone())
        .collect();
    let closure = cache.resolve(&roots, Limits::default())?;
    let paths = checked_paths(record, &closure)?;
    let key = build_key(canonical, &paths);
    let nix_store = super::nix_store_dir(&settings.data_dir);
    cache.realize(&closure, &nix_store)?;
    let selected = &record.outputs[output];

    let store = settings.data_dir.join("store");
    io("create", &store, fs::create_dir_all(&store))?;
    let staging = io("stage output", &store, tempfile::tempdir_in(&store))?;
    let tree = staging.path().join("output");
    let metadata = tree.join(".vug");
    io("create", &metadata, fs::create_dir_all(&metadata))?;
    io(
        "write record",
        &metadata,
        fs::write(metadata.join("record.toml"), canonical),
    )?;
    let mut manifest = serde_json::to_vec(&Manifest {
        format: 1,
        build_key: &key,
        output,
        paths: paths.values().map(|info| Entry::from(*info)).collect(),
    })
    .map_err(|error| error.to_string())?;
    manifest.push(b'\n');
    io(
        "write Nix closure",
        &metadata,
        fs::write(tree.join(NIX_CLOSURE_MANIFEST), manifest),
    )?;
    for path in &selected.expose {
        check_exposure(&nix_store, &paths, &selected.path, path)?;
        let link = tree.join(path);
        let parent = link.parent().unwrap();
        io("create", parent, fs::create_dir_all(parent))?;
        io(
            "create Nix output link",
            &link,
            symlink(Path::new(selected.path.as_str()).join(path), &link),
        )?;
    }
    let id = treehash::encode_base32(&io("hash", &tree, treehash::hash_tree(&tree))?);
    let destination = store.join(&id);
    let fresh = match renameat_with(CWD, &tree, CWD, &destination, RenameFlags::NOREPLACE) {
        Ok(()) => true,
        Err(error) if error == rustix::io::Errno::EXIST => {
            if !io(
                "read store output",
                &destination,
                fs::symlink_metadata(&destination),
            )?
            .is_dir()
                || treehash::encode_base32(&io(
                    "hash",
                    &destination,
                    treehash::hash_tree(&destination),
                )?) != id
            {
                return Err(format!(
                    "{}: existing output does not match its ID",
                    destination.display()
                ));
            }
            false
        }
        Err(error) => return Err(format!("publish output {}: {error}", destination.display())),
    };
    Ok(Built {
        name: name.to_owned(),
        version,
        closure: BTreeSet::from([id.clone()]),
        id,
        fresh,
        exposed: BTreeSet::new(),
        private: false,
    })
}

fn check_exposure(
    store: &Path,
    paths: &BTreeMap<&StorePath, &NarInfo>,
    root: &StorePath,
    path: &str,
) -> Result<(), String> {
    let mut pending: VecDeque<OsString> = std::iter::once(root.basename())
        .chain(path.split('/'))
        .map(OsString::from)
        .collect();
    let mut location = PathBuf::new();
    let mut links = 0;
    while let Some(part) = pending.pop_front() {
        if part == OsStr::new("..") {
            if !location.pop() {
                break;
            }
            continue;
        }
        if location.as_os_str().is_empty()
            && !part.to_str().is_some_and(|name| {
                format!("/nix/store/{name}")
                    .parse::<StorePath>()
                    .is_ok_and(|path| paths.contains_key(&path))
            })
        {
            break;
        }
        let resolved = store.join(&location).join(&part);
        let metadata = io(
            "inspect exposed Nix path",
            &resolved,
            fs::symlink_metadata(&resolved),
        )?;
        if metadata.is_symlink() {
            links += 1;
            if links > 40 {
                break;
            }
            let target = io("read Nix symlink", &resolved, fs::read_link(&resolved))?;
            let target = if target.is_absolute() {
                let Ok(target) = target.strip_prefix("/nix/store") else {
                    break;
                };
                location.clear();
                target
            } else {
                &target
            };
            for component in target.components().rev() {
                match component {
                    Component::Normal(name) => pending.push_front(name.to_os_string()),
                    Component::ParentDir => pending.push_front(OsString::from("..")),
                    _ => {}
                }
            }
        } else if pending.is_empty() && metadata.is_file() {
            if path.starts_with("bin/") && metadata.permissions().mode() & 0o100 == 0 {
                return Err(format!("{root}/{path}: expected an executable file"));
            }
            return Ok(());
        } else if metadata.is_dir() {
            location.push(part);
        } else {
            break;
        }
    }
    Err(format!(
        "{root}/{path}: expected a path to a file in the signed Nix closure"
    ))
}
