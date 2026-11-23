use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::process::Stdio;

use crate::build::{self, Builder, Settings, elf};

use super::{STORE, cpio, entry_names, io, run_tool, strings, symlink, unknown_keys, write_file};

pub(super) struct Config {
    available: Vec<String>,
    force: Vec<String>,
    firmware: Vec<PathBuf>,
}

impl Config {
    pub(super) fn load(value: &toml::Value) -> Result<Self, String> {
        let table = value.as_table().ok_or("boot.initrd: expected a table")?;
        unknown_keys(
            table,
            &["available-modules", "force-modules", "firmware-files"],
            "boot.initrd.",
        )?;
        let modules = |key| {
            let values = if table.contains_key(key) {
                strings(table, key, "boot.initrd.")?
            } else {
                Vec::new()
            };
            for value in &values {
                if value.is_empty()
                    || !value
                        .bytes()
                        .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-'))
                {
                    return Err(format!("boot.initrd.{key}: invalid module {value:?}"));
                }
            }
            Ok(values)
        };
        Ok(Self {
            available: modules("available-modules")?,
            force: modules("force-modules")?,
            firmware: super::firmware::selection(table)?,
        })
    }
}

pub(super) fn valid_root(root: &str) -> bool {
    let Some(uuid) = root.strip_prefix("PARTUUID=") else {
        return false;
    };
    uuid.len() == 36
        && uuid.bytes().enumerate().all(|(i, c)| {
            if matches!(i, 8 | 13 | 18 | 23) {
                c == b'-'
            } else {
                c.is_ascii_hexdigit()
            }
        })
}

fn module_name(path: &str) -> String {
    let name = path.rsplit('/').next().unwrap_or(path);
    name.split('.').next().unwrap_or(name).replace('-', "_")
}

fn install_modules(
    settings: &Settings,
    stage: &Path,
    kernel: &Path,
    config: &Config,
    kmod: &str,
) -> Result<(), String> {
    let base = kernel.join("lib/modules");
    let releases = entry_names(&base);
    let [release] = releases.as_slice() else {
        return Err(format!("{}: expected one kernel release", base.display()));
    };
    let source = base.join(release);
    let target = stage.join("lib/modules").join(release);
    io("create", &target, fs::create_dir_all(&target))?;
    let index = source.join("modules.dep");
    let contents = io("read", &index, fs::read_to_string(&index))?;
    let deps: BTreeMap<_, _> = contents
        .lines()
        .filter_map(|line| line.split_once(':'))
        .collect();
    let names: BTreeMap<_, _> = deps.keys().map(|path| (module_name(path), *path)).collect();
    let builtin = source.join("modules.builtin");
    let builtin = io("read", &builtin, fs::read_to_string(&builtin))?;
    let builtin: BTreeSet<_> = builtin.lines().map(module_name).collect();
    let mut pending = Vec::new();
    for name in config.available.iter().chain(&config.force) {
        let normalized = name.replace('-', "_");
        match names.get(&normalized) {
            Some(path) => pending.push(*path),
            None if builtin.contains(&normalized) => {}
            None => {
                return Err(format!(
                    "boot.initrd: {name}: module missing from {}",
                    source.display()
                ));
            }
        }
    }
    let mut copied = BTreeSet::new();
    while let Some(path) = pending.pop() {
        if !copied.insert(path) {
            continue;
        }
        if !Path::new(path)
            .components()
            .all(|part| matches!(part, Component::Normal(_)))
        {
            return Err(format!("boot.initrd: invalid module path {path:?}"));
        }
        let from = source.join(path);
        let into = target.join(path);
        let parent = into.parent().unwrap();
        io("create", parent, fs::create_dir_all(parent))?;
        io("copy", &from, fs::copy(&from, &into))?;
        let dependencies = deps
            .get(path)
            .ok_or_else(|| format!("{path}: missing from modules.dep"))?;
        pending.extend(dependencies.split_whitespace());
    }
    for name in [
        "modules.builtin",
        "modules.builtin.modinfo",
        "modules.order",
    ] {
        let from = source.join(name);
        if from.is_file() {
            io("copy", &from, fs::copy(&from, target.join(name)))?;
        }
    }
    run_tool(
        settings,
        &[
            format!("{kmod}/bin/depmod"),
            "-b".to_owned(),
            stage.display().to_string(),
            release.to_str().ok_or("invalid kernel release")?.to_owned(),
        ],
        None,
    )
}

fn normalize(path: &Path) -> PathBuf {
    let mut result = PathBuf::new();
    for part in path.components() {
        match part {
            Component::ParentDir => {
                result.pop();
            }
            Component::CurDir => {}
            part => result.push(part),
        }
    }
    result
}

struct Executables<'a> {
    stage: &'a Path,
    store: &'a Path,
    copied: BTreeSet<PathBuf>,
}

impl Executables<'_> {
    fn source(&self, path: &Path) -> Result<PathBuf, String> {
        let mut target = normalize(path);
        for _ in 0..40 {
            let relative = target
                .strip_prefix(STORE)
                .map_err(|_| format!("initrd: {}: expected a store path", target.display()))?;
            let source = self.store.join(relative);
            if source.is_symlink() {
                let link = io("readlink", &source, fs::read_link(&source))?;
                target = normalize(&target.parent().unwrap().join(link));
            } else {
                return Ok(source);
            }
        }
        Err(format!(
            "initrd: {}: too many symbolic links",
            path.display()
        ))
    }

    fn copy(&mut self, path: &Path) -> Result<(), String> {
        let path = normalize(path);
        if !self.copied.insert(path.clone()) {
            return Ok(());
        }
        let source = self.source(&path)?;
        let data = io("read", &source, fs::read(&source))?;
        let elf =
            elf::dynamic(&data)?.ok_or_else(|| format!("{}: expected ELF", source.display()))?;
        let mode = io("stat", &source, fs::metadata(&source))?.permissions();
        use std::os::unix::fs::PermissionsExt as _;
        write_file(
            &self.stage.join(path.strip_prefix("/").unwrap()),
            &data,
            mode.mode() & 0o777,
        )?;
        let mut musl = false;
        if let Some(range) = elf.interpreter {
            let interpreter = std::str::from_utf8(&data[range]).map_err(|e| e.to_string())?;
            musl = interpreter.contains("/ld-musl-");
            self.copy(Path::new(interpreter))?;
        }
        let origin = path
            .parent()
            .unwrap()
            .to_str()
            .ok_or("initrd: invalid ELF path")?;
        let mut search = Vec::new();
        for range in elf
            .runpath
            .iter()
            .chain(elf.search_paths.iter().filter(|_| elf.runpath.is_none()))
        {
            let dirs = std::str::from_utf8(&data[range.clone()]).map_err(|e| e.to_string())?;
            search.extend(dirs.split(':').map(|dir| {
                PathBuf::from(dir.replace("${ORIGIN}", origin).replace("$ORIGIN", origin))
            }));
        }
        for range in elf.needed {
            let name = std::str::from_utf8(&data[range]).map_err(|e| e.to_string())?;
            if name == "libc.so"
                && (musl
                    || self.copied.iter().any(|p| {
                        p.file_name()
                            .and_then(|n| n.to_str())
                            .is_some_and(|n| n.starts_with("ld-musl-"))
                    }))
            {
                continue;
            }
            let dependency = if name.starts_with('/') {
                Some(PathBuf::from(name))
            } else {
                search
                    .iter()
                    .map(|dir| dir.join(name))
                    .find(|file| self.source(file).is_ok_and(|source| source.is_file()))
            };
            let dependency = dependency.ok_or_else(|| {
                format!(
                    "initrd: {}: unresolved ELF dependency {name}",
                    path.display()
                )
            })?;
            self.copy(&dependency)?;
        }
        Ok(())
    }

    fn link(&mut self, program: &str, alias: &str) -> Result<(), String> {
        self.copy(Path::new(program))?;
        symlink(program, &self.stage.join("bin").join(alias))
    }
}

pub(super) fn build(
    settings: &Settings,
    builder: &mut Builder,
    config: &Config,
    kernel: &str,
    busybox: &Path,
    work: &Path,
    firmware: &super::firmware::Files,
) -> Result<String, String> {
    let stage = work.join("initrd");
    let store = settings.data_dir.join("store");
    for dir in ["bin", "dev", "proc", "sys", "run", "new-root"] {
        let dir = stage.join(dir);
        io("create", &dir, fs::create_dir_all(&dir))?;
    }
    let shell = stage.join("bin/busybox");
    write_file(&shell, &io("read", busybox, fs::read(busybox))?, 0o755)?;
    for applet in ["cat", "cut", "mount", "sh", "sleep", "switch_root", "tr"] {
        symlink("busybox", &stage.join("bin").join(applet))?;
    }
    let package = |builder: &mut Builder, name| {
        builder
            .build(name, None)
            .map(|built| format!("{STORE}/{}", built.id))
    };
    let kmod = package(builder, "kmod")?;
    let systemd = package(builder, "systemd")?;
    let util = package(builder, "util-linux")?;
    let gzip = package(builder, "gzip")?;
    let mut executables = Executables {
        stage: &stage,
        store: &store,
        copied: BTreeSet::new(),
    };
    executables.link(&format!("{kmod}/bin/kmod"), "modprobe")?;
    executables.copy(Path::new(&format!("{kmod}/lib/libkmod.so.2")))?;
    executables.link(&format!("{systemd}/bin/udevadm"), "udevadm")?;
    executables.link(&format!("{systemd}/bin/udevadm"), "systemd-udevd")?;
    executables.link(&format!("{util}/sbin/blkid"), "blkid")?;
    let rules = store
        .join(systemd.strip_prefix("/vug/store/").unwrap())
        .join("lib/udev/rules.d/80-drivers.rules");
    write_file(
        &stage.join("etc/udev/rules.d/80-drivers.rules"),
        &io("read", &rules, fs::read(&rules))?,
        0o644,
    )?;
    install_modules(settings, &stage, &store.join(kernel), config, &kmod)?;
    super::firmware::copy_selected(firmware, &config.firmware, &stage.join("lib/firmware"))?;
    write_file(&stage.join("init"), include_bytes!("stage1.sh"), 0o755)?;
    let force: String = config
        .force
        .iter()
        .map(|name| format!("{name}\n"))
        .collect();
    write_file(&stage.join("etc/force-modules"), force.as_bytes(), 0o644)?;

    let raw = work.join("initrd.cpio");
    let mut archive = io("create", &raw, fs::File::create(&raw))?;
    io("archive", &raw, cpio::write_newc(&stage, &mut archive))?;
    let destination = work.join("out/boot/initrd.gz");
    let compressed = io("create", &destination, fs::File::create(&destination))?;
    let status = build::store_command(settings, false)?
        .args([&format!("{gzip}/bin/gzip"), "-n", "-c"])
        .stdin(io("open", &raw, fs::File::open(&raw))?)
        .stdout(Stdio::from(compressed))
        .status()
        .map_err(|e| format!("gzip: {e}"))?;
    if !status.success() {
        return Err(format!("gzip: {status}"));
    }
    let data = io("read", &destination, fs::read(&destination))?;
    Ok(format!("{}.initrd.gz", blake3::hash(&data)))
}
