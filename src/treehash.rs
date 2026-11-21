//! Deterministic hash of a directory tree.
//!
//! The tree is serialized to a byte stream and hashed with BLAKE3. The hash
//! is the first 20 bytes of the BLAKE3 output.
//!
//! Serialization of an entry (all integers are u64 little-endian):
//!
//! - regular file: `b"F"`, then `b"x"` when the owner-execute bit
//!   (`mode & 0o100`) is set and `b"-"` otherwise, then the content length,
//!   then the content bytes.
//! - symbolic link: `b"L"`, then the target length in bytes, then the target
//!   bytes. Links are never followed.
//! - directory: `b"D"`, then the number of entries, then for each entry in
//!   ascending byte order of its file name: the name length in bytes, the
//!   name bytes, the serialization of the entry.
//! - anything else (socket, fifo, device): `io::ErrorKind::InvalidData`.
//!
//! Timestamps, owners, and every mode bit other than owner-execute do not
//! affect the hash. The name of the root itself does not affect the hash.

use std::fs::{self, File};
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

/// Hashes the tree rooted at `root`. `root` may be a directory, a regular
/// file, or a symbolic link.
pub fn hash_tree(root: &Path) -> io::Result<[u8; 20]> {
    let mut hasher = blake3::Hasher::new();
    write_entry(root, &fs::symlink_metadata(root)?, &mut hasher)?;
    Ok(hasher.finalize().as_bytes()[..20].try_into().unwrap())
}

/// Encodes 20 bytes as 32 characters of RFC 4648 base32 with the lowercase
/// alphabet `abcdefghijklmnopqrstuvwxyz234567` and no padding.
pub fn encode_base32(hash: &[u8; 20]) -> String {
    const ALPHABET: &[u8; 32] = b"abcdefghijklmnopqrstuvwxyz234567";
    let mut out = String::with_capacity(32);
    for chunk in hash.as_chunks::<5>().0 {
        let n = u64::from_be_bytes([0, 0, 0, chunk[0], chunk[1], chunk[2], chunk[3], chunk[4]]);
        for shift in (0..40).step_by(5).rev() {
            out.push(ALPHABET[((n >> shift) & 0x1f) as usize] as char);
        }
    }
    out
}

fn write_entry(
    path: &Path,
    metadata: &fs::Metadata,
    hasher: &mut blake3::Hasher,
) -> io::Result<()> {
    let file_type = metadata.file_type();
    if file_type.is_file() {
        hasher.update(b"F");
        hasher.update(if metadata.permissions().mode() & 0o100 != 0 {
            b"x"
        } else {
            b"-"
        });
        hasher.update(&metadata.len().to_le_bytes());
        io::copy(&mut File::open(path)?, hasher)?;
    } else if file_type.is_symlink() {
        let target = fs::read_link(path)?;
        let target = target.as_os_str().as_bytes();
        hasher.update(b"L");
        hasher.update(&(target.len() as u64).to_le_bytes());
        hasher.update(target);
    } else if file_type.is_dir() {
        let mut entries = fs::read_dir(path)?.collect::<io::Result<Vec<_>>>()?;
        entries.sort_by(|a, b| a.file_name().as_bytes().cmp(b.file_name().as_bytes()));
        hasher.update(b"D");
        hasher.update(&(entries.len() as u64).to_le_bytes());
        for entry in entries {
            let name = entry.file_name();
            hasher.update(&(name.as_bytes().len() as u64).to_le_bytes());
            hasher.update(name.as_bytes());
            let path = entry.path();
            write_entry(&path, &fs::symlink_metadata(&path)?, hasher)?;
        }
    } else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "unsupported file type",
        ));
    }
    Ok(())
}
