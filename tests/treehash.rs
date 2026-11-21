use std::fs::{self, File};
use std::io::{self, Write};
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::Path;
use std::process::Command;
use std::time::{Duration, SystemTime};

use tempfile::tempdir;
use vug::treehash::{encode_base32, hash_tree};

fn blake3_20(data: &[u8]) -> [u8; 20] {
    blake3::hash(data).as_bytes()[..20].try_into().unwrap()
}

fn write_file(path: &Path, content: &[u8], mode: u32) {
    let mut file = File::create(path).unwrap();
    file.write_all(content).unwrap();
    file.set_permissions(fs::Permissions::from_mode(mode))
        .unwrap();
}

fn build_tree(root: &Path) {
    write_file(&root.join("f"), b"hi", 0o644);
    write_file(&root.join("run"), b"#!/bin/sh\n", 0o755);
    fs::create_dir(root.join("d")).unwrap();
    write_file(&root.join("d").join("nested"), b"n", 0o644);
    symlink("f", root.join("link")).unwrap();
}

#[test]
fn base32_zeros() {
    assert_eq!(encode_base32(&[0u8; 20]), "a".repeat(32));
}

#[test]
fn base32_ones() {
    assert_eq!(encode_base32(&[0xffu8; 20]), "7".repeat(32));
}

#[test]
fn base32_sequence() {
    let mut bytes = [0u8; 20];
    for (i, b) in bytes.iter_mut().enumerate() {
        *b = i as u8;
    }
    assert_eq!(encode_base32(&bytes), "aaaqeayeaudaocajbifqydiob4ibceqt");
}

#[test]
fn empty_directory() {
    let dir = tempdir().unwrap();
    let expected = blake3_20(&[b'D', 0, 0, 0, 0, 0, 0, 0, 0]);
    assert_eq!(hash_tree(dir.path()).unwrap(), expected);
}

#[test]
fn regular_file_not_executable() {
    let dir = tempdir().unwrap();
    let file = dir.path().join("f");
    write_file(&file, b"hi", 0o644);
    let mut data = Vec::new();
    data.extend_from_slice(b"F-");
    data.extend_from_slice(&2u64.to_le_bytes());
    data.extend_from_slice(b"hi");
    assert_eq!(hash_tree(&file).unwrap(), blake3_20(&data));
}

#[test]
fn regular_file_executable() {
    let dir = tempdir().unwrap();
    let file = dir.path().join("f");
    write_file(&file, b"hi", 0o755);
    let mut data = Vec::new();
    data.extend_from_slice(b"Fx");
    data.extend_from_slice(&2u64.to_le_bytes());
    data.extend_from_slice(b"hi");
    assert_eq!(hash_tree(&file).unwrap(), blake3_20(&data));
}

#[test]
fn directory_entries_sorted() {
    let dir = tempdir().unwrap();
    write_file(&dir.path().join("b"), b"1", 0o644);
    write_file(&dir.path().join("a"), b"2", 0o644);
    let mut data = Vec::new();
    data.extend_from_slice(b"D");
    data.extend_from_slice(&2u64.to_le_bytes());
    for (name, content) in [(&b"a"[..], &b"2"[..]), (b"b", b"1")] {
        data.extend_from_slice(&1u64.to_le_bytes());
        data.extend_from_slice(name);
        data.extend_from_slice(b"F-");
        data.extend_from_slice(&1u64.to_le_bytes());
        data.extend_from_slice(content);
    }
    assert_eq!(hash_tree(dir.path()).unwrap(), blake3_20(&data));
}

#[test]
fn identical_trees_equal_hash() {
    let a = tempdir().unwrap();
    let b = tempdir().unwrap();
    build_tree(a.path());
    build_tree(b.path());
    assert_eq!(hash_tree(a.path()).unwrap(), hash_tree(b.path()).unwrap());
}

#[test]
fn changes_affect_hash() {
    let base = tempdir().unwrap();
    build_tree(base.path());
    let baseline = hash_tree(base.path()).unwrap();

    let dir = tempdir().unwrap();
    build_tree(dir.path());
    write_file(&dir.path().join("f"), b"changed", 0o644);
    assert_ne!(hash_tree(dir.path()).unwrap(), baseline);

    let dir = tempdir().unwrap();
    build_tree(dir.path());
    fs::set_permissions(dir.path().join("f"), fs::Permissions::from_mode(0o744)).unwrap();
    assert_ne!(hash_tree(dir.path()).unwrap(), baseline);

    let dir = tempdir().unwrap();
    build_tree(dir.path());
    fs::rename(dir.path().join("f"), dir.path().join("g")).unwrap();
    assert_ne!(hash_tree(dir.path()).unwrap(), baseline);

    let dir = tempdir().unwrap();
    build_tree(dir.path());
    fs::remove_file(dir.path().join("link")).unwrap();
    symlink("run", dir.path().join("link")).unwrap();
    assert_ne!(hash_tree(dir.path()).unwrap(), baseline);

    let dir = tempdir().unwrap();
    build_tree(dir.path());
    fs::remove_file(dir.path().join("f")).unwrap();
    fs::create_dir(dir.path().join("f")).unwrap();
    assert_ne!(hash_tree(dir.path()).unwrap(), baseline);
}

#[test]
fn mtime_and_other_mode_bits_ignored() {
    let dir = tempdir().unwrap();
    let file = dir.path().join("f");
    write_file(&file, b"hi", 0o644);
    let before = hash_tree(dir.path()).unwrap();

    let opened = File::options().write(true).open(&file).unwrap();
    opened
        .set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000))
        .unwrap();
    fs::set_permissions(&file, fs::Permissions::from_mode(0o600)).unwrap();

    assert_eq!(hash_tree(dir.path()).unwrap(), before);
}

#[test]
fn dangling_symlink() {
    let dir = tempdir().unwrap();
    let link = dir.path().join("l");
    let target = "nonexistent-target";
    symlink(target, &link).unwrap();
    let mut data = Vec::new();
    data.extend_from_slice(b"L");
    data.extend_from_slice(&(target.len() as u64).to_le_bytes());
    data.extend_from_slice(target.as_bytes());
    assert_eq!(hash_tree(&link).unwrap(), blake3_20(&data));
}

#[test]
fn fifo_returns_invalid_data() {
    let dir = tempdir().unwrap();
    let status = Command::new("mkfifo")
        .arg(dir.path().join("pipe"))
        .status()
        .unwrap();
    assert!(status.success());
    let err = hash_tree(dir.path()).unwrap_err();
    assert_eq!(err.kind(), io::ErrorKind::InvalidData);
}
