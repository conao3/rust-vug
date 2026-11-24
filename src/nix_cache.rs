use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::ffi::OsStr;
use std::fmt;
use std::io::{self, BufReader, Read, Seek, Write};
use std::path::Path;
use std::str::FromStr;
use std::time::Duration;

use base64::prelude::{BASE64_STANDARD, Engine};
use ed25519_dalek::{Signature, VerifyingKey};
use rustix::fs::{Mode, OFlags, open};
use sha2::{Digest, Sha256};
use tempfile::TempDir;

mod nar;

const ORIGIN: &str = "https://cache.nixos.org";
const STORE: &str = "/nix/store";
const KEY_NAME: &str = "cache.nixos.org-1";
const PUBLIC_KEY: &str = "6NCHdD59X431o0gWypbMrAURkbJ16ZPMQFGspcDShjY=";
const NIX32: &[u8; 32] = b"0123456789abcdfghijklmnpqrsvwxyz";

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct StorePath(String);

impl StorePath {
    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn basename(&self) -> &str {
        &self.0[STORE.len() + 1..]
    }

    fn narinfo_path(&self) -> String {
        format!("{}.narinfo", &self.basename()[..32])
    }

    fn validate(path: &str) -> Option<Self> {
        let basename = path.strip_prefix(STORE)?.strip_prefix('/')?;
        let (hash, name) = basename.split_at_checked(32)?;
        let name = name.strip_prefix('-')?;
        if !hash.bytes().all(|b| NIX32.contains(&b))
            || name.is_empty()
            || name.len() > 211
            || matches!(name.split('-').next(), Some("." | ".."))
            || !name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"+-._?=".contains(&b))
        {
            return None;
        }
        Some(Self(path.to_owned()))
    }
}

impl FromStr for StorePath {
    type Err = String;

    fn from_str(path: &str) -> Result<Self, Self::Err> {
        Self::validate(path).ok_or_else(|| format!("invalid Nix store path {path:?}"))
    }
}

impl fmt::Display for StorePath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Limits {
    pub paths: usize,
    pub narinfo_bytes: usize,
    pub file_bytes: u64,
    pub nar_bytes: u64,
    pub total_file_bytes: u64,
    pub total_nar_bytes: u64,
}

impl Default for Limits {
    fn default() -> Self {
        const GIB: u64 = 1024 * 1024 * 1024;
        Self {
            paths: 4096,
            narinfo_bytes: 64 * 1024,
            file_bytes: 4 * GIB,
            nar_bytes: 8 * GIB,
            total_file_bytes: 16 * GIB,
            total_nar_bytes: 32 * GIB,
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum Compression {
    None,
    Zstd,
    Xz,
    Bzip2,
}

impl Compression {
    fn parse(value: &str) -> Result<Self, String> {
        match value {
            "none" => Ok(Self::None),
            "zstd" => Ok(Self::Zstd),
            "xz" => Ok(Self::Xz),
            "" | "bzip2" => Ok(Self::Bzip2),
            _ => Err(format!("unsupported NAR compression {value:?}")),
        }
    }

    fn reader(self, input: impl Read + 'static) -> io::Result<Box<dyn Read>> {
        match self {
            Self::None => Ok(Box::new(input)),
            Self::Zstd => {
                let mut decoder = zstd::stream::read::Decoder::new(input)?;
                decoder.window_log_max(27)?;
                Ok(Box::new(decoder))
            }
            Self::Xz => Ok(Box::new(lzma_rust2::XzReader::new_mem_limit(
                input,
                true,
                128 * 1024,
            ))),
            Self::Bzip2 => Ok(Box::new(bzip2::read::MultiBzDecoder::new(input))),
        }
    }
}

#[derive(Debug)]
pub struct NarInfo {
    store_path: StorePath,
    references: BTreeSet<StorePath>,
    nar_hash: String,
    nar_size: u64,
    url: String,
    compression: Compression,
    file_hash: Option<String>,
    file_size: Option<u64>,
}

impl NarInfo {
    pub fn store_path(&self) -> &StorePath {
        &self.store_path
    }

    pub fn references(&self) -> &BTreeSet<StorePath> {
        &self.references
    }

    pub fn nar_hash(&self) -> &str {
        &self.nar_hash
    }

    pub fn nar_size(&self) -> u64 {
        self.nar_size
    }

    pub fn file_size(&self) -> Option<u64> {
        self.file_size
    }

    fn parse(text: &str, requested: &StorePath, key: &VerifyingKey) -> Result<Self, String> {
        let mut fields = BTreeMap::new();
        let mut signatures = Vec::new();
        for line in text.lines() {
            let (field, value) = line.split_once(": ").ok_or("invalid narinfo line")?;
            if field.is_empty() || !field.bytes().all(|b| b.is_ascii_alphanumeric()) {
                return Err("invalid narinfo field name".into());
            }
            if field == "Sig" {
                signatures.push(value);
            } else if fields.insert(field, value).is_some() {
                return Err(format!("duplicate narinfo field {field}"));
            }
        }
        let required = |field| {
            fields
                .get(field)
                .copied()
                .ok_or_else(|| format!("missing {field}"))
        };
        let store_path = required("StorePath")?.parse::<StorePath>()?;
        if &store_path != requested {
            return Err(format!(
                "narinfo StorePath is {store_path}, expected {requested}"
            ));
        }
        let references = fields
            .get("References")
            .copied()
            .unwrap_or_default()
            .split_ascii_whitespace()
            .map(|basename| format!("{STORE}/{basename}").parse())
            .collect::<Result<BTreeSet<StorePath>, _>>()?;
        let info = Self {
            store_path,
            references,
            nar_hash: parse_hash(required("NarHash")?)?,
            nar_size: parse_size(required("NarSize")?)?,
            url: parse_url(required("URL")?)?,
            compression: Compression::parse(
                fields.get("Compression").copied().unwrap_or_default(),
            )?,
            file_hash: fields.get("FileHash").map(|v| parse_hash(v)).transpose()?,
            file_size: fields.get("FileSize").map(|v| parse_size(v)).transpose()?,
        };
        let fingerprint = info.fingerprint();
        if !signatures.into_iter().any(|value| {
            let Some((KEY_NAME, encoded)) = value.split_once(':') else {
                return false;
            };
            BASE64_STANDARD
                .decode(encoded)
                .ok()
                .and_then(|bytes| Signature::from_slice(&bytes).ok())
                .is_some_and(|sig| key.verify_strict(fingerprint.as_bytes(), &sig).is_ok())
        }) {
            return Err(format!("narinfo lacks a valid {KEY_NAME} signature"));
        }
        Ok(info)
    }

    fn fingerprint(&self) -> String {
        let references = self
            .references
            .iter()
            .map(StorePath::as_str)
            .collect::<Vec<_>>()
            .join(",");
        format!(
            "1;{};{};{};{references}",
            self.store_path, self.nar_hash, self.nar_size
        )
    }
}

fn parse_hash(value: &str) -> Result<String, String> {
    let hash = value.strip_prefix("sha256:").unwrap_or_default().as_bytes();
    if hash.len() == 52 && matches!(hash[0], b'0' | b'1') && hash.iter().all(|b| NIX32.contains(b))
    {
        Ok(value.to_owned())
    } else {
        Err(format!("expected a SHA-256 hash in Nix32, got {value:?}"))
    }
}

fn nix_hash(digest: &[u8; 32]) -> String {
    let mut result = String::from("sha256:");
    for digit in (0..52).rev() {
        let offset = digit * 5;
        let i = offset / 8;
        let shift = offset % 8;
        let word = u16::from_le_bytes([digest[i], digest.get(i + 1).copied().unwrap_or(0)]);
        result.push(NIX32[((word >> shift) & 31) as usize] as char);
    }
    result
}

fn parse_size(value: &str) -> Result<u64, String> {
    value
        .parse::<u64>()
        .ok()
        .filter(|size| *size != 0 && value.bytes().all(|b| b.is_ascii_digit()))
        .ok_or_else(|| format!("invalid NAR size {value:?}"))
}

fn parse_url(value: &str) -> Result<String, String> {
    let (path, query) = value.split_once('?').unwrap_or((value, ""));
    let unreserved = |b: u8| b.is_ascii_alphanumeric() || b"-._~".contains(&b);
    if path
        .split('/')
        .all(|p| !matches!(p, "" | "." | "..") && p.bytes().all(unreserved))
        && query.bytes().all(|b| unreserved(b) || b"=&".contains(&b))
    {
        Ok(value.to_owned())
    } else {
        Err(format!("invalid relative narinfo URL {value:?}"))
    }
}

fn public_key() -> VerifyingKey {
    let bytes = BASE64_STANDARD
        .decode(PUBLIC_KEY)
        .expect("built-in key must be Base64");
    VerifyingKey::from_bytes(&bytes.try_into().expect("built-in key must be 32 bytes"))
        .expect("built-in key must be an Ed25519 public key")
}

#[derive(Debug)]
pub struct Closure {
    entries: Vec<NarInfo>,
    limits: Limits,
}

impl Closure {
    pub fn entries(&self) -> &[NarInfo] {
        &self.entries
    }
}

pub struct Cache {
    agent: ureq::Agent,
}

impl Default for Cache {
    fn default() -> Self {
        Self {
            agent: ureq::Agent::config_builder()
                .https_only(true)
                .max_redirects(0)
                .http_status_as_error(false)
                .timeout_global(Some(Duration::from_secs(300)))
                .build()
                .into(),
        }
    }
}

impl Cache {
    fn get(&self, path: &str) -> Result<impl Read + use<>, String> {
        let response = self
            .agent
            .get(format!("{ORIGIN}/{path}"))
            .header("Accept-Encoding", "identity")
            .call()
            .map_err(|e| format!("request {path}: {e}"))?;
        check_status(response.status().as_u16())?;
        if response.headers().contains_key("Content-Encoding") {
            return Err("unexpected HTTP Content-Encoding for Nix cache response".into());
        }
        Ok(response.into_body().into_reader())
    }

    pub fn resolve(&self, roots: &[StorePath], limits: Limits) -> Result<Closure, String> {
        resolve(roots, limits, &public_key(), |path| self.get(path))
    }

    pub fn fetch(&self, closure: &Closure, parent: &Path) -> Result<TempDir, String> {
        fetch(closure, parent, |path| self.get(path))
    }
}

fn check_status(status: u16) -> Result<(), String> {
    if status == 200 {
        Ok(())
    } else {
        Err(format!("Nix cache returned HTTP {status}"))
    }
}

fn resolve<R: Read>(
    roots: &[StorePath],
    limits: Limits,
    key: &VerifyingKey,
    get: impl Fn(&str) -> Result<R, String>,
) -> Result<Closure, String> {
    if roots.is_empty() {
        return Err("at least one Nix store path is required".into());
    }
    let mut visited = BTreeSet::new();
    let mut pending = VecDeque::new();
    let mut enqueue = |path: &StorePath, pending: &mut VecDeque<StorePath>| {
        if visited.insert(path.clone()) {
            if visited.len() > limits.paths {
                return Err("Nix closure exceeds path count limit".to_owned());
            }
            pending.push_back(path.clone());
        }
        Ok(())
    };
    for path in roots {
        enqueue(path, &mut pending)?;
    }
    let mut entries = Vec::new();
    let mut nar_remaining = limits.total_nar_bytes;
    let mut file_remaining = limits.total_file_bytes;
    while let Some(path) = pending.pop_front() {
        let info = read_narinfo(&path, limits.narinfo_bytes, key, &get)
            .map_err(|e| format!("{path}: {e}"))?;
        if info.nar_size > limits.nar_bytes
            || info.file_size.is_some_and(|size| size > limits.file_bytes)
        {
            return Err(format!("{path}: NAR exceeds per-path size limit"));
        }
        nar_remaining = nar_remaining
            .checked_sub(info.nar_size)
            .ok_or("Nix closure exceeds total NAR size limit")?;
        file_remaining = file_remaining
            .checked_sub(info.file_size.unwrap_or(0))
            .ok_or("Nix closure exceeds total transfer size limit")?;
        for reference in &info.references {
            enqueue(reference, &mut pending)?;
        }
        entries.push(info);
    }
    entries.sort_unstable_by(|a, b| a.store_path.cmp(&b.store_path));
    Ok(Closure { entries, limits })
}

fn read_narinfo<R: Read>(
    path: &StorePath,
    limit: usize,
    key: &VerifyingKey,
    get: &impl Fn(&str) -> Result<R, String>,
) -> Result<NarInfo, String> {
    let mut bytes = Vec::new();
    get(&path.narinfo_path())?
        .take((limit as u64).saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|e| format!("read narinfo: {e}"))?;
    if bytes.len() > limit {
        return Err("narinfo response exceeds size limit".into());
    }
    let text = std::str::from_utf8(&bytes).map_err(|e| format!("decode narinfo: {e}"))?;
    NarInfo::parse(text, path, key)
}

fn fetch<R: Read>(
    closure: &Closure,
    parent: &Path,
    get: impl Fn(&str) -> Result<R, String>,
) -> Result<TempDir, String> {
    let parent = parent
        .canonicalize()
        .map_err(|e| format!("resolve staging parent: {e}"))?;
    if parent.starts_with(STORE) {
        return Err("NAR staging parent must be outside /nix/store".into());
    }
    let staging = tempfile::Builder::new()
        .prefix("vug-nix-")
        .tempdir_in(parent)
        .map_err(|e| format!("create NAR staging directory: {e}"))?;
    let dir = open(
        staging.path(),
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(|e| format!("open NAR staging directory: {e}"))?;
    let mut remaining = closure.limits.total_file_bytes;
    for info in &closure.entries {
        let transfer_limit = remaining.min(closure.limits.file_bytes);
        if info.file_size.is_some_and(|size| size > transfer_limit) {
            return Err(format!(
                "{}: NAR exceeds remaining transfer size limit",
                info.store_path
            ));
        }
        let response = get(&info.url).map_err(|e| format!("{}: {e}", info.store_path))?;
        let (archive, transferred) = verified_nar(response, info, transfer_limit, staging.path())
            .map_err(|e| format!("{}: {e}", info.store_path))?;
        remaining -= transferred;
        nar::unpack(
            BufReader::new(archive),
            &dir,
            OsStr::new(info.store_path.basename()),
        )
        .map_err(|e| format!("{}: unpack NAR: {e}", info.store_path))?;
    }
    Ok(staging)
}

fn verified_nar(
    response: impl Read,
    info: &NarInfo,
    transfer_limit: u64,
    staging: &Path,
) -> io::Result<(std::fs::File, u64)> {
    let mut transfer = tempfile::tempfile_in(staging)?;
    let transferred = checked_copy(
        response,
        &mut transfer,
        transfer_limit,
        info.file_size,
        info.file_hash.as_deref(),
        "File",
    )?;
    transfer.rewind()?;
    let mut archive = tempfile::tempfile_in(staging)?;
    let decoded = info.compression.reader(BufReader::new(transfer))?;
    checked_copy(
        decoded,
        &mut archive,
        info.nar_size,
        Some(info.nar_size),
        Some(&info.nar_hash),
        "Nar",
    )?;
    archive.rewind()?;
    Ok((archive, transferred))
}

fn checked_copy(
    mut input: impl Read,
    mut output: impl Write,
    limit: u64,
    size: Option<u64>,
    hash: Option<&str>,
    field: &str,
) -> io::Result<u64> {
    if size.is_some_and(|size| size > limit) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("{field} response exceeds size limit"),
        ));
    }
    let mut hasher = Sha256::new();
    let mut bounded = input.by_ref().take(size.unwrap_or(limit));
    let mut buffer = [0; 64 * 1024];
    let mut written = 0;
    loop {
        let count = bounded.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        let bytes = &buffer[..count];
        hasher.update(bytes);
        output.write_all(bytes)?;
        written += count as u64;
    }
    let excess = input.read(&mut [0])? != 0;
    if excess && size.is_none() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("{field} response exceeds size limit"),
        ));
    }
    if excess || size.is_some_and(|size| size != written) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("{field}Size mismatch"),
        ));
    }
    if hash.is_some_and(|expected| expected != nix_hash(&hasher.finalize().into())) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("{field}Hash mismatch"),
        ));
    }
    output.flush()?;
    Ok(written)
}

#[cfg(test)]
#[path = "../tests/nix_cache/cases.rs"]
mod tests;
