use std::collections::BTreeSet;

use super::{Errors, VersionRecord, dependency_output, model, req_string, req_table, unknown_keys};
use super::{write_bool_field, write_escaped, write_string_field};

const SYSTEMS: [(&str, &str); 2] = [
    ("runtime-x86_64", "x86_64-linux"),
    ("runtime-i686", "i686-linux"),
];

pub(crate) struct Fhs {
    pub(crate) program: String,
    pub(crate) runtime: [Vec<Mapping>; 2],
    pub(crate) graphics: bool,
}

#[derive(Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct Mapping {
    pub(crate) output: String,
    pub(crate) from: String,
    pub(crate) to: String,
}

impl Fhs {
    pub(crate) fn mappings(&self) -> impl Iterator<Item = (&str, &str, &Mapping)> {
        SYSTEMS
            .into_iter()
            .zip(&self.runtime)
            .flat_map(|((field, system), entries)| {
                entries.iter().map(move |entry| (field, system, entry))
            })
    }

    pub(crate) fn dependencies(&self) -> impl Iterator<Item = (String, &str)> {
        self.mappings()
            .map(|(field, _, entry)| (format!("fhs.{field}"), entry.output.as_str()))
            .collect::<BTreeSet<_>>()
            .into_iter()
    }
}

pub(crate) fn runtime_path<'a>(
    record: &'a VersionRecord,
    selector: &str,
    system: &str,
) -> Result<&'a vug::nix_cache::StorePath, String> {
    let (_, output) = dependency_output(selector);
    match &record.build {
        super::BuildOptions::NixOutput(nix) if nix.system == system => nix
            .outputs
            .get(output)
            .map(|entry| &entry.path)
            .ok_or_else(|| format!("{selector}: unknown Nix output")),
        _ => Err(format!("{selector}: expected a nix-output for {system}")),
    }
}

pub(crate) fn relative_path(path: &str) -> bool {
    path.split('/').all(|part| {
        !matches!(part, "" | "." | "..")
            && !part
                .chars()
                .any(|c| c.is_control() || matches!(c, '*' | '?' | '\\'))
    })
}

pub(super) fn parse(map: &toml::Table, errors: &mut Errors) -> Option<Fhs> {
    if !map.contains_key("fhs") {
        return None;
    }
    let table = req_table(map, "fhs", "", errors)?;
    unknown_keys(
        table,
        &["program", "runtime-x86_64", "runtime-i686", "graphics"],
        "fhs.",
        errors,
    );
    let program = req_string(table, "program", "fhs.", errors).unwrap_or_default();
    if !relative_path(&program) || matches!(program.split('/').next(), Some(".vug")) {
        errors.push("fhs.program: expected a relative payload path".into());
    }
    let runtime = SYSTEMS.map(|(field, _)| mappings(table, field, errors));
    let graphics = if table.contains_key("graphics") {
        super::req_bool(table, "graphics", "fhs.", errors).unwrap_or_default()
    } else {
        false
    };
    Some(Fhs {
        program,
        runtime,
        graphics,
    })
}

fn mappings(table: &toml::Table, field: &str, errors: &mut Errors) -> Vec<Mapping> {
    let Some(value) = table.get(field) else {
        if field == "runtime-x86_64" {
            errors.push(format!("fhs.{field}: required key missing"));
        }
        return Vec::new();
    };
    let Some(entries) = value.as_array() else {
        errors.push(format!("fhs.{field}: expected an array of tables"));
        return Vec::new();
    };
    if entries.is_empty() && field == "runtime-x86_64" {
        errors.push(format!("fhs.{field}: expected at least one mapping"));
    }
    let mut mappings = Vec::new();
    for (index, entry) in entries.iter().enumerate() {
        let prefix = format!("fhs.{field}[{index}].");
        let Some(entry) = entry.as_table() else {
            errors.push(format!("fhs.{field}[{index}]: expected a table"));
            continue;
        };
        unknown_keys(entry, &["output", "from", "to"], &prefix, errors);
        let output = req_string(entry, "output", &prefix, errors).unwrap_or_default();
        let (package, name) = dependency_output(&output);
        if !output.contains(':') || !model::valid_name(package) || !model::valid_name(name) {
            errors.push(format!(
                "{prefix}output: expected an explicit package:output selector"
            ));
        }
        let from = req_string(entry, "from", &prefix, errors).unwrap_or_default();
        if !relative_path(&from) {
            errors.push(format!(
                "{prefix}from: expected a relative path in the Nix output"
            ));
        }
        let to = req_string(entry, "to", &prefix, errors).unwrap_or_default();
        let directory = to.split('/').nth(1).unwrap_or_default();
        if !relative_path(&to)
            || !to.starts_with("usr/")
            || if field == "runtime-i686" {
                directory != "lib32"
            } else {
                matches!(directory, "lib" | "lib32")
            }
        {
            errors.push(format!(
                "{prefix}to: expected a relative path under /usr for {field}"
            ));
        }
        mappings.push(Mapping { output, from, to });
    }
    mappings.sort();
    if mappings.windows(2).any(|pair| pair[0] == pair[1]) {
        errors.push(format!("fhs.{field}: duplicate mapping"));
    }
    mappings
}

pub(super) fn write(out: &mut String, record: &Fhs) {
    out.push_str("\n[fhs]\n");
    write_string_field(out, "program", &record.program);
    for ((field, _), entries) in SYSTEMS.into_iter().zip(&record.runtime) {
        if entries.is_empty() {
            continue;
        }
        out.push_str(field);
        out.push_str(" = [\n");
        for entry in entries {
            out.push_str("  { output = ");
            write_escaped(out, &entry.output);
            out.push_str(", from = ");
            write_escaped(out, &entry.from);
            out.push_str(", to = ");
            write_escaped(out, &entry.to);
            out.push_str(" },\n");
        }
        out.push_str("]\n");
    }
    write_bool_field(out, "graphics", record.graphics);
}
