use std::collections::{BTreeMap, BTreeSet, btree_map::Entry};
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::naming::model;
use crate::record::{FileCheck, RecordError, error_at};

pub(crate) mod output;

#[derive(Clone, Copy)]
pub(crate) enum Kind {
    Home,
    Fragment,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct HomeRecord {
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    fragments: Option<BTreeSet<String>>,
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    packages: BTreeSet<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    environment: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    file: Vec<HomeFile>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    unit: Vec<Unit>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    secret: Vec<Secret>,
}

#[derive(Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct HomeFile {
    path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    package: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    source: Option<String>,
}

#[derive(Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct Unit {
    name: String,
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    enable: BTreeSet<String>,
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    secrets: BTreeSet<String>,
    content: String,
}

#[derive(Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct Secret {
    name: String,
}

#[derive(Default)]
struct Parts {
    packages: BTreeSet<String>,
    environment: BTreeMap<String, String>,
    files: BTreeMap<String, HomeFile>,
    units: BTreeMap<String, Unit>,
    secrets: BTreeMap<String, Secret>,
}

fn valid_relative_path(path: &str) -> bool {
    !path.contains('\0')
        && path
            .split('/')
            .all(|component| !matches!(component, "" | "." | ".."))
}

fn valid_environment_name(name: &str) -> bool {
    let mut chars = name.bytes();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == b'_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == b'_')
}

fn unit_suffix(name: &str) -> Option<&str> {
    let (stem, suffix) = name.rsplit_once('.')?;
    if name.len() > 255 || stem.is_empty() || stem.starts_with('@') || stem.matches('@').count() > 1
    {
        return None;
    }
    let mut bytes = stem.bytes();
    while let Some(c) = bytes.next() {
        match c {
            c if c.is_ascii_alphanumeric() || b":_.-@".contains(&c) => {}
            b'\\'
                if bytes.next() == Some(b'x')
                    && bytes.next().is_some_and(|c| c.is_ascii_hexdigit())
                    && bytes.next().is_some_and(|c| c.is_ascii_hexdigit()) => {}
            _ => return None,
        }
    }
    Some(suffix)
}

fn validate_file(file: &HomeFile) -> Result<(), String> {
    if !valid_relative_path(&file.path) {
        return Err(format!(
            "file.path: {:?}: expected a normalized relative path",
            file.path
        ));
    }
    let path = Path::new(&file.path);
    for reserved in [
        Path::new(".local/state/vug"),
        Path::new(".local/share/vug/home"),
        Path::new(".config/systemd/user"),
    ] {
        if path.starts_with(reserved) || reserved.starts_with(path) {
            return Err(format!(
                "file.path: {:?}: overlaps reserved path {:?}",
                file.path, reserved
            ));
        }
    }
    match (&file.content, &file.package, &file.source) {
        (Some(_), None, None) => Ok(()),
        (None, Some(package), Some(source))
            if model::valid_name(package) && valid_relative_path(source) =>
        {
            Ok(())
        }
        _ => Err(format!(
            "file {:?}: expected content or a valid package and normalized relative source",
            file.path
        )),
    }
}

fn validate_unit(unit: &Unit) -> Result<(), String> {
    if !matches!(
        unit_suffix(&unit.name),
        Some(
            "service"
                | "socket"
                | "device"
                | "mount"
                | "automount"
                | "swap"
                | "target"
                | "path"
                | "timer"
                | "slice"
                | "scope"
        )
    ) {
        return Err(format!("unit.name: {:?}: invalid unit name", unit.name));
    }
    for target in &unit.enable {
        if unit_suffix(target) != Some("target") {
            return Err(format!("unit {:?}: invalid target {target:?}", unit.name));
        }
    }
    for secret in &unit.secrets {
        if !model::valid_name(secret) {
            return Err(format!(
                "unit {:?}: invalid secret name {secret:?}",
                unit.name
            ));
        }
    }
    if unit.content.contains('\0') {
        return Err(format!("unit {:?}: content contains a NUL byte", unit.name));
    }
    Ok(())
}

fn merge_definition<T: Eq>(
    entries: &mut BTreeMap<String, T>,
    field: &str,
    key: String,
    value: T,
) -> Result<(), String> {
    match entries.entry(key) {
        Entry::Vacant(entry) => {
            entry.insert(value);
            Ok(())
        }
        Entry::Occupied(entry) if entry.get() == &value => Ok(()),
        Entry::Occupied(entry) => Err(format!(
            "{field}: conflicting definitions for {:?}",
            entry.key()
        )),
    }
}

impl Parts {
    fn insert(&mut self, record: HomeRecord) -> Result<(), String> {
        for package in &record.packages {
            if !model::valid_name(package) {
                return Err(format!("packages: {package:?}: invalid package name"));
            }
        }
        self.packages.extend(record.packages);
        for (name, value) in record.environment {
            if !valid_environment_name(&name) || value.contains('\0') {
                return Err(format!(
                    "environment: {name:?}: invalid name or NUL in value"
                ));
            }
            merge_definition(&mut self.environment, "environment", name, value)?;
        }
        for file in record.file {
            validate_file(&file)?;
            merge_definition(&mut self.files, "file.path", file.path.clone(), file)?;
        }
        for unit in record.unit {
            validate_unit(&unit)?;
            merge_definition(&mut self.units, "unit.name", unit.name.clone(), unit)?;
        }
        for secret in record.secret {
            if !model::valid_name(&secret.name) {
                return Err(format!(
                    "secret.name: {:?}: invalid secret name",
                    secret.name
                ));
            }
            merge_definition(
                &mut self.secrets,
                "secret.name",
                secret.name.clone(),
                secret,
            )?;
        }
        Ok(())
    }

    fn check_paths(&self) -> Result<(), String> {
        for path in self.files.keys() {
            for (index, _) in path.match_indices('/') {
                let parent = &path[..index];
                if self.files.contains_key(parent) {
                    return Err(format!(
                        "file.path: {path:?} is inside listed path {parent:?}"
                    ));
                }
            }
        }
        Ok(())
    }

    fn check_references(&self) -> Result<(), String> {
        for file in self.files.values() {
            if let Some(package) = &file.package
                && !self.packages.contains(package)
            {
                return Err(format!(
                    "file {:?}: package {package:?} is not selected",
                    file.path
                ));
            }
        }
        for unit in self.units.values() {
            for secret in &unit.secrets {
                if !self.secrets.contains_key(secret) {
                    return Err(format!(
                        "unit {:?}: secret {secret:?} is not declared",
                        unit.name
                    ));
                }
            }
        }
        Ok(())
    }

    fn into_record(self, name: String, fragments: Option<BTreeSet<String>>) -> HomeRecord {
        HomeRecord {
            name,
            fragments: fragments.filter(|names| !names.is_empty()),
            packages: self.packages,
            environment: self.environment,
            file: self.files.into_values().collect(),
            unit: self.units.into_values().collect(),
            secret: self.secrets.into_values().collect(),
        }
    }
}

impl HomeRecord {
    fn read(path: &Path, kind: Kind) -> Result<(String, Self), RecordError> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| error_at(path, format!("cannot read: {e}")))?;
        let record: Self =
            toml::from_str(&text).map_err(|e| error_at(path, format!("invalid record: {e}")))?;
        if !model::valid_name(&record.name)
            || path.file_name().and_then(|file| file.to_str())
                != Some(&format!("{}.toml", record.name))
        {
            return Err(error_at(
                path,
                format!(
                    "name: {:?}: expected a valid name matching the file name",
                    record.name
                ),
            ));
        }
        if let Some(fragments) = &record.fragments {
            if matches!(kind, Kind::Fragment) {
                return Err(error_at(
                    path,
                    "fragments: unknown key in a home fragment".to_owned(),
                ));
            }
            for name in fragments {
                if !model::valid_name(name) {
                    return Err(error_at(
                        path,
                        format!("fragments: {name:?}: invalid fragment name"),
                    ));
                }
            }
        }
        Ok((text, record))
    }

    pub(crate) fn canonical(&self) -> Result<String, toml::ser::Error> {
        toml::to_string_pretty(self)
    }
}

pub(crate) fn check_file(path: &Path, kind: Kind) -> Result<FileCheck, RecordError> {
    let (text, mut record) = HomeRecord::read(path, kind)?;
    let name = record.name.clone();
    let fragments = record.fragments.take();
    let mut parts = Parts::default();
    parts.insert(record).map_err(|e| error_at(path, e))?;
    parts.check_paths().map_err(|e| error_at(path, e))?;
    let canonical = parts
        .into_record(name, fragments)
        .canonical()
        .map_err(|e| error_at(path, e.to_string()))?;
    Ok(FileCheck {
        changed: canonical != text,
        canonical,
    })
}

pub(crate) fn load(root: &Path, name: &str) -> Result<HomeRecord, RecordError> {
    let homes = root.join("homes");
    if !model::valid_name(name) {
        return Err(error_at(
            &homes,
            format!("name: {name:?}: invalid home name"),
        ));
    }
    let path = homes.join(format!("{name}.toml"));
    let (_, mut record) = HomeRecord::read(&path, Kind::Home)?;
    let fragments = record.fragments.take().unwrap_or_default();
    let mut parts = Parts::default();
    parts.insert(record).map_err(|e| error_at(&path, e))?;
    parts.check_paths().map_err(|e| error_at(&path, e))?;
    for fragment in fragments {
        let fragment_path = root.join("home-fragments").join(format!("{fragment}.toml"));
        let (_, record) = HomeRecord::read(&fragment_path, Kind::Fragment)?;
        parts
            .insert(record)
            .map_err(|e| error_at(&fragment_path, e))?;
        parts
            .check_paths()
            .map_err(|e| error_at(&fragment_path, e))?;
    }
    parts.check_references().map_err(|e| error_at(&path, e))?;
    Ok(parts.into_record(name.to_owned(), None))
}
