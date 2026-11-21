//! Fetching a source file into a cache keyed by its SHA-256.

use sha2::{Digest, Sha256};
use std::fmt::Write as _;
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

#[derive(Debug)]
pub enum FetchError {
    /// The request failed or the status was not 200. Holds a message.
    Http(String),
    /// The downloaded bytes do not have the expected SHA-256.
    HashMismatch {
        expected: String,
        actual: String,
    },
    Io(io::Error),
}

/// SHA-256 of a file as 64 lowercase hex characters. Reads in chunks.
pub fn sha256_file(path: &Path) -> io::Result<String> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 8192];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    let mut hex = String::with_capacity(64);
    for b in hasher.finalize() {
        let _ = write!(hex, "{b:02x}");
    }
    Ok(hex)
}

/// Returns the path `cache_dir/<sha256>` holding the bytes of `url`.
///
/// - `sha256` is 64 lowercase hex characters.
/// - `cache_dir` is created when missing.
/// - When `cache_dir/<sha256>` exists and its SHA-256 equals `sha256`, it is
///   returned without any network access.
/// - When it exists with different content, it is deleted and fetched again.
/// - Otherwise the body of a GET to `url` is streamed to
///   `cache_dir/<sha256>.<process id>.partial` (so that two processes fetching
///   the same file do not write to one file), hashed, and renamed to
///   `cache_dir/<sha256>` when the hash matches.
/// - On a hash mismatch the partial file is removed and
///   `FetchError::HashMismatch` is returned with both hex values.
/// - A status other than 200 or a transport error is `FetchError::Http`, and
///   no file is left in `cache_dir`.
/// - There is no limit on the body size (ureq limits bodies by default;
///   remove the limit).
pub fn fetch(url: &str, sha256: &str, cache_dir: &Path) -> Result<PathBuf, FetchError> {
    let target = cache_dir.join(sha256);
    if target.exists() {
        if sha256_file(&target).map_err(FetchError::Io)? == sha256 {
            return Ok(target);
        }
        fs::remove_file(&target).map_err(FetchError::Io)?;
    }
    fs::create_dir_all(cache_dir).map_err(FetchError::Io)?;

    let mut res = ureq::get(url)
        .call()
        .map_err(|e| FetchError::Http(e.to_string()))?;
    if res.status().as_u16() != 200 {
        return Err(FetchError::Http(format!("status {}", res.status())));
    }

    let partial = cache_dir.join(format!("{sha256}.{}.partial", std::process::id()));
    let result = (|| {
        let mut out = File::create(&partial).map_err(FetchError::Io)?;
        let mut reader = res.body_mut().with_config().limit(u64::MAX).reader();
        let mut hasher = Sha256::new();
        let mut buf = [0u8; 64 * 1024];
        loop {
            let n = match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => n,
                Err(e) => return Err(FetchError::Http(e.to_string())),
            };
            out.write_all(&buf[..n]).map_err(FetchError::Io)?;
            hasher.update(&buf[..n]);
        }
        let mut actual = String::with_capacity(64);
        for b in hasher.finalize() {
            let _ = write!(actual, "{b:02x}");
        }
        if actual != sha256 {
            return Err(FetchError::HashMismatch {
                expected: sha256.to_string(),
                actual,
            });
        }
        fs::rename(&partial, &target).map_err(FetchError::Io)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&partial);
    }
    result?;
    Ok(target)
}
