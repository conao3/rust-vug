use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use super::{
    Errors, RecordError, VersionRecord, check_unique, current_version, dependency_output, error_at,
    opt_string_array, req_table, unknown_keys,
};
use crate::naming::model;

pub(crate) struct Output {
    pub(crate) paths: Vec<String>,
    pub(crate) run: Vec<String>,
}

pub(super) fn check_dependencies(
    field: &str,
    deps: &[String],
    allow_local: bool,
    errors: &mut Errors,
) {
    let mut seen = BTreeMap::new();
    for dep in deps {
        let (package, output) = dependency_output(dep);
        if !(model::valid_name(package) || allow_local && dep.starts_with(':'))
            || !model::valid_name(output)
        {
            errors.push(format!("{field}: invalid dependency {dep:?}"));
        } else if let Some(other) = seen.insert((package, output), dep)
            && dep != other
        {
            errors.push(format!(
                "{field}: {dep:?} and {other:?} select the same output"
            ));
        }
    }
}

pub(super) fn parse(
    record: &toml::Table,
    deps_run: &[String],
    errors: &mut Errors,
) -> BTreeMap<String, Output> {
    let mut outputs = BTreeMap::new();
    if !record.contains_key("outputs") {
        return outputs;
    }
    if !deps_run.is_empty() {
        errors.push("dependencies.run: expected an empty array when outputs are declared".into());
    }
    let Some(table) = req_table(record, "outputs", "", errors) else {
        return outputs;
    };
    for name in table.keys() {
        if !model::valid_name(name) {
            errors.push(format!("outputs: invalid output name {name:?}"));
            continue;
        }
        let Some(output) = req_table(table, name, "outputs.", errors) else {
            continue;
        };
        let prefix = format!("outputs.{name}.");
        unknown_keys(output, &["paths", "run"], &prefix, errors);
        let mut paths = opt_string_array(output, "paths", &prefix, errors).unwrap_or_default();
        let mut run = opt_string_array(output, "run", &prefix, errors).unwrap_or_default();
        check_unique("paths", &prefix, &paths, errors);
        check_unique("run", &prefix, &run, errors);
        check_dependencies(&format!("{prefix}run"), &run, true, errors);
        paths.retain(|path| {
            let valid = !path.chars().any(char::is_control)
                && path.split('/').all(|part| !matches!(part, "" | "." | ".."));
            if !valid {
                errors.push(format!(
                    "{prefix}paths: expected a path inside the staging tree, got {path:?}"
                ));
            }
            valid
        });
        paths.sort();
        run.sort();
        outputs.insert(name.clone(), Output { paths, run });
    }
    if !outputs.contains_key("out") {
        errors.push("outputs.out: required table missing".into());
    }
    check_local_dependencies(&outputs, errors);
    let mut patterns: Vec<(&str, &str, Vec<&str>)> = Vec::new();
    for (name, output) in &outputs {
        for path in &output.paths {
            let parts: Vec<_> = path.split('/').collect();
            for (other_name, other_path, other_parts) in &patterns {
                if name != *other_name && paths_overlap(&parts, other_parts) {
                    errors.push(format!(
                        "outputs.{name}.paths: {path:?} overlaps {other_path:?} in outputs.{other_name}.paths"
                    ));
                }
            }
            patterns.push((name, path, parts));
        }
    }
    outputs
}

fn check_local_dependencies(outputs: &BTreeMap<String, Output>, errors: &mut Errors) {
    let mut pending = BTreeMap::new();
    let mut dependants = BTreeMap::<&str, Vec<&str>>::new();
    let mut ready = Vec::new();
    for (name, output) in outputs {
        let mut count = 0;
        for dep in &output.run {
            if let Some(target) = dep.strip_prefix(':') {
                if outputs.contains_key(target) {
                    dependants.entry(target).or_default().push(name);
                    count += 1;
                } else {
                    errors.push(format!("outputs.{name}.run: unknown output {target:?}"));
                }
            }
        }
        if count == 0 {
            ready.push(name.as_str());
        }
        pending.insert(name.as_str(), count);
    }
    while let Some(name) = ready.pop() {
        pending.remove(name);
        for dependant in dependants.remove(name).unwrap_or_default() {
            let count = pending.get_mut(dependant).unwrap();
            *count -= 1;
            if *count == 0 {
                ready.push(dependant);
            }
        }
    }
    if !pending.is_empty() {
        let names: Vec<_> = pending.keys().copied().collect();
        errors.push(format!(
            "outputs: dependency cycle prevents ordering {}",
            names.join(", ")
        ));
    }
}

fn paths_overlap(left: &[&str], right: &[&str]) -> bool {
    let mut pending = vec![(0, 0)];
    let mut seen = BTreeSet::new();
    while let Some((a, b)) = pending.pop() {
        if !seen.insert((a, b)) {
            continue;
        }
        match (left.get(a).copied(), right.get(b).copied()) {
            (None, None) => return true,
            (Some("**"), Some("**")) => pending.extend([(a + 1, b), (a, b + 1)]),
            (Some("**"), other) => {
                pending.push((a + 1, b));
                if other.is_some() {
                    pending.push((a, b + 1));
                }
            }
            (other, Some("**")) => {
                pending.push((a, b + 1));
                if other.is_some() {
                    pending.push((a + 1, b));
                }
            }
            (Some(left), Some(right)) if components_overlap(left, right) => {
                pending.push((a + 1, b + 1));
            }
            _ => {}
        }
    }
    false
}

#[derive(Clone, Copy, Eq, Ord, PartialEq, PartialOrd)]
enum Component {
    Empty,
    Dot,
    DotDot,
    Normal,
}

fn components_overlap(left: &str, right: &str) -> bool {
    let left: Vec<_> = left.chars().collect();
    let right: Vec<_> = right.chars().collect();
    let mut pending = vec![(0, 0, Component::Empty)];
    let mut seen = BTreeSet::new();
    while let Some((a, b, component)) = pending.pop() {
        if !seen.insert((a, b, component)) {
            continue;
        }
        let (x, y) = (left.get(a).copied(), right.get(b).copied());
        if x.is_none() && y.is_none() && component == Component::Normal {
            return true;
        }
        if x == Some('*') {
            pending.push((a + 1, b, component));
        }
        if y == Some('*') {
            pending.push((a, b + 1, component));
        }
        let (Some(x), Some(y)) = (x, y) else {
            continue;
        };
        let wildcard = |c| matches!(c, '*' | '?');
        let next_a = a + usize::from(x != '*');
        let next_b = b + usize::from(y != '*');
        if (x == '.' || wildcard(x)) && (y == '.' || wildcard(y)) {
            let dots = match component {
                Component::Empty => Component::Dot,
                Component::Dot => Component::DotDot,
                Component::DotDot | Component::Normal => Component::Normal,
            };
            pending.push((next_a, next_b, dots));
        }
        if x == y && x != '.' || wildcard(x) && y != '.' || wildcard(y) && x != '.' {
            pending.push((next_a, next_b, Component::Normal));
        }
    }
    false
}

pub(super) fn check_tree(
    root: &Path,
    versions: &BTreeMap<PathBuf, VersionRecord>,
    errors: &mut Vec<RecordError>,
) {
    let mut latest = BTreeMap::new();
    let mut reported = BTreeSet::new();
    for (path, record) in versions
        .iter()
        .filter(|(_, record)| record.uses_output_keys())
    {
        let Some(name) = path
            .parent()
            .and_then(Path::parent)
            .and_then(Path::file_name)
            .and_then(|name| name.to_str())
        else {
            continue;
        };
        let mut active = BTreeSet::from([name]);
        let mut visited = BTreeSet::new();
        let mut stack = vec![(name, path, record.dependencies())];
        while let Some((name, path, deps)) = stack.last_mut() {
            let Some((field, dep)) = deps.next() else {
                active.remove(name);
                stack.pop();
                continue;
            };
            let path = *path;
            let (package, output) = dependency_output(dep);
            if package.is_empty() {
                continue;
            }
            let selected = latest.entry(package).or_insert_with(|| {
                let dir = root.join(model::package_dir(package)).join("versions");
                current_version(&dir)
                    .map(|version| root.join(model::version_path(package, &version)))
            });
            let message = match selected {
                Err(error) => format!("{field}: cannot resolve {dep:?}: {error}"),
                Ok(selected) => {
                    let Some((selected, target)) = versions.get_key_value(selected) else {
                        continue;
                    };
                    if !target.has_output(output) {
                        format!("{field}: unknown output {dep:?} in {}", selected.display())
                    } else if active.contains(package) {
                        let mut cycle: Vec<_> = stack.iter().map(|(name, _, _)| *name).collect();
                        cycle.push(package);
                        format!("dependency cycle: {}", cycle.join(" -> "))
                    } else {
                        if visited.insert(package) {
                            active.insert(package);
                            stack.push((package, selected, target.dependencies()));
                        }
                        continue;
                    }
                }
            };
            if reported.insert((path.as_path(), message.clone())) {
                errors.push(error_at(path, message));
            }
        }
    }
}
