use super::{fetch, steps::Step};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::Value;
use std::{collections::BTreeMap, fs, path::Path};

pub(super) fn fetch_locked(build_dir: &Path, cache: &Path) -> Result<Vec<String>, String> {
    let lock_path = build_dir.join("src/package-lock.json");
    if !fs::symlink_metadata(&lock_path).is_ok_and(|metadata| metadata.is_file()) {
        return Err("package-lock.json: expected a regular file".to_owned());
    }
    let lock: Value = serde_json::from_slice(
        &fs::read(&lock_path).map_err(|e| format!("package-lock.json: {e}"))?,
    )
    .map_err(|e| format!("package-lock.json: {e}"))?;
    if lock["lockfileVersion"] != 3 {
        return Err("package-lock.json: expected lockfileVersion 3".to_owned());
    }
    let packages = lock["packages"]
        .as_object()
        .filter(|packages| packages.get("").is_some_and(Value::is_object))
        .ok_or("package-lock.json: expected packages with a root entry")?;
    let mut locked = BTreeMap::new();
    for (name, package) in packages.iter().filter(|(name, _)| !name.is_empty()) {
        let invalid = || {
            format!(
                "package-lock.json: {name}: expected npm registry tarball with SHA-512 integrity"
            )
        };
        let url = package["resolved"]
            .as_str()
            .filter(|url| url.starts_with("https://registry.npmjs.org/") && url.ends_with(".tgz"))
            .ok_or_else(invalid)?;
        let checksum = package["integrity"]
            .as_str()
            .and_then(|integrity| integrity.strip_prefix("sha512-"))
            .and_then(|encoded| STANDARD.decode(encoded).ok())
            .filter(|digest| digest.len() == 64)
            .ok_or_else(invalid)?;
        let supports = |key: &str, platform: &str| {
            package[key].as_array().is_none_or(|values| {
                !values.iter().any(|value| {
                    value
                        .as_str()
                        .is_some_and(|value| value.strip_prefix('!') == Some(platform))
                }) && (values
                    .iter()
                    .all(|value| value.as_str().is_some_and(|value| value.starts_with('!')))
                    || values.iter().any(|value| value == platform))
            })
        };
        let arch = match std::env::consts::ARCH {
            "x86_64" => "x64",
            "aarch64" => "arm64",
            arch => arch,
        };
        if package["optional"] == true
            && !(supports("os", "linux") && supports("cpu", arch) && supports("libc", "musl"))
        {
            continue;
        }
        let digest: String = checksum.iter().map(|byte| format!("{byte:02x}")).collect();
        locked.entry(digest).or_insert(url);
    }

    let dir = build_dir.join("npm-tarballs");
    fs::create_dir_all(&dir).map_err(|e| format!("npm tarballs: {e}"))?;
    let mut archives = Vec::new();
    for (digest, url) in locked {
        let file = fetch::fetch_sha512(url, &digest, cache).map_err(|e| format!("{url}: {e:?}"))?;
        let name = format!("{digest}.tgz");
        fs::copy(file, dir.join(&name)).map_err(|e| format!("npm tarballs: {e}"))?;
        archives.push(format!("/build/npm-tarballs/{name}"));
    }
    Ok(archives)
}

pub(super) fn steps(
    prefix: &str,
    node: &str,
    npm: &str,
    archives: &[String],
    relocations: &[Step],
    build: bool,
    test: bool,
) -> Vec<Step> {
    let npm = |args: &[&str]| {
        [
            "env".to_owned(),
            "npm_config_cache=/build/npm-cache".to_owned(),
            "npm_config_offline=true".to_owned(),
            "npm_config_ignore_scripts=true".to_owned(),
            "npm_config_audit=false".to_owned(),
            "npm_config_fund=false".to_owned(),
            "npm_config_libc=musl".to_owned(),
            format!("{npm}/bin/npm"),
        ]
        .into_iter()
        .chain(args.iter().map(|arg| (*arg).to_owned()))
        .collect()
    };
    let mut steps = Vec::new();
    for batch in archives.chunks(3) {
        let mut args = vec!["cache", "add"];
        args.extend(batch.iter().map(String::as_str));
        steps.push(npm(&args));
    }
    steps.push(npm(&["ci", "--include=dev"]));
    steps.extend_from_slice(relocations);
    if build {
        steps.push(npm(&["run", "build"]));
    }
    if test {
        steps.push(npm(&["test"]));
    }
    steps.push(npm(&["ci", "--omit=dev"]));
    steps.extend_from_slice(relocations);
    steps.push(vec![
        format!("{node}/bin/node"),
        "--input-type=commonjs".to_owned(),
        "-e".to_owned(),
        include_str!("node.cjs").to_owned(),
        prefix.to_owned(),
        format!("{node}/bin/node"),
    ]);
    steps
}
