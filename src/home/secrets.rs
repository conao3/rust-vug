use std::collections::BTreeSet;
use std::fs::{self, DirBuilder, File};
use std::io::ErrorKind;
use std::os::unix::fs::{DirBuilderExt, PermissionsExt, symlink};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::output::{io, lock, read_link};
use crate::naming::model;
use crate::secret::{Sources, private};

pub(super) const UNIT: &str = "vug-home-secrets.service";

#[derive(Deserialize, Serialize)]
pub(super) struct Change {
    previous: Option<PathBuf>,
    pub(super) next: PathBuf,
}

pub(super) struct Runtime {
    root: PathBuf,
    _lock: File,
}

impl Runtime {
    pub(super) fn open() -> Result<Self, String> {
        let base =
            PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR").ok_or("XDG_RUNTIME_DIR is not set")?);
        if !base.is_absolute() {
            return Err("XDG_RUNTIME_DIR must be absolute".to_owned());
        }
        private(&base, true)?;
        let root = base.join("vug");
        match DirBuilder::new().mode(0o700).create(&root) {
            Ok(()) => (),
            Err(error) if error.kind() == ErrorKind::AlreadyExists => (),
            Err(error) => return io(&root, Err(error)),
        }
        private(&root, true)?;
        let guard = lock(&root.join("secrets.lock"))?;
        Ok(Self { root, _lock: guard })
    }

    fn path(&self, snapshot: &Path) -> Result<PathBuf, String> {
        let name = snapshot
            .to_str()
            .ok_or("invalid credential directory name")?;
        if !name.strip_prefix("secrets-").is_some_and(|suffix| {
            !suffix.is_empty() && suffix.bytes().all(|byte| byte.is_ascii_alphanumeric())
        }) {
            return Err("invalid credential directory name".to_owned());
        }
        Ok(self.root.join(snapshot))
    }

    fn current(&self) -> Result<Option<PathBuf>, String> {
        let target = read_link(&self.root.join("secrets"))?;
        if let Some(target) = &target {
            private(&self.path(target)?, true)?;
        }
        Ok(target)
    }

    pub(super) fn prepare(
        &self,
        home: &Path,
        output: &Path,
        names: &BTreeSet<String>,
    ) -> Result<Change, String> {
        let previous = self.current()?;
        let dir = io(
            &self.root,
            tempfile::Builder::new()
                .permissions(fs::Permissions::from_mode(0o700))
                .prefix("secrets-")
                .tempdir_in(&self.root),
        )?;
        private(dir.path(), true)?;
        if !names.is_empty() {
            let config = home.join(".config/vug/home/secrets");
            let sources = Sources::open(&config)?;
            for name in names {
                if !model::valid_name(name) {
                    return Err("invalid logical secret name in home manifest".to_owned());
                }
                sources.decrypt(name, &dir.path().join(name))?;
            }
        }
        let marker = dir.path().join(".output");
        io(
            &marker,
            fs::write(&marker, output.as_os_str().as_encoded_bytes()),
        )?;
        let next = dir.keep().file_name().unwrap().into();
        Ok(Change { previous, next })
    }

    fn select(&self, target: Option<&Path>) -> Result<(), String> {
        let link = self.root.join("secrets");
        match target {
            Some(target) => {
                private(&self.path(target)?, true)?;
                let temp = io(&self.root, tempfile::tempdir_in(&self.root))?;
                let replacement = temp.path().join("link");
                io(&replacement, symlink(target, &replacement))?;
                io(&link, fs::rename(replacement, &link))
            }
            None => match fs::remove_file(&link) {
                Ok(()) => Ok(()),
                Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
                Err(error) => io(&link, Err(error)),
            },
        }
    }

    pub(super) fn publish(&self, change: &Change) -> Result<(), String> {
        self.select(Some(&change.next))
    }

    fn remove(&self, target: &Path) -> Result<(), String> {
        let path = self.path(target)?;
        match fs::remove_dir_all(&path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
            Err(error) => io(&path, Err(error)),
        }
    }

    pub(super) fn finish(&self, change: &Change, committed: bool) -> Result<(), String> {
        let current = self.current()?;
        if current.as_ref() == Some(&change.next) {
            if committed {
                if let Some(previous) = &change.previous {
                    self.remove(previous)?;
                }
            } else {
                self.select(change.previous.as_deref())?;
            }
        }
        if !committed {
            self.remove(&change.next)?;
        }
        Ok(())
    }

    pub(super) fn login(&self, home: &Path) -> Result<(), String> {
        let profile = home.join(".config/vug/home/profile");
        let output = io(&profile, fs::canonicalize(&profile))?;
        if let Some(current) = self.current()? {
            let marker = self.path(&current)?.join(".output");
            if io(&marker, fs::read(&marker))? == output.as_os_str().as_encoded_bytes() {
                return Ok(());
            }
        }
        let names = super::activate::secret_names(&output)?;
        let change = self.prepare(home, &output, &names)?;
        if let Err(error) = self.publish(&change) {
            self.finish(&change, false)?;
            return Err(error);
        }
        self.finish(&change, true)
    }
}
