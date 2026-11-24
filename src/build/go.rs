use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::time::{Duration, Instant};

use base64::{Engine, engine::general_purpose::STANDARD};
use sha2::{Digest, Sha256};
use vug::{fetch, steps};

use super::{io, run_sandbox};

#[derive(Default)]
struct Module<'a> {
    manifest: Option<&'a str>,
    archive: Option<&'a str>,
}

fn escape(value: &str) -> String {
    let mut escaped = String::new();
    for ch in value.chars() {
        if ch.is_ascii_uppercase() {
            escaped.push('!');
        }
        escaped.push(ch.to_ascii_lowercase());
    }
    escaped
}

fn valid_component(value: &str) -> bool {
    !value.is_empty()
        && !value.starts_with('.')
        && !value.ends_with('.')
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._+-~".contains(&b))
}

fn read_sum(path: &Path) -> Result<String, String> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_file() => io("read", path, fs::read_to_string(path)),
        Ok(_) => Err("Go go.sum must be a regular file".to_owned()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(e) => Err(format!("Go go.sum: {e}")),
    }
}

fn modules(sum: &str) -> Result<BTreeMap<(&str, &str), Module<'_>>, String> {
    let mut modules = BTreeMap::<_, Module>::new();
    for (index, line) in sum.lines().enumerate() {
        let fields: Vec<_> = line.split_whitespace().collect();
        let [path, version, hash] = fields.as_slice() else {
            return Err(format!(
                "go.sum:{}: expected module, version and hash",
                index + 1
            ));
        };
        let (version, manifest) = version
            .strip_suffix("/go.mod")
            .map_or((*version, false), |version| (version, true));
        if !path.split('/').all(valid_component)
            || !version.starts_with('v')
            || !valid_component(version)
            || !hash.strip_prefix("h1:").is_some_and(|encoded| {
                STANDARD
                    .decode(encoded)
                    .is_ok_and(|bytes| bytes.len() == 32)
            })
        {
            return Err(format!(
                "go.sum:{}: invalid module, version or hash",
                index + 1
            ));
        }
        let module = modules.entry((*path, version)).or_default();
        let expected = if manifest {
            &mut module.manifest
        } else {
            &mut module.archive
        };
        if expected.replace(hash).is_some_and(|old| old != *hash) {
            return Err(format!("go.sum: conflicting hashes for {path}@{version}"));
        }
    }
    Ok(modules)
}

fn download(agent: &ureq::Agent, url: &str) -> Result<impl std::io::Read, String> {
    let mut attempts = 0;
    loop {
        attempts += 1;
        match agent.get(url).call() {
            Ok(response) if response.status().as_u16() == 200 => {
                return Ok(response
                    .into_body()
                    .into_with_config()
                    .limit(u64::MAX)
                    .reader());
            }
            Ok(response) => return Err(format!("fetch {url}: status {}", response.status())),
            Err(e)
                if attempts == 3
                    || matches!(&e, ureq::Error::StatusCode(code) if (400..500).contains(code) && *code != 429) =>
            {
                return Err(format!("fetch {url}: {e}"));
            }
            Err(_) => std::thread::sleep(Duration::from_secs(attempts)),
        }
    }
}

fn populate_proxy(
    proxy: &Path,
    modules: &BTreeMap<(&str, &str), Module<'_>>,
) -> Result<(), String> {
    let started = Instant::now();
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_connect(Some(Duration::from_secs(10)))
        .timeout_recv_response(Some(Duration::from_secs(30)))
        .timeout_recv_body(Some(Duration::from_secs(180)))
        .build()
        .into();
    let mut bytes = 0;
    for (&(name, version), module) in modules {
        let basename = format!("{}/@v/{}", escape(name), escape(version));
        let dir = proxy.join(escape(name)).join("@v");
        io("create", &dir, fs::create_dir_all(&dir))?;
        let manifest = module
            .manifest
            .ok_or_else(|| format!("go.sum: missing go.mod hash for {name}@{version}"))?;
        let extensions: &[_] = if module.archive.is_some() {
            &["mod", "info", "zip"]
        } else {
            &["mod", "info"]
        };
        for ext in extensions {
            let url = format!("https://proxy.golang.org/{basename}.{ext}");
            let mut source = download(&agent, &url)?;
            let dest = proxy.join(format!("{basename}.{ext}"));
            if *ext == "mod" {
                let mut body = Vec::new();
                use std::io::Read;
                source
                    .take((16 << 20) + 1)
                    .read_to_end(&mut body)
                    .map_err(|e| format!("fetch {url}: {e}"))?;
                if body.len() > 16 << 20 {
                    return Err(format!("fetch {url}: go.mod exceeds 16 MiB"));
                }
                let hex = fetch::sha256_reader(body.as_slice()).map_err(|e| e.to_string())?;
                let entry = format!("{hex}  go.mod\n");
                let actual = format!("h1:{}", STANDARD.encode(Sha256::digest(entry)));
                if actual != manifest {
                    return Err(format!(
                        "go.sum: {name}@{version}/go.mod: expected {manifest}, got {actual}"
                    ));
                }
                io("write", &dest, fs::write(&dest, &body))?;
                bytes += body.len() as u64;
            } else {
                let mut file = io("create", &dest, fs::File::create(&dest))?;
                bytes += io("download", &dest, std::io::copy(&mut source, &mut file))?;
            }
        }
    }
    eprintln!(
        "Go proxy: {} module versions, {bytes} bytes, {:.2}s",
        modules.len(),
        started.elapsed().as_secs_f64()
    );
    Ok(())
}

pub(super) fn prepare(
    build_dir: &Path,
    sandbox: &steps::Sandbox,
    goroot: &str,
    targets: &[(String, String)],
    check: bool,
    jobs: u32,
    offline: bool,
) -> Result<bool, String> {
    let source = build_dir.join("src");
    let manifest_path = source.join("go.mod");
    let manifest = io("stat", &manifest_path, fs::symlink_metadata(&manifest_path))?;
    if !manifest.is_file() {
        return Err("Go go.mod must be a regular file".to_owned());
    }
    if source.join("vendor").is_dir() {
        return Ok(true);
    }

    let sum_path = source.join("go.sum");
    let sum = read_sum(&sum_path)?;
    let modules = modules(&sum)?;
    if offline && !modules.is_empty() {
        return Err("offline sources: Go module dependencies must be vendored".into());
    }
    if !offline {
        populate_proxy(&build_dir.join("go-proxy"), &modules)?;
    }
    let go = steps::go_command(goroot, jobs, "file:///build/go-proxy");
    let mut script = Vec::new();
    let archives: Vec<_> = modules
        .iter()
        .filter(|(_, module)| module.archive.is_some())
        .map(|((name, version), _)| format!("{name}@{version}"))
        .collect();
    for batch in archives.chunks(128) {
        let mut command = go.clone();
        command.extend(["mod", "download"].map(str::to_owned));
        command.extend_from_slice(batch);
        script.push(command);
    }
    let mut list = go;
    list.extend(["list", "-mod=readonly", "-deps", "-buildvcs=false"].map(str::to_owned));
    if check {
        list.extend(["-test", "./..."].map(str::to_owned));
    }
    list.extend(targets.iter().map(|(path, _)| path.clone()));
    script.push(list);
    let path = build_dir.join("build.sh");
    let mut script = steps::shell_script(&script);
    script.pop();
    script.push_str(" >/dev/null\n");
    io("write", &path, fs::write(&path, script))?;
    run_sandbox(sandbox).map_err(|e| format!("Go dependency verification: {e}"))?;
    for (&(name, version), module) in &modules {
        if let Some(expected) = module.archive {
            let hash_path = build_dir
                .join("go-modcache/cache/download")
                .join(escape(name))
                .join("@v")
                .join(format!("{}.ziphash", escape(version)));
            let actual = io("read", &hash_path, fs::read_to_string(&hash_path))?;
            if actual != expected {
                return Err(format!(
                    "go.sum: {name}@{version}: expected {expected}, got {actual}"
                ));
            }
        }
    }
    if read_sum(&sum_path)? != sum {
        return Err("Go dependency verification: go.sum lacks required checksums".to_owned());
    }
    Ok(false)
}
