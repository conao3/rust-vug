//! Package-record parsing, validation, and canonical byte output.
//!
//! Name, version, and path rules are defined in Lean (`naming::model`) and
//! generated to Rust. This module owns TOML structure validation, the
//! canonical writer, and the package-tree checks of `docs/record.md`.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use crate::naming::model;
use crate::version;

mod output;

/// A validation or tree error: the file it belongs to and what is expected.
#[derive(Clone)]
pub struct RecordError {
    pub path: String,
    pub message: String,
}

/// The result of checking a single record file.
pub struct FileCheck {
    /// The canonical byte form of the record.
    pub canonical: String,
    /// Whether the file bytes differ from the canonical form.
    pub changed: bool,
}

/// The result of `check-tree`.
pub struct TreeReport {
    pub packages: usize,
    pub versions: usize,
    pub errors: Vec<RecordError>,
}

pub(crate) enum Parsed {
    Package(PackageRecord),
    Version(Box<VersionRecord>),
}

pub(crate) struct PackageRecord {
    pub(crate) name: String,
    pub(crate) license: String,
    pub(crate) homepage: Option<String>,
    pub(crate) sources: Vec<String>,
}

pub(crate) struct VersionRecord {
    pub(crate) version: String,
    pub(crate) patches: Vec<String>,
    pub(crate) url: String,
    pub(crate) sha256: String,
    pub(crate) strip_components: i64,
    pub(crate) build: BuildOptions,
    pub(crate) deps_build: Vec<String>,
    pub(crate) deps_run: Vec<String>,
    pub(crate) deps_expose: Vec<String>,
    pub(crate) outputs: BTreeMap<String, output::Output>,
    pub(crate) check_enabled: bool,
    pub(crate) check_target: Option<String>,
    pub(crate) check_skip: Vec<String>,
    pub(crate) private_cxx_runtime: bool,
    /// (path in the output, link target), ascending by path.
    pub(crate) links: Vec<(String, String)>,
    /// (path in the output, command the program at that path runs),
    /// ascending by path.
    pub(crate) wrappers: Vec<(String, Vec<String>)>,
}

impl VersionRecord {
    pub(crate) fn dependencies(&self) -> impl Iterator<Item = (String, &str)> {
        [
            ("dependencies.build", &self.deps_build),
            ("dependencies.run", &self.deps_run),
            ("dependencies.expose", &self.deps_expose),
        ]
        .into_iter()
        .flat_map(|(field, deps)| deps.iter().map(move |dep| (field.to_owned(), dep.as_str())))
        .chain(self.outputs.iter().flat_map(|(name, output)| {
            output
                .run
                .iter()
                .map(move |dep| (format!("outputs.{name}.run"), dep.as_str()))
        }))
    }

    pub(crate) fn uses_output_keys(&self) -> bool {
        !self.outputs.is_empty()
            || self
                .deps_build
                .iter()
                .chain(&self.deps_run)
                .chain(&self.deps_expose)
                .any(|dep| dep.contains(':'))
    }

    pub(crate) fn has_output(&self, name: &str) -> bool {
        name == "out" || self.outputs.contains_key(name)
    }
}

pub(crate) fn dependency_output(spec: &str) -> (&str, &str) {
    spec.split_once(':').unwrap_or((spec, "out"))
}

pub(crate) enum BuildOptions {
    Autotools {
        path: String,
        autoreconf: bool,
        configure_flags: Vec<String>,
        make_flags: Vec<String>,
        install_flags: Vec<String>,
        install_targets: Vec<String>,
        configure_command: Vec<String>,
    },
    BootstrapMake {
        configure_flags: Vec<String>,
    },
    Make {
        make_flags: Vec<String>,
        install_flags: Vec<String>,
        build_targets: Vec<String>,
        install_targets: Vec<String>,
    },
    Cmake {
        path: String,
        setup_flags: Vec<String>,
    },
    Cargo {
        path: String,
        flags: Vec<String>,
    },
    Commands {
        commands: Vec<Vec<String>>,
    },
    Meson {
        path: String,
        setup_flags: Vec<String>,
    },
    Linux {
        defconfig: String,
        modules: bool,
        config: Vec<String>,
    },
    Prebuilt {
        install: Vec<(String, String)>,
    },
}

type Errors = Vec<String>;

fn unknown_keys(map: &toml::Table, allowed: &[&str], prefix: &str, errors: &mut Errors) {
    for (key, value) in map {
        if !allowed.contains(&key.as_str()) {
            let what = if value.is_table() { "table" } else { "key" };
            errors.push(format!("{prefix}{key}: unknown {what}"));
        }
    }
}

fn req_table<'a>(
    map: &'a toml::Table,
    key: &str,
    prefix: &str,
    errors: &mut Errors,
) -> Option<&'a toml::Table> {
    match map.get(key) {
        None => {
            errors.push(format!("{prefix}{key}: required table missing"));
            None
        }
        Some(toml::Value::Table(t)) => Some(t),
        Some(_) => {
            errors.push(format!("{prefix}{key}: expected a table"));
            None
        }
    }
}

fn req_string(map: &toml::Table, key: &str, prefix: &str, errors: &mut Errors) -> Option<String> {
    match map.get(key) {
        None => {
            errors.push(format!("{prefix}{key}: required key missing"));
            None
        }
        Some(toml::Value::String(s)) => Some(s.clone()),
        Some(_) => {
            errors.push(format!("{prefix}{key}: expected a string"));
            None
        }
    }
}

fn source_path(build_table: &toml::Table, errors: &mut Errors) -> String {
    match build_table.get("path") {
        None => ".".to_owned(),
        Some(value) => match value.as_str() {
            Some(path)
                if !path.is_empty()
                    && !path.starts_with('/')
                    && path.split('/').all(|part| part != "..") =>
            {
                path.to_owned()
            }
            _ => {
                errors.push("build.path: expected a path inside the source".to_owned());
                ".".to_owned()
            }
        },
    }
}

fn opt_string(map: &toml::Table, key: &str, prefix: &str, errors: &mut Errors) -> Option<String> {
    match map.get(key) {
        None => None,
        Some(toml::Value::String(s)) => Some(s.clone()),
        Some(_) => {
            errors.push(format!("{prefix}{key}: expected a string"));
            None
        }
    }
}

fn req_bool(map: &toml::Table, key: &str, prefix: &str, errors: &mut Errors) -> Option<bool> {
    match map.get(key) {
        None => {
            errors.push(format!("{prefix}{key}: required key missing"));
            None
        }
        Some(toml::Value::Boolean(b)) => Some(*b),
        Some(_) => {
            errors.push(format!("{prefix}{key}: expected a boolean"));
            None
        }
    }
}

fn opt_int(map: &toml::Table, key: &str, prefix: &str, errors: &mut Errors) -> Option<i64> {
    match map.get(key) {
        None => None,
        Some(toml::Value::Integer(i)) => Some(*i),
        Some(_) => {
            errors.push(format!("{prefix}{key}: expected an integer"));
            None
        }
    }
}

fn req_string_array(
    map: &toml::Table,
    key: &str,
    prefix: &str,
    errors: &mut Errors,
) -> Option<Vec<String>> {
    match map.get(key) {
        None => {
            errors.push(format!("{prefix}{key}: required key missing"));
            None
        }
        Some(v) => string_array(v, key, prefix, errors),
    }
}

fn opt_string_array(
    map: &toml::Table,
    key: &str,
    prefix: &str,
    errors: &mut Errors,
) -> Option<Vec<String>> {
    match map.get(key) {
        None => Some(Vec::new()),
        Some(v) => string_array(v, key, prefix, errors),
    }
}

fn string_array(
    value: &toml::Value,
    key: &str,
    prefix: &str,
    errors: &mut Errors,
) -> Option<Vec<String>> {
    match value {
        toml::Value::Array(items) => {
            let mut out = Vec::with_capacity(items.len());
            for item in items {
                match item.as_str() {
                    Some(s) => out.push(s.to_owned()),
                    None => {
                        errors.push(format!("{prefix}{key}: expected an array of strings"));
                        return None;
                    }
                }
            }
            Some(out)
        }
        _ => {
            errors.push(format!("{prefix}{key}: expected an array of strings"));
            None
        }
    }
}

/// Report duplicate entries of a "no duplicates" array.
fn check_unique(key: &str, prefix: &str, items: &[String], errors: &mut Errors) {
    let mut sorted = items.to_vec();
    sorted.sort();
    for pair in sorted.windows(2) {
        if pair[0] == pair[1] {
            errors.push(format!("{prefix}{key}: duplicate entry {:?}", pair[0]));
        }
    }
}

fn check_https_slash(key: &str, prefix: &str, url: &str, errors: &mut Errors) {
    if !url.starts_with("https://") {
        errors.push(format!("{prefix}{key}: expected an \"https://\" URL"));
    } else if !url.ends_with('/') {
        errors.push(format!("{prefix}{key}: expected a URL ending in \"/\""));
    }
}

fn parse_package(map: &toml::Table, dir_name: Option<&str>) -> (PackageRecord, Errors) {
    let mut errors = Vec::new();
    unknown_keys(map, &["name", "license", "upstream"], "", &mut errors);
    let name = req_string(map, "name", "", &mut errors).unwrap_or_default();
    if !model::valid_name(&name) {
        errors.push(
            "name: expected a name matching [a-z0-9][a-z0-9+._-]* (at most 64 characters)"
                .to_owned(),
        );
    } else if let Some(dir) = dir_name
        && name != dir
    {
        errors.push(format!("name: expected \"{dir}\" (the directory name)"));
    }
    let license = req_string(map, "license", "", &mut errors).unwrap_or_default();
    let upstream = req_table(map, "upstream", "", &mut errors);
    let (mut homepage, mut sources) = (None, Vec::new());
    if let Some(up) = upstream {
        unknown_keys(up, &["homepage", "sources"], "upstream.", &mut errors);
        homepage = opt_string(up, "homepage", "upstream.", &mut errors);
        sources = req_string_array(up, "sources", "upstream.", &mut errors).unwrap_or_default();
        if sources.is_empty() {
            errors.push("upstream.sources: expected a non-empty array".to_owned());
        }
        for source in &sources {
            check_https_slash("sources", "upstream.", source, &mut errors);
        }
        check_unique("sources", "upstream.", &sources, &mut errors);
    }
    (
        PackageRecord {
            name,
            license,
            homepage,
            sources,
        },
        errors,
    )
}

fn parse_version(
    map: &toml::Table,
    stem: &str,
    upstream_sources: Option<Vec<String>>,
) -> (VersionRecord, Errors) {
    let mut errors = Vec::new();
    unknown_keys(
        map,
        &[
            "version",
            "patches",
            "source",
            "build",
            "dependencies",
            "outputs",
            "check",
            "links",
            "wrappers",
        ],
        "",
        &mut errors,
    );
    let version = req_string(map, "version", "", &mut errors).unwrap_or_default();
    if !model::valid_version(&version) {
        errors.push(
            "version: expected a version matching [A-Za-z0-9][A-Za-z0-9+._~-]* (at most 64 characters)"
                .to_owned(),
        );
    } else if version != stem {
        errors.push(format!("version: expected \"{stem}\" (the file name)"));
    }
    let patches = opt_string_array(map, "patches", "", &mut errors).unwrap_or_default();
    for patch in &patches {
        check_patch_name(&version, patch, &mut errors);
    }

    let (mut url, mut sha256, mut strip_components) = (String::new(), String::new(), 1);
    if let Some(source) = req_table(map, "source", "", &mut errors) {
        unknown_keys(
            source,
            &["url", "sha256", "strip-components"],
            "source.",
            &mut errors,
        );
        url = req_string(source, "url", "source.", &mut errors).unwrap_or_default();
        if !url.starts_with("https://") {
            errors.push("source.url: expected an \"https://\" URL".to_owned());
        } else {
            match &upstream_sources {
                None => errors.push(
                    "source.url: cannot check upstream.sources (no readable package.toml)"
                        .to_owned(),
                ),
                Some(sources) => {
                    if !sources.iter().any(|p| url.starts_with(p.as_str())) {
                        errors.push(
                            "source.url: expected a URL starting with one of upstream.sources"
                                .to_owned(),
                        );
                    }
                }
            }
        }
        sha256 = req_string(source, "sha256", "source.", &mut errors).unwrap_or_default();
        if !(sha256.len() == 64
            && sha256
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)))
        {
            errors.push("source.sha256: expected 64 lowercase hex characters".to_owned());
        }
        strip_components = opt_int(source, "strip-components", "source.", &mut errors).unwrap_or(1);
    }

    let mut build = BuildOptions::Autotools {
        path: ".".to_owned(),
        autoreconf: false,
        configure_flags: Vec::new(),
        make_flags: Vec::new(),
        install_flags: Vec::new(),
        install_targets: Vec::new(),
        configure_command: Vec::new(),
    };
    if let Some(build_table) = req_table(map, "build", "", &mut errors) {
        let kind = req_string(build_table, "kind", "build.", &mut errors).unwrap_or_default();
        match kind.as_str() {
            "autotools" => {
                unknown_keys(
                    build_table,
                    &[
                        "kind",
                        "autoreconf",
                        "path",
                        "configure-command",
                        "configure-flags",
                        "make-flags",
                        "install-flags",
                        "install-targets",
                    ],
                    "build.",
                    &mut errors,
                );
                build = BuildOptions::Autotools {
                    path: source_path(build_table, &mut errors),
                    autoreconf: build_table.contains_key("autoreconf")
                        && req_bool(build_table, "autoreconf", "build.", &mut errors)
                            .unwrap_or(false),
                    configure_flags: opt_string_array(
                        build_table,
                        "configure-flags",
                        "build.",
                        &mut errors,
                    )
                    .unwrap_or_default(),
                    make_flags: opt_string_array(build_table, "make-flags", "build.", &mut errors)
                        .unwrap_or_default(),
                    install_flags: opt_string_array(
                        build_table,
                        "install-flags",
                        "build.",
                        &mut errors,
                    )
                    .unwrap_or_default(),
                    install_targets: opt_string_array(
                        build_table,
                        "install-targets",
                        "build.",
                        &mut errors,
                    )
                    .unwrap_or_default(),
                    configure_command: opt_string_array(
                        build_table,
                        "configure-command",
                        "build.",
                        &mut errors,
                    )
                    .unwrap_or_default(),
                };
            }
            "bootstrap-make" => {
                unknown_keys(
                    build_table,
                    &["kind", "configure-flags"],
                    "build.",
                    &mut errors,
                );
                build = BuildOptions::BootstrapMake {
                    configure_flags: opt_string_array(
                        build_table,
                        "configure-flags",
                        "build.",
                        &mut errors,
                    )
                    .unwrap_or_default(),
                };
            }
            "make" => {
                unknown_keys(
                    build_table,
                    &[
                        "kind",
                        "make-flags",
                        "install-flags",
                        "build-targets",
                        "install-targets",
                    ],
                    "build.",
                    &mut errors,
                );
                let list = |key: &str, errors: &mut Errors| {
                    opt_string_array(build_table, key, "build.", errors).unwrap_or_default()
                };
                build = BuildOptions::Make {
                    make_flags: list("make-flags", &mut errors),
                    install_flags: list("install-flags", &mut errors),
                    build_targets: list("build-targets", &mut errors),
                    install_targets: list("install-targets", &mut errors),
                };
            }
            "cmake" => {
                unknown_keys(
                    build_table,
                    &["kind", "path", "setup-flags"],
                    "build.",
                    &mut errors,
                );
                let path = source_path(build_table, &mut errors);
                build = BuildOptions::Cmake {
                    path,
                    setup_flags: opt_string_array(
                        build_table,
                        "setup-flags",
                        "build.",
                        &mut errors,
                    )
                    .unwrap_or_default(),
                };
            }
            "meson" => {
                unknown_keys(
                    build_table,
                    &["kind", "path", "setup-flags"],
                    "build.",
                    &mut errors,
                );
                build = BuildOptions::Meson {
                    path: source_path(build_table, &mut errors),
                    setup_flags: opt_string_array(
                        build_table,
                        "setup-flags",
                        "build.",
                        &mut errors,
                    )
                    .unwrap_or_default(),
                };
            }
            "linux" => {
                unknown_keys(
                    build_table,
                    &["kind", "defconfig", "config", "modules"],
                    "build.",
                    &mut errors,
                );
                let defconfig =
                    req_string(build_table, "defconfig", "build.", &mut errors).unwrap_or_default();
                if !defconfig.is_empty()
                    && !defconfig
                        .bytes()
                        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
                {
                    errors.push(
                        "build.defconfig: expected lowercase letters, digits, and \"_\"".to_owned(),
                    );
                }
                let config = opt_string_array(build_table, "config", "build.", &mut errors)
                    .unwrap_or_default();
                for entry in &config {
                    let valid = entry.split_once('=').is_some_and(|(name, value)| {
                        !name.is_empty()
                            && name
                                .bytes()
                                .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_')
                            && !value.is_empty()
                    });
                    if !valid {
                        errors.push(format!(
                            "build.config: {entry:?}: expected \"NAME=y\", \"NAME=n\", or \"NAME=value\""
                        ));
                    }
                }
                let modules = build_table.contains_key("modules")
                    && req_bool(build_table, "modules", "build.", &mut errors).unwrap_or(false);
                build = BuildOptions::Linux {
                    defconfig,
                    config,
                    modules,
                };
            }
            "cargo" => {
                unknown_keys(
                    build_table,
                    &["kind", "path", "flags"],
                    "build.",
                    &mut errors,
                );
                let path = match build_table.get("path") {
                    None => ".".to_owned(),
                    Some(value) => match value.as_str() {
                        Some(path)
                            if !path.is_empty()
                                && !path.starts_with('/')
                                && path.split('/').all(|part| part != "..") =>
                        {
                            path.to_owned()
                        }
                        _ => {
                            errors.push("build.path: expected a path inside the source".to_owned());
                            ".".to_owned()
                        }
                    },
                };
                build = BuildOptions::Cargo {
                    path,
                    flags: opt_string_array(build_table, "flags", "build.", &mut errors)
                        .unwrap_or_default(),
                };
            }
            "commands" => {
                unknown_keys(build_table, &["kind", "commands"], "build.", &mut errors);
                let mut commands = Vec::new();
                match build_table.get("commands") {
                    Some(toml::Value::Array(items)) if !items.is_empty() => {
                        for item in items {
                            let words: Option<Vec<String>> = item.as_array().and_then(|words| {
                                words
                                    .iter()
                                    .map(|w| w.as_str().map(str::to_owned))
                                    .collect()
                            });
                            match words {
                                Some(words) if !words.is_empty() => commands.push(words),
                                _ => errors.push(
                                    "build.commands: expected non-empty arrays of strings"
                                        .to_owned(),
                                ),
                            }
                        }
                    }
                    _ => errors.push(
                        "build.commands: expected a non-empty array of arrays of strings"
                            .to_owned(),
                    ),
                }
                build = BuildOptions::Commands { commands };
            }
            "prebuilt" => {
                unknown_keys(build_table, &["kind", "install"], "build.", &mut errors);
                build = BuildOptions::Prebuilt {
                    install: parse_install(build_table, &mut errors),
                };
            }
            _ => {
                errors.push(
                    "build.kind: expected \"autotools\", \"bootstrap-make\", \"cargo\", \"cmake\", \"commands\", \"linux\", \"make\", \"meson\", or \"prebuilt\""
                        .to_owned(),
                );
            }
        }
    }

    let (mut deps_build, mut deps_run, mut deps_expose) = (Vec::new(), Vec::new(), Vec::new());
    if let Some(deps) = req_table(map, "dependencies", "", &mut errors) {
        unknown_keys(
            deps,
            &["build", "run", "expose"],
            "dependencies.",
            &mut errors,
        );
        deps_build =
            req_string_array(deps, "build", "dependencies.", &mut errors).unwrap_or_default();
        deps_run = req_string_array(deps, "run", "dependencies.", &mut errors).unwrap_or_default();
        check_unique("build", "dependencies.", &deps_build, &mut errors);
        check_unique("run", "dependencies.", &deps_run, &mut errors);
        deps_expose =
            opt_string_array(deps, "expose", "dependencies.", &mut errors).unwrap_or_default();
        check_unique("expose", "dependencies.", &deps_expose, &mut errors);
    }
    for (field, deps) in [
        ("dependencies.build", &deps_build),
        ("dependencies.run", &deps_run),
        ("dependencies.expose", &deps_expose),
    ] {
        output::check_dependencies(field, deps, false, &mut errors);
    }
    let outputs = output::parse(map, &deps_run, &mut errors);
    if matches!(
        build,
        BuildOptions::Autotools {
            autoreconf: true,
            ..
        }
    ) {
        for tool in ["autoconf", "automake", "libtool"] {
            if !deps_build
                .iter()
                .any(|dep| dependency_output(dep) == (tool, "out"))
            {
                errors.push(format!(
                    "build.autoreconf: dependencies.build must include {tool}"
                ));
            }
        }
    }

    let (mut check_enabled, mut check_skip) = (true, Vec::new());
    let mut check_target = None;
    let mut private_cxx_runtime = false;
    if let Some(check) = req_table(map, "check", "", &mut errors) {
        unknown_keys(
            check,
            &["enabled", "skip", "private-cxx-runtime", "target"],
            "check.",
            &mut errors,
        );
        if check.contains_key("private-cxx-runtime") {
            private_cxx_runtime =
                req_bool(check, "private-cxx-runtime", "check.", &mut errors).unwrap_or(false);
        }
        check_target =
            opt_string(check, "target", "check.", &mut errors).filter(|target| target != "check");
        check_enabled = req_bool(check, "enabled", "check.", &mut errors).unwrap_or(true);
        check_skip = req_string_array(check, "skip", "check.", &mut errors).unwrap_or_default();
        check_unique("skip", "check.", &check_skip, &mut errors);
    }
    if let Some(target) = check_target.as_deref() {
        if !matches!(
            build,
            BuildOptions::Autotools { .. } | BuildOptions::Make { .. }
        ) {
            errors.push("check.target: unsupported for this build kind".to_owned());
        } else if target.is_empty()
            || target.starts_with('-')
            || !target
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || b"._/+-".contains(&c))
        {
            errors.push("check.target: expected a make target name".to_owned());
        }
    }
    if !check_skip.is_empty() {
        if !matches!(
            build,
            BuildOptions::Autotools { .. }
                | BuildOptions::Cmake { .. }
                | BuildOptions::Meson { .. }
                | BuildOptions::Cargo { .. }
        ) {
            errors.push("check.skip: unsupported for this build kind".to_owned());
        }
        for name in &check_skip {
            if name.trim().is_empty() || name.chars().any(char::is_control) {
                errors.push(
                    "check.skip: expected a nonempty test name without control characters"
                        .to_owned(),
                );
            } else if matches!(build, BuildOptions::Autotools { .. })
                && !name
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"._/+-".contains(&b))
            {
                errors.push(format!("check.skip: invalid Automake test name {name:?}"));
            }
        }
        if let BuildOptions::Autotools { make_flags, .. } = &build
            && make_flags.iter().any(|flag| {
                flag.split_once('=').is_some_and(|(name, _)| {
                    matches!(
                        name.trim_end_matches([' ', ':', '+', '?', '!']),
                        "TESTS" | "check_PROGRAMS"
                    )
                })
            })
        {
            errors.push("check.skip: build.make-flags must leave Automake TESTS and check_PROGRAMS to the test harness".to_owned());
        }
    }

    let mut links = Vec::new();
    match map.get("links") {
        None => {}
        Some(toml::Value::Table(table)) => {
            for (path, target) in table {
                let inside = !path.is_empty()
                    && !path.starts_with('/')
                    && path
                        .split('/')
                        .all(|part| !part.is_empty() && part != "." && part != "..");
                match target.as_str() {
                    Some(target) if inside && !target.is_empty() => {
                        links.push((path.clone(), target.to_owned()));
                    }
                    _ => errors.push(format!(
                        "links.{path}: expected a path inside the output and a target string"
                    )),
                }
            }
            if table.is_empty() {
                errors.push("links: expected at least one entry".to_owned());
            }
            links.sort();
        }
        Some(_) => errors.push("links: expected a table".to_owned()),
    }

    let mut wrappers = Vec::new();
    match map.get("wrappers") {
        None => {}
        Some(toml::Value::Table(table)) => {
            for (path, command) in table {
                let inside = !path.is_empty()
                    && !path.starts_with('/')
                    && path
                        .split('/')
                        .all(|part| !part.is_empty() && part != "." && part != "..");
                let words: Option<Vec<String>> = command.as_array().and_then(|words| {
                    words
                        .iter()
                        .map(|w| w.as_str().map(str::to_owned))
                        .collect()
                });
                match words {
                    Some(words) if inside && !words.is_empty() => {
                        wrappers.push((path.clone(), words));
                    }
                    _ => errors.push(format!(
                        "wrappers.{path}: expected a path inside the output and a non-empty array of strings"
                    )),
                }
            }
            if table.is_empty() {
                errors.push("wrappers: expected at least one entry".to_owned());
            }
            wrappers.sort();
        }
        Some(_) => errors.push("wrappers: expected a table".to_owned()),
    }

    (
        VersionRecord {
            version,
            patches,
            url,
            sha256,
            strip_components,
            build,
            deps_build,
            deps_run,
            deps_expose,
            outputs,
            check_enabled,
            check_target,
            check_skip,
            private_cxx_runtime,
            links,
            wrappers,
        },
        errors,
    )
}

fn parse_install(map: &toml::Table, errors: &mut Errors) -> Vec<(String, String)> {
    match map.get("install") {
        None => {
            errors.push("build.install: required key missing for kind \"prebuilt\"".to_owned());
            Vec::new()
        }
        Some(toml::Value::Array(items)) => {
            if items.is_empty() {
                errors.push("build.install: expected a non-empty array".to_owned());
            }
            let mut out = Vec::with_capacity(items.len());
            for (i, item) in items.iter().enumerate() {
                let Some(table) = item.as_table() else {
                    errors.push(format!(
                        "build.install[{i}]: expected an inline table {{ from = ..., to = ... }}"
                    ));
                    continue;
                };
                unknown_keys(
                    table,
                    &["from", "to"],
                    &format!("build.install[{i}]."),
                    errors,
                );
                let from = req_string(table, "from", &format!("build.install[{i}]."), errors)
                    .unwrap_or_default();
                let to = req_string(table, "to", &format!("build.install[{i}]."), errors)
                    .unwrap_or_default();
                check_install_path(&format!("build.install[{i}].from"), &from, errors);
                check_install_path(&format!("build.install[{i}].to"), &to, errors);
                out.push((from, to));
            }
            out
        }
        Some(_) => {
            errors.push("build.install: expected an array of inline tables".to_owned());
            Vec::new()
        }
    }
}

fn check_install_path(key: &str, path: &str, errors: &mut Errors) {
    if path.is_empty() {
        errors.push(format!("{key}: expected a non-empty relative path"));
        return;
    }
    if path.starts_with('/') {
        errors.push(format!("{key}: expected a relative path"));
        return;
    }
    if path
        .split('/')
        .any(|component| component.is_empty() || component == "..")
    {
        errors.push(format!("{key}: expected a path without \"..\" components"));
    }
}

fn check_patch_name(version: &str, patch: &str, errors: &mut Errors) {
    let prefix = format!("{version}-");
    let ok = patch
        .strip_prefix(&prefix)
        .and_then(|rest| rest.strip_suffix(".patch"))
        .is_some_and(|slug| !slug.is_empty() && !slug.contains('/'));
    if !ok {
        errors.push(format!(
            "patches: expected a name \"{version}-<slug>.patch\", got \"{patch}\""
        ));
    }
}

// --- canonical writer -----------------------------------------------------

fn write_escaped(out: &mut String, value: &str) {
    out.push('"');
    for c in value.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if (c as u32) < 0x20 || c as u32 == 0x7f => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

fn write_string_field(out: &mut String, key: &str, value: &str) {
    out.push_str(key);
    out.push_str(" = ");
    write_escaped(out, value);
    out.push('\n');
}

fn write_int_field(out: &mut String, key: &str, value: i64) {
    let _ = writeln!(out, "{key} = {value}");
}

fn write_bool_field(out: &mut String, key: &str, value: bool) {
    let _ = writeln!(out, "{key} = {value}");
}

fn write_array(out: &mut String, key: &str, items: &[String]) {
    if items.is_empty() {
        let _ = writeln!(out, "{key} = []");
        return;
    }
    let _ = writeln!(out, "{key} = [");
    for item in items {
        out.push_str("  ");
        write_escaped(out, item);
        out.push_str(",\n");
    }
    out.push_str("]\n");
}

fn write_install(out: &mut String, items: &[(String, String)]) {
    if items.is_empty() {
        out.push_str("install = []\n");
        return;
    }
    out.push_str("install = [\n");
    for (from, to) in items {
        out.push_str("  { from = ");
        write_escaped(out, from);
        out.push_str(", to = ");
        write_escaped(out, to);
        out.push_str(" },\n");
    }
    out.push_str("]\n");
}

fn canonical_package(record: &PackageRecord) -> String {
    let mut out = String::new();
    write_string_field(&mut out, "name", &record.name);
    write_string_field(&mut out, "license", &record.license);
    out.push('\n');
    out.push_str("[upstream]\n");
    if let Some(homepage) = &record.homepage {
        write_string_field(&mut out, "homepage", homepage);
    }
    let mut sources = record.sources.clone();
    sources.sort();
    write_array(&mut out, "sources", &sources);
    out
}

fn canonical_version(record: &VersionRecord) -> String {
    let mut out = String::new();
    write_string_field(&mut out, "version", &record.version);
    if !record.patches.is_empty() {
        write_array(&mut out, "patches", &record.patches);
    }
    out.push('\n');
    out.push_str("[source]\n");
    write_string_field(&mut out, "url", &record.url);
    write_string_field(&mut out, "sha256", &record.sha256);
    if record.strip_components != 1 {
        write_int_field(&mut out, "strip-components", record.strip_components);
    }
    out.push('\n');
    out.push_str("[build]\n");
    match &record.build {
        BuildOptions::Autotools {
            path,
            autoreconf,
            configure_flags,
            make_flags,
            install_flags,
            install_targets,
            configure_command,
        } => {
            write_string_field(&mut out, "kind", "autotools");
            if path != "." {
                write_string_field(&mut out, "path", path);
            }
            if *autoreconf {
                write_bool_field(&mut out, "autoreconf", true);
            }
            if !configure_command.is_empty() {
                write_array(&mut out, "configure-command", configure_command);
            }
            if !configure_flags.is_empty() {
                write_array(&mut out, "configure-flags", configure_flags);
            }
            if !make_flags.is_empty() {
                write_array(&mut out, "make-flags", make_flags);
            }
            if !install_flags.is_empty() {
                write_array(&mut out, "install-flags", install_flags);
            }
            if !install_targets.is_empty() {
                write_array(&mut out, "install-targets", install_targets);
            }
        }
        BuildOptions::BootstrapMake { configure_flags } => {
            write_string_field(&mut out, "kind", "bootstrap-make");
            if !configure_flags.is_empty() {
                write_array(&mut out, "configure-flags", configure_flags);
            }
        }
        BuildOptions::Make {
            make_flags,
            install_flags,
            build_targets,
            install_targets,
        } => {
            write_string_field(&mut out, "kind", "make");
            if !make_flags.is_empty() {
                write_array(&mut out, "make-flags", make_flags);
            }
            if !install_flags.is_empty() {
                write_array(&mut out, "install-flags", install_flags);
            }
            if !build_targets.is_empty() {
                write_array(&mut out, "build-targets", build_targets);
            }
            if !install_targets.is_empty() {
                write_array(&mut out, "install-targets", install_targets);
            }
        }
        BuildOptions::Cmake { path, setup_flags } => {
            write_string_field(&mut out, "kind", "cmake");
            if path != "." {
                write_string_field(&mut out, "path", path);
            }
            if !setup_flags.is_empty() {
                write_array(&mut out, "setup-flags", setup_flags);
            }
        }
        BuildOptions::Meson { path, setup_flags } => {
            write_string_field(&mut out, "kind", "meson");
            if path != "." {
                write_string_field(&mut out, "path", path);
            }
            if !setup_flags.is_empty() {
                write_array(&mut out, "setup-flags", setup_flags);
            }
        }
        BuildOptions::Linux {
            defconfig,
            config,
            modules,
        } => {
            write_string_field(&mut out, "kind", "linux");
            write_string_field(&mut out, "defconfig", defconfig);
            if *modules {
                write_bool_field(&mut out, "modules", true);
            }
            if !config.is_empty() {
                write_array(&mut out, "config", config);
            }
        }
        BuildOptions::Cargo { path, flags } => {
            write_string_field(&mut out, "kind", "cargo");
            if path != "." {
                write_string_field(&mut out, "path", path);
            }
            if !flags.is_empty() {
                write_array(&mut out, "flags", flags);
            }
        }
        BuildOptions::Commands { commands } => {
            write_string_field(&mut out, "kind", "commands");
            out.push_str("commands = [\n");
            for words in commands {
                out.push_str("  [");
                for (index, word) in words.iter().enumerate() {
                    if index > 0 {
                        out.push_str(", ");
                    }
                    write_escaped(&mut out, word);
                }
                out.push_str("],\n");
            }
            out.push_str("]\n");
        }
        BuildOptions::Prebuilt { install } => {
            write_string_field(&mut out, "kind", "prebuilt");
            write_install(&mut out, install);
        }
    }
    out.push('\n');
    out.push_str("[dependencies]\n");
    let mut deps_build = record.deps_build.clone();
    deps_build.sort();
    write_array(&mut out, "build", &deps_build);
    let mut deps_run = record.deps_run.clone();
    deps_run.sort();
    write_array(&mut out, "run", &deps_run);
    if !record.deps_expose.is_empty() {
        let mut deps_expose = record.deps_expose.clone();
        deps_expose.sort();
        write_array(&mut out, "expose", &deps_expose);
    }
    for (name, output) in &record.outputs {
        out.push_str("\n[outputs.");
        if name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_".contains(&b))
        {
            out.push_str(name);
        } else {
            write_escaped(&mut out, name);
        }
        out.push_str("]\n");
        if !output.paths.is_empty() {
            write_array(&mut out, "paths", &output.paths);
        }
        write_array(&mut out, "run", &output.run);
    }
    out.push('\n');
    out.push_str("[check]\n");
    write_bool_field(&mut out, "enabled", record.check_enabled);
    let mut check_skip = record.check_skip.clone();
    check_skip.sort();
    write_array(&mut out, "skip", &check_skip);
    if let Some(target) = &record.check_target {
        write_string_field(&mut out, "target", target);
    }
    if record.private_cxx_runtime {
        write_bool_field(&mut out, "private-cxx-runtime", true);
    }
    if !record.links.is_empty() {
        out.push_str("\n[links]\n");
        for (path, target) in &record.links {
            write_escaped(&mut out, path);
            out.push_str(" = ");
            write_escaped(&mut out, target);
            out.push('\n');
        }
    }
    if !record.wrappers.is_empty() {
        out.push_str("\n[wrappers]\n");
        for (path, words) in &record.wrappers {
            write_escaped(&mut out, path);
            out.push_str(" = [");
            for (index, word) in words.iter().enumerate() {
                if index > 0 {
                    out.push_str(", ");
                }
                write_escaped(&mut out, word);
            }
            out.push_str("]\n");
        }
    }
    out
}

// --- entry points ---------------------------------------------------------

/// Read and validate a record file; on success return its canonical form.
pub fn check_file(path: &Path, dir_name: Option<&str>) -> Result<FileCheck, Vec<RecordError>> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| vec![error_at(path, format!("cannot read: {e}"))])?;
    let display = path.display().to_string();
    let parsed = parse_text(&text, path, dir_name).map_err(|msgs| {
        msgs.into_iter()
            .map(|message| RecordError {
                path: display.clone(),
                message,
            })
            .collect::<Vec<_>>()
    })?;
    let canonical = match &parsed {
        Parsed::Package(r) => canonical_package(r),
        Parsed::Version(r) => canonical_version(r),
    };
    Ok(FileCheck {
        changed: canonical != text,
        canonical,
    })
}

/// Read and validate a version record; return it with its canonical form.
pub(crate) fn load_version(path: &Path) -> Result<(VersionRecord, String), Vec<RecordError>> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| vec![error_at(path, format!("cannot read: {e}"))])?;
    let to_errors = |msgs: Errors| {
        msgs.into_iter()
            .map(|message| error_at(path, message))
            .collect::<Vec<_>>()
    };
    match parse_text(&text, path, None).map_err(to_errors)? {
        Parsed::Version(record) => {
            let canonical = canonical_version(&record);
            Ok((*record, canonical))
        }
        Parsed::Package(_) => Err(vec![error_at(path, "expected a version record".to_owned())]),
    }
}

pub(crate) fn error_at(path: &Path, message: String) -> RecordError {
    RecordError {
        path: path.display().to_string(),
        message,
    }
}

/// Parse and validate record text.
///
/// `kind` selects package-record or version-record rules; `stem` is the
/// expected `version` value (the file name without `.toml`); `dir_name` is the
/// expected `name` value for package records; `upstream_sources` carries the
/// package's `upstream.sources` for version records.
fn parse_text(text: &str, path: &Path, dir_name: Option<&str>) -> Result<Parsed, Errors> {
    let table = text
        .parse::<toml::Table>()
        .map_err(|e| vec![format!("invalid TOML: {e}")])?;
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    if name == "package.toml" {
        let (record, errors) = parse_package(&table, dir_name);
        if errors.is_empty() {
            Ok(Parsed::Package(record))
        } else {
            Err(errors)
        }
    } else if name.ends_with(".toml") {
        let stem = Path::new(name)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_owned();
        let sources = upstream_sources_of(path);
        let (record, errors) = parse_version(&table, &stem, sources);
        if errors.is_empty() {
            Ok(Parsed::Version(Box::new(record)))
        } else {
            Err(errors)
        }
    } else {
        Err(vec![format!(
            "file name \"{name}\": expected \"package.toml\" or \"<version>.toml\""
        )])
    }
}

/// The `upstream.sources` of the `package.toml` next to a `versions/` file.
fn upstream_sources_of(path: &Path) -> Option<Vec<String>> {
    let package_path = path.parent()?.parent()?.join("package.toml");
    let text = std::fs::read_to_string(package_path).ok()?;
    let table = text.parse::<toml::Table>().ok()?;
    let (record, errors) = parse_package(&table, None);
    errors.is_empty().then_some(record.sources)
}

fn version_stems(versions_dir: &Path) -> Result<Vec<String>, String> {
    let mut stems = Vec::new();
    let read_error = |error| format!("read {}: {error}", versions_dir.display());
    for entry in std::fs::read_dir(versions_dir).map_err(read_error)? {
        let entry = entry.map_err(read_error)?;
        if let Some(stem) = entry
            .file_name()
            .to_str()
            .and_then(|n| n.strip_suffix(".toml"))
        {
            stems.push(stem.to_owned());
        }
    }
    Ok(stems)
}

pub(crate) fn current_version(versions_dir: &Path) -> Result<String, String> {
    version_stems(versions_dir)?
        .into_iter()
        .reduce(|best, v| {
            if version::compare(&v, &best) == version::model::Ordering::Gt {
                v
            } else {
                best
            }
        })
        .ok_or_else(|| format!("{}: no version records", versions_dir.display()))
}

/// Check every record under `<root>/packages`.
pub fn check_tree(root: &Path) -> TreeReport {
    let mut report = TreeReport {
        packages: 0,
        versions: 0,
        errors: Vec::new(),
    };
    let packages_dir = root.join("packages");
    if !packages_dir.is_dir() {
        report.errors.push(error_at(
            &packages_dir,
            "expected a \"packages\" directory".to_owned(),
        ));
        return report;
    }
    let mut versions = BTreeMap::new();
    for shard_entry in sorted_entries(&packages_dir) {
        let shard = shard_entry.file_name().to_string_lossy().into_owned();
        if !shard_entry.path().is_dir() {
            report.errors.push(error_at(
                &shard_entry.path(),
                "expected a shard directory".to_owned(),
            ));
            continue;
        }
        check_shard_dir(&shard_entry.path(), &shard, &mut report, &mut versions);
    }
    output::check_tree(root, &versions, &mut report.errors);
    report
        .errors
        .sort_by(|a, b| a.path.cmp(&b.path).then(a.message.cmp(&b.message)));
    report
}

fn sorted_entries(dir: &Path) -> Vec<std::fs::DirEntry> {
    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .map(|rd| rd.flatten().collect())
        .unwrap_or_default();
    entries.sort_by_key(|e| e.file_name());
    entries
}

fn check_shard_dir(
    dir: &Path,
    shard: &str,
    report: &mut TreeReport,
    versions: &mut BTreeMap<PathBuf, VersionRecord>,
) {
    if shard.chars().count() != 2 {
        report.errors.push(error_at(
            dir,
            format!("expected a two-character shard directory, found \"{shard}\""),
        ));
    }
    for name_entry in sorted_entries(dir) {
        let name = name_entry.file_name().to_string_lossy().into_owned();
        if !name_entry.path().is_dir() {
            report.errors.push(error_at(
                &name_entry.path(),
                "expected a package directory".to_owned(),
            ));
            continue;
        }
        check_package_dir(&name_entry.path(), &name, shard, report, versions);
    }
}

fn check_package_dir(
    dir: &Path,
    name: &str,
    shard: &str,
    report: &mut TreeReport,
    versions: &mut BTreeMap<PathBuf, VersionRecord>,
) {
    if !model::valid_name(name) {
        report.errors.push(error_at(
            dir,
            format!("expected a valid package name, found \"{name}\""),
        ));
    } else if model::shard(name) != shard {
        report.errors.push(error_at(
            dir,
            format!(
                "expected shard \"{}\" for package \"{name}\"",
                model::shard(name)
            ),
        ));
    }
    let package_path = dir.join("package.toml");
    let mut upstream_sources: Option<Vec<String>> = None;
    let mut has_package = false;
    let mut has_versions = false;
    let mut has_patches = false;
    for entry in sorted_entries(dir) {
        let entry_name = entry.file_name().to_string_lossy().into_owned();
        match entry_name.as_str() {
            "package.toml" => {
                has_package = true;
                report.packages += 1;
                if !package_path.is_file() {
                    report
                        .errors
                        .push(error_at(&package_path, "expected a file".to_owned()));
                    continue;
                }
                let dir_name = dir.file_name().and_then(|n| n.to_str());
                match check_file(&package_path, dir_name) {
                    Ok(check) => {
                        if check.changed {
                            report
                                .errors
                                .push(error_at(&package_path, "not in canonical form".to_owned()));
                        }
                        if let Ok(text) = std::fs::read_to_string(&package_path)
                            && let Ok(table) = text.parse::<toml::Table>()
                        {
                            let (record, errors) = parse_package(&table, None);
                            if errors.is_empty() {
                                upstream_sources = Some(record.sources);
                            }
                        }
                    }
                    Err(errors) => report.errors.extend(errors),
                }
            }
            "versions" | "patches" if !entry.path().is_dir() => {
                report.errors.push(error_at(
                    &entry.path(),
                    format!("expected a directory, found \"{entry_name}\""),
                ));
            }
            "versions" => has_versions = true,
            "patches" => has_patches = true,
            _ => {
                report.errors.push(error_at(
                    &entry.path(),
                    format!("unexpected entry \"{entry_name}\""),
                ));
            }
        }
    }
    if !has_package {
        report
            .errors
            .push(error_at(&package_path, "required file missing".to_owned()));
    }
    let mut version_stems = Vec::new();
    if has_versions {
        let versions_dir = dir.join("versions");
        for entry in sorted_entries(&versions_dir) {
            let entry_name = entry.file_name().to_string_lossy().into_owned();
            let entry_path = entry.path();
            if !entry_path.is_file() || !entry_name.ends_with(".toml") {
                report.errors.push(error_at(
                    &entry_path,
                    format!("expected a \"<version>.toml\" file, found \"{entry_name}\""),
                ));
                continue;
            }
            let stem = Path::new(&entry_name)
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("");
            version_stems.push(stem.to_owned());
        }
    }
    let mut patch_names = Vec::new();
    if has_patches {
        let patches_dir = dir.join("patches");
        for entry in sorted_entries(&patches_dir) {
            let entry_name = entry.file_name().to_string_lossy().into_owned();
            let entry_path = entry.path();
            if !entry_path.is_file() || !entry_name.ends_with(".patch") {
                report.errors.push(error_at(
                    &entry_path,
                    format!("expected a \"<version>-<slug>.patch\" file, found \"{entry_name}\""),
                ));
                continue;
            }
            check_patch_file(&entry_name, &version_stems, &entry_path, report);
            patch_names.push(entry_name);
        }
    }
    if has_versions {
        let versions_dir = dir.join("versions");
        for entry in sorted_entries(&versions_dir) {
            let entry_name = entry.file_name().to_string_lossy().into_owned();
            if !entry.path().is_file() || !entry_name.ends_with(".toml") {
                continue;
            }
            check_version_file(
                &entry.path(),
                upstream_sources.as_deref(),
                &patch_names,
                report,
                versions,
            );
        }
    }
}

fn check_patch_file(
    file_name: &str,
    version_stems: &[String],
    path: &Path,
    report: &mut TreeReport,
) {
    let ok = version_stems.iter().any(|version| {
        file_name
            .strip_prefix(&format!("{version}-"))
            .and_then(|rest| rest.strip_suffix(".patch"))
            .is_some_and(|slug| !slug.is_empty())
    });
    if !ok {
        report.errors.push(error_at(
            path,
            format!("expected a \"<version>-<slug>.patch\" name, found \"{file_name}\""),
        ));
    }
}

fn check_version_file(
    path: &Path,
    upstream_sources: Option<&[String]>,
    patch_names: &[String],
    report: &mut TreeReport,
    versions: &mut BTreeMap<PathBuf, VersionRecord>,
) {
    report.versions += 1;
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_owned();
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) => {
            report
                .errors
                .push(error_at(path, format!("cannot read: {e}")));
            return;
        }
    };
    let table = match text.parse::<toml::Table>() {
        Ok(t) => t,
        Err(e) => {
            report
                .errors
                .push(error_at(path, format!("invalid TOML: {e}")));
            return;
        }
    };
    let (record, errors) = parse_version(&table, &stem, upstream_sources.map(<[String]>::to_vec));
    let had_errors = !errors.is_empty();
    report
        .errors
        .extend(errors.into_iter().map(|message| RecordError {
            path: path.display().to_string(),
            message,
        }));
    if had_errors {
        return;
    }
    for patch in &record.patches {
        if !patch_names.contains(patch) {
            report.errors.push(error_at(
                path,
                format!("patches: file \"{patch}\" not found in patches/"),
            ));
        }
    }
    let canonical = canonical_version(&record);
    if canonical != text {
        report
            .errors
            .push(error_at(path, "not in canonical form".to_owned()));
    }
    versions.insert(path.to_owned(), record);
}

#[cfg(test)]
mod tests {
    use super::*;

    const MESSY_PACKAGE: &str = "# the hello package
license = \"GPL-3.0-or-later\"
name = \"hello\"


[upstream]
sources = [
  \"https://ftp.gnu.org/gnu/hello/\",   # the main mirror
  \"https://alpha.gnu.org/gnu/hello/\",
]
homepage = \"https://www.gnu.org/software/hello/\"
";

    const CANONICAL_PACKAGE: &str = "name = \"hello\"
license = \"GPL-3.0-or-later\"

[upstream]
homepage = \"https://www.gnu.org/software/hello/\"
sources = [
  \"https://alpha.gnu.org/gnu/hello/\",
  \"https://ftp.gnu.org/gnu/hello/\",
]
";

    #[test]
    fn package_canonical_bytes() {
        let table = MESSY_PACKAGE.parse::<toml::Table>().unwrap();
        let (record, errors) = parse_package(&table, Some("hello"));
        assert_eq!(errors, Vec::<String>::new());
        assert_eq!(canonical_package(&record), CANONICAL_PACKAGE);
    }

    #[test]
    fn package_canonical_idempotent() {
        let table = CANONICAL_PACKAGE.parse::<toml::Table>().unwrap();
        let (record, errors) = parse_package(&table, Some("hello"));
        assert_eq!(errors, Vec::<String>::new());
        assert_eq!(canonical_package(&record), CANONICAL_PACKAGE);
    }

    fn version_record() -> VersionRecord {
        VersionRecord {
            version: "2.12.2".to_owned(),
            patches: Vec::new(),
            url: "https://ftp.gnu.org/gnu/hello/hello-2.12.2.tar.gz".to_owned(),
            sha256: "5a9a996dc292cc24dcf411cee87e92f6aae5b8d13bd9c6819b4c7a9dce0818ab".to_owned(),
            strip_components: 1,
            build: BuildOptions::Autotools {
                path: ".".to_owned(),
                autoreconf: false,
                configure_flags: vec!["--disable-nls".to_owned()],
                make_flags: Vec::new(),
                install_flags: Vec::new(),
                install_targets: Vec::new(),
                configure_command: Vec::new(),
            },
            deps_build: vec!["make".to_owned()],
            deps_run: Vec::new(),
            deps_expose: Vec::new(),
            outputs: BTreeMap::new(),
            check_enabled: true,
            check_target: None,
            check_skip: Vec::new(),
            private_cxx_runtime: false,
            links: Vec::new(),
            wrappers: Vec::new(),
        }
    }

    const CANONICAL_VERSION: &str = "version = \"2.12.2\"

[source]
url = \"https://ftp.gnu.org/gnu/hello/hello-2.12.2.tar.gz\"
sha256 = \"5a9a996dc292cc24dcf411cee87e92f6aae5b8d13bd9c6819b4c7a9dce0818ab\"

[build]
kind = \"autotools\"
configure-flags = [
  \"--disable-nls\",
]

[dependencies]
build = [
  \"make\",
]
run = []

[check]
enabled = true
skip = []
";

    #[test]
    fn version_canonical_bytes() {
        assert_eq!(canonical_version(&version_record()), CANONICAL_VERSION);
    }

    #[test]
    fn autotools_autoreconf_is_opt_in_and_requires_build_dependencies() {
        let mut table = CANONICAL_VERSION.parse::<toml::Table>().unwrap();
        let build = table["build"].as_table_mut().unwrap();
        build.insert("autoreconf".to_owned(), false.into());
        let parse = |table: &toml::Table| {
            parse_version(
                table,
                "2.12.2",
                Some(vec!["https://ftp.gnu.org/gnu/hello/".to_owned()]),
            )
        };
        let (record, errors) = parse(&table);
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(canonical_version(&record), CANONICAL_VERSION);

        table["build"]["autoreconf"] = "true".into();
        let (_, errors) = parse(&table);
        assert_eq!(errors, ["build.autoreconf: expected a boolean"]);

        table["build"]["autoreconf"] = true.into();
        let (_, errors) = parse(&table);
        assert_eq!(
            errors,
            [
                "build.autoreconf: dependencies.build must include autoconf",
                "build.autoreconf: dependencies.build must include automake",
                "build.autoreconf: dependencies.build must include libtool",
            ]
        );
        table["dependencies"]["build"] = toml::Value::Array(
            ["autoconf", "automake", "libtool", "make"]
                .map(Into::into)
                .to_vec(),
        );
        let (record, errors) = parse(&table);
        assert!(errors.is_empty(), "{errors:?}");
        let canonical = canonical_version(&record);
        assert!(canonical.contains("kind = \"autotools\"\nautoreconf = true\n"));
        let (record, errors) = parse(&canonical.parse().unwrap());
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(canonical_version(&record), canonical);
    }

    #[test]
    fn version_canonical_idempotent() {
        let table = CANONICAL_VERSION.parse::<toml::Table>().unwrap();
        let (record, errors) = parse_version(
            &table,
            "2.12.2",
            Some(vec!["https://ftp.gnu.org/gnu/hello/".to_owned()]),
        );
        assert_eq!(errors, Vec::<String>::new());
        assert_eq!(canonical_version(&record), CANONICAL_VERSION);
    }

    #[test]
    fn prebuilt_canonical_bytes() {
        let record = VersionRecord {
            version: "0.9".to_owned(),
            patches: vec!["0.9-fix.patch".to_owned()],
            url: "https://dl.example.org/xfetch/xfetch-0.9.tar.gz".to_owned(),
            sha256: "5a9a996dc292cc24dcf411cee87e92f6aae5b8d13bd9c6819b4c7a9dce0818ab".to_owned(),
            strip_components: 0,
            build: BuildOptions::Prebuilt {
                install: vec![("bin/xfetch".to_owned(), "bin/xfetch".to_owned())],
            },
            deps_build: Vec::new(),
            deps_run: vec!["ca-certificates".to_owned()],
            deps_expose: Vec::new(),
            outputs: BTreeMap::new(),
            check_enabled: false,
            check_target: None,
            check_skip: Vec::new(),
            private_cxx_runtime: false,
            links: Vec::new(),
            wrappers: Vec::new(),
        };
        assert_eq!(
            canonical_version(&record),
            "version = \"0.9\"
patches = [
  \"0.9-fix.patch\",
]

[source]
url = \"https://dl.example.org/xfetch/xfetch-0.9.tar.gz\"
sha256 = \"5a9a996dc292cc24dcf411cee87e92f6aae5b8d13bd9c6819b4c7a9dce0818ab\"
strip-components = 0

[build]
kind = \"prebuilt\"
install = [
  { from = \"bin/xfetch\", to = \"bin/xfetch\" },
]

[dependencies]
build = []
run = [
  \"ca-certificates\",
]

[check]
enabled = false
skip = []
"
        );
    }

    #[test]
    fn messy_version_canonical() {
        let text = "# release 2.12.2
patches = []
version = \"2.12.2\"

[check]
skip = []
enabled = true

[source]
strip-components = 1
sha256 = \"5a9a996dc292cc24dcf411cee87e92f6aae5b8d13bd9c6819b4c7a9dce0818ab\"
url = \"https://ftp.gnu.org/gnu/hello/hello-2.12.2.tar.gz\"


[build]
kind = \"autotools\"
make-flags = []
configure-flags = [
  \"--disable-nls\",
]

[dependencies]
run = []
build = [
  \"make\",
]
";
        let table = text.parse::<toml::Table>().unwrap();
        let (record, errors) = parse_version(
            &table,
            "2.12.2",
            Some(vec!["https://ftp.gnu.org/gnu/hello/".to_owned()]),
        );
        assert_eq!(errors, Vec::<String>::new());
        assert_eq!(canonical_version(&record), CANONICAL_VERSION);
    }

    #[test]
    fn string_escapes() {
        let mut out = String::new();
        write_escaped(&mut out, "a\"b\\c\nd");
        assert_eq!(out, "\"a\\\"b\\\\c\\u000ad\"");
    }
}
