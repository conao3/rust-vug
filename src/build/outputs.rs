use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsStr;
use std::fs::{self, File};
use std::io::{ErrorKind, Read, Write as _};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::symlink;
use std::path::{Component, Path, PathBuf};

use memchr::memmem;
use serde::{Deserialize, Serialize};
use vug::{rewrite, treehash};

use super::{Built, STORE, elf, io, placeholder_path, valid_store_id};
use crate::record::VersionRecord;

#[derive(Default)]
pub(super) struct Runtime {
    direct_local: BTreeSet<String>,
    direct_external: BTreeSet<String>,
    pub(super) local: BTreeSet<String>,
    pub(super) external: BTreeSet<String>,
}

pub(super) fn selected_run<'a>(record: &'a VersionRecord, output: &'a str) -> BTreeSet<&'a str> {
    if record.outputs.is_empty() {
        return record.deps_run.iter().map(String::as_str).collect();
    }
    let mut external = BTreeSet::new();
    let mut pending = vec![output];
    let mut seen = BTreeSet::new();
    while let Some(output) = pending.pop() {
        if seen.insert(output) {
            for dep in &record.outputs[output].run {
                if let Some(local) = dep.strip_prefix(':') {
                    pending.push(local);
                } else {
                    external.insert(dep.as_str());
                }
            }
        }
    }
    external
}

pub(super) fn runtime(
    record: &VersionRecord,
    external: &BTreeMap<&str, Built>,
) -> BTreeMap<String, Runtime> {
    let mut result = BTreeMap::<String, Runtime>::new();
    for name in order(record) {
        let mut runtime = Runtime::default();
        for dep in &record.outputs[name].run {
            if let Some(local) = dep.strip_prefix(':') {
                let other = &result[local];
                runtime.direct_local.insert(local.to_owned());
                runtime.local.insert(local.to_owned());
                runtime.local.extend(other.local.iter().cloned());
                runtime.external.extend(other.external.iter().cloned());
            } else {
                runtime
                    .direct_external
                    .insert(external[dep.as_str()].id.clone());
                runtime
                    .external
                    .extend(external[dep.as_str()].closure.iter().cloned());
            }
        }
        result.insert(name.to_owned(), runtime);
    }
    result
}

fn order(record: &VersionRecord) -> Vec<&str> {
    let mut remaining: BTreeSet<_> = record.outputs.keys().map(String::as_str).collect();
    let mut ordered = Vec::with_capacity(remaining.len());
    while let Some(&name) = remaining.iter().find(|&&name| {
        record.outputs[name]
            .run
            .iter()
            .filter_map(|dep| dep.strip_prefix(':'))
            .all(|dep| !remaining.contains(dep))
    }) {
        remaining.remove(name);
        ordered.push(name);
    }
    assert!(
        remaining.is_empty(),
        "record validation checks output cycles"
    );
    ordered
}

#[derive(Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Manifest {
    format: u32,
    pub(super) key: String,
    pub(super) outputs: BTreeMap<String, Entry>,
}

#[derive(Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Entry {
    pub(super) id: String,
    run: BTreeSet<String>,
}

impl Manifest {
    fn new(key: &str, ids: &BTreeMap<String, String>, runtime: &BTreeMap<String, Runtime>) -> Self {
        let outputs = runtime
            .iter()
            .map(|(name, runtime)| {
                let mut run = runtime.direct_external.clone();
                run.extend(runtime.direct_local.iter().map(|local| ids[local].clone()));
                (
                    name.clone(),
                    Entry {
                        id: ids[name].clone(),
                        run,
                    },
                )
            })
            .collect();
        Self {
            format: 1,
            key: key.to_owned(),
            outputs,
        }
    }

    pub(super) fn read(
        path: &Path,
        key: &str,
        runtime: &BTreeMap<String, Runtime>,
    ) -> Result<Option<Self>, String> {
        let bytes = match fs::read(path) {
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
            result => io("read", path, result)?,
        };
        let source = path.display().to_string();
        let manifest = Self::parse(&bytes, &source)?;
        if manifest.key != key {
            return Ok(None);
        }
        manifest.validate(key, runtime, &source)?;
        Ok(Some(manifest))
    }

    pub(super) fn parse(bytes: &[u8], source: &str) -> Result<Self, String> {
        let manifest: Self =
            serde_json::from_slice(bytes).map_err(|error| format!("{source}: {error}"))?;
        let ids: BTreeSet<_> = manifest.outputs.values().map(|entry| &entry.id).collect();
        if manifest.format != 1
            || !valid_store_id(&manifest.key)
            || !manifest.outputs.contains_key("out")
            || ids.len() != manifest.outputs.len()
            || manifest.outputs.iter().any(|(name, entry)| {
                !crate::naming::model::valid_name(name)
                    || !valid_store_id(&entry.id)
                    || !entry.run.iter().all(|id| valid_store_id(id))
            })
            || bytes != manifest.bytes()?
        {
            return Err(format!("{source}: invalid or noncanonical output manifest"));
        }
        Ok(manifest)
    }

    pub(super) fn validate(
        &self,
        key: &str,
        runtime: &BTreeMap<String, Runtime>,
        source: &str,
    ) -> Result<(), String> {
        let ids: BTreeMap<_, _> = self
            .outputs
            .iter()
            .map(|(name, entry)| (name.clone(), entry.id.clone()))
            .collect();
        if !ids.keys().eq(runtime.keys()) || *self != Self::new(key, &ids, runtime) {
            return Err(format!(
                "{source}: output manifest disagrees with the record"
            ));
        }
        Ok(())
    }

    pub(super) fn bytes(&self) -> Result<Vec<u8>, String> {
        let mut json = serde_json::to_vec(self).map_err(|error| error.to_string())?;
        json.push(b'\n');
        Ok(json)
    }

    pub(super) fn check_tree(&self, name: &str, store: &Path) -> Result<(), String> {
        let tree = store.join(&self.outputs[name].id);
        let meta = tree.join(".vug");
        let path = meta.join("output");
        if !io("read output metadata", &meta, fs::symlink_metadata(&meta))?.is_dir()
            || !io("read output metadata", &path, fs::symlink_metadata(&path))?.is_file()
            || io("read output name", &path, fs::read_to_string(&path))? != format!("{name}\n")
        {
            return Err(format!(
                "{}: expected metadata for output {name:?}",
                tree.display()
            ));
        }
        Ok(())
    }

    pub(super) fn publish(
        &self,
        path: &Path,
        runtime: &BTreeMap<String, Runtime>,
    ) -> Result<(), String> {
        let parent = path.parent().unwrap();
        io("create", parent, fs::create_dir_all(parent))?;
        let lock = io("open lock", parent, File::open(parent))?;
        io("lock output index", parent, lock.lock())?;
        if let Some(existing) = Self::read(path, &self.key, runtime)? {
            if existing != *self {
                return Err(format!(
                    "{}: different output IDs for build key {}",
                    path.display(),
                    self.key
                ));
            }
            return Ok(());
        }
        let bytes = self.bytes()?;
        let temp = path.with_file_name(format!(".output-{}.tmp", std::process::id()));
        let result = (|| {
            let mut file = io("create", &temp, File::create(&temp))?;
            io("write", &temp, file.write_all(&bytes))?;
            io("sync", &temp, file.sync_all())?;
            io("rename", path, fs::rename(&temp, path))
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temp);
        }
        result
    }
}

fn component_matches(pattern: &str, name: &OsStr) -> bool {
    let text = name.to_string_lossy();
    let mut chars = text.chars();
    let mut parts = pattern.chars();
    let mut star = None;
    loop {
        match (parts.next(), chars.clone().next()) {
            (None, None) => return true,
            (Some('*'), _) => star = Some((parts.clone(), chars.clone())),
            (Some(p), Some(c)) if p == '?' || p == c => {
                chars.next();
            }
            _ => {
                let Some((tail, text)) = &mut star else {
                    return false;
                };
                if text.next().is_none() {
                    return false;
                }
                parts = tail.clone();
                chars = text.clone();
            }
        }
    }
}

fn matches(pattern: &[&str], path: &[&OsStr]) -> bool {
    let mut row = vec![false; path.len() + 1];
    row[0] = true;
    for part in pattern {
        if *part == "**" {
            for i in 1..row.len() {
                row[i] |= row[i - 1];
            }
        } else {
            for i in (1..row.len()).rev() {
                row[i] = row[i - 1] && component_matches(part, path[i - 1]);
            }
            row[0] = false;
        }
    }
    row[path.len()]
}

struct Layout {
    files: BTreeMap<PathBuf, String>,
    directories: BTreeMap<PathBuf, BTreeSet<String>>,
}

impl Layout {
    fn scan(root: &Path, record: &VersionRecord) -> Result<Self, String> {
        let mut patterns: Vec<_> = record
            .outputs
            .iter()
            .flat_map(|(name, spec)| {
                spec.paths
                    .iter()
                    .map(move |path| (name, path, path.split('/').collect::<Vec<_>>(), false))
            })
            .collect();
        let mut paths = Vec::new();
        list(root, Path::new(""), &mut paths)?;
        paths.sort_unstable();
        let mut files = BTreeMap::new();
        let mut directories = BTreeMap::<PathBuf, BTreeSet<String>>::new();
        for path in paths {
            let mut owner = None;
            let root_overlay = path.starts_with(".vug/root");
            if path.starts_with(".vug") && !root_overlay {
                return Err(format!(
                    "{}: .vug is reserved for output metadata",
                    path.display()
                ));
            }
            let components: Vec<_> = path.iter().collect();
            for (name, pattern, parts, matched) in &mut patterns {
                if !root_overlay && (1..=components.len()).any(|n| matches(parts, &components[..n]))
                {
                    *matched = true;
                    if owner.is_some_and(|other| other != *name) {
                        return Err(format!(
                            "{}: pattern {pattern:?} assigns the path to multiple outputs",
                            path.display()
                        ));
                    }
                    owner = Some(*name);
                }
            }
            let owner = owner.map_or("out", String::as_str).to_owned();
            let metadata = io(
                "stat",
                &root.join(&path),
                fs::symlink_metadata(root.join(&path)),
            )?;
            let first_dir = if metadata.is_dir() {
                Some(path.as_path())
            } else {
                path.parent()
            };
            for parent in first_dir.unwrap().ancestors() {
                directories
                    .entry(parent.to_owned())
                    .or_default()
                    .insert(owner.clone());
            }
            files.insert(path, owner);
        }
        for (name, pattern, _, matched) in patterns {
            if !matched {
                return Err(format!(
                    "outputs.{name}.paths: {pattern:?} did not match the staging tree"
                ));
            }
        }
        Ok(Self { files, directories })
    }

    fn owners(&self, path: &Path) -> BTreeSet<&str> {
        for ancestor in path.ancestors() {
            if let Some(owner) = self.files.get(ancestor) {
                return BTreeSet::from([owner.as_str()]);
            }
            if let Some(owners) = self.directories.get(ancestor) {
                return owners.iter().map(String::as_str).collect();
            }
        }
        BTreeSet::new()
    }
}

fn list(root: &Path, dir: &Path, leaves: &mut Vec<PathBuf>) -> Result<(), String> {
    let path = root.join(dir);
    let mut entries = io("read", &path, fs::read_dir(&path))?.peekable();
    if entries.peek().is_none() && !dir.as_os_str().is_empty() {
        leaves.push(dir.to_owned());
    }
    for entry in entries {
        let entry = io("read", &path, entry)?;
        let relative = dir.join(entry.file_name());
        let kind = io("stat", &entry.path(), entry.file_type())?;
        if kind.is_dir() {
            list(root, &relative, leaves)?;
        } else if kind.is_file() || kind.is_symlink() {
            leaves.push(relative);
        } else {
            return Err(format!("{}: unsupported file type", relative.display()));
        }
    }
    Ok(())
}

pub(super) fn rpath_space(count: usize) -> String {
    format!("{}/.vug/{}", placeholder_path(), "0".repeat(count * 48))
}

pub(super) struct SplitBuild<'a> {
    pub(super) staging: &'a Path,
    pub(super) work: &'a Path,
    pub(super) store: &'a Path,
    pub(super) record: &'a VersionRecord,
    pub(super) canonical: &'a str,
    pub(super) runtime: &'a BTreeMap<String, Runtime>,
    pub(super) auto_rpaths: &'a BTreeSet<String>,
}

impl SplitBuild<'_> {
    pub(super) fn install(&self, key: &str) -> Result<Manifest, String> {
        let layout = Layout::scan(self.staging, self.record)?;
        let outputs = self.work.join("outputs");
        let mut ids = BTreeMap::new();
        for output in order(self.record) {
            let tree = outputs.join(output);
            let meta = tree.join(".vug");
            io("create", &meta, fs::create_dir_all(&meta))?;
            io(
                "write",
                &meta.join("output"),
                fs::write(meta.join("output"), format!("{output}\n")),
            )?;
            if output == "out" {
                let path = meta.join("record.toml");
                io("write", &path, fs::write(&path, self.canonical))?;
            }
            let runtime = &self.runtime[output];
            let relocation = Relocation {
                output,
                prefix: placeholder_path(),
                padding: rpath_space(self.record.outputs.len()),
                runtime,
                ids: &ids,
                layout: &layout,
                build: self,
            };
            for (file, _) in layout.files.iter().filter(|(_, owner)| *owner == output) {
                relocation
                    .copy(file, &tree.join(file))
                    .map_err(|error| format!("outputs.{output} {}: {error}", file.display()))?;
            }
            ids.insert(
                output.to_owned(),
                install_tree(self.store, &tree, &relocation)?,
            );
        }
        Ok(Manifest::new(key, &ids, self.runtime))
    }
}

fn install_tree(store: &Path, tree: &Path, relocation: &Relocation<'_>) -> Result<String, String> {
    let hash = |path: &Path| io("hash", path, treehash::hash_tree(path));
    let id = treehash::encode_base32(&hash(tree)?);
    let inside = format!("{STORE}/{id}");
    io(
        "rewrite",
        tree,
        rewrite::rewrite_tree(tree, relocation.prefix.as_bytes(), inside.as_bytes()),
    )?;
    io("create", store, fs::create_dir_all(store))?;
    let path = store.join(&id);
    if matches!(fs::symlink_metadata(&path), Err(error) if error.kind() == ErrorKind::NotFound) {
        match fs::rename(tree, &path) {
            Ok(()) => return Ok(id),
            Err(error) if !path.is_dir() => {
                return Err(format!("install {}: {error}", path.display()));
            }
            Err(_) => {}
        }
    }
    if !io("stat", &path, fs::symlink_metadata(&path))?.is_dir() || hash(&path)? != hash(tree)? {
        return Err(format!(
            "{}: existing store tree disagrees with output {id}",
            path.display()
        ));
    }
    Ok(id)
}

struct Relocation<'a> {
    output: &'a str,
    prefix: String,
    padding: String,
    runtime: &'a Runtime,
    ids: &'a BTreeMap<String, String>,
    layout: &'a Layout,
    build: &'a SplitBuild<'a>,
}

impl Relocation<'_> {
    fn allowed(&self, id: &str) -> bool {
        self.runtime.external.contains(id)
            || self.runtime.local.iter().any(|local| self.ids[local] == id)
    }

    fn prefix_for(&self, output: &str) -> Result<String, String> {
        if output == self.output {
            Ok(self.prefix.clone())
        } else if self.runtime.local.contains(output) {
            Ok(format!("{STORE}/{}", self.ids[output]))
        } else {
            Err(format!("undeclared reference to output {output:?}"))
        }
    }

    fn local_path(&self, path: &Path) -> Result<PathBuf, String> {
        Ok(Path::new(&self.local_prefix(path, false)?).join(path))
    }

    fn local_prefix(&self, path: &Path, prefer_own_dir: bool) -> Result<String, String> {
        if path.as_os_str().is_empty() {
            return Ok(self.prefix.clone());
        }
        let owners = self.layout.owners(path);
        if let Some(owner) = owners.first()
            && owners.len() == 1
        {
            return self.prefix_for(owner);
        }
        if prefer_own_dir && owners.contains(self.output) {
            return Ok(self.prefix.clone());
        }
        Err(format!(
            "cannot assign staging path {}; use a path specific to one output",
            path.display()
        ))
    }

    fn copy(&self, relative: &Path, dest: &Path) -> Result<(), String> {
        let source = self.build.staging.join(relative);
        let metadata = io("stat", &source, fs::symlink_metadata(&source))?;
        let dir = if metadata.is_dir() {
            dest
        } else {
            dest.parent().unwrap()
        };
        io("create", dir, fs::create_dir_all(dir))?;
        if metadata.is_dir() {
            return Ok(());
        }
        if metadata.is_symlink() {
            let target = io("read link", &source, fs::read_link(&source))?;
            let origin = Path::new(&self.prefix).join(relative.parent().unwrap());
            let absolute = normalize(&origin.join(&target));
            let actual = if let Ok(local) = absolute.strip_prefix(&self.prefix) {
                let relocated = self.local_path(local)?;
                if !target.is_absolute() && relocated.starts_with(&self.prefix) {
                    target
                } else {
                    relocated
                }
            } else {
                if target.starts_with(&self.prefix) {
                    return Err(format!(
                        "staging link escapes the output: {}",
                        target.display()
                    ));
                }
                self.check_references(target.as_os_str().as_bytes())?;
                self.check_references(absolute.as_os_str().as_bytes())?;
                target
            };
            return io("create link", dest, symlink(actual, dest));
        }

        let mut data = io("read", &source, fs::read(&source))?;
        let dynamic = elf::dynamic(&data)?;
        let pkg_config = relative.extension().is_some_and(|ext| ext == "pc");
        if dynamic.is_none()
            && pkg_config
            && let Ok(text) = str::from_utf8(&data)
        {
            data = self.expand_pc_paths(text).into_bytes();
        }
        if let Some(dynamic) = &dynamic {
            let mut start = 0;
            for range in &dynamic.search_paths {
                if range.start >= start {
                    self.relocate_paths(&mut data[start..range.start], false)?;
                }
                start = start.max(range.end);
            }
            self.relocate_paths(&mut data[start..], false)?;
            self.relocate_elf(relative, &mut data, dynamic)?;
            self.check_elf(relative, &data, dynamic)?;
        } else {
            self.relocate_paths(&mut data, pkg_config)?;
        }
        self.check_references(&data)?;
        io("write", dest, fs::write(dest, data))?;
        io(
            "chmod",
            dest,
            fs::set_permissions(dest, metadata.permissions()),
        )
    }

    fn relocate_paths(&self, data: &mut [u8], prefer_own_dir: bool) -> Result<(), String> {
        let needle = self.prefix.as_bytes();
        let finder = memmem::Finder::new(needle);
        let mut offset = 0;
        while let Some(pos) = finder.find(&data[offset..]) {
            let start = offset + pos;
            let end = start + needle.len();
            offset = end;
            let suffix = &data[end..];
            if suffix.starts_with(b"/.vug/") {
                return Err("unresolved staging path in output".into());
            }
            if suffix.first() != Some(&b'/') {
                if suffix.first().is_some_and(u8::is_ascii_alphanumeric) {
                    return Err("unresolved staging path in output".into());
                }
                continue;
            }
            let path_end = end + path_bytes(suffix).len();
            let absolute = normalize(Path::new(OsStr::from_bytes(&data[start..path_end])));
            let local = absolute.strip_prefix(&self.prefix).map_err(|_| {
                format!(
                    "staging reference escapes the output: {}",
                    absolute.display()
                )
            })?;
            let destination = self.local_prefix(local, prefer_own_dir)?;
            let original = Path::new(OsStr::from_bytes(&data[start..path_end]));
            if original
                .components()
                .any(|component| component == Component::ParentDir)
            {
                let trailing_slash = usize::from(data[path_end - 1] == b'/');
                let local = local.as_os_str().as_bytes();
                let path_start = path_end - trailing_slash - local.len();
                data[end..path_start].fill(b'/');
                data[path_start..path_end - trailing_slash].copy_from_slice(local);
            }
            data[start..end].copy_from_slice(destination.as_bytes());
        }
        Ok(())
    }

    fn check_references(&self, data: &[u8]) -> Result<(), String> {
        for index in memmem::find_iter(data, b"/vug/store/") {
            let path = Path::new(OsStr::from_bytes(path_bytes(&data[index..])));
            let Some((id, _)) = split_store_path(path) else {
                continue;
            };
            if id != super::PLACEHOLDER_ID && !valid_store_id(id) {
                continue;
            }
            let normalized = normalize(path);
            let Some((target, _)) = split_store_path(&normalized) else {
                return Err(format!("reference escapes the store: {}", path.display()));
            };
            for candidate in [id, target] {
                if candidate != super::PLACEHOLDER_ID && !self.allowed(candidate) {
                    return Err(format!("undeclared reference to {STORE}/{candidate}"));
                }
            }
        }
        Ok(())
    }

    fn expand_pc_paths(&self, text: &str) -> String {
        let mut vars = BTreeMap::<&str, String>::new();
        for _ in 0..text.lines().count() {
            let mut changed = false;
            for line in text.lines() {
                if let Some((name, value)) = line.split_once('=')
                    && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
                    && !vars.contains_key(name)
                {
                    let mut value = value.to_owned();
                    for (var, path) in &vars {
                        value = value.replace(&format!("${{{var}}}"), path);
                    }
                    if value.starts_with(&self.prefix) && !value.contains("${") {
                        vars.insert(name, value);
                        changed = true;
                    }
                }
            }
            if !changed {
                break;
            }
        }

        let mut text = text.to_owned();
        for (var, path) in vars {
            text = text.replace(&format!("${{{var}}}/"), &format!("{path}/"));
        }
        text
    }

    fn relocate_elf(
        &self,
        file: &Path,
        data: &mut [u8],
        dynamic: &elf::Dynamic,
    ) -> Result<(), String> {
        let origin = Path::new(&self.prefix).join(file.parent().unwrap());
        for range in &dynamic.search_paths {
            let mut search = Vec::new();
            for entry in data[range.clone()].split(|&byte| byte == b':') {
                if entry == self.padding.as_bytes() {
                    continue;
                }
                if let Ok(entry) = str::from_utf8(entry)
                    && self.build.auto_rpaths.contains(entry)
                    && let Some((id, _)) = split_store_path(Path::new(entry))
                    && !self.allowed(id)
                {
                    continue;
                }
                let path = Path::new(OsStr::from_bytes(entry));
                let default_lib = entry == format!("{}/lib/.", self.prefix).as_bytes();
                let absolute = expand_origin(&origin, path);
                if let Ok(local) = absolute.strip_prefix(&self.prefix) {
                    let owners = self.layout.directories.get(local);
                    for owner in owners.into_iter().flatten() {
                        if owner != self.output && !self.runtime.local.contains(owner) {
                            if default_lib {
                                continue;
                            }
                            return Err(format!(
                                "undeclared ELF search path in output {owner:?}: {}",
                                path.display()
                            ));
                        }
                        let prefix = self.prefix_for(owner)?;
                        let resolved = Path::new(&prefix).join(local);
                        let encoded = if owner == self.output && !path.is_absolute() {
                            path
                        } else {
                            &resolved
                        };
                        push_path(&mut search, encoded);
                    }
                } else if let Some((id, _)) = split_store_path(&absolute) {
                    if !self.allowed(id) {
                        return Err(format!("undeclared ELF search path {}", absolute.display()));
                    }
                    let path = if path.is_absolute() { &absolute } else { path };
                    push_path(&mut search, path);
                } else if !path.as_os_str().is_empty() {
                    return Err(format!(
                        "ELF search path is outside the output store: {}",
                        path.display()
                    ));
                }
            }
            if search.is_empty() {
                push_path(&mut search, &Path::new(&self.prefix).join("lib"));
            }
            if search.len() > range.len() {
                return Err("relocated ELF search path does not fit in its string table; the build must preserve the vug linker flags".into());
            }
            data[range.clone()].fill(0);
            data[range.start..range.start + search.len()].copy_from_slice(&search);
        }
        Ok(())
    }

    fn check_elf(&self, file: &Path, data: &[u8], dynamic: &elf::Dynamic) -> Result<(), String> {
        let origin = Path::new(&self.prefix).join(file.parent().unwrap());
        let mut search = Vec::new();
        let search_paths = dynamic
            .runpath
            .as_ref()
            .map_or(&dynamic.search_paths[..], std::slice::from_ref);
        for range in search_paths {
            let bytes = data[range.clone()].split(|&b| b == 0).next().unwrap();
            search.extend(
                bytes
                    .split(|&b| b == b':')
                    .map(|path| expand_origin(&origin, Path::new(OsStr::from_bytes(path)))),
            );
        }
        let mut musl = false;
        if let Some(range) = &dynamic.interpreter {
            let interpreter = Path::new(OsStr::from_bytes(&data[range.clone()]));
            if !self.elf_at(interpreter)? {
                return Err(format!(
                    "unresolved ELF interpreter {}",
                    interpreter.display()
                ));
            }
            musl = is_musl_loader(interpreter);
        }
        for range in &dynamic.needed {
            let needed = Path::new(OsStr::from_bytes(&data[range.clone()]));
            if needed.is_absolute() && self.elf_at(needed)? {
                continue;
            }
            if needed.components().count() == 1 {
                let mut found = false;
                for dir in &search {
                    if self.elf_at(&dir.join(needed))? {
                        found = true;
                        break;
                    }
                }
                if found || needed == Path::new("libc.so") && (musl || self.has_musl_loader()) {
                    continue;
                }
            }
            return Err(format!("unresolved ELF SONAME {}", needed.display()));
        }
        Ok(())
    }

    fn has_musl_loader(&self) -> bool {
        self.runtime
            .external
            .iter()
            .any(|id| super::loader_name(&self.build.store.join(id).join("lib")).is_some())
    }

    fn elf_at(&self, path: &Path) -> Result<bool, String> {
        let mut path = normalize(path);
        'follow: for _ in 0..40 {
            let Some((id, relative)) = split_store_path(&path) else {
                return Ok(false);
            };
            let (base, own) = if id == super::PLACEHOLDER_ID {
                (self.build.staging.to_owned(), true)
            } else if self.allowed(id) {
                (self.build.store.join(id), false)
            } else {
                return Err(format!("undeclared reference to {STORE}/{id}"));
            };
            let mut current = PathBuf::new();
            let mut components = relative.components();
            while let Some(component) = components.next() {
                current.push(component);
                let host = base.join(&current);
                let metadata = match fs::symlink_metadata(&host) {
                    Err(error)
                        if matches!(
                            error.kind(),
                            ErrorKind::NotFound | ErrorKind::NotADirectory
                        ) =>
                    {
                        return Ok(false);
                    }
                    result => io("stat", &host, result)?,
                };
                if own
                    && !metadata.is_dir()
                    && self
                        .layout
                        .files
                        .get(&current)
                        .is_none_or(|owner| owner != self.output)
                {
                    return Ok(false);
                }
                if metadata.is_symlink() {
                    let target = io("read link", &host, fs::read_link(&host))?;
                    let mut next = normalize(
                        &Path::new(STORE)
                            .join(id)
                            .join(current.parent().unwrap())
                            .join(target)
                            .join(components.as_path()),
                    );
                    if own && let Ok(local) = next.strip_prefix(&self.prefix) {
                        next = self.local_path(local)?;
                    }
                    path = next;
                    continue 'follow;
                }
            }
            let host = base.join(relative);
            if !host.is_file() {
                return Ok(false);
            }
            let mut header = [0; 64];
            let file = io("open", &host, File::open(&host))?;
            let size = io("read", &host, (&file).read(&mut header))?;
            return Ok(elf::loadable(&header[..size]));
        }
        Err(format!("too many symlinks resolving {}", path.display()))
    }
}

fn push_path(search: &mut Vec<u8>, path: &Path) {
    let bytes = path.as_os_str().as_bytes();
    if !search.split(|&b| b == b':').any(|entry| entry == bytes) {
        if !search.is_empty() {
            search.push(b':');
        }
        search.extend(bytes);
    }
}

fn path_bytes(bytes: &[u8]) -> &[u8] {
    let end = bytes
        .iter()
        .position(|byte| byte.is_ascii_control() || b" \"':;,)}]<>".contains(byte))
        .unwrap_or(bytes.len());
    &bytes[..end]
}

fn split_store_path(path: &Path) -> Option<(&str, &Path)> {
    let mut parts = path.strip_prefix(STORE).ok()?.components();
    let id = parts.next()?.as_os_str().to_str()?;
    Some((id, parts.as_path()))
}

fn is_musl_loader(path: &Path) -> bool {
    path.file_name().is_some_and(|name| {
        let bytes = name.as_bytes();
        bytes.starts_with(b"ld-musl-") && bytes.ends_with(b".so.1")
    })
}

fn expand_origin(origin: &Path, path: &Path) -> PathBuf {
    for name in ["$ORIGIN", "${ORIGIN}"] {
        if let Ok(rest) = path.strip_prefix(name) {
            return normalize(&origin.join(rest));
        }
    }
    normalize(path)
}

fn normalize(path: &Path) -> PathBuf {
    let mut result = PathBuf::new();
    for component in path.components() {
        match component {
            Component::ParentDir if result.has_root() => {
                result.pop();
            }
            Component::CurDir => {}
            component => result.push(component),
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::{Layout, Manifest, Runtime, matches};
    use std::collections::{BTreeMap, BTreeSet};
    use std::ffi::OsStr;
    use std::fs;
    use std::sync::Barrier;

    const KEY: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const OUT: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    const LIB: &str = "cccccccccccccccccccccccccccccccc";
    const OTHER: &str = "dddddddddddddddddddddddddddddddd";

    #[test]
    fn matches_output_patterns_by_component() {
        let matches_path = |pattern: &str, path: &str| {
            matches(
                &pattern.split('/').collect::<Vec<_>>(),
                &path.split('/').map(OsStr::new).collect::<Vec<_>>(),
            )
        };
        assert!(matches_path("lib/**/lib?.so.*", "lib/libx.so.1"));
        assert!(matches_path("lib/**/lib?.so.*", "lib/sub/dir/libx.so.1"));
        assert!(!matches_path("lib/*/lib?.so.*", "lib/sub/dir/libx.so.1"));
        assert!(matches_path("lib/[x]", "lib/[x]"));
        assert!(!matches_path("lib/[x]", "lib/x"));
    }

    #[test]
    fn keeps_destdir_overlay_in_the_default_output() {
        let temp = tempfile::tempdir().unwrap();
        let versions = temp.path().join("versions");
        fs::create_dir(&versions).unwrap();
        fs::write(
            temp.path().join("package.toml"),
            r#"name = "example"
license = "MIT"

[upstream]
sources = ["https://example.com/"]
"#,
        )
        .unwrap();
        let version = versions.join("1.toml");
        fs::write(
            &version,
            r#"
version = "1"

[source]
url = "https://example.com/package.tar.gz"
sha256 = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"

[build]
kind = "meson"

[dependencies]
build = []
run = []

[outputs.out]
run = []

[outputs.dev]
paths = ["include/**"]
run = []

[check]
enabled = false
skip = []
"#,
        )
        .unwrap();
        let (record, _) = crate::record::load_version(&version).unwrap_or_else(|errors| {
            panic!("{}", errors[0].message);
        });
        let stage = temp.path().join("stage");
        fs::create_dir_all(stage.join(".vug/root/etc")).unwrap();
        fs::create_dir(stage.join("include")).unwrap();
        fs::write(stage.join(".vug/root/etc/example.conf"), "").unwrap();

        let layout = Layout::scan(&stage, &record).unwrap();
        assert_eq!(
            layout.files[std::path::Path::new(".vug/root/etc/example.conf")],
            "out"
        );
        assert_eq!(layout.files[std::path::Path::new("include")], "dev");

        fs::write(stage.join(".vug/output"), "").unwrap();
        assert!(Layout::scan(&stage, &record).is_err());
    }

    #[test]
    fn rejects_incomplete_manifest_and_inconsistent_edges() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("package/version");
        let runtime = runtime();
        let valid = manifest(OUT);
        assert_eq!(Manifest::read(&path, KEY, &runtime).unwrap(), None);
        valid.publish(&path, &runtime).unwrap();

        let mut incomplete = manifest(OUT);
        incomplete.outputs.remove("lib");
        fs::write(&path, incomplete.bytes().unwrap()).unwrap();
        assert!(Manifest::read(&path, KEY, &runtime).is_err());

        let mut wrong_edge = manifest(OUT);
        wrong_edge.outputs.get_mut("out").unwrap().run.clear();
        fs::write(&path, wrong_edge.bytes().unwrap()).unwrap();
        assert!(Manifest::read(&path, KEY, &runtime).is_err());
    }

    #[test]
    fn concurrent_publication_preserves_the_first_complete_manifest() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("package/version");
        let barrier = Barrier::new(2);
        let runtime = runtime();
        let first = manifest(OUT);
        let second = manifest(OTHER);
        let publish = |manifest: &Manifest| {
            barrier.wait();
            manifest.publish(&path, &runtime)
        };
        let (first_result, second_result) = std::thread::scope(|scope| {
            let a = scope.spawn(|| publish(&first));
            let b = scope.spawn(|| publish(&second));
            (a.join().unwrap(), b.join().unwrap())
        });
        let expected = match (first_result, second_result) {
            (Ok(()), Err(error)) => {
                assert!(error.contains("different output IDs"), "{error}");
                first
            }
            (Err(error), Ok(())) => {
                assert!(error.contains("different output IDs"), "{error}");
                second
            }
            result => panic!("unexpected publication outcomes: {result:?}"),
        };
        assert_eq!(
            Manifest::read(&path, KEY, &runtime).unwrap(),
            Some(expected)
        );
        assert_eq!(fs::read_dir(path.parent().unwrap()).unwrap().count(), 1);
    }

    fn runtime() -> BTreeMap<String, Runtime> {
        BTreeMap::from([
            (
                "out".into(),
                Runtime {
                    direct_local: BTreeSet::from(["lib".into()]),
                    ..Runtime::default()
                },
            ),
            ("lib".into(), Runtime::default()),
        ])
    }

    fn manifest(out: &str) -> Manifest {
        Manifest::new(
            KEY,
            &BTreeMap::from([("out".into(), out.into()), ("lib".into(), LIB.into())]),
            &runtime(),
        )
    }
}
