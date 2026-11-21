use std::fs;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::time::{Duration, UNIX_EPOCH};

use tempfile::tempdir;
use vug::rewrite::rewrite_tree;

#[test]
fn different_lengths_is_invalid_input() {
    let dir = tempdir().unwrap();
    let file = dir.path().join("f");
    fs::write(&file, b"aXXb").unwrap();
    let err = rewrite_tree(dir.path(), b"X", b"YY").unwrap_err();
    assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
    assert_eq!(fs::read(&file).unwrap(), b"aXXb");
}

#[test]
fn empty_from_and_to_is_invalid_input() {
    let dir = tempdir().unwrap();
    let err = rewrite_tree(dir.path(), b"", b"").unwrap_err();
    assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
}

#[test]
fn replaces_every_occurrence() {
    let dir = tempdir().unwrap();
    let file = dir.path().join("f");
    fs::write(&file, b"aXXbXXc").unwrap();
    assert_eq!(rewrite_tree(dir.path(), b"XX", b"YY").unwrap(), 2);
    assert_eq!(fs::read(&file).unwrap(), b"aYYbYYc");
}

#[test]
fn occurrences_do_not_overlap() {
    let dir = tempdir().unwrap();
    let file = dir.path().join("f");
    fs::write(&file, b"XXX").unwrap();
    assert_eq!(rewrite_tree(dir.path(), b"XX", b"YY").unwrap(), 1);
    assert_eq!(fs::read(&file).unwrap(), b"YYX");
}

#[test]
fn binary_file_is_rewritten_in_place() {
    let dir = tempdir().unwrap();
    let file = dir.path().join("bin");
    let mut contents: Vec<u8> = (0u8..=255).collect();
    contents.extend_from_slice(b"XX");
    contents.push(0);
    fs::write(&file, &contents).unwrap();
    assert_eq!(rewrite_tree(dir.path(), b"XX", b"YY").unwrap(), 1);
    let mut expected: Vec<u8> = (0u8..=255).collect();
    expected.extend_from_slice(b"YY");
    expected.push(0);
    let after = fs::read(&file).unwrap();
    assert_eq!(after, expected);
    assert_eq!(after.len(), contents.len());
}

#[test]
fn file_without_occurrence_keeps_mtime() {
    let dir = tempdir().unwrap();
    let file = dir.path().join("f");
    fs::write(&file, b"nothing here").unwrap();
    let old = UNIX_EPOCH + Duration::from_secs(1_000_000);
    let f = fs::File::options()
        .read(true)
        .write(true)
        .open(&file)
        .unwrap();
    f.set_modified(old).unwrap();
    drop(f);
    assert_eq!(rewrite_tree(dir.path(), b"XX", b"YY").unwrap(), 0);
    assert_eq!(fs::metadata(&file).unwrap().modified().unwrap(), old);
}

#[test]
fn executable_file_keeps_mode() {
    let dir = tempdir().unwrap();
    let file = dir.path().join("f");
    fs::write(&file, b"aXXb").unwrap();
    fs::set_permissions(&file, fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(rewrite_tree(dir.path(), b"XX", b"YY").unwrap(), 1);
    assert_eq!(fs::read(&file).unwrap(), b"aYYb");
    assert_eq!(
        fs::metadata(&file).unwrap().permissions().mode() & 0o777,
        0o755
    );
}

#[test]
fn read_only_file_is_rewritten_and_keeps_mode() {
    let dir = tempdir().unwrap();
    let file = dir.path().join("f");
    fs::write(&file, b"aXXb").unwrap();
    fs::set_permissions(&file, fs::Permissions::from_mode(0o444)).unwrap();
    assert_eq!(rewrite_tree(dir.path(), b"XX", b"YY").unwrap(), 1);
    assert_eq!(fs::read(&file).unwrap(), b"aYYb");
    assert_eq!(
        fs::metadata(&file).unwrap().permissions().mode() & 0o777,
        0o444
    );
}

#[test]
fn symlink_target_is_rewritten() {
    let dir = tempdir().unwrap();
    let link = dir.path().join("l");
    std::os::unix::fs::symlink("/s/XX/bin", &link).unwrap();
    assert_eq!(rewrite_tree(dir.path(), b"XX", b"YY").unwrap(), 1);
    assert_eq!(fs::read_link(&link).unwrap(), Path::new("/s/YY/bin"));
    assert!(
        fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink()
    );
}

#[test]
fn symlink_to_outside_file_is_not_followed() {
    let dir = tempdir().unwrap();
    let outside = dir.path().join("outside.txt");
    fs::write(&outside, b"dataXXdata").unwrap();
    let root = dir.path().join("root");
    fs::create_dir(&root).unwrap();
    std::os::unix::fs::symlink(&outside, root.join("l")).unwrap();
    rewrite_tree(&root, b"XX", b"YY").unwrap();
    assert_eq!(fs::read(&outside).unwrap(), b"dataXXdata");
}

#[test]
fn nested_directories_count_all_occurrences() {
    let dir = tempdir().unwrap();
    let deep = dir.path().join("a/b");
    fs::create_dir_all(&deep).unwrap();
    fs::write(deep.join("one"), b"XX").unwrap();
    fs::write(deep.join("two"), b"XXxXX").unwrap();
    fs::write(dir.path().join("a/zero"), b"plain").unwrap();
    std::os::unix::fs::symlink("vXX", deep.join("link")).unwrap();
    assert_eq!(rewrite_tree(dir.path(), b"XX", b"YY").unwrap(), 4);
    assert_eq!(fs::read(deep.join("one")).unwrap(), b"YY");
    assert_eq!(fs::read(deep.join("two")).unwrap(), b"YYxYY");
    assert_eq!(fs::read(dir.path().join("a/zero")).unwrap(), b"plain");
    assert_eq!(fs::read_link(deep.join("link")).unwrap(), Path::new("vYY"));
}

#[test]
fn root_can_be_a_single_file() {
    let dir = tempdir().unwrap();
    let file = dir.path().join("f");
    fs::write(&file, b"XX").unwrap();
    assert_eq!(rewrite_tree(&file, b"XX", b"YY").unwrap(), 1);
    assert_eq!(fs::read(&file).unwrap(), b"YY");
}

#[test]
fn occurrence_straddling_1_mib() {
    let dir = tempdir().unwrap();
    let file = dir.path().join("big");
    let mib = 1024 * 1024;
    let mut contents = vec![b'A'; 3 * mib];
    contents[mib - 1] = b'X';
    contents[mib] = b'X';
    fs::write(&file, &contents).unwrap();
    assert_eq!(rewrite_tree(dir.path(), b"XX", b"YY").unwrap(), 1);
    let mut expected = vec![b'A'; 3 * mib];
    expected[mib - 1] = b'Y';
    expected[mib] = b'Y';
    assert_eq!(fs::read(&file).unwrap(), expected);
}
