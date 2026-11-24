#[path = "../src/build/source_zip.rs"]
mod source_zip;

use std::fs::{self, File};
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use zip::{CompressionMethod, ZipWriter, write::SimpleFileOptions};

fn archive(path: &Path, name: &str, mode: u32, link: Option<&str>) {
    let mut writer = ZipWriter::new(File::create(path).unwrap());
    let options = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Deflated)
        .unix_permissions(mode);
    writer.start_file("source/readme", options).unwrap();
    writer.write_all(b"readme").unwrap();
    if let Some(target) = link {
        writer.add_symlink(name, target, options).unwrap();
    } else {
        writer.start_file(name, options).unwrap();
        writer.write_all(b"source content").unwrap();
    }
    writer.finish().unwrap();
}

#[test]
fn extracts_deflated_source_and_strips_prefix() {
    let dir = tempfile::tempdir().unwrap();
    let zip = dir.path().join("source.zip");
    let output = dir.path().join("output");
    archive(&zip, "source/nested/file", 0o644, None);
    source_zip::extract(&zip, &output, 1).unwrap();
    assert_eq!(
        fs::read(output.join("nested/file")).unwrap(),
        b"source content"
    );
}

#[test]
fn rejects_escaping_paths_before_extracting() {
    for name in ["../outside", "/outside", "source/../../outside"] {
        let dir = tempfile::tempdir().unwrap();
        let zip = dir.path().join("source.zip");
        let output = dir.path().join("output");
        archive(&zip, name, 0o644, None);
        assert!(source_zip::extract(&zip, &output, 1).is_err());
        assert!(!output.exists());
    }
}

#[test]
fn rejects_escaping_symlinks_before_extracting() {
    for target in ["../outside", "/outside"] {
        let dir = tempfile::tempdir().unwrap();
        let zip = dir.path().join("source.zip");
        let output = dir.path().join("output");
        archive(&zip, "source/link", 0o644, Some(target));
        assert!(source_zip::extract(&zip, &output, 1).is_err());
        assert!(!output.exists());
    }
}

#[test]
fn preserves_executable_permissions() {
    let dir = tempfile::tempdir().unwrap();
    let zip = dir.path().join("source.zip");
    let output = dir.path().join("output");
    archive(&zip, "source/script", 0o755, None);
    source_zip::extract(&zip, &output, 1).unwrap();
    assert_eq!(
        fs::metadata(output.join("script"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o755
    );
}
