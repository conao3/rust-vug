use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde::Deserialize;

fn io<T>(path: &Path, result: std::io::Result<T>) -> Result<T, String> {
    result.map_err(|error| format!("{}: {error}", path.display()))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Source {
    document: PathBuf,
    field: Vec<String>,
}

pub(crate) fn private(path: &Path, directory: bool) -> Result<(), String> {
    let metadata = io(path, fs::symlink_metadata(path))?;
    let valid = if directory {
        metadata.is_dir() && metadata.mode() & 0o777 == 0o700
    } else {
        metadata.is_file() && metadata.mode() & 0o777 == 0o600
    };
    if !valid || metadata.uid() != unsafe { libc::geteuid() } {
        return Err(format!(
            "{}: expected a private path owned by the current user",
            path.display()
        ));
    }
    Ok(())
}

pub(crate) struct Sources {
    key: PathBuf,
    sources: BTreeMap<String, Source>,
}

impl Sources {
    pub(crate) fn open(config: &Path) -> Result<Self, String> {
        private(config, true)?;
        let key = config.join("age.key");
        private(&key, false)?;
        let mapping = config.join("sources.toml");
        private(&mapping, false)?;
        let sources = toml::from_str(&io(&mapping, fs::read_to_string(&mapping))?)
            .map_err(|_| format!("{}: invalid secret source map", mapping.display()))?;
        Ok(Self { key, sources })
    }

    pub(crate) fn decrypt(&self, name: &str, destination: &Path) -> Result<(), String> {
        let source = self
            .sources
            .get(name)
            .ok_or_else(|| format!("missing source for secret {name:?}"))?;
        if !source.document.is_absolute()
            || source.field.is_empty()
            || source.field.iter().any(|key| {
                key.is_empty()
                    || !key
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || b"_-".contains(&byte))
            })
        {
            return Err(format!("invalid source for secret {name:?}"));
        }
        let extract = source
            .field
            .iter()
            .map(|key| format!("[\"{key}\"]"))
            .collect::<String>();
        let file = io(
            destination,
            OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(destination),
        )?;
        let status = Command::new("/bin/sops")
            .env_clear()
            .env("SOPS_AGE_KEY_FILE", &self.key)
            .args(["decrypt", "--extract", &extract, "--"])
            .arg(&source.document)
            .stdin(Stdio::null())
            .stdout(file)
            .stderr(Stdio::null())
            .status()
            .map_err(|error| format!("could not run /bin/sops: {error}"))?;
        if !status.success() {
            return Err(format!("could not decrypt secret {name:?}"));
        }
        Ok(())
    }
}
