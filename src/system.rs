//! Building a bootable system from a system record (`docs/system.md`).

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use vug::cpio;

use crate::build::{Builder, Settings};
use crate::naming::model;

const STORE: &str = "/vug/store";

pub struct SystemFile {
    path: String,
    mode: u32,
    content: String,
}

pub struct SystemRecord {
    kernel: String,
    packages: Vec<String>,
    files: Vec<SystemFile>,
    /// (package, program path inside the package)
    setuid: Vec<(String, String)>,
}

pub struct BuiltSystem {
    pub name: String,
    pub kernel: PathBuf,
    pub initrd: PathBuf,
    pub packages: usize,
}

fn io<T>(what: &str, path: &Path, result: std::io::Result<T>) -> Result<T, String> {
    result.map_err(|e| format!("{what} {}: {e}", path.display()))
}

fn string(table: &toml::Table, key: &str, context: &str) -> Result<String, String> {
    table
        .get(key)
        .and_then(|v| v.as_str())
        .map(str::to_owned)
        .ok_or_else(|| format!("{context}{key}: expected a string"))
}

fn unknown_keys(table: &toml::Table, allowed: &[&str], context: &str) -> Result<(), String> {
    match table.keys().find(|k| !allowed.contains(&k.as_str())) {
        Some(key) => Err(format!("{context}{key}: unknown key")),
        None => Ok(()),
    }
}

fn valid_image_path(path: &str) -> bool {
    path.starts_with('/')
        && path.len() > 1
        && !path.ends_with('/')
        && path[1..]
            .split('/')
            .all(|part| !part.is_empty() && part != "." && part != "..")
}

pub fn load(path: &Path, expected_name: &str) -> Result<SystemRecord, String> {
    let text = io("read", path, fs::read_to_string(path))?;
    let table = text
        .parse::<toml::Table>()
        .map_err(|e| format!("{}: {e}", path.display()))?;
    unknown_keys(
        &table,
        &["name", "kernel", "packages", "file", "setuid"],
        "",
    )?;
    let name = string(&table, "name", "")?;
    if name != expected_name {
        return Err(format!("name: expected {expected_name:?}"));
    }
    let kernel = string(&table, "kernel", "")?;
    let packages = table
        .get("packages")
        .and_then(|v| v.as_array())
        .ok_or("packages: expected an array of strings")?
        .iter()
        .map(|v| v.as_str().map(str::to_owned))
        .collect::<Option<Vec<_>>>()
        .ok_or("packages: expected an array of strings")?;
    for package in packages.iter().chain([&kernel]) {
        if !model::valid_name(package) {
            return Err(format!("{package:?}: not a valid package name"));
        }
    }
    let mut files = Vec::new();
    let mut seen = BTreeSet::new();
    if let Some(value) = table.get("file") {
        let entries = value
            .as_array()
            .ok_or("file: expected an array of tables")?;
        for entry in entries {
            let entry = entry
                .as_table()
                .ok_or("file: expected an array of tables")?;
            unknown_keys(entry, &["path", "mode", "content"], "file.")?;
            let path = string(entry, "path", "file.")?;
            if !valid_image_path(&path) {
                return Err(format!("file.path: {path:?}: expected an absolute path"));
            }
            if !seen.insert(path.clone()) {
                return Err(format!("file.path: {path:?}: listed twice"));
            }
            let mode = match entry.get("mode") {
                None => 0o644,
                Some(value) => value
                    .as_str()
                    .and_then(|m| u32::from_str_radix(m, 8).ok())
                    .filter(|m| *m <= 0o7777)
                    .ok_or_else(|| {
                        format!("file.mode: {path}: expected octal digits in a string")
                    })?,
            };
            files.push(SystemFile {
                path,
                mode,
                content: string(entry, "content", "file.")?,
            });
        }
    }
    let mut setuid = Vec::new();
    if let Some(value) = table.get("setuid") {
        let entries = value
            .as_array()
            .ok_or("setuid: expected an array of tables")?;
        for entry in entries {
            let entry = entry
                .as_table()
                .ok_or("setuid: expected an array of tables")?;
            unknown_keys(entry, &["package", "program"], "setuid.")?;
            let package = string(entry, "package", "setuid.")?;
            let program = string(entry, "program", "setuid.")?;
            if !packages.contains(&package) {
                return Err(format!(
                    "setuid.package: {package:?} is not a package of this system"
                ));
            }
            if !valid_image_path(&format!("/{program}")) {
                return Err(format!(
                    "setuid.program: {program:?}: expected a path relative to the package"
                ));
            }
            setuid.push((package, program));
        }
    }
    Ok(SystemRecord {
        kernel,
        packages,
        files,
        setuid,
    })
}

fn copy_tree(from: &Path, to: &Path) -> Result<(), String> {
    let metadata = io("stat", from, fs::symlink_metadata(from))?;
    let file_type = metadata.file_type();
    if file_type.is_symlink() {
        let target = io("readlink", from, fs::read_link(from))?;
        io("symlink", to, std::os::unix::fs::symlink(target, to))
    } else if file_type.is_dir() {
        io("create", to, fs::create_dir_all(to))?;
        let mut entries: Vec<_> = io("read", from, fs::read_dir(from))?
            .filter_map(|entry| entry.ok().map(|e| e.file_name()))
            .collect();
        entries.sort();
        for name in entries {
            copy_tree(&from.join(&name), &to.join(&name))?;
        }
        io("chmod", to, fs::set_permissions(to, metadata.permissions()))
    } else {
        io("copy", to, fs::copy(from, to)).map(|_| ())
    }
}

/// Replaces `{pkg:NAME}` by the store path of the package NAME of the system.
fn expand(text: &str, paths: &BTreeMap<String, String>) -> Result<String, String> {
    let mut out = String::new();
    let mut rest = text;
    while let Some(start) = rest.find("{pkg:") {
        let (before, tail) = rest.split_at(start);
        let Some(end) = tail.find('}') else {
            break;
        };
        let name = &tail["{pkg:".len()..end];
        let path = paths
            .get(name)
            .ok_or_else(|| format!("{{pkg:{name}}}: {name:?} is not a package of this system"))?;
        out.push_str(before);
        out.push_str(path);
        rest = &tail[end + 1..];
    }
    out.push_str(rest);
    Ok(out)
}

pub fn build(settings: &Settings, name: &str) -> Result<BuiltSystem, String> {
    if !model::valid_name(name) {
        return Err(format!("{name:?}: not a valid system name"));
    }
    let record = load(
        &settings.root.join("systems").join(format!("{name}.toml")),
        name,
    )?;
    let mut builder = Builder::new(settings)?;
    let kernel = builder.build(&record.kernel, None)?;
    let mut closure = BTreeSet::new();
    let mut paths = BTreeMap::new();
    let mut roots = Vec::new();
    for package in &record.packages {
        let built = builder.build(package, None)?;
        closure.extend(built.closure().iter().cloned());
        paths.insert(package.clone(), format!("{STORE}/{}", built.id));
        roots.push(built.id);
    }

    let store_dir = settings.data_dir.join("store");
    let out_dir = settings.data_dir.join("systems").join(name);
    let staging = out_dir.join("root");
    if staging.exists() {
        io("remove", &staging, fs::remove_dir_all(&staging))?;
    }
    for dir in [
        "bin",
        "dev",
        "etc",
        "proc",
        "root",
        "run",
        "sys",
        "tmp",
        "usr",
        "var",
        "vug/store",
    ] {
        let path = staging.join(dir);
        io("create", &path, fs::create_dir_all(&path))?;
    }
    io(
        "chmod",
        &staging,
        fs::set_permissions(staging.join("tmp"), fs::Permissions::from_mode(0o1777)),
    )?;
    for (link, target) in [
        ("sbin", "bin"),
        ("usr/bin", "../bin"),
        ("usr/sbin", "../bin"),
    ] {
        let path = staging.join(link);
        io("symlink", &path, std::os::unix::fs::symlink(target, &path))?;
    }
    for id in &closure {
        copy_tree(&store_dir.join(id), &staging.join("vug/store").join(id))?;
    }
    for id in &roots {
        for dir in ["bin", "sbin"] {
            let Ok(entries) = fs::read_dir(store_dir.join(id).join(dir)) else {
                continue;
            };
            let mut names: Vec<_> = entries
                .filter_map(|entry| entry.ok().map(|e| e.file_name()))
                .collect();
            names.sort();
            for program in names {
                let link = staging.join("bin").join(&program);
                if link.symlink_metadata().is_ok() {
                    io("remove", &link, fs::remove_file(&link))?;
                }
                let target = Path::new(STORE).join(id).join(dir).join(&program);
                io("symlink", &link, std::os::unix::fs::symlink(&target, &link))?;
            }
        }
    }
    for (package, program) in &record.setuid {
        let id = paths[package]
            .trim_start_matches(STORE)
            .trim_start_matches('/');
        let source = store_dir.join(id).join(program);
        let name = Path::new(program)
            .file_name()
            .ok_or_else(|| format!("setuid.program: {program:?}"))?;
        let target = staging.join("bin").join(name);
        if target.symlink_metadata().is_ok() {
            io("remove", &target, fs::remove_file(&target))?;
        }
        io("copy", &target, fs::copy(&source, &target))?;
        io(
            "chmod",
            &target,
            fs::set_permissions(&target, fs::Permissions::from_mode(0o4755)),
        )?;
    }
    for id in &roots {
        for dir in ["lib", "share"] {
            let Ok(entries) = fs::read_dir(store_dir.join(id).join(dir)) else {
                continue;
            };
            let mut names: Vec<_> = entries
                .filter_map(|entry| entry.ok().map(|e| e.file_name()))
                .collect();
            names.sort();
            let into = staging.join("usr").join(dir);
            io("create", &into, fs::create_dir_all(&into))?;
            for name in names {
                let link = into.join(&name);
                if link.symlink_metadata().is_ok() {
                    io("remove", &link, fs::remove_file(&link))?;
                }
                let target = Path::new(STORE).join(id).join(dir).join(&name);
                io("symlink", &link, std::os::unix::fs::symlink(&target, &link))?;
            }
        }
    }
    let lib = staging.join("lib");
    io("symlink", &lib, std::os::unix::fs::symlink("usr/lib", &lib))?;
    for file in &record.files {
        let path = staging.join(&file.path[1..]);
        if let Some(parent) = path.parent() {
            io("create", parent, fs::create_dir_all(parent))?;
        }
        if path.symlink_metadata().is_ok() {
            io("remove", &path, fs::remove_file(&path))?;
        }
        io(
            "write",
            &path,
            fs::write(&path, expand(&file.content, &paths)?),
        )?;
        io(
            "chmod",
            &path,
            fs::set_permissions(&path, fs::Permissions::from_mode(file.mode)),
        )?;
    }

    let initrd = out_dir.join("initrd");
    let mut archive = std::io::BufWriter::new(io("create", &initrd, fs::File::create(&initrd))?);
    io("write", &initrd, cpio::write_newc(&staging, &mut archive))?;
    drop(archive);
    io("remove", &staging, fs::remove_dir_all(&staging))?;
    let kernel_image = out_dir.join("bzImage");
    io(
        "copy",
        &kernel_image,
        fs::copy(
            store_dir.join(&kernel.id).join("boot/bzImage"),
            &kernel_image,
        ),
    )?;
    Ok(BuiltSystem {
        name: name.to_owned(),
        kernel: kernel_image,
        initrd,
        packages: closure.len(),
    })
}
