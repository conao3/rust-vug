use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::process::Command;

use serde::{Deserialize, Serialize};

pub(super) type Units = BTreeMap<String, (Vec<u8>, BTreeSet<String>, BTreeSet<String>)>;

#[derive(Default, Deserialize, Serialize)]
pub(super) struct Changes {
    stopped: BTreeSet<String>,
    restarted: BTreeSet<String>,
    started: BTreeSet<String>,
}

impl Changes {
    pub(super) fn plan(
        old: &Units,
        new: &Units,
        environment_changed: bool,
        credentials_changed: bool,
    ) -> Result<Self, String> {
        let mut changes = Self::default();
        for (name, (content, _, _)) in old {
            if !active(name)? {
                continue;
            }
            match new.get(name) {
                None => {
                    changes.stopped.insert(name.clone());
                }
                Some((next, _, secrets))
                    if next != content
                        || (credentials_changed && !secrets.is_empty())
                        || (environment_changed && name.ends_with(".service")) =>
                {
                    changes.restarted.insert(name.clone());
                }
                Some(_) => (),
            }
        }
        for (name, (_, targets, _)) in new {
            let mut enabled = false;
            for target in targets {
                if !old
                    .get(name)
                    .is_some_and(|(_, targets, _)| targets.contains(target))
                    && active(target)?
                {
                    enabled = true;
                    break;
                }
            }
            if enabled && !active(name)? {
                changes.started.insert(name.clone());
            }
        }
        Ok(changes)
    }

    pub(super) fn before(&self) -> Result<(), String> {
        units("stop", &self.stopped)
    }

    pub(super) fn after(&self) -> Result<(), String> {
        systemctl(&["daemon-reload"])?;
        units("restart", &self.restarted)?;
        units("start", &self.started)
    }

    pub(super) fn restore(&self) -> Result<(), String> {
        for name in &self.started {
            if active(name)? {
                systemctl(&["stop", "--", name])?;
            }
        }
        systemctl(&["daemon-reload"])?;
        units("restart", &self.restarted)?;
        units("start", &self.stopped)
    }
}

pub(super) fn available(home: &Path) -> Result<bool, String> {
    let socket = std::env::var_os("XDG_RUNTIME_DIR")
        .map(std::path::PathBuf::from)
        .is_some_and(|path| path.join("systemd/private").exists());
    if !socket {
        return Ok(false);
    }
    let config = home.join(".config/systemd/user");
    let paths = systemctl(&["show", "--property=UnitPath", "--value"])?;
    Ok(paths
        .split_ascii_whitespace()
        .any(|path| Path::new(path) == config))
}

fn active(name: &str) -> Result<bool, String> {
    let state = systemctl(&["show", "--property=ActiveState", "--value", "--", name])?;
    Ok(matches!(
        state.trim(),
        "active" | "activating" | "reloading"
    ))
}

fn units(action: &str, units: &BTreeSet<String>) -> Result<(), String> {
    if !units.is_empty() {
        let mut arguments = vec![action, "--"];
        arguments.extend(units.iter().map(String::as_str));
        systemctl(&arguments)?;
    }
    Ok(())
}

fn systemctl(arguments: &[&str]) -> Result<String, String> {
    let output = Command::new("systemctl")
        .arg("--user")
        .args(arguments)
        .output()
        .map_err(|error| format!("systemctl --user: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "systemctl --user {}: {}",
            arguments.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}
