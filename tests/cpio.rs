use std::os::unix::fs::PermissionsExt;

use vug::cpio::write_newc;

/// Reads the entries of a newc archive as (name, mode, data).
fn entries(mut archive: &[u8]) -> Vec<(String, u32, Vec<u8>)> {
    let mut found = Vec::new();
    loop {
        assert_eq!(&archive[..6], b"070701");
        let field = |index: usize| {
            let start = 6 + index * 8;
            u32::from_str_radix(std::str::from_utf8(&archive[start..start + 8]).unwrap(), 16)
                .unwrap() as usize
        };
        let (mode, size, name_size) = (field(1) as u32, field(6), field(11));
        let name = String::from_utf8(archive[110..110 + name_size - 1].to_vec()).unwrap();
        let data_start = (110 + name_size).div_ceil(4) * 4;
        let data = archive[data_start..data_start + size].to_vec();
        archive = &archive[(data_start + size).div_ceil(4) * 4..];
        if name == "TRAILER!!!" {
            assert!(archive.is_empty());
            return found;
        }
        found.push((name, mode, data));
    }
}

fn tree() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("bin")).unwrap();
    std::fs::write(dir.path().join("bin/tool"), b"#!/bin/sh\n").unwrap();
    std::fs::set_permissions(
        dir.path().join("bin/tool"),
        std::fs::Permissions::from_mode(0o755),
    )
    .unwrap();
    std::fs::write(dir.path().join("a.txt"), b"hello").unwrap();
    std::fs::set_permissions(
        dir.path().join("a.txt"),
        std::fs::Permissions::from_mode(0o644),
    )
    .unwrap();
    std::os::unix::fs::symlink("bin/tool", dir.path().join("init")).unwrap();
    dir
}

#[test]
fn lists_every_entry_in_order() {
    let dir = tree();
    let mut archive = Vec::new();
    write_newc(dir.path(), &mut archive).unwrap();
    let found = entries(&archive);
    let names: Vec<&str> = found.iter().map(|e| e.0.as_str()).collect();
    assert_eq!(names, ["a.txt", "bin", "bin/tool", "init"]);
    assert_eq!(found[0].1, 0o100644);
    assert_eq!(found[0].2, b"hello");
    assert_eq!(found[1].1 & 0o170000, 0o040000);
    assert_eq!(found[2].1, 0o100755);
    assert_eq!(found[3].1, 0o120777);
    assert_eq!(found[3].2, b"bin/tool");
    assert_eq!(archive.len() % 4, 0);
}

#[test]
fn same_tree_gives_the_same_bytes() {
    let (first, second) = (tree(), tree());
    let (mut a, mut b) = (Vec::new(), Vec::new());
    write_newc(first.path(), &mut a).unwrap();
    write_newc(second.path(), &mut b).unwrap();
    assert_eq!(a, b);
}

#[test]
fn empty_tree_is_only_the_trailer() {
    let dir = tempfile::tempdir().unwrap();
    let mut archive = Vec::new();
    write_newc(dir.path(), &mut archive).unwrap();
    assert!(entries(&archive).is_empty());
}
