use std::collections::{BTreeMap, BTreeSet};

use vug::nix_cache::{StorePath, parse_hash};

use super::{
    Errors, model, req_string, req_string_array, req_table, unknown_keys, write_array,
    write_escaped, write_string_field,
};

pub(crate) struct NixOutput {
    pub(crate) system: String,
    pub(crate) nixpkgs_rev: String,
    pub(crate) attribute: String,
    pub(crate) outputs: BTreeMap<String, Output>,
}

pub(crate) struct Output {
    pub(crate) path: StorePath,
    pub(crate) nar_hash: String,
    pub(crate) expose: Vec<String>,
}

pub(super) fn parse(table: &toml::Table, errors: &mut Errors) -> NixOutput {
    unknown_keys(
        table,
        &["kind", "system", "nixpkgs-rev", "attribute", "nix-outputs"],
        "build.",
        errors,
    );
    let system = req_string(table, "system", "build.", errors).unwrap_or_default();
    if !system.split_once('-').is_some_and(|(arch, os)| {
        [arch, os].into_iter().all(|part| {
            !part.is_empty()
                && part
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
        })
    }) {
        errors.push("build.system: expected a Nix system such as x86_64-linux".into());
    }
    let nixpkgs_rev = req_string(table, "nixpkgs-rev", "build.", errors).unwrap_or_default();
    if nixpkgs_rev.len() != 40
        || !nixpkgs_rev
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        errors.push("build.nixpkgs-rev: expected 40 lowercase hex characters".into());
    }
    let attribute = req_string(table, "attribute", "build.", errors).unwrap_or_default();
    if !attribute.split('.').all(|part| {
        !part.is_empty()
            && part
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_-+'".contains(&b))
    }) {
        errors.push("build.attribute: expected a dot-separated Nix attribute path".into());
    }
    let mut outputs = BTreeMap::new();
    let mut hashes = BTreeMap::new();
    if let Some(entries) = req_table(table, "nix-outputs", "build.", errors) {
        if entries.is_empty() {
            errors.push("build.nix-outputs: expected at least one output".into());
        }
        for (name, entry) in entries {
            let prefix = format!("build.nix-outputs.{name}.");
            if !model::valid_name(name) {
                errors.push(format!("build.nix-outputs: invalid output name {name:?}"));
            }
            let Some(table) = entry.as_table() else {
                errors.push(format!("build.nix-outputs.{name}: expected a table"));
                continue;
            };
            unknown_keys(
                table,
                &["store-path", "nar-hash", "expose"],
                &prefix,
                errors,
            );
            let path = req_string(table, "store-path", &prefix, errors).and_then(|value| {
                parse_field(value.parse::<StorePath>(), &prefix, "store-path", errors)
            });
            let hash = req_string(table, "nar-hash", &prefix, errors)
                .and_then(|value| parse_field(parse_hash(&value), &prefix, "nar-hash", errors));
            let mut expose = req_string_array(table, "expose", &prefix, errors).unwrap_or_default();
            expose.sort();
            let mut seen = BTreeSet::new();
            for path in &expose {
                if !path
                    .split_once('/')
                    .is_some_and(|(dir, _)| matches!(dir, "bin" | "share"))
                    || !path.split('/').all(|part| {
                        !matches!(part, "" | "." | "..")
                            && !part
                                .chars()
                                .any(|c| c.is_control() || matches!(c, '*' | '?' | '\\'))
                    })
                {
                    errors.push(format!("{prefix}expose: expected a relative file path under bin/ or share/, got {path:?}"));
                }
                if !seen.insert(path.as_str())
                    || path
                        .match_indices('/')
                        .any(|(i, _)| seen.contains(&path[..i]))
                {
                    errors.push(format!(
                        "{prefix}expose: duplicate or overlapping path {path:?}"
                    ));
                }
            }
            if let (Some(path), Some(nar_hash)) = (path, hash) {
                if let Some(previous) = hashes.insert(path.clone(), nar_hash.clone())
                    && previous != nar_hash
                {
                    errors.push(format!("{prefix}nar-hash: conflicting hashes for {path}"));
                }
                outputs.insert(
                    name.clone(),
                    Output {
                        path,
                        nar_hash,
                        expose,
                    },
                );
            }
        }
    }
    NixOutput {
        system,
        nixpkgs_rev,
        attribute,
        outputs,
    }
}

fn parse_field<T>(
    result: Result<T, String>,
    prefix: &str,
    field: &str,
    errors: &mut Errors,
) -> Option<T> {
    result
        .map_err(|error| errors.push(format!("{prefix}{field}: {error}")))
        .ok()
}

pub(super) fn write(out: &mut String, record: &NixOutput) {
    write_string_field(out, "kind", "nix-output");
    write_string_field(out, "system", &record.system);
    write_string_field(out, "nixpkgs-rev", &record.nixpkgs_rev);
    write_string_field(out, "attribute", &record.attribute);
    for (name, output) in &record.outputs {
        out.push_str("\n[build.nix-outputs.");
        if name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-_".contains(&byte))
        {
            out.push_str(name);
        } else {
            write_escaped(out, name);
        }
        out.push_str("]\n");
        write_string_field(out, "store-path", output.path.as_str());
        write_string_field(out, "nar-hash", &output.nar_hash);
        write_array(out, "expose", &output.expose);
    }
}
