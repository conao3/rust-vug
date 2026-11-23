use std::fs::{self, Permissions};
use std::os::unix::fs::{MetadataExt, PermissionsExt, chown};
use std::path::Path;

use super::{FileKind, SystemFile, io, number, string, tables, unknown_keys};
use crate::naming::model;
use crate::secret::Sources;

const RUNTIME: &str = "/run/vug/secrets";

fn file(path: String, content: String) -> SystemFile {
    SystemFile {
        path: format!("systemd/system/{path}"),
        kind: FileKind::Link,
        mode: 0o644,
        content,
        target: None,
    }
}

pub(super) fn files(table: &toml::Table) -> Result<Vec<SystemFile>, String> {
    let mut files = Vec::new();
    for entry in tables(table, "secret")? {
        unknown_keys(entry, &["name", "uid", "unit"], "secret.")?;
        let name = string(entry, "name", "secret.")?;
        let uid = number(entry, "uid", "secret.")?;
        let unit = string(entry, "unit", "secret.")?;
        if !model::valid_name(&name) || uid == u32::MAX {
            return Err("secret: expected a logical name and a valid UID".to_owned());
        }
        if !unit.strip_suffix(".service").is_some_and(|stem| {
            !stem.is_empty()
                && stem
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
                && !stem.starts_with("vug-secret-")
        }) {
            return Err(format!("secret.unit: invalid service name {unit:?}"));
        }
        let prepare = format!("vug-secret-{name}.service");
        files.push(file(
            prepare.clone(),
            format!(
                "[Service]\nType=oneshot\nRemainAfterExit=yes\nRuntimeDirectory=vug/secrets/{name}\nRuntimeDirectoryMode=0755\nExecStart=/bin/vug system prepare-secret --name {name} --uid {uid}\n"
            ),
        ));
        files.push(file(
            format!("{unit}.d/50-vug-secret-{name}.conf"),
            format!(
                "[Unit]\nRequires={prepare}\nAfter={prepare}\nPartOf={prepare}\n\n[Service]\nLoadCredential={name}:{RUNTIME}/{name}/value\n"
            ),
        ));
    }
    Ok(files)
}

pub(crate) fn prepare(name: &str, uid: u32) -> Result<(), String> {
    if unsafe { libc::geteuid() } != 0 || !model::valid_name(name) || uid == u32::MAX {
        return Err(
            "preparing a system secret requires root, a logical name and a valid UID".to_owned(),
        );
    }
    let root = Path::new(RUNTIME).join(name);
    for path in root.ancestors().take(4) {
        let metadata = io("stat", path, fs::symlink_metadata(path))?;
        if !metadata.is_dir() || metadata.uid() != 0 || metadata.mode() & 0o022 != 0 {
            return Err(format!(
                "{}: expected a root-controlled directory",
                path.display()
            ));
        }
    }
    let temp = io(
        "create temporary directory",
        &root,
        tempfile::Builder::new()
            .permissions(Permissions::from_mode(0o700))
            .tempdir_in(&root),
    )?;
    let source = temp.path().join("value");
    Sources::open(Path::new("/etc/vug/secrets"))?.decrypt(name, &source)?;
    io(
        "set secret owner",
        &source,
        chown(&source, Some(uid), Some(0)),
    )?;
    io(
        "publish secret",
        &root,
        fs::rename(&source, root.join("value")),
    )?;
    Ok(())
}
