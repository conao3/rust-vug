use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{self, Read};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Component, Path, PathBuf};

use zip::ZipArchive;

enum Entry {
    Directory,
    File { index: usize, mode: u32 },
    Symlink(PathBuf),
}

fn portable(path: &Path) -> bool {
    let bytes = path.as_os_str().as_bytes();
    !bytes.is_empty() && !bytes.iter().any(|byte| matches!(byte, 0 | b'\\' | b':'))
}

pub(super) fn safe_target(path: &Path, target: &Path) -> bool {
    if !portable(target) {
        return false;
    }
    let mut depth = path.ancestors().count() - 2;
    let mut descended = false;
    for component in target.components() {
        match component {
            Component::CurDir => {}
            Component::Normal(_) => descended = true,
            Component::ParentDir if depth > 0 && !descended => depth -= 1,
            _ => return false,
        }
    }
    true
}

pub(crate) fn extract(source: &Path, destination: &Path, strip: u32) -> io::Result<()> {
    let mut archive = ZipArchive::new(File::open(source)?)?;
    let mut entries = BTreeMap::new();
    for index in 0..archive.len() {
        let mut file = archive.by_index(index)?;
        let name = file.name()?.into_owned();
        let name = Path::new(&name);
        let invalid = || {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("invalid ZIP path: {}", name.display()),
            )
        };
        if !portable(name)
            || name
                .components()
                .any(|component| !matches!(component, Component::Normal(_) | Component::CurDir))
        {
            return Err(invalid());
        }
        let path: PathBuf = name
            .components()
            .filter(|component| matches!(component, Component::Normal(_)))
            .skip(strip as usize)
            .collect();
        let entry = if file.is_symlink() {
            let mut bytes = Vec::new();
            (&mut file).take(4097).read_to_end(&mut bytes)?;
            let target = Path::new(std::ffi::OsStr::from_bytes(&bytes));
            if bytes.len() > 4096 || path.as_os_str().is_empty() || !safe_target(&path, target) {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("unsafe ZIP symlink: {}", name.display()),
                ));
            }
            Entry::Symlink(target.to_owned())
        } else if path.as_os_str().is_empty() {
            continue;
        } else if file.is_dir() {
            Entry::Directory
        } else {
            Entry::File {
                index,
                mode: file.unix_mode().unwrap_or(0o644) & 0o777,
            }
        };
        if entries.insert(path, entry).is_some() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "duplicate ZIP path after stripping components",
            ));
        }
    }
    for path in entries.keys() {
        for parent in path.ancestors().skip(1) {
            if let Some(Entry::File { .. } | Entry::Symlink(_)) = entries.get(parent) {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("ZIP entry parent is not a directory: {}", parent.display()),
                ));
            }
        }
    }
    for (path, entry) in entries {
        let output = destination.join(path);
        fs::create_dir_all(output.parent().unwrap())?;
        match entry {
            Entry::Directory => fs::create_dir_all(&output)?,
            Entry::File { index, mode } => {
                let mut file = File::create_new(&output)?;
                io::copy(&mut archive.by_index(index)?, &mut file)?;
                file.set_permissions(fs::Permissions::from_mode(mode))?;
            }
            Entry::Symlink(target) => symlink(target, output)?,
        }
    }
    Ok(())
}
