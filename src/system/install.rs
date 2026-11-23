use std::fs::{self, File, OpenOptions};
use std::io::{self, Read as _, Seek as _, SeekFrom};
use std::os::unix::fs::{FileTypeExt as _, MetadataExt as _, OpenOptionsExt as _};
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use super::{Builder, MIB, Settings, image_with, load};

#[derive(Debug, PartialEq, Eq)]
struct Disk {
    path: PathBuf,
    number: u64,
    sysfs: PathBuf,
    sequence: String,
    bytes: u64,
}

fn read(path: &Path) -> Result<String, String> {
    fs::read_to_string(path)
        .map(|text| text.trim().to_owned())
        .map_err(|error| format!("{}: {error}", path.display()))
}

fn inspect(path: &Path) -> Result<Disk, String> {
    let path = fs::canonicalize(path).map_err(|error| format!("{}: {error}", path.display()))?;
    let metadata = fs::metadata(&path).map_err(|error| format!("{}: {error}", path.display()))?;
    if !metadata.file_type().is_block_device() {
        return Err(format!("{}: expected a whole block disk", path.display()));
    }
    let number = metadata.rdev();
    let sysfs = fs::canonicalize(format!(
        "/sys/dev/block/{}:{}",
        libc::major(number),
        libc::minor(number)
    ))
    .map_err(|error| format!("{}: cannot identify block disk: {error}", path.display()))?;
    if sysfs.starts_with("/sys/devices/virtual")
        || !read(&sysfs.join("uevent"))?
            .lines()
            .any(|line| line == "DEVTYPE=disk")
    {
        return Err(format!(
            "{}: expected a physical or VM whole disk",
            path.display()
        ));
    }
    if read(&sysfs.join("queue/logical_block_size"))? != "512" || read(&sysfs.join("ro"))? != "0" {
        return Err(format!(
            "{}: expected a writable disk with 512-byte logical sectors",
            path.display()
        ));
    }
    let sequence = read(&sysfs.join("diskseq"))?;
    let bytes = read(&sysfs.join("size"))?
        .parse::<u64>()
        .ok()
        .and_then(|sectors| sectors.checked_mul(512))
        .ok_or("invalid disk capacity")?;
    Ok(Disk {
        path,
        number,
        sysfs,
        sequence,
        bytes,
    })
}

fn claim(disk: &Disk, write: bool) -> Result<File, String> {
    let file = OpenOptions::new()
        .read(true)
        .write(write)
        .custom_flags(libc::O_EXCL | libc::O_NOFOLLOW)
        .open(&disk.path)
        .map_err(|error| {
            format!(
                "{}: cannot exclusively open disk: {error}",
                disk.path.display()
            )
        })?;
    let metadata = file.metadata().map_err(|error| error.to_string())?;
    if !metadata.file_type().is_block_device() || metadata.rdev() != disk.number {
        return Err("target device changed while opening it".into());
    }
    Ok(file)
}

fn headers(disk: &mut File) -> io::Result<[u8; 1024]> {
    let mut headers = [0; 1024];
    disk.seek(SeekFrom::Start(0))?;
    disk.read_exact(&mut headers)?;
    Ok(headers)
}

fn partition_table(headers: &[u8; 1024]) -> &'static str {
    if &headers[512..520] == b"EFI PART" {
        "gpt"
    } else if headers[510..512] == [0x55, 0xaa] {
        "mbr"
    } else {
        "unrecognized"
    }
}

pub fn run(settings: &Settings, name: &str, path: &Path, apply: bool) -> Result<Value, String> {
    let record = load(&settings.root, name)?;
    if record.boot.is_none() {
        return Err("installation requires an EFI boot loader".into());
    }
    let layout = record
        .disk
        .as_ref()
        .ok_or("installation requires a GPT disk layout")?;
    let target = inspect(path)?;
    let root_bytes = target
        .bytes
        .checked_sub(layout.esp_bytes + 2 * MIB)
        .filter(|bytes| *bytes >= 64 * MIB && bytes.is_multiple_of(1024))
        .ok_or("target capacity cannot accommodate this layout in whole KiB")?;

    let mut disk = claim(&target, false)?;
    let original =
        headers(&mut disk).map_err(|error| format!("inspect partition table: {error}"))?;
    let mut plan = json!({
        "Disk": target.path,
        "Device": format!("{}:{}", libc::major(target.number), libc::minor(target.number)),
        "DiskSequence": target.sequence,
        "Bytes": target.bytes,
        "ExistingTable": partition_table(&original),
        "Partitions": [
            { "Name": layout.esp_name, "Bytes": layout.esp_bytes, "Type": "vfat", "Mount": "/boot" },
            { "Name": layout.root_name, "Bytes": root_bytes, "Type": "ext4", "Mount": "/" }
        ],
        "Applied": false
    });
    if let Some(name) = &layout.luks {
        plan["Partitions"][1]["Luks"] = json!({ "Type": "luks2", "Name": name });
    }
    drop(disk);
    if !apply {
        return Ok(plan);
    }
    if unsafe { libc::geteuid() } != 0 {
        return Err("installation requires root".into());
    }
    if settings.binary_cache.is_none() || settings.trusted_key.is_none() {
        return Err("installation requires a binary cache and its trusted public key".into());
    }
    fs::create_dir_all(&settings.data_dir).map_err(|error| error.to_string())?;
    let work = tempfile::tempdir_in(&settings.data_dir).map_err(|error| error.to_string())?;
    let isolated = Settings {
        data_dir: work.path().join("data"),
        root: settings.root.clone(),
        cache_dir: settings.cache_dir.clone(),
        jobs: settings.jobs,
        binary_cache: settings.binary_cache.clone(),
        trusted_key: settings.trusted_key.clone(),
    };
    let image = work.path().join("disk.img");
    let system = image_with(
        &isolated,
        name,
        record,
        &image,
        &format!("{}k", root_bytes / 1024),
        &mut Builder::cached(&isolated)?,
    )?;
    let mut source = File::open(&image).map_err(|error| format!("open staged image: {error}"))?;
    if !system.efi || source.metadata().map_err(|error| error.to_string())?.len() != target.bytes {
        return Err("staged image does not match the planned bootable disk".into());
    }
    let expected = headers(&mut source).map_err(|error| format!("inspect staged GPT: {error}"))?;
    source.rewind().map_err(|error| error.to_string())?;
    let mut disk = claim(&target, true)?;
    if inspect(path)? != target
        || headers(&mut disk).map_err(|error| error.to_string())? != original
    {
        return Err("target changed while preparing the installation".into());
    }
    disk.seek(SeekFrom::Start(0))
        .map_err(|error| error.to_string())?;
    let written = io::copy(&mut source, &mut disk)
        .and_then(|bytes| disk.sync_all().map(|()| bytes))
        .map_err(|error| {
            format!("write or synchronize target: {error}. Target may be partially overwritten")
        })?;
    if written != target.bytes
        || headers(&mut disk).map_err(|error| format!("verify written GPT: {error}"))? != expected
    {
        return Err("written target failed verification and may be incomplete".into());
    }
    plan["Applied"] = json!(true);
    plan["System"] = json!(system.id);
    Ok(plan)
}
