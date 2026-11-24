use super::*;
use ed25519_dalek::{Signer, SigningKey};
use std::io::Cursor;
use std::os::unix::fs::PermissionsExt;

const ROOT: &str = "/nix/store/11111111111111111111111111111111-root";
const DEP: &str = "/nix/store/00000000000000000000000000000000-dep";
const ROOT_NAR: &str = "nar/root.nar.zst";
const DEP_NAR: &str = "nar/dep.nar.zst";

fn key() -> SigningKey {
    SigningKey::from_bytes(&[0x42; 32])
}

fn digest(bytes: &[u8]) -> String {
    nix_hash(&Sha256::digest(bytes).into())
}

fn narinfo(path: &str, url: &str, references: &[&str], nar: &[u8], file: &[u8]) -> String {
    let hash = digest(nar);
    let size = nar.len();
    let sorted = BTreeSet::from_iter(references.iter().copied())
        .into_iter()
        .collect::<Vec<_>>()
        .join(",");
    let fingerprint = format!("1;{path};{hash};{size};{sorted}");
    let signature = BASE64_STANDARD.encode(key().sign(fingerprint.as_bytes()).to_bytes());
    let refs = references
        .iter()
        .map(|p| p.rsplit('/').next().unwrap())
        .collect::<Vec<_>>()
        .join(" ");
    format!(
        "StorePath: {path}\nURL: {url}\nCompression: zstd\nFileHash: {}\nFileSize: {}\nNarHash: {hash}\nNarSize: {size}\nReferences: {refs}\nSig: other-key:ignored\nSig: {KEY_NAME}:{signature}\n",
        digest(file),
        file.len()
    )
}

fn tokens(output: &mut Vec<u8>, values: &[&[u8]]) {
    for value in values {
        output.extend((value.len() as u64).to_le_bytes());
        output.extend(*value);
        output.resize(output.len().next_multiple_of(8), 0);
    }
}

fn regular(mode: &[&[u8]], contents: &[u8]) -> Vec<u8> {
    let mut node = Vec::new();
    tokens(&mut node, &[b"(", b"type", b"regular"]);
    tokens(&mut node, mode);
    tokens(&mut node, &[b"contents", contents, b")"]);
    node
}

fn symlink(target: &[u8]) -> Vec<u8> {
    let mut node = Vec::new();
    tokens(
        &mut node,
        &[b"(", b"type", b"symlink", b"target", target, b")"],
    );
    node
}

fn directory(entries: &[(&[u8], &[u8])]) -> Vec<u8> {
    let mut node = Vec::new();
    tokens(&mut node, &[b"(", b"type", b"directory"]);
    for (name, entry) in entries {
        tokens(&mut node, &[b"entry", b"(", b"name", name, b"node"]);
        node.extend(*entry);
        tokens(&mut node, &[b")"]);
    }
    tokens(&mut node, &[b")"]);
    node
}

fn archive(node: &[u8]) -> Vec<u8> {
    let mut nar = Vec::new();
    tokens(&mut nar, &[b"nix-archive-1"]);
    nar.extend(node);
    nar
}

type Responses = BTreeMap<String, Vec<u8>>;

fn add(responses: &mut Responses, path: &str, url: &str, refs: &[&str], nar: &[u8]) {
    let compressed = zstd::encode_all(nar, 1).unwrap();
    let info = narinfo(path, url, refs, nar, &compressed);
    responses.insert(
        path.parse::<StorePath>().unwrap().narinfo_path(),
        info.into_bytes(),
    );
    responses.insert(url.to_owned(), compressed);
}

fn response<'a>(responses: &'a Responses, path: &str) -> Result<Cursor<&'a [u8]>, String> {
    match responses.get(path) {
        Some(body) => Ok(Cursor::new(body)),
        None => Err(check_status(404).unwrap_err()),
    }
}

fn resolve_fixture(responses: &Responses, limits: Limits) -> Result<Closure, String> {
    let roots = &[ROOT.parse().unwrap()];
    resolve(roots, limits, &key().verifying_key(), |path| {
        response(responses, path)
    })
}

fn replace_info(responses: &mut Responses, path: &str, edit: impl FnOnce(&str) -> String) {
    let body = responses
        .get_mut(&path.parse::<StorePath>().unwrap().narinfo_path())
        .unwrap();
    *body = edit(std::str::from_utf8(body).unwrap()).into_bytes();
}

fn fixture() -> Responses {
    let mut responses = Responses::new();
    let dep = archive(&regular(&[], b"library"));
    let root = archive(&directory(&[
        (b"bin", &regular(&[b"executable", b""], b"program")),
        (b"lib", &symlink(DEP.as_bytes())),
    ]));
    add(&mut responses, ROOT, ROOT_NAR, &[ROOT, DEP, DEP], &root);
    add(&mut responses, DEP, DEP_NAR, &[ROOT], &dep);
    responses
}

#[test]
fn verifies_and_extracts_a_closure_with_cycles() {
    let responses = fixture();
    let closure = resolve_fixture(&responses, Limits::default()).unwrap();
    let paths: Vec<_> = closure
        .entries()
        .iter()
        .map(|info| info.store_path().as_str())
        .collect();
    assert_eq!(paths, [DEP, ROOT]);
    assert_eq!(closure.entries()[1].references().len(), 2);
    let parent = tempfile::tempdir().unwrap();
    let staging = fetch(&closure, parent.path(), |path| response(&responses, path)).unwrap();
    let root = staging.path().join(ROOT.rsplit('/').next().unwrap());
    let dep = staging.path().join(DEP.rsplit('/').next().unwrap());
    assert_eq!(std::fs::read(root.join("bin")).unwrap(), b"program");
    assert_eq!(
        std::fs::read_link(root.join("lib")).unwrap(),
        Path::new(DEP)
    );
    assert_eq!(std::fs::read(&dep).unwrap(), b"library");
    assert_eq!(
        std::fs::metadata(root.join("bin"))
            .unwrap()
            .permissions()
            .mode()
            & 0o7777,
        0o755
    );
    assert_eq!(
        std::fs::metadata(dep).unwrap().permissions().mode() & 0o7777,
        0o644
    );
}

#[test]
fn rejects_changed_signature_and_signed_metadata() {
    let mut responses = fixture();
    replace_info(&mut responses, ROOT, |text| {
        text.replace(
            "References: ",
            "References: 22222222222222222222222222222222-added ",
        )
    });
    let error = resolve_fixture(&responses, Limits::default()).unwrap_err();
    assert!(error.contains("signature"), "{error}");

    let mut responses = fixture();
    replace_info(&mut responses, ROOT, |text| {
        let (prefix, sig) = text.rsplit_once(&format!("{KEY_NAME}:")).unwrap();
        let changed = if sig.starts_with('A') { 'B' } else { 'A' };
        format!("{prefix}{KEY_NAME}:{changed}{}", &sig[1..])
    });
    let error = resolve_fixture(&responses, Limits::default()).unwrap_err();
    assert!(error.contains("signature"), "{error}");
}

#[test]
fn rejects_duplicate_narinfo_fields_and_unsafe_urls() {
    let mut responses = fixture();
    replace_info(&mut responses, ROOT, |text| {
        format!("{text}URL: {ROOT_NAR}\n")
    });
    let error = resolve_fixture(&responses, Limits::default()).unwrap_err();
    assert!(error.contains("duplicate narinfo field URL"), "{error}");
    for url in [
        "https://elsewhere.invalid/nar",
        "/nar",
        "//elsewhere.invalid/nar",
        "nar/../file",
        "nar/%2e%2e/file",
        "nar/./file",
        "nar\\file",
        "nar/file#fragment",
    ] {
        let mut responses = fixture();
        replace_info(&mut responses, ROOT, |text| text.replace(ROOT_NAR, url));
        let error = resolve_fixture(&responses, Limits::default()).unwrap_err();
        assert!(
            error.contains("invalid relative narinfo URL"),
            "{url}: {error}"
        );
    }
    assert!(parse_url("nar/file.nar.xz?sha256=01234").is_ok());
    assert!(check_status(302).is_err());
}

#[test]
fn reports_missing_narinfo_and_archive() {
    let mut responses = fixture();
    let removed = responses
        .remove(&DEP.parse::<StorePath>().unwrap().narinfo_path())
        .unwrap();
    let error = resolve_fixture(&responses, Limits::default()).unwrap_err();
    assert!(error.contains(DEP) && error.contains("HTTP 404"), "{error}");
    responses.insert(DEP.parse::<StorePath>().unwrap().narinfo_path(), removed);
    responses.remove(ROOT_NAR).unwrap();
    assert_fetch_error(&responses, Limits::default(), "HTTP 404");
}

fn assert_fetch_error(responses: &Responses, limits: Limits, expected: &str) {
    let closure = resolve_fixture(responses, limits).unwrap();
    let parent = tempfile::tempdir().unwrap();
    let error = fetch(&closure, parent.path(), |path| response(responses, path)).unwrap_err();
    assert!(error.contains(expected), "{error}");
    assert_eq!(parent.path().read_dir().unwrap().count(), 0);
}

#[test]
fn rejects_file_hash_size_nar_hash_and_nar_size_mismatches() {
    let mut responses = fixture();
    responses.get_mut(ROOT_NAR).unwrap()[0] ^= 1;
    assert_fetch_error(&responses, Limits::default(), "FileHash mismatch");

    let mut responses = fixture();
    responses.get_mut(ROOT_NAR).unwrap().push(0);
    assert_fetch_error(&responses, Limits::default(), "FileSize mismatch");

    let mut responses = fixture();
    let actual = archive(&regular(&[], b"data"));
    let compressed = zstd::encode_all(actual.as_slice(), 1).unwrap();
    let other_hash = archive(&regular(&[], b"xxxx"));
    let info = narinfo(ROOT, ROOT_NAR, &[], &other_hash, &compressed);
    replace_info(&mut responses, ROOT, |_| info);
    responses.insert(ROOT_NAR.into(), compressed.clone());
    assert_fetch_error(&responses, Limits::default(), "NarHash mismatch");

    let other_size = archive(&regular(&[], b"larger data"));
    let info = narinfo(ROOT, ROOT_NAR, &[], &other_size, &compressed);
    replace_info(&mut responses, ROOT, |_| info);
    assert_fetch_error(&responses, Limits::default(), "NarSize mismatch");
}

#[test]
fn rejects_traversal_duplicates_setuid_and_special_nodes() {
    let file = regular(&[], b"file");
    let symlink = symlink(b"../../outside");
    let mut special = Vec::new();
    tokens(&mut special, &[b"(", b"type", b"character-device", b")"]);
    let cases = [
        (directory(&[(b"..", &file)]), "invalid NAR entry name"),
        (directory(&[(b"../out", &file)]), "invalid NAR entry name"),
        (
            directory(&[(b"/absolute", &file)]),
            "invalid NAR entry name",
        ),
        (
            directory(&[(b"same", &symlink), (b"same", &file)]),
            "duplicate",
        ),
        (
            regular(&[b"mode", b"04755"], b"file"),
            "expected NAR contents",
        ),
        (special, "unsupported NAR node type"),
    ];
    for (node, expected) in cases {
        let mut responses = Responses::new();
        add(&mut responses, ROOT, ROOT_NAR, &[], &archive(&node));
        assert_fetch_error(&responses, Limits::default(), expected);
    }
}

#[test]
fn enforces_declared_and_streamed_limits() {
    let mut responses = fixture();
    let defaults = Limits::default();
    let closure = resolve_fixture(&responses, defaults).unwrap();
    let nar_bytes = closure.entries().iter().map(NarInfo::nar_size).sum::<u64>();
    let file_bytes = closure
        .entries()
        .iter()
        .map(|i| i.file_size().unwrap())
        .sum::<u64>();
    for (limits, expected) in [
        (
            Limits {
                paths: 1,
                ..defaults
            },
            "path count",
        ),
        (
            Limits {
                narinfo_bytes: 1,
                ..defaults
            },
            "narinfo response",
        ),
        (
            Limits {
                nar_bytes: 1,
                ..defaults
            },
            "per-path size",
        ),
        (
            Limits {
                file_bytes: 1,
                ..defaults
            },
            "per-path size",
        ),
        (
            Limits {
                total_nar_bytes: nar_bytes - 1,
                ..defaults
            },
            "total NAR size",
        ),
        (
            Limits {
                total_file_bytes: file_bytes - 1,
                ..defaults
            },
            "total transfer size",
        ),
    ] {
        let error = resolve_fixture(&responses, limits).unwrap_err();
        assert!(error.contains(expected), "{error}");
    }

    for path in [ROOT, DEP] {
        replace_info(&mut responses, path, |text| {
            text.lines()
                .filter(|line| !line.starts_with("FileSize: ") && !line.starts_with("FileHash: "))
                .collect::<Vec<_>>()
                .join("\n")
        });
    }
    assert_fetch_error(
        &responses,
        Limits {
            total_file_bytes: file_bytes - 1,
            ..defaults
        },
        "response exceeds size limit",
    );
    assert_fetch_error(
        &responses,
        Limits {
            file_bytes: 1,
            ..defaults
        },
        "response exceeds size limit",
    );
}

#[test]
fn accepts_legacy_bzip2_with_no_compression_field() {
    let nar = archive(&regular(&[], b"legacy"));
    let mut encoder = bzip2::write::BzEncoder::new(Vec::new(), bzip2::Compression::fast());
    encoder.write_all(&nar).unwrap();
    let file = encoder.finish().unwrap();
    let mut responses = Responses::new();
    let info = narinfo(ROOT, ROOT_NAR, &[], &nar, &file).replace("Compression: zstd\n", "");
    responses.insert(
        ROOT.parse::<StorePath>().unwrap().narinfo_path(),
        info.into_bytes(),
    );
    responses.insert(ROOT_NAR.into(), file);
    let closure = resolve_fixture(&responses, Limits::default()).unwrap();
    let parent = tempfile::tempdir().unwrap();
    let staging = fetch(&closure, parent.path(), |path| response(&responses, path)).unwrap();
    assert_eq!(
        std::fs::read(
            staging
                .path()
                .join(closure.entries()[0].store_path().basename())
        )
        .unwrap(),
        b"legacy"
    );
}
