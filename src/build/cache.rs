use std::collections::BTreeMap;
use std::fs;
use std::io::{ErrorKind, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use super::outputs::{Manifest, Runtime};
use super::{PRIVATE_OUTPUT, Settings, fetch, io, single_output_index, valid_store_id};
use crate::naming::model;

pub(super) fn fetch_manifest(
    settings: &Settings,
    key: &str,
    runtime: &BTreeMap<String, Runtime>,
) -> Result<Option<Manifest>, String> {
    let (Some(base), Some(trusted)) = (&settings.binary_cache, &settings.trusted_key) else {
        return Ok(None);
    };
    let url = format!("{}/output-keys/{key}", base.trim_end_matches('/'));
    let Some(bytes) = fetch::get(&url).map_err(|error| format!("fetch {url}: {error}"))? else {
        return Ok(None);
    };
    let text = std::str::from_utf8(&bytes).map_err(|error| format!("{url}: {error}"))?;
    let (json, signature) = text
        .split_once('\n')
        .ok_or_else(|| format!("{url}: invalid signed output manifest"))?;
    if !vug::sign::verify(trusted, json, signature) {
        return Err(format!("{url}: not signed by the trusted key"));
    }
    let manifest = Manifest::parse(&bytes[..=json.len()], &url)?;
    if manifest.private {
        return Err(format!("{url}: private output manifest in binary cache"));
    }
    manifest.validate(key, runtime, &url)?;
    Ok(Some(manifest))
}

/// Writes cache entries and returns the numbers of keys and archives written.
pub fn export_cache(
    data_dir: &Path,
    directory: &Path,
    secret: Option<&str>,
) -> Result<(usize, usize), String> {
    let keys_dir = directory.join("keys");
    let output_keys = directory.join("output-keys");
    let paths_dir = directory.join("paths");
    for dir in [&keys_dir, &output_keys, &paths_dir] {
        io("create", dir, fs::create_dir_all(dir))?;
    }
    let store = data_dir.join("store");
    let (mut keys, mut paths) = (0, 0);
    for version in index_files(&data_dir.join("index"))? {
        let line = io("read", &version, fs::read_to_string(&version))?;
        let Some((key, id, private)) = single_output_index(&line) else {
            continue;
        };
        if !valid_store_id(key) || !valid_store_id(id) {
            return Err(format!(
                "{}: invalid build key or store id",
                version.display()
            ));
        }
        if private
            || private_output(&store, id)?
            || !write_archive(&store, &paths_dir, id, &mut paths)?
        {
            continue;
        }
        let mut answer = format!("{id}\n");
        if let Some(secret) = secret {
            answer.push_str(&vug::sign::sign(secret, &format!("{key} {id}"))?);
            answer.push('\n');
        }
        publish(&keys_dir.join(key), answer.as_bytes())?;
        keys += 1;
    }
    for version in index_files(&data_dir.join("index-outputs"))? {
        let bytes = io("read", &version, fs::read(&version))?;
        let manifest = Manifest::parse(&bytes, &version.display().to_string())?;
        if manifest.private {
            continue;
        }
        let private = manifest.outputs.values().try_fold(false, |found, entry| {
            private_output(&store, &entry.id).map(|private| found || private)
        })?;
        if private {
            continue;
        }
        let mut complete = true;
        for entry in manifest.outputs.values() {
            complete &= write_archive(&store, &paths_dir, &entry.id, &mut paths)?;
        }
        if complete {
            let mut answer = bytes;
            if let Some(secret) = secret {
                let json = std::str::from_utf8(&answer[..answer.len() - 1])
                    .map_err(|error| format!("{}: {error}", version.display()))?;
                let signature = vug::sign::sign(secret, json)?;
                answer.extend_from_slice(signature.as_bytes());
                answer.push(b'\n');
            }
            publish(&output_keys.join(manifest.key), &answer)?;
            keys += 1;
        }
    }
    Ok((keys, paths))
}

fn private_output(store: &Path, id: &str) -> Result<bool, String> {
    let path = store.join(id).join(PRIVATE_OUTPUT);
    match fs::symlink_metadata(&path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(false),
        Err(error) => Err(format!("check {}: {error}", path.display())),
    }
}

fn index_files(index: &Path) -> Result<Vec<PathBuf>, String> {
    let entries = match fs::read_dir(index) {
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
        result => io("read", index, result)?,
    };
    let mut files = Vec::new();
    for entry in entries {
        let package = io("read entry", index, entry)?;
        if !package.file_name().to_str().is_some_and(model::valid_name) {
            continue;
        }
        let package = package.path();
        for entry in io("read", &package, fs::read_dir(&package))? {
            let entry = io("read entry", &package, entry)?;
            if entry.file_name().to_str().is_some_and(model::valid_version) {
                files.push(entry.path());
            }
        }
    }
    files.sort();
    Ok(files)
}

/// The zstd level of the archives under `paths/`.
const ARCHIVE_LEVEL: i32 = 3;

fn write_archive(
    store: &Path,
    paths: &Path,
    id: &str,
    written: &mut usize,
) -> Result<bool, String> {
    let archive = paths.join(format!("{id}.cpio.zst"));
    if archive.is_file() {
        return Ok(true);
    }
    let tree = store.join(id);
    if !tree.is_dir() {
        return Ok(false);
    }
    let mut temp = cache_file(paths)?;
    let mut out = io(
        "compress archive",
        &archive,
        zstd::Encoder::new(std::io::BufWriter::new(temp.as_file_mut()), ARCHIVE_LEVEL),
    )?;
    io(
        "write archive",
        &archive,
        vug::cpio::write_newc(&tree, &mut out),
    )?;
    let mut out = io("compress archive", &archive, out.finish())?;
    io("flush archive", &archive, out.flush())?;
    drop(out);
    match temp.persist_noclobber(&archive) {
        Ok(_) => *written += 1,
        Err(error) if error.error.kind() == ErrorKind::AlreadyExists && archive.is_file() => {}
        Err(error) => return Err(format!("publish {}: {error}", archive.display())),
    }
    Ok(true)
}

pub(super) fn publish(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let mut temp = cache_file(path.parent().unwrap())?;
    io("write", path, temp.write_all(bytes))?;
    temp.persist(path)
        .map_err(|error| format!("publish {}: {error}", path.display()))?;
    Ok(())
}

fn cache_file(dir: &Path) -> Result<tempfile::NamedTempFile, String> {
    io(
        "create temporary cache file",
        dir,
        tempfile::Builder::new()
            .permissions(fs::Permissions::from_mode(0o666))
            .tempfile_in(dir),
    )
}

#[cfg(test)]
mod tests {
    use super::{ARCHIVE_LEVEL, Manifest, Settings, export_cache, fs, model};
    use crate::build::{Builder, build_key, install_output, list_cache, outputs, placeholder_path};
    use crate::record;
    use std::collections::{BTreeMap, BTreeSet};
    use std::io::{BufRead, BufReader, Write};
    use std::net::{SocketAddr, TcpListener, TcpStream};
    use std::path::{Path, PathBuf};
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    use std::thread::{self, JoinHandle};

    #[test]
    fn signed_cache_restores_the_selected_runtime_closure_and_reads_legacy_keys() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let mut settings = Settings {
            root: root.to_owned(),
            data_dir: root.join("producer"),
            cache_dir: root.join("sources"),
            source_cache_dir: None,
            jobs: 1,
            binary_cache: None,
            trusted_key: None,
        };
        fs::write(root.join("seed.toml"), "").unwrap();
        let seed = Builder::new(&settings).unwrap().seed_id;
        let store = settings.data_dir.join("store");

        let (_, canonical) = write_record(root, "legacy", "");
        let legacy_key = build_key(&canonical, &seed, &BTreeSet::new(), None);
        let tree = root.join("legacy-tree");
        fs::create_dir(&tree).unwrap();
        fs::write(tree.join("file"), "legacy").unwrap();
        let legacy = install_output(&store, &tree).unwrap();
        let index = settings.data_dir.join("index/legacy");
        fs::create_dir_all(&index).unwrap();
        fs::write(index.join("1"), format!("{legacy_key} {legacy}\n")).unwrap();

        let (record, canonical) = write_record(
            root,
            "split",
            r#"
[outputs.out]
run = [":lib"]

[outputs.lib]
paths = ["lib/**"]

[outputs.dev]
paths = ["include/**"]
"#,
        );
        let key = build_key(&canonical, &seed, &BTreeSet::new(), Some(&BTreeSet::new()));
        let work = root.join("work");
        let staging = work.join("stage");
        for dir in ["bin", "lib", "include"] {
            fs::create_dir_all(staging.join(dir)).unwrap();
        }
        fs::write(staging.join("bin/program"), placeholder_path()).unwrap();
        fs::write(staging.join("lib/shared"), "runtime").unwrap();
        fs::write(staging.join("include/header"), "development").unwrap();
        let runtime = outputs::runtime(&record, &BTreeMap::new());
        let manifest = outputs::SplitBuild {
            staging: &staging,
            work: &work,
            store: &store,
            record: &record,
            canonical: &canonical,
            runtime: &runtime,
            auto_rpaths: &BTreeSet::new(),
            private: false,
        }
        .install(&key)
        .unwrap();
        manifest
            .publish(&settings.data_dir.join("index-outputs/split/1"), &runtime)
            .unwrap();
        let out = &manifest.outputs["out"].id;
        let lib = &manifest.outputs["lib"].id;
        let dev = &manifest.outputs["dev"].id;

        let (secret, public) = vug::sign::generate().unwrap();
        let cache = root.join("binary-cache");
        assert_eq!(
            export_cache(&settings.data_dir, &cache, Some(&secret)),
            Ok((2, 4))
        );
        let server = FileServer::start(cache.clone());
        settings.binary_cache = Some(format!("http://{}", server.addr));
        settings.trusted_key = Some(public);
        settings.data_dir = root.join("consumer");

        let reachable = list_cache(&settings).unwrap();
        assert_eq!(
            reachable,
            BTreeSet::from([
                format!("keys/{legacy_key}"),
                format!("output-keys/{key}"),
                format!("paths/{legacy}.cpio.zst"),
                format!("paths/{out}.cpio.zst"),
                format!("paths/{lib}.cpio.zst"),
                format!("paths/{dev}.cpio.zst"),
            ])
        );
        assert!(!settings.data_dir.exists());
        assert_eq!(server.requests.load(Ordering::Relaxed), 2);
        let mut builder = Builder::new(&settings).unwrap();
        let built = builder.build("split", None).unwrap();
        assert_eq!(server.requests.load(Ordering::Relaxed), 5);
        assert!(!built.fresh);
        assert_eq!(built.closure(), &BTreeSet::from([out.clone(), lib.clone()]));
        let store = settings.data_dir.join("store");
        assert_eq!(
            fs::read_to_string(store.join(out).join("bin/program")).unwrap(),
            format!("/vug/store/{out}")
        );
        assert!(store.join(lib).join("lib/shared").is_file());
        assert!(!store.join(dev).exists());
        assert_eq!(builder.build("legacy", None).unwrap().id, legacy);
        assert_eq!(server.requests.load(Ordering::Relaxed), 7);
        assert_eq!(
            Manifest::read(
                &settings.data_dir.join("index-outputs/split/1"),
                &key,
                &runtime
            )
            .unwrap(),
            Some(manifest)
        );
        assert_eq!(
            export_cache(
                &settings.data_dir,
                &root.join("partial-export"),
                Some(&secret)
            ),
            Ok((1, 3))
        );
        assert!(!root.join("partial-export/output-keys").join(&key).exists());
    }

    #[test]
    fn verifies_the_manifest_before_using_it_and_handles_missing_archives() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let cache = root.join("binary-cache");
        fs::create_dir_all(cache.join("output-keys")).unwrap();
        fs::create_dir_all(cache.join("paths")).unwrap();
        let server = FileServer::start(cache.clone());
        let (secret, public) = vug::sign::generate().unwrap();
        let settings = Settings {
            root: root.to_owned(),
            data_dir: root.join("consumer"),
            cache_dir: root.join("sources"),
            source_cache_dir: None,
            jobs: 1,
            binary_cache: Some(format!("http://{}", server.addr)),
            trusted_key: Some(public),
        };
        fs::write(root.join("seed.toml"), "").unwrap();
        let mut builder = Builder::new(&settings).unwrap();
        let mut cached = Builder::cached(&settings).unwrap();
        let (_, canonical) = write_record(root, "split", "[outputs.out]\n");
        let key = build_key(
            &canonical,
            &builder.seed_id,
            &BTreeSet::new(),
            Some(&BTreeSet::new()),
        );
        let id = "a".repeat(32);
        let json =
            format!(r#"{{"format":1,"key":"{key}","outputs":{{"out":{{"id":"{id}","run":[]}}}}}}"#);
        let signature = vug::sign::sign(&secret, &json).unwrap();
        let answer = cache.join("output-keys").join(&key);

        fs::write(&answer, format!("{json}\n{}\n", "0".repeat(128))).unwrap();
        let error = builder.build("split", None).err().unwrap();
        assert!(error.contains("not signed by the trusted key"), "{error}");

        let wrong_key = json.replace(&key, &"b".repeat(32));
        let wrong_signature = vug::sign::sign(&secret, &wrong_key).unwrap();
        fs::write(&answer, format!("{wrong_key}\n{wrong_signature}\n")).unwrap();
        let error = builder.build("split", None).err().unwrap();
        assert!(
            error.contains("output manifest disagrees with the record"),
            "{error}"
        );

        let wrong_edge = json.replace("[]", &format!(r#"["{id}"]"#));
        let wrong_signature = vug::sign::sign(&secret, &wrong_edge).unwrap();
        fs::write(&answer, format!("{wrong_edge}\n{wrong_signature}\n")).unwrap();
        let error = builder.build("split", None).err().unwrap();
        assert!(
            error.contains("output manifest disagrees with the record"),
            "{error}"
        );

        fs::write(&answer, format!("{json}\n{signature}\n")).unwrap();
        let error = cached.build("split", None).err().unwrap();
        assert!(
            error.contains("output missing from the store and binary cache"),
            "{error}"
        );
        let error = builder.build("split", None).err().unwrap();
        assert!(error.contains("seed.toml: busybox.url"), "{error}");

        let tree = root.join("wrong-tree");
        fs::create_dir_all(&tree).unwrap();
        fs::write(tree.join("file"), "unexpected content").unwrap();
        let mut archive = zstd::Encoder::new(Vec::new(), ARCHIVE_LEVEL).unwrap();
        vug::cpio::write_newc(&tree, &mut archive).unwrap();
        fs::write(
            cache.join("paths").join(format!("{id}.cpio.zst")),
            archive.finish().unwrap(),
        )
        .unwrap();
        let error = builder.build("split", None).err().unwrap();
        assert!(error.contains("the content hashes to"), "{error}");
        assert!(!settings.data_dir.join("index-outputs/split/1").exists());
        assert!(!settings.data_dir.join("store").join(id).exists());
    }

    fn write_record(root: &Path, name: &str, outputs: &str) -> (record::VersionRecord, String) {
        let path = root.join(model::version_path(name, "1"));
        let versions = path.parent().unwrap();
        fs::create_dir_all(versions).unwrap();
        fs::write(
            versions.join("../package.toml"),
            format!("name = \"{name}\"\nlicense = \"MIT\"\n[upstream]\nsources = [\"https://example.org/\"]\n"),
        )
        .unwrap();
        fs::write(
            &path,
            format!(
                "version = \"1\"\n[source]\nurl = \"https://example.org/source.tar.gz\"\nsha256 = \"{}\"\n[build]\nkind = \"autotools\"\n[dependencies]\nbuild = []\nrun = []\n[check]\nenabled = false\nskip = []\n{outputs}",
                "0".repeat(64)
            ),
        )
        .unwrap();
        record::load_version(&path).unwrap_or_else(|errors| {
            panic!(
                "{}: {}",
                path.display(),
                errors
                    .iter()
                    .map(|error| error.message.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        })
    }

    struct FileServer {
        addr: SocketAddr,
        thread: Option<JoinHandle<()>>,
        requests: Arc<AtomicUsize>,
    }

    impl FileServer {
        fn start(root: PathBuf) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let addr = listener.local_addr().unwrap();
            let requests = Arc::new(AtomicUsize::new(0));
            let observed = requests.clone();
            let thread = thread::spawn(move || {
                for stream in listener.incoming() {
                    let mut stream = stream.unwrap();
                    let mut reader = BufReader::new(&stream);
                    let mut request = String::new();
                    if reader.read_line(&mut request).unwrap() == 0 {
                        break;
                    }
                    observed.fetch_add(1, Ordering::Relaxed);
                    let path = request.split_ascii_whitespace().nth(1).unwrap();
                    loop {
                        let mut header = String::new();
                        if reader.read_line(&mut header).unwrap() == 0 || header == "\r\n" {
                            break;
                        }
                    }
                    if let Ok(body) = fs::read(root.join(path.trim_start_matches('/'))) {
                        write!(
                            stream,
                            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                            body.len()
                        )
                        .unwrap();
                        stream.write_all(&body).unwrap();
                    } else {
                        stream.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap();
                    }
                }
            });
            Self {
                addr,
                thread: Some(thread),
                requests,
            }
        }
    }

    impl Drop for FileServer {
        fn drop(&mut self) {
            drop(TcpStream::connect(self.addr).unwrap());
            self.thread.take().unwrap().join().unwrap();
        }
    }
}
