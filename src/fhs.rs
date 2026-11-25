use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::{ErrorKind, Result as IoResult, Write};
use std::os::unix::fs::{FileTypeExt, PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Serialize};
use vug::nix_cache::StorePath;

use crate::record::{Fhs, fhs_relative_path};

const MANIFEST: &str = ".vug/fhs.json";
const USR: &str = ".vug/fhs/usr";

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    program: String,
    i686: bool,
    graphics: bool,
}

pub(crate) struct Runtime {
    usr: PathBuf,
    manifest: Manifest,
}

fn io<T>(path: &Path, result: IoResult<T>) -> Result<T, String> {
    result.map_err(|error| format!("{}: {error}", path.display()))
}

pub(crate) fn install(
    output: &Path,
    fhs: &Fhs,
    nix_store: &Path,
    paths: &BTreeMap<&str, StorePath>,
) -> Result<(), String> {
    executable(output, Path::new(&fhs.program))?;
    let metadata = output.join(".vug");
    directory(&metadata)?;
    let projection = metadata.join("fhs");
    io(&projection, fs::create_dir(&projection))?;
    let root = output.join(USR);
    let i686 = !fhs.runtime[1].is_empty();
    for lib in ["lib64"].into_iter().chain(i686.then_some("lib32")) {
        let directory = root.join(lib);
        io(&directory, fs::create_dir_all(&directory))?;
    }
    io(&root, symlink("lib64", root.join("lib")))?;
    for (_, _, mapping) in fhs.mappings() {
        let path = &paths[mapping.output.as_str()];
        let source = nix_store.join(path.basename());
        if !io(&source, fs::symlink_metadata(&source))?.is_dir() {
            return Err(format!(
                "{}: expected a Nix output directory",
                source.display()
            ));
        }
        let from = Path::new(&mapping.from);
        let mut parent = source.clone();
        for part in from.parent().unwrap().components() {
            parent.push(part);
            if !io(&parent, fs::symlink_metadata(&parent))?.is_dir() {
                return Err(format!(
                    "{}: expected a Nix output directory",
                    parent.display()
                ));
            }
        }
        let to = Path::new(&mapping.to);
        let mut target = projection.clone();
        for part in to.parent().unwrap().components() {
            target.push(part);
            directory(&target)?;
        }
        merge(
            &parent.join(from.file_name().unwrap()),
            &Path::new(path.as_str()).join(from),
            &target.join(to.file_name().unwrap()),
        )?;
    }
    for loader in ["lib64/ld-linux-x86-64.so.2"]
        .into_iter()
        .chain(i686.then_some("lib32/ld-linux.so.2"))
    {
        let path = root.join(loader);
        if !io(&path, fs::symlink_metadata(&path))?.is_symlink() {
            return Err(format!(
                "fhs: expected a Nix output providing /usr/{loader}"
            ));
        }
    }
    let path = output.join(MANIFEST);
    let manifest = Manifest {
        program: fhs.program.clone(),
        i686,
        graphics: fhs.graphics,
    };
    let bytes = serde_json::to_vec(&manifest).map_err(|error| error.to_string())?;
    let mut file = io(
        &path,
        OpenOptions::new().write(true).create_new(true).open(&path),
    )?;
    io(&path, file.write_all(&bytes))
}

fn directory(path: &Path) -> Result<(), String> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() => Ok(()),
        Ok(_) => Err(format!("{}: conflicting FHS paths", path.display())),
        Err(error) if error.kind() == ErrorKind::NotFound => io(path, fs::create_dir(path)),
        Err(error) => io(path, Err(error)),
    }
}

fn merge(source: &Path, inside: &Path, target: &Path) -> Result<(), String> {
    let metadata = io(source, fs::symlink_metadata(source))?;
    if metadata.is_dir() {
        directory(target)?;
        for entry in io(source, fs::read_dir(source))? {
            let entry = io(source, entry)?;
            merge(
                &entry.path(),
                &inside.join(entry.file_name()),
                &target.join(entry.file_name()),
            )?;
        }
        Ok(())
    } else if metadata.is_file() || metadata.is_symlink() {
        match symlink(inside, target) {
            Err(error) if error.kind() == ErrorKind::AlreadyExists => {
                Err(format!("{}: conflicting FHS paths", target.display()))
            }
            result => io(target, result),
        }
    } else {
        Err(format!("{}: unsupported FHS file type", source.display()))
    }
}

fn executable(output: &Path, relative: &Path) -> Result<PathBuf, String> {
    if !relative.to_str().is_some_and(fhs_relative_path) {
        return Err(format!(
            "{}: expected a relative FHS program",
            relative.display()
        ));
    }
    let root = io(output, output.canonicalize())?;
    let path = output.join(relative);
    let resolved = io(&path, path.canonicalize())?;
    let metadata = io(&resolved, fs::metadata(&resolved))?;
    if !resolved.starts_with(root)
        || !metadata.is_file()
        || metadata.permissions().mode() & 0o111 == 0
    {
        return Err(format!(
            "{}: expected an executable inside the prebuilt output",
            path.display()
        ));
    }
    Ok(resolved)
}

pub(crate) fn load(output: &Path) -> Result<Option<Runtime>, String> {
    let metadata = output.join(".vug");
    match fs::symlink_metadata(&metadata) {
        Ok(info) if info.is_dir() => {}
        Ok(_) => return Ok(None),
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => return io(&metadata, Err(error)),
    }
    let path = output.join(MANIFEST);
    match fs::symlink_metadata(&path) {
        Ok(metadata) if metadata.is_file() => {}
        Ok(_) => {
            return Err(format!(
                "{}: expected a regular FHS manifest",
                path.display()
            ));
        }
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => return io(&path, Err(error)),
    }
    let manifest = serde_json::from_slice(&io(&path, fs::read(&path))?)
        .map_err(|error| format!("{}: {error}", path.display()))?;
    let usr = output.join(USR);
    if !io(&usr, fs::symlink_metadata(&usr))?.is_dir() {
        return Err(format!("{}: expected a FHS directory", usr.display()));
    }
    Ok(Some(Runtime { usr, manifest }))
}

impl Runtime {
    pub(crate) fn program(
        &self,
        output: &Path,
        requested: &Path,
        store: &str,
        id: &str,
    ) -> Result<PathBuf, String> {
        if !cfg!(all(target_os = "linux", target_arch = "x86_64")) {
            return Err("fhs: requires an x86_64 Linux host".into());
        }
        let package = Path::new(store).join(id);
        let relative = requested.strip_prefix(&package).map_err(|_| {
            format!(
                "{}: expected a program in {}",
                requested.display(),
                package.display()
            )
        })?;
        let program = Path::new(&self.manifest.program);
        let resolved = executable(output, program)?;
        if relative != program && executable(output, relative)? != resolved {
            return Err(format!(
                "{}: expected FHS program {}",
                requested.display(),
                program.display()
            ));
        }
        Ok(package.join(program))
    }

    pub(crate) fn replaces(&self, path: &Path) -> bool {
        path.to_str().is_some_and(|path| {
            matches!(
                path,
                "/usr" | "/bin" | "/lib" | "/lib32" | "/lib64" | "/dev"
            )
        })
    }

    pub(crate) fn configure(&self, command: &mut Command) -> Result<(), String> {
        command.arg("--ro-bind").arg(&self.usr).arg("/usr");
        command.args([
            "--symlink",
            "usr/bin",
            "/bin",
            "--symlink",
            "usr/lib64",
            "/lib64",
            "--symlink",
            if self.manifest.i686 {
                "usr/lib32"
            } else {
                "usr/lib64"
            },
            "/lib",
            "--dev",
            "/dev",
            "--tmpfs",
            "/dev/shm",
            "--setenv",
            "PATH",
            "/usr/bin",
            "--setenv",
            "LD_LIBRARY_PATH",
            if self.manifest.i686 {
                "/usr/lib64:/usr/lib32"
            } else {
                "/usr/lib64"
            },
        ]);
        if self.manifest.i686 {
            command.args(["--symlink", "usr/lib32", "/lib32"]);
        }
        let cache = Path::new("/etc/ld.so.cache");
        if cache.is_file() {
            command.args(["--ro-bind", "/dev/null", "/etc/ld.so.cache"]);
        }
        if self.manifest.graphics {
            self.graphics(command)?;
        }
        Ok(())
    }

    fn graphics(&self, command: &mut Command) -> Result<(), String> {
        let dri = Path::new("/dev/dri");
        match fs::read_dir(dri) {
            Ok(entries) => {
                for entry in entries {
                    let path = io(dri, entry)?.path();
                    if io(&path, fs::symlink_metadata(&path))?
                        .file_type()
                        .is_char_device()
                        && rustix::fs::access(
                            &path,
                            rustix::fs::Access::READ_OK | rustix::fs::Access::WRITE_OK,
                        )
                        .is_ok()
                    {
                        command.arg("--dev-bind").arg(&path).arg(&path);
                    }
                }
            }
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(error) => return io(dri, Err(error)),
        }
        command.args([
            "--setenv",
            "LIBGL_DRIVERS_PATH",
            "/usr/lib64/dri:/usr/lib32/dri",
            "--setenv",
            "LIBVA_DRIVERS_PATH",
            "/usr/lib64/dri:/usr/lib32/dri",
            "--setenv",
            "__EGL_VENDOR_LIBRARY_DIRS",
            "/usr/share/glvnd/egl_vendor.d:/usr/lib32/glvnd/egl_vendor.d",
            "--setenv",
            "VK_DRIVER_FILES",
            "/usr/share/vulkan/icd.d:/usr/lib32/vulkan/icd.d",
            "--unsetenv",
            "VK_ICD_FILENAMES",
            "--setenv",
            "XDG_DATA_DIRS",
            "/usr/share:/usr/lib32",
        ]);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merges_directories_and_rejects_leaf_collisions() {
        let temp = tempfile::tempdir().unwrap();
        let first = temp.path().join("first");
        let second = temp.path().join("second");
        let target = temp.path().join("target");
        fs::create_dir(&first).unwrap();
        fs::create_dir(&second).unwrap();
        fs::write(first.join("one.so"), b"one").unwrap();
        fs::write(second.join("two.so"), b"two").unwrap();
        merge(&first, &first, &target).unwrap();
        merge(&second, &second, &target).unwrap();
        assert_eq!(fs::read(target.join("one.so")).unwrap(), b"one");
        assert_eq!(fs::read(target.join("two.so")).unwrap(), b"two");
        assert!(
            merge(&first, &first, &target)
                .unwrap_err()
                .contains("conflicting FHS paths")
        );
        assert!(
            directory(&target.join("one.so"))
                .unwrap_err()
                .contains("conflicting FHS paths")
        );
    }
}
