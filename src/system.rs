//! Building a system from a system record (`docs/system.md`).

mod disk;
mod firmware;
mod initrd;
#[cfg(target_os = "linux")]
pub mod install;

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use vug::cpio;

use crate::build::{self, Builder, Settings};
use crate::naming::model;
use crate::record::dependency_output;
use vug::steps::shell_quote;

const STORE: &str = "/vug/store";
const IMAGE_UUID: &str = "76756700-0000-4000-8000-000000000001";

const INIT: &str = r#"#!{self}/bin/busybox sh
b={self}/bin/busybox
$b mountpoint -q /proc || $b mount -t proc proc /proc
$b mount -o remount,rw /
for d in boot dev etc home root run sys tmp var; do
  if [ ! -d /$d ]; then $b mkdir /$d; fi
done
$b chmod 1777 /tmp
$b mountpoint -q /dev || $b mount -t devtmpfs devtmpfs /dev
$b mountpoint -q /run || $b mount -t tmpfs -o mode=755,nosuid,nodev tmpfs /run
{self}/activate boot
if [ -L {self}/kernel-modules ]; then
  $b ln -s "$($b readlink {self}/kernel-modules)" /run/vug-kernel-modules
fi
exec {init}
"#;

const ACTIVATE: &str = r#"#!{self}/bin/busybox sh
set -eu
self={self}
b=$self/bin/busybox
mode=${1:-switch}
m=$self/manifest
state=/etc/.vug-links

conflict=0
while IFS= read -r p; do
  t=/etc/$p
  if [ -L "$t" ]; then
    if [ "$($b readlink "$t")" = "/run/current-system/etc/$p" ]; then continue; fi
  elif [ ! -e "$t" ]; then
    continue
  fi
  echo "vug: /etc/$p exists and is not the link vug manages" >&2
  conflict=1
done < $m/etc-links
targets=/etc/.vug-targets
while read -r p want; do
  t=/etc/$p
  if [ -L "$t" ]; then
    have=$($b readlink "$t")
    if [ "$have" = "$want" ]; then continue; fi
    if [ -f $targets ] && $b grep -qxF -- "$p $have" $targets; then continue; fi
  elif [ ! -e "$t" ]; then
    continue
  fi
  echo "vug: /etc/$p exists and is not the link vug manages" >&2
  conflict=1
done < $m/etc-targets
if [ "$mode" = check ]; then exit $conflict; fi
if [ $conflict = 1 ] && [ "$mode" != boot ]; then exit 1; fi

link() {
  if [ -L "$2" ] && [ "$($b readlink "$2")" = "$1" ]; then return 0; fi
  $b ln -s "$1" "$2"
}
link /run/current-system/sw /usr
link usr/bin /bin
link usr/bin /sbin
link usr/lib /lib

$b ln -s "$self" /run/.vug-current.$$
$b mv -T /run/.vug-current.$$ /run/current-system

if [ -f $state ]; then
  while IFS= read -r p; do
    if $b grep -qxF -- "$p" $m/etc-links; then continue; fi
    t=/etc/$p
    if [ -L "$t" ] && [ "$($b readlink "$t")" = "/run/current-system/etc/$p" ]; then
      $b rm -f "$t"
      d=$($b dirname "$t")
      while [ "$d" != /etc ] && $b rmdir "$d" 2>/dev/null; do d=$($b dirname "$d"); done
    fi
  done < $state
fi
while IFS= read -r p; do
  t=/etc/$p
  if [ -L "$t" ] || [ -e "$t" ]; then continue; fi
  $b mkdir -p "$($b dirname "$t")"
  $b ln -s "/run/current-system/etc/$p" "$t"
done < $m/etc-links
$b cp $m/etc-links $state.tmp
$b mv -f $state.tmp $state

if [ -f $targets ]; then
  while read -r p had; do
    if $b grep -q -- "^$p " $m/etc-targets; then continue; fi
    t=/etc/$p
    if [ -L "$t" ] && [ "$($b readlink "$t")" = "$had" ]; then $b rm -f "$t"; fi
  done < $targets
fi
while read -r p want; do
  t=/etc/$p
  if [ -L "$t" ]; then
    have=$($b readlink "$t")
    if [ "$have" = "$want" ]; then continue; fi
    if [ ! -f $targets ] || ! $b grep -qxF -- "$p $have" $targets; then continue; fi
  elif [ -e "$t" ]; then
    continue
  fi
  $b mkdir -p "$($b dirname "$t")"
  $b ln -s "$want" "$t.vug-tmp"
  $b mv -T "$t.vug-tmp" "$t"
done < $m/etc-targets
$b cp $m/etc-targets $targets.tmp
$b mv -f $targets.tmp $targets

while read -r perm p; do
  t=/etc/$p
  if [ -L "$t" ] || [ -e "$t" ]; then continue; fi
  $b mkdir -p "$($b dirname "$t")"
  $b cp "$self/seed/$p" "$t.vug-tmp"
  $b chmod "$perm" "$t.vug-tmp"
  $b mv -f "$t.vug-tmp" "$t"
done < $m/etc-seeds

if [ -s $m/passwd ]; then
  for f in passwd group; do
    $b cp $m/$f /etc/$f.vug-tmp
    $b chmod 644 /etc/$f.vug-tmp
    $b mv -f /etc/$f.vug-tmp /etc/$f
  done
  umask 077
  if [ -f /etc/shadow ]; then
    $b awk -F: 'FILENAME == ARGV[1] { old[$1] = $0; next } ($1 in old) { print old[$1]; next } { print }' /etc/shadow $m/shadow > /etc/shadow.vug-tmp
  else
    $b cp $m/shadow /etc/shadow.vug-tmp
  fi
  $b chmod 600 /etc/shadow.vug-tmp
  $b mv -f /etc/shadow.vug-tmp /etc/shadow
  umask 022
  while read -r uid gid home; do
    if [ ! -e "$home" ]; then
      $b mkdir -p "$home"
      $b chown "$uid:$gid" "$home"
      $b chmod 700 "$home"
    fi
  done < $m/homes
fi

w=/run/wrappers
if ! $b mountpoint -q $w; then
  $b mkdir -p $w
  $b mount -t tmpfs -o mode=755,nodev tmpfs $w
fi
d=$w/${self##*/}.$$
$b mkdir "$d"
while read -r name path; do
  $b cp "$path" "$d/$name"
  $b chown 0:0 "$d/$name"
  $b chmod 4755 "$d/$name"
done < $m/setuid
$b ln -s "${d##*/}" $w/.bin.$$
$b mv -T $w/.bin.$$ $w/bin
for o in $w/*; do
  if [ "$o" != "$d" ] && [ "$o" != $w/bin ]; then $b rm -rf "$o"; fi
done
"#;

const SWITCH: &str = r#"#!{self}/bin/busybox sh
set -eu
self={self}
b=$self/bin/busybox
if [ "$($b id -u)" != 0 ]; then
  echo "vug: switch-to-system must run as root" >&2
  exit 1
fi
while IFS= read -r path; do
  if [ ! -d "$path" ]; then
    echo "vug: missing store path $path required by $self" >&2
    exit 1
  fi
done < "$self/manifest/closure"
$self/activate check
if [ -f "$self/boot/layout" ] || [ -f /vug/disk/layout ]; then
  if ! $b cmp -s "$self/boot/layout" /vug/disk/layout; then
    echo "vug: the requested disk layout differs from the installed layout" >&2
    exit 1
  fi
fi
if [ -x $self/bin/install-boot ] && ! $b mountpoint -q /boot; then
  echo "vug: /boot is not mounted; nothing was changed" >&2
  exit 1
fi
old=$($b readlink /run/current-system)

p=/vug/profiles
$b mkdir -p $p
n=
max=0
for l in $p/system-*; do
  if [ ! -L "$l" ]; then continue; fi
  k=${l##*/system-}
  case $k in '' | *[!0-9]*) continue ;; esac
  if [ "$($b readlink "$l")" = "$self" ]; then n=$k; fi
  if [ "$k" -gt "$max" ]; then max=$k; fi
done
if [ -z "$n" ]; then
  n=$((max + 1))
  $b ln -s "$self" $p/system-$n
fi
$b ln -s system-$n $p/.system.$$
$b mv -T $p/.system.$$ $p/system
if [ -x $self/bin/install-boot ]; then $self/bin/install-boot; fi

sc=$self/sw/bin/systemctl
live=0
if [ -x $sc ] && [ -d /run/systemd/system ]; then live=1; fi

unit_dirs="etc/systemd/system sw/lib/systemd/system"
names() {
  for d in $unit_dirs; do
    if [ -d "$1/$d" ]; then $b ls "$1/$d"; fi
  done
}
unit() {
  for d in $unit_dirs; do
    if [ -f "$1/$d/$2" ]; then
      $b cat "$1/$d/$2"
      break
    fi
  done
  for d in $unit_dirs; do
    for c in "$1/$d/$2.d"/*.conf; do
      if [ -f "$c" ]; then $b cat "$c"; fi
    done
  done
}
changed=
removed=
if [ $live = 1 ] && [ "$old" != "$self" ]; then
  for u in $({ names "$old"; names "$self"; } | $b sed 's/\.d$//' | $b sort -u); do
    case $u in
      *@.*) continue ;;
      *.service | *.socket | *.timer | *.path) ;;
      *) continue ;;
    esac
    before=$(unit "$old" "$u")
    after=$(unit "$self" "$u")
    if [ "$before" = "$after" ]; then continue; fi
    if [ -z "$after" ]; then removed="$removed $u"; else changed="$changed $u"; fi
  done
fi

if [ -n "$removed" ]; then $sc stop $removed || true; fi
$self/activate switch
if [ $live = 1 ]; then
  $sc daemon-reload
  if [ -n "$changed" ]; then $sc try-restart --no-block $changed || true; fi
  for t in $($sc list-units --type=target --state=active --no-legend --plain | $b awk '{ print $1 }'); do
    if [ "$($sc show -p RefuseManualStart --value "$t")" = no ]; then
      $sc start --no-block "$t" || true
    fi
  done
fi
echo "vug: generation $n is $self"
"#;

const FRAGMENT_KEYS: [&str; 5] = ["packages", "file", "group", "user", "setuid"];

const INSTALL_BOOT: &str = r#"#!{self}/bin/busybox sh
set -eu
b={self}/bin/busybox
esp=/boot
if ! $b mountpoint -q $esp; then
  echo "vug: $esp is not mounted; the boot entries are unchanged" >&2
  exit 1
fi
p=/vug/profiles
$b mkdir -p $esp/EFI/BOOT $esp/vug
$b cp {self}/boot/loader $esp/EFI/BOOT/BOOTX64.EFI.tmp
$b mv -f $esp/EFI/BOOT/BOOTX64.EFI.tmp $esp/EFI/BOOT/BOOTX64.EFI
default=$($b readlink $p/system)
default=${default#system-}
body=$esp/limine.conf.body
: > $body
keep=
index=0
chosen=1
for g in $($b ls $p | $b sed -n 's/^system-\([0-9][0-9]*\)$/\1/p' | $b sort -rn); do
  s=$p/system-$g
  if [ ! -f $s/boot/entry ]; then continue; fi
  index=$((index + 1))
  k=$($b readlink -f $s/kernel)
  id=${k#/vug/store/}
  id=${id%%/*}
  if [ ! -f $esp/vug/$id.bzImage ]; then
    $b cp "$k" $esp/vug/$id.bzImage.tmp
    $b mv -f $esp/vug/$id.bzImage.tmp $esp/vug/$id.bzImage
  fi
  keep="$keep $id.bzImage"
  if [ -f "$s/boot/initrd.gz" ]; then
    archive=$($b cat "$s/boot/initrd-name")
    $b mkdir -p "$esp/vug-initrd"
    if [ ! -f "$esp/vug-initrd/$archive" ]; then
      $b cp "$s/boot/initrd.gz" "$esp/vug-initrd/$archive.tmp"
      $b mv -f "$esp/vug-initrd/$archive.tmp" "$esp/vug-initrd/$archive"
    fi
    keep="$keep $archive"
  fi
  if [ "$g" = "$default" ]; then chosen=$index; fi
  echo "/vug generation $g" >> $body
  if [ -f "$s/boot/layout" ]; then
    root_uuid=$($b cat /vug/disk/root-partuuid)
    $b sed "s/@VUG_ROOT@/$root_uuid/" "$s/boot/entry" >> $body
  else
    $b cat "$s/boot/entry" >> $body
  fi
done
{
  echo "timeout: 3"
  echo "default_entry: $chosen"
  $b cat $body
} > $esp/limine.conf.tmp
$b mv -f $esp/limine.conf.tmp $esp/limine.conf
$b rm -f $body
for f in "$esp/vug/"* "$esp/vug-initrd/"*; do
  if [ ! -f "$f" ]; then continue; fi
  case " $keep " in
    *" ${f##*/} "*) ;;
    *) $b rm -f "$f" ;;
  esac
done
$b sync
"#;

const ESP_NAME: &str = "vug-esp";
const ROOT_NAME: &str = "vug-root";
const ESP_UUID: &str = "76756700-0000-4000-8000-000000000002";
const ROOT_UUID: &str = "76756700-0000-4000-8000-000000000003";
const ESP_BYTES: u64 = 256 << 20;
const MIB: u64 = 1 << 20;

#[derive(Clone, Copy, PartialEq)]
enum FileKind {
    Link,
    Seed,
}

#[derive(PartialEq)]
pub struct SystemFile {
    /// Path below `/etc`, without a leading slash.
    path: String,
    kind: FileKind,
    mode: u32,
    content: String,
    /// For a link file that is a link to another path instead of content.
    target: Option<String>,
}

#[derive(PartialEq)]
struct Group {
    name: String,
    gid: u32,
}

#[derive(PartialEq)]
struct User {
    name: String,
    uid: u32,
    group: String,
    groups: Vec<String>,
    home: String,
    shell: String,
    password: String,
}

struct Fragment {
    packages: Vec<String>,
    files: Vec<SystemFile>,
    groups: Vec<Group>,
    users: Vec<User>,
    /// (package, program path inside the package)
    setuid: Vec<(String, String)>,
}

pub struct SystemRecord {
    /// (path below `.vug`, text) of the system record and its service records.
    texts: Vec<(String, String)>,
    kernel: String,
    init: String,
    sh: Option<String>,
    boot: Option<Boot>,
    disk: Option<disk::Layout>,
    firmware: Vec<String>,
    parts: Fragment,
}

struct Boot {
    /// The EFI program of the loader, with `{pkg:NAME}`.
    loader: String,
    /// What the kernel takes as `root=`.
    root: String,
    initrd: Option<initrd::Config>,
    arguments: Vec<String>,
}

pub struct BuiltSystem {
    pub name: String,
    pub id: String,
    /// The kernel image on the host.
    pub kernel: PathBuf,
    /// Ids of the system, its kernel, its packages, and what they need at
    /// run time.
    pub closure: BTreeSet<String>,
    /// Whether the system has boot entries for an EFI loader.
    pub efi: bool,
    disk: Option<disk::Layout>,
}

fn io<T>(what: &str, path: &Path, result: std::io::Result<T>) -> Result<T, String> {
    result.map_err(|e| format!("{what} {}: {e}", path.display()))
}

fn string(table: &toml::Table, key: &str, context: &str) -> Result<String, String> {
    table
        .get(key)
        .and_then(|v| v.as_str())
        .map(str::to_owned)
        .ok_or_else(|| format!("{context}{key}: expected a string"))
}

fn unknown_keys(table: &toml::Table, allowed: &[&str], context: &str) -> Result<(), String> {
    match table.keys().find(|k| !allowed.contains(&k.as_str())) {
        Some(key) => Err(format!("{context}{key}: unknown key")),
        None => Ok(()),
    }
}

fn tables<'a>(table: &'a toml::Table, key: &str) -> Result<Vec<&'a toml::Table>, String> {
    let error = || format!("{key}: expected an array of tables");
    match table.get(key) {
        None => Ok(Vec::new()),
        Some(value) => value
            .as_array()
            .ok_or_else(error)?
            .iter()
            .map(|entry| entry.as_table().ok_or_else(error))
            .collect(),
    }
}

fn number(table: &toml::Table, key: &str, context: &str) -> Result<u32, String> {
    table
        .get(key)
        .and_then(|v| v.as_integer())
        .and_then(|n| u32::try_from(n).ok())
        .ok_or_else(|| format!("{context}{key}: expected a non-negative integer"))
}

fn strings(table: &toml::Table, key: &str, context: &str) -> Result<Vec<String>, String> {
    let error = || format!("{context}{key}: expected an array of strings");
    table
        .get(key)
        .and_then(|v| v.as_array())
        .ok_or_else(error)?
        .iter()
        .map(|v| v.as_str().map(str::to_owned).ok_or_else(error))
        .collect()
}

fn valid_account(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_lowercase() || c == '_')
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
}

fn valid_image_path(path: &str) -> bool {
    path.starts_with('/')
        && path.len() > 1
        && !path.ends_with('/')
        && path.chars().all(|c| c.is_ascii_graphic())
        && path[1..]
            .split('/')
            .all(|part| !part.is_empty() && part != "." && part != "..")
}

fn parse(path: &Path) -> Result<(String, toml::Table), String> {
    let text = io("read", path, fs::read_to_string(path))?;
    let table = text
        .parse::<toml::Table>()
        .map_err(|e| format!("{}: {e}", path.display()))?;
    Ok((text, table))
}

/// Reads the packages, files, groups, users, and setuid programs of a system
/// or service record. `parameters` holds the values of `{param:NAME}` for a
/// service record.
fn fragment(
    table: &toml::Table,
    parameters: Option<&BTreeMap<String, String>>,
) -> Result<Fragment, String> {
    let fill = |text: String| match parameters {
        None => Ok(text),
        Some(values) => expand(&text, "param", values, "is not a parameter of this service"),
    };
    let packages = match table.get("packages") {
        None if parameters.is_some() => Vec::new(),
        _ => strings(table, "packages", "")?,
    };
    let packages = packages
        .iter()
        .map(|package| package_selector(package).map(str::to_owned))
        .collect::<Result<_, _>>()?;

    let mut groups = Vec::new();
    for entry in tables(table, "group")? {
        unknown_keys(entry, &["name", "gid"], "group.")?;
        let name = string(entry, "name", "group.")?;
        if !valid_account(&name) {
            return Err(format!("group.name: {name:?}: not a valid group name"));
        }
        groups.push(Group {
            name,
            gid: number(entry, "gid", "group.")?,
        });
    }
    let mut users = Vec::new();
    for entry in tables(table, "user")? {
        unknown_keys(
            entry,
            &[
                "name", "uid", "group", "groups", "home", "shell", "password",
            ],
            "user.",
        )?;
        let name = string(entry, "name", "user.")?;
        if !valid_account(&name) {
            return Err(format!("user.name: {name:?}: not a valid user name"));
        }
        let home = string(entry, "home", "user.")?;
        let shell = string(entry, "shell", "user.")?;
        if home != "/" && !valid_image_path(&home) {
            return Err(format!("user.home: {home:?}: expected an absolute path"));
        }
        if !valid_image_path(&shell) {
            return Err(format!("user.shell: {shell:?}: expected an absolute path"));
        }
        let password = match entry.get("password") {
            None => "!".to_owned(),
            Some(_) => string(entry, "password", "user.")?,
        };
        if password.chars().any(|c| c == ':' || !c.is_ascii_graphic()) {
            return Err(format!(
                "user.password: {name}: expected a shadow password field"
            ));
        }
        users.push(User {
            uid: number(entry, "uid", "user.")?,
            group: string(entry, "group", "user.")?,
            groups: match entry.get("groups") {
                None => Vec::new(),
                Some(_) => strings(entry, "groups", "user.")?,
            },
            name,
            home,
            shell,
            password,
        });
    }

    let mut files = Vec::new();
    for entry in tables(table, "file")? {
        unknown_keys(
            entry,
            &["path", "kind", "mode", "content", "target"],
            "file.",
        )?;
        let path = fill(string(entry, "path", "file.")?)?;
        let Some(relative) = path
            .strip_prefix("/etc/")
            .filter(|_| valid_image_path(&path))
        else {
            return Err(format!("file.path: {path:?}: expected a path below /etc"));
        };
        let kind = match entry.get("kind").map(|v| v.as_str()) {
            None | Some(Some("link")) => FileKind::Link,
            Some(Some("seed")) => FileKind::Seed,
            Some(_) => return Err(format!("file.kind: {path}: expected \"link\" or \"seed\"")),
        };
        let mode = match entry.get("mode") {
            None => 0o644,
            Some(value) => value
                .as_str()
                .and_then(|m| u32::from_str_radix(m, 8).ok())
                .filter(|m| *m <= 0o7777)
                .ok_or_else(|| format!("file.mode: {path}: expected octal digits in a string"))?,
        };
        if kind == FileKind::Link && mode != 0o644 && mode != 0o755 {
            return Err(format!(
                "file.mode: {path}: a link file takes \"644\" or \"755\""
            ));
        }
        let target = match entry.get("target") {
            None => None,
            Some(_) => Some(fill(string(entry, "target", "file.")?)?),
        };
        if target.is_some()
            && (kind != FileKind::Link
                || entry.contains_key("content")
                || mode != 0o644
                || relative.starts_with(".vug-"))
        {
            return Err(format!(
                "file.target: {path}: goes with a link file without content or mode"
            ));
        }
        files.push(SystemFile {
            path: relative.to_owned(),
            kind,
            mode,
            content: match target {
                Some(_) => String::new(),
                None => fill(string(entry, "content", "file.")?)?,
            },
            target,
        });
    }

    let mut setuid = Vec::new();
    for entry in tables(table, "setuid")? {
        unknown_keys(entry, &["package", "program"], "setuid.")?;
        let package = string(entry, "package", "setuid.")?;
        let package = package_selector(&package)?.to_owned();
        let program = string(entry, "program", "setuid.")?;
        if !valid_image_path(&format!("/{program}")) {
            return Err(format!(
                "setuid.program: {program:?}: expected a path relative to the package"
            ));
        }
        setuid.push((package, program));
    }
    Ok(Fragment {
        packages,
        files,
        groups,
        users,
        setuid,
    })
}

/// Adds `from` to `into`. An entry both hold must be the same in both.
fn merge(into: &mut Fragment, from: Fragment) -> Result<(), String> {
    for package in from.packages {
        if !into.packages.contains(&package) {
            into.packages.push(package);
        }
    }
    for file in from.files {
        match into.files.iter().find(|f| f.path == file.path) {
            None => into.files.push(file),
            Some(other) if *other == file => {}
            Some(_) => {
                return Err(format!(
                    "file.path: \"/etc/{}\": listed twice with different contents",
                    file.path
                ));
            }
        }
    }
    for group in from.groups {
        match into.groups.iter().find(|g| g.name == group.name) {
            None => into.groups.push(group),
            Some(other) if *other == group => {}
            Some(_) => {
                return Err(format!(
                    "group.name: {:?}: listed twice with different ids",
                    group.name
                ));
            }
        }
    }
    for user in from.users {
        match into.users.iter().find(|u| u.name == user.name) {
            None => into.users.push(user),
            Some(other) if *other == user => {}
            Some(_) => {
                return Err(format!(
                    "user.name: {:?}: listed twice with different fields",
                    user.name
                ));
            }
        }
    }
    for entry in from.setuid {
        if !into.setuid.contains(&entry) {
            into.setuid.push(entry);
        }
    }
    Ok(())
}

fn validate(parts: &Fragment) -> Result<(), String> {
    for (index, group) in parts.groups.iter().enumerate() {
        if parts.groups[..index].iter().any(|g| g.gid == group.gid) {
            return Err(format!("group.gid: {}: used by two groups", group.gid));
        }
    }
    for (index, user) in parts.users.iter().enumerate() {
        if parts.users[..index].iter().any(|u| u.uid == user.uid) {
            return Err(format!("user.uid: {}: used by two users", user.uid));
        }
        for group in user.groups.iter().chain([&user.group]) {
            if !parts.groups.iter().any(|g| &g.name == group) {
                return Err(format!(
                    "user.{}: {group:?} is not a group of this system",
                    user.name
                ));
            }
        }
    }
    for (index, file) in parts.files.iter().enumerate() {
        let reserved = matches!(file.path.as_str(), ".vug-links" | ".vug-targets")
            || (!parts.users.is_empty()
                && matches!(file.path.as_str(), "passwd" | "group" | "shadow"));
        if reserved {
            return Err(format!(
                "file.path: \"/etc/{}\": written by vug itself",
                file.path
            ));
        }
        let inside = |a: &str, b: &str| a.starts_with(&format!("{b}/"));
        for other in &parts.files[..index] {
            if inside(&file.path, &other.path) || inside(&other.path, &file.path) {
                return Err(format!(
                    "file.path: \"/etc/{}\": inside another listed path",
                    file.path.as_str().max(other.path.as_str())
                ));
            }
        }
    }
    for (package, _) in &parts.setuid {
        if !parts.packages.contains(package) {
            return Err(format!(
                "setuid.package: {package:?} is not a package of this system"
            ));
        }
    }
    Ok(())
}

fn package_selector(selector: &str) -> Result<&str, String> {
    let (package, output) = dependency_output(selector);
    if !model::valid_name(package) || !model::valid_name(output) {
        return Err(format!(
            "{selector:?}: not a valid package or output selector"
        ));
    }
    Ok(if output == "out" { package } else { selector })
}

/// Reads `services/<name>.toml` with the parameter values `given`.
fn load_service(
    root: &Path,
    name: &str,
    given: &toml::Table,
) -> Result<(String, Fragment), String> {
    if !model::valid_name(name) {
        return Err(format!("{name:?}: not a valid service name"));
    }
    let (text, table) = parse(&root.join("services").join(format!("{name}.toml")))?;
    let mut allowed = vec!["name", "parameter"];
    allowed.extend(FRAGMENT_KEYS);
    unknown_keys(&table, &allowed, "")?;
    if string(&table, "name", "")? != name {
        return Err(format!("name: expected {name:?}"));
    }
    let declared = match table.get("parameter") {
        None => toml::Table::new(),
        Some(value) => value
            .as_table()
            .ok_or("parameter: expected a table")?
            .clone(),
    };
    let mut values = BTreeMap::new();
    for (parameter, settings) in &declared {
        let settings = settings
            .as_table()
            .ok_or_else(|| format!("parameter.{parameter}: expected a table"))?;
        unknown_keys(settings, &["default"], &format!("parameter.{parameter}."))?;
        let value = match (given.get(parameter), settings.get("default")) {
            (Some(value), _) | (None, Some(value)) => value
                .as_str()
                .ok_or_else(|| format!("{parameter}: expected a string"))?,
            (None, None) => return Err(format!("{parameter}: needs a value")),
        };
        values.insert(parameter.clone(), value.to_owned());
    }
    if let Some(unknown) = given.keys().find(|key| !declared.contains_key(*key)) {
        return Err(format!("{unknown}: not a parameter of this service"));
    }
    Ok((text, fragment(&table, Some(&values))?))
}

/// Reads `systems/<name>.toml` under `root` and the service records it
/// names, and merges them.
pub fn load(root: &Path, name: &str) -> Result<SystemRecord, String> {
    if !model::valid_name(name) {
        return Err(format!("{name:?}: not a valid system name"));
    }
    let (text, table) = parse(&root.join("systems").join(format!("{name}.toml")))?;
    let mut allowed = vec![
        "name",
        "kernel",
        "init",
        "sh",
        "boot",
        "service",
        "firmware-packages",
        "disk",
        "filesystem",
    ];
    allowed.extend(FRAGMENT_KEYS);
    unknown_keys(&table, &allowed, "")?;
    if string(&table, "name", "")? != name {
        return Err(format!("name: expected {name:?}"));
    }
    let kernel = string(&table, "kernel", "")?;
    let kernel = package_selector(&kernel)?.to_owned();
    let init = string(&table, "init", "")?;
    if init.split_whitespace().next().is_none() {
        return Err("init: expected a command".to_owned());
    }
    let sh = match table.get("sh") {
        None => None,
        Some(_) => Some(string(&table, "sh", "")?),
    };
    let disk = disk::Layout::load(&table)?;
    let firmware = firmware::packages(&table)?;
    let boot = match table.get("boot") {
        None => None,
        Some(value) => {
            let entry = value.as_table().ok_or("boot: expected a table")?;
            unknown_keys(entry, &["loader", "root", "arguments", "initrd"], "boot.")?;
            let initrd = entry.get("initrd").map(initrd::Config::load).transpose()?;
            let arguments = match entry.get("arguments") {
                None => Vec::new(),
                Some(_) => strings(entry, "arguments", "boot.")?,
            };
            if disk.is_some() && (initrd.is_none() || entry.contains_key("root")) {
                return Err("disk: requires boot.initrd and derives boot.root".into());
            }
            let root = match entry.get("root") {
                None if disk.is_some() => "PARTUUID=@VUG_ROOT@".to_owned(),
                None if initrd.is_some() => format!("PARTUUID={ROOT_UUID}"),
                None => format!("PARTLABEL={ROOT_NAME}"),
                Some(_) => string(entry, "root", "boot.")?,
            };
            if arguments
                .iter()
                .chain([&root])
                .any(|word| word.is_empty() || word.chars().any(|c| !c.is_ascii_graphic()))
            {
                return Err("boot: an argument is empty or has white space".to_owned());
            }
            if initrd.is_some() && disk.is_none() && !initrd::valid_root(&root) {
                return Err("boot.root: initrd requires PARTUUID=<uuid>".to_owned());
            }
            if initrd.is_some()
                && arguments.iter().any(|arg| {
                    ["root=", "init=", "rdinit="]
                        .iter()
                        .any(|key| arg.starts_with(key))
                })
            {
                return Err("boot.arguments: initrd reserves root=, init= and rdinit=".to_owned());
            }
            Some(Boot {
                loader: string(entry, "loader", "boot.")?,
                root,
                initrd,
                arguments,
            })
        }
    };
    let mut parts = fragment(&table, None)?;
    let mut texts = vec![("system.toml".to_owned(), text)];
    let services = match table.get("service") {
        None => toml::Table::new(),
        Some(value) => value.as_table().ok_or("service: expected a table")?.clone(),
    };
    let mut names: Vec<&String> = services.keys().collect();
    names.sort();
    for service in names {
        let context = |error: String| format!("service.{service}: {error}");
        let given = services[service]
            .as_table()
            .ok_or_else(|| context("expected a table".to_owned()))?;
        let (text, part) = load_service(root, service, given).map_err(context)?;
        merge(&mut parts, part).map_err(context)?;
        texts.push((format!("services/{service}.toml"), text));
    }
    if disk.is_some()
        && (boot.is_none()
            || services.contains_key("esp")
            || parts
                .files
                .iter()
                .any(|file| matches!(file.path.as_str(), "fstab" | "systemd/system/boot.mount")))
    {
        return Err("disk: requires EFI boot and owns fstab and the ESP mount".into());
    }
    validate(&parts)?;
    Ok(SystemRecord {
        texts,
        kernel,
        init,
        sh,
        boot,
        disk,
        firmware,
        parts,
    })
}

fn copy_tree(from: &Path, to: &Path) -> Result<(), String> {
    let metadata = io("stat", from, fs::symlink_metadata(from))?;
    let file_type = metadata.file_type();
    if file_type.is_symlink() {
        let target = io("readlink", from, fs::read_link(from))?;
        io("symlink", to, std::os::unix::fs::symlink(target, to))
    } else if file_type.is_dir() {
        io("create", to, fs::create_dir_all(to))?;
        let mut entries: Vec<_> = io("read", from, fs::read_dir(from))?
            .filter_map(|entry| entry.ok().map(|e| e.file_name()))
            .collect();
        entries.sort();
        for name in entries {
            copy_tree(&from.join(&name), &to.join(&name))?;
        }
        io("chmod", to, fs::set_permissions(to, metadata.permissions()))
    } else {
        io("copy", to, fs::copy(from, to)).map(|_| ())
    }
}

/// Replaces `{KEY:NAME}` by the value of NAME. `missing` completes the
/// error for a NAME without a value.
pub(crate) fn expand(
    text: &str,
    key: &str,
    values: &BTreeMap<String, String>,
    missing: &str,
) -> Result<String, String> {
    let open = format!("{{{key}:");
    let mut out = String::new();
    let mut rest = text;
    while let Some(start) = rest.find(&open) {
        let (before, tail) = rest.split_at(start);
        let Some(end) = tail.find('}') else {
            break;
        };
        let name = &tail[open.len()..end];
        let value = values
            .get(name)
            .ok_or_else(|| format!("{open}{name}}}: {name:?} {missing}"))?;
        out.push_str(before);
        out.push_str(value);
        rest = &tail[end + 1..];
    }
    out.push_str(rest);
    Ok(out)
}

fn write_file(path: &Path, content: &[u8], mode: u32) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        io("create", parent, fs::create_dir_all(parent))?;
    }
    io("write", path, fs::write(path, content))?;
    io(
        "chmod",
        path,
        fs::set_permissions(path, fs::Permissions::from_mode(mode)),
    )
}

fn symlink(target: impl AsRef<Path>, link: &Path) -> Result<(), String> {
    if let Some(parent) = link.parent() {
        io("create", parent, fs::create_dir_all(parent))?;
    }
    if link.symlink_metadata().is_ok() {
        io("remove", link, fs::remove_file(link))?;
    }
    io("symlink", link, std::os::unix::fs::symlink(target, link))
}

/// The directory on the host behind a store path, when it is a directory
/// and not a link.
fn store_directory(store_dir: &Path, path: &Path) -> Option<PathBuf> {
    let host = store_dir.join(path.strip_prefix(STORE).ok()?);
    fs::symlink_metadata(&host)
        .ok()
        .filter(|metadata| metadata.is_dir())
        .map(|_| host)
}

/// Puts the store path `target` at `link` in a link farm. Where `link`
/// already is a directory of another package and `target` is a directory
/// too, the two are merged entry by entry; otherwise `target` replaces what
/// is there.
fn merge_link(store_dir: &Path, target: &Path, link: &Path) -> Result<(), String> {
    let Some(new_dir) = store_directory(store_dir, target) else {
        return symlink(target, link);
    };
    let Ok(metadata) = fs::symlink_metadata(link) else {
        return symlink(target, link);
    };
    if metadata.file_type().is_symlink() {
        let earlier = io("readlink", link, fs::read_link(link))?;
        let Some(earlier_dir) = store_directory(store_dir, &earlier) else {
            return symlink(target, link);
        };
        io("remove", link, fs::remove_file(link))?;
        io("create", link, fs::create_dir(link))?;
        for entry in entry_names(&earlier_dir) {
            symlink(earlier.join(&entry), &link.join(&entry))?;
        }
    } else if !metadata.is_dir() {
        return symlink(target, link);
    }
    for entry in entry_names(&new_dir) {
        merge_link(store_dir, &target.join(&entry), &link.join(&entry))?;
    }
    Ok(())
}

fn entry_names(dir: &Path) -> Vec<std::ffi::OsString> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut names: Vec<_> = entries
        .filter_map(|entry| entry.ok().map(|e| e.file_name()))
        .collect();
    names.sort();
    names
}

fn remove_tree(path: &Path) -> Result<(), String> {
    if path.symlink_metadata().is_ok() {
        io("remove", path, fs::remove_dir_all(path))?;
    }
    Ok(())
}

fn work_dir(settings: &Settings, what: &str) -> Result<PathBuf, String> {
    let dir = settings
        .data_dir
        .join("tmp")
        .join(format!("{what}-{}", std::process::id()));
    remove_tree(&dir)?;
    io("create", &dir, fs::create_dir_all(&dir))?;
    Ok(dir)
}

pub fn build(settings: &Settings, name: &str) -> Result<BuiltSystem, String> {
    let record = load(&settings.root, name)?;
    build_with(settings, name, record, &mut Builder::new(settings)?)
}

fn build_with(
    settings: &Settings,
    name: &str,
    record: SystemRecord,
    builder: &mut Builder<'_>,
) -> Result<BuiltSystem, String> {
    let pkg = |text: &str, paths: &BTreeMap<String, String>| {
        expand(text, "pkg", paths, "is not a package of this system")
    };
    let kernel = builder.build(&record.kernel, None)?;
    let mut closure: BTreeSet<String> = kernel.closure().clone();
    let mut paths = BTreeMap::new();
    let mut roots = Vec::new();
    for package in &record.parts.packages {
        let built = builder.build(package, None)?;
        closure.extend(built.closure().iter().cloned());
        let path = format!("{STORE}/{}", built.id);
        if !package.contains(':') {
            paths.insert(format!("{package}:out"), path.clone());
        }
        paths.insert(package.clone(), path);
        roots.push(built.id);
    }

    let store_dir = settings.data_dir.join("store");
    let this = build::placeholder_path();
    let work = work_dir(settings, "system")?;
    let out = work.join("out");

    let mut firmware = firmware::Files::new();
    for package in &record.firmware {
        let built = builder.build(package, None)?;
        closure.extend(built.closure().iter().cloned());
        firmware::collect(
            &store_dir.join(&built.id).join("lib/firmware"),
            Path::new(""),
            &mut firmware,
        )?;
    }

    let busybox = builder.busybox()?;
    write_file(
        &out.join("bin/busybox"),
        &io("read", &busybox, fs::read(&busybox))?,
        0o755,
    )?;
    let init = pkg(&record.init, &paths)?
        .split_whitespace()
        .map(shell_quote)
        .collect::<Vec<_>>()
        .join(" ");
    for (path, text) in [
        ("init", INIT.replace("{init}", &init)),
        ("activate", ACTIVATE.to_owned()),
        ("bin/switch-to-system", SWITCH.to_owned()),
    ] {
        write_file(
            &out.join(path),
            text.replace("{self}", &this).as_bytes(),
            0o755,
        )?;
    }
    symlink(
        format!("{STORE}/{}/boot/bzImage", kernel.id),
        &out.join("kernel"),
    )?;
    if let Some(layout) = &record.disk {
        write_file(
            &out.join("boot/layout"),
            layout.partition_spec().as_bytes(),
            0o644,
        )?;
    }
    if let Some(boot) = &record.boot {
        let loader = pkg(&boot.loader, &paths)?;
        if !valid_image_path(&loader) {
            return Err(format!(
                "boot.loader: {loader:?}: expected an absolute path"
            ));
        }
        symlink(loader, &out.join("boot/loader"))?;
        let mut cmdline = format!("root={} rw init={this}/init", boot.root);
        for argument in &boot.arguments {
            cmdline.push(' ');
            cmdline.push_str(argument);
        }
        let module = if let Some(config) = &boot.initrd {
            let archive = initrd::build(
                settings, builder, config, &kernel.id, &busybox, &work, &firmware,
            )?;
            write_file(&out.join("boot/initrd-name"), archive.as_bytes(), 0o644)?;
            format!("    module_path: boot():/vug-initrd/{archive}\n")
        } else {
            String::new()
        };
        write_file(
            &out.join("boot/entry"),
            format!(
                "    protocol: linux\n    path: boot():/vug/{}.bzImage\n{module}    cmdline: {cmdline}\n",
                kernel.id
            )
            .as_bytes(),
            0o644,
        )?;
        write_file(
            &out.join("bin/install-boot"),
            INSTALL_BOOT.replace("{self}", &this).as_bytes(),
            0o755,
        )?;
    }

    for dir in ["bin", "lib", "share"] {
        io("create", &out, fs::create_dir_all(out.join("sw").join(dir)))?;
    }
    symlink("bin", &out.join("sw/sbin"))?;
    for id in &roots {
        for (from, into) in [
            ("bin", "bin"),
            ("sbin", "bin"),
            ("lib", "lib"),
            ("share", "share"),
        ] {
            for entry in entry_names(&store_dir.join(id).join(from)) {
                merge_link(
                    &store_dir,
                    &Path::new(STORE).join(id).join(from).join(&entry),
                    &out.join("sw").join(into).join(&entry),
                )?;
            }
        }
    }

    if !firmware.is_empty() {
        if out.join("sw/lib/firmware").symlink_metadata().is_ok() {
            return Err(
                "firmware: use firmware-packages for outputs containing lib/firmware".to_owned(),
            );
        }
        firmware::install(&firmware, &store_dir, &out.join("firmware"))?;
        symlink("../../firmware", &out.join("sw/lib/firmware"))?;
    }

    if store_dir.join(&kernel.id).join("lib/modules").is_dir() {
        symlink(
            format!("{STORE}/{}/lib/modules", kernel.id),
            &out.join("kernel-modules"),
        )?;
        symlink("/run/vug-kernel-modules", &out.join("sw/lib/modules"))?;
    }

    if let Some(sh) = &record.sh {
        let target = pkg(sh, &paths)?;
        if !valid_image_path(&target) {
            return Err(format!("sh: {sh:?}: expected an absolute path"));
        }
        symlink(target, &out.join("sw/bin/sh"))?;
    }

    let mut links = String::new();
    let mut targets = String::new();
    let mut seeds = String::new();
    let mut files: Vec<&SystemFile> = record.parts.files.iter().collect();
    files.sort_by(|a, b| a.path.cmp(&b.path));
    for file in files {
        if let Some(target) = &file.target {
            let target = pkg(target, &paths)?;
            if !valid_image_path(&target) {
                return Err(format!(
                    "file.target: /etc/{}: {target:?}: expected an absolute path",
                    file.path
                ));
            }
            targets.push_str(&format!("{} {target}\n", file.path));
            continue;
        }
        let content = pkg(&file.content, &paths)?;
        match file.kind {
            FileKind::Link => {
                write_file(
                    &out.join("etc").join(&file.path),
                    content.as_bytes(),
                    file.mode,
                )?;
                links.push_str(&format!("{}\n", file.path));
            }
            FileKind::Seed => {
                write_file(
                    &out.join("seed").join(&file.path),
                    content.as_bytes(),
                    0o644,
                )?;
                seeds.push_str(&format!("{:o} {}\n", file.mode, file.path));
            }
        }
    }

    let gid_of = |group: &str| {
        record
            .parts
            .groups
            .iter()
            .find(|g| g.name == group)
            .map(|g| g.gid)
            .ok_or_else(|| format!("{group:?} is not a group of this system"))
    };
    let mut passwd = String::new();
    let mut shadow = String::new();
    let mut homes = String::new();
    for user in &record.parts.users {
        let gid = gid_of(&user.group)?;
        passwd.push_str(&format!(
            "{0}:x:{1}:{gid}:{0}:{2}:{3}\n",
            user.name, user.uid, user.home, user.shell
        ));
        shadow.push_str(&format!("{}:{}:1::::::\n", user.name, user.password));
        if user.home != "/" {
            homes.push_str(&format!("{} {gid} {}\n", user.uid, user.home));
        }
    }
    let mut group = String::new();
    for entry in &record.parts.groups {
        let members: Vec<&str> = record
            .parts
            .users
            .iter()
            .filter(|u| u.groups.contains(&entry.name))
            .map(|u| u.name.as_str())
            .collect();
        group.push_str(&format!(
            "{}:x:{}:{}\n",
            entry.name,
            entry.gid,
            members.join(",")
        ));
    }
    let mut setuid = String::new();
    for (package, program) in &record.parts.setuid {
        let name = Path::new(program)
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| format!("setuid.program: {program:?}"))?;
        setuid.push_str(&format!("{name} {}/{program}\n", paths[package]));
    }
    let mut ids = format!("{this}\n");
    for id in &closure {
        ids.push_str(&format!("{STORE}/{id}\n"));
    }
    for (file, text) in [
        ("etc-links", &links),
        ("etc-targets", &targets),
        ("etc-seeds", &seeds),
        ("passwd", &passwd),
        ("group", &group),
        ("shadow", &shadow),
        ("homes", &homes),
        ("setuid", &setuid),
        ("closure", &ids),
    ] {
        write_file(&out.join("manifest").join(file), text.as_bytes(), 0o644)?;
    }
    for (path, text) in &record.texts {
        write_file(&out.join(".vug").join(path), text.as_bytes(), 0o644)?;
    }

    let id = build::install_output(&store_dir, &out)?;
    remove_tree(&work)?;
    let kernel_image = store_dir.join(&kernel.id).join("boot/bzImage");
    closure.insert(id.clone());
    Ok(BuiltSystem {
        name: name.to_owned(),
        id,
        kernel: kernel_image,
        closure,
        efi: record.boot.is_some(),
        disk: record.disk,
    })
}

fn copy_store(settings: &Settings, ids: &BTreeSet<String>, root: &Path) -> Result<(), String> {
    let store = root.join("vug/store");
    io("create", &store, fs::create_dir_all(&store))?;
    for id in ids {
        copy_tree(&settings.data_dir.join("store").join(id), &store.join(id))?;
    }
    Ok(())
}

/// A size as `mke2fs` takes it: a number of bytes with a k, m, g, or t suffix.
fn size_in_bytes(size: &str) -> Result<u64, String> {
    let error = || format!("{size:?}: expected a number with a k, m, g, or t suffix");
    let (digits, shift) = match size.chars().last().map(|c| c.to_ascii_lowercase()) {
        Some('k') => (&size[..size.len() - 1], 10),
        Some('m') => (&size[..size.len() - 1], 20),
        Some('g') => (&size[..size.len() - 1], 30),
        Some('t') => (&size[..size.len() - 1], 40),
        _ => return Err(error()),
    };
    digits
        .parse::<u64>()
        .ok()
        .and_then(|n| n.checked_mul(1 << shift))
        .filter(|n| *n > 0)
        .ok_or_else(error)
}

fn run_tool(settings: &Settings, words: &[String], input: Option<&str>) -> Result<(), String> {
    use std::io::Write as _;
    use std::os::fd::AsFd as _;

    let mut command = build::store_command(settings, true)?;
    let diagnostic = std::io::stderr()
        .as_fd()
        .try_clone_to_owned()
        .map_err(|error| format!("duplicate stderr: {error}"))?;
    command.args(words).stdout(diagnostic);
    if input.is_some() {
        command.stdin(std::process::Stdio::piped());
    }
    let mut child = command.spawn().map_err(|e| format!("bwrap: {e}"))?;
    if let (Some(text), Some(mut stdin)) = (input, child.stdin.take()) {
        stdin
            .write_all(text.as_bytes())
            .map_err(|e| format!("{}: {e}", words[0]))?;
    }
    let status = child.wait().map_err(|e| format!("{}: {e}", words[0]))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("{}: {status}", words[0]))
    }
}

/// Writes the partition table and the EFI system partition of a disk image
/// whose root partition takes `root_bytes`.
fn write_efi_parts(
    settings: &Settings,
    system: &BuiltSystem,
    work: &Path,
    image: &Path,
    root_bytes: u64,
    builder: &mut Builder<'_>,
) -> Result<(), String> {
    let layout = system.disk.as_ref();
    let tool = |builder: &mut Builder, package: &str, program: &str| {
        builder
            .build(package, None)
            .map(|built| format!("{STORE}/{}/{program}", built.id))
    };
    let sfdisk = tool(builder, "util-linux", "sbin/sfdisk")?;
    let mkfs = tool(builder, "dosfstools", "sbin/mkfs.fat")?;
    let mcopy = tool(builder, "mtools", "bin/mcopy")?;
    let image_name = image.display().to_string();
    let new_uuid = || {
        fs::read_to_string("/proc/sys/kernel/random/uuid")
            .map(|uuid| uuid.trim().to_owned())
            .map_err(|error| format!("generate PARTUUID: {error}"))
    };
    let (esp_bytes, esp_name, root_name, disk_uuid, esp_uuid, root_uuid) = match layout {
        Some(layout) => (
            layout.esp_bytes,
            layout.esp_name.as_str(),
            layout.root_name.as_str(),
            new_uuid()?,
            new_uuid()?,
            new_uuid()?,
        ),
        None => (
            ESP_BYTES,
            ESP_NAME,
            ROOT_NAME,
            IMAGE_UUID.into(),
            ESP_UUID.into(),
            ROOT_UUID.into(),
        ),
    };
    if let Some(layout) = layout {
        let root = work.join("root");
        write_file(
            &root.join("vug/disk/layout"),
            layout.partition_spec().as_bytes(),
            0o644,
        )?;
        write_file(
            &root.join("vug/disk/root-partuuid"),
            root_uuid.as_bytes(),
            0o644,
        )?;
        write_file(
            &root.join("vug/disk/esp-partuuid"),
            esp_uuid.as_bytes(),
            0o644,
        )?;
        write_file(
            &root.join("etc/fstab"),
            format!(
                "PARTUUID={root_uuid} / ext4 defaults 0 0\nPARTUUID={esp_uuid} /boot vfat {} 0 0\n",
                layout.esp_options
            )
            .as_bytes(),
            0o644,
        )?;
    }
    let table = format!(
        "label: gpt\nlabel-id: {disk_uuid}\nfirst-lba: 2048\n\
         start=2048, size={}, type=U, uuid={esp_uuid}, name={esp_name}\n\
         start={}, size={}, type=L, uuid={root_uuid}, name={root_name}\n",
        esp_bytes / 512,
        (MIB + esp_bytes) / 512,
        root_bytes / 512,
    );
    run_tool(
        settings,
        &[
            sfdisk,
            "--quiet".to_owned(),
            "--no-reread".to_owned(),
            "--no-tell-kernel".to_owned(),
            image_name.clone(),
        ],
        Some(&table),
    )?;
    run_tool(
        settings,
        &[
            mkfs,
            "-F".to_owned(),
            "32".to_owned(),
            "-s".to_owned(),
            if layout.is_some() { "1" } else { "2" }.to_owned(),
            "-n".to_owned(),
            "VUGESP".to_owned(),
            "-i".to_owned(),
            "76756700".to_owned(),
            "--invariant".to_owned(),
            "--offset".to_owned(),
            (MIB / 512).to_string(),
            image_name.clone(),
            (esp_bytes / 1024).to_string(),
        ],
        None,
    )?;

    let store = settings.data_dir.join("store").join(&system.id);
    let esp = work.join("esp");
    let loader = esp.join("EFI/BOOT/BOOTX64.EFI");
    let resolve = |link: &Path| -> Result<PathBuf, String> {
        let target = io("readlink", link, fs::read_link(link))?;
        target
            .strip_prefix(STORE)
            .map(|inside| settings.data_dir.join("store").join(inside))
            .map_err(|_| format!("{}: not a link into the store", link.display()))
    };
    let from = resolve(&store.join("boot/loader"))?;
    write_file(&loader, &io("read", &from, fs::read(&from))?, 0o644)?;
    let kernel = resolve(&store.join("kernel"))?;
    let kernel_id = kernel
        .parent()
        .and_then(Path::parent)
        .and_then(Path::file_name)
        .and_then(|name| name.to_str())
        .ok_or("kernel: not a store path")?;
    write_file(
        &esp.join("vug").join(format!("{kernel_id}.bzImage")),
        &io("read", &kernel, fs::read(&kernel))?,
        0o644,
    )?;
    let initrd = store.join("boot/initrd.gz");
    if initrd.is_file() {
        let name = store.join("boot/initrd-name");
        let name = io("read", &name, fs::read_to_string(&name))?;
        let directory = esp.join("vug-initrd");
        io("create", &directory, fs::create_dir_all(&directory))?;
        let destination = directory.join(name);
        io("copy", &destination, fs::copy(&initrd, &destination))?;
    }
    let entry = store.join("boot/entry");
    write_file(
        &esp.join("limine.conf"),
        format!(
            "timeout: 3\ndefault_entry: 1\n/vug generation 1\n{}",
            io("read", &entry, fs::read_to_string(&entry))?.replace("@VUG_ROOT@", &root_uuid)
        )
        .as_bytes(),
        0o644,
    )?;
    let mut words = vec![
        mcopy,
        "-i".to_owned(),
        format!("{image_name}@@{MIB}"),
        "-s".to_owned(),
    ];
    for entry in entry_names(&esp) {
        words.push(esp.join(entry).display().to_string());
    }
    words.push("::/".to_owned());
    run_tool(settings, &words, None)
}

/// Writes an ext4 image whose root holds the system as generation 1. For a
/// system with boot entries the image is a GPT disk with an EFI system
/// partition and the root partition.
pub fn build_image(
    settings: &Settings,
    name: &str,
    image: &Path,
    size: &str,
) -> Result<BuiltSystem, String> {
    let record = load(&settings.root, name)?;
    image_with(
        settings,
        name,
        record,
        image,
        size,
        &mut Builder::new(settings)?,
    )
}

fn image_with(
    settings: &Settings,
    name: &str,
    record: SystemRecord,
    image: &Path,
    size: &str,
    builder: &mut Builder<'_>,
) -> Result<BuiltSystem, String> {
    let system = build_with(settings, name, record, builder)?;
    let tools = builder.build("e2fsprogs", None)?;
    let work = work_dir(settings, "image")?;
    let root = work.join("root");
    copy_store(settings, &system.closure, &root)?;
    for dir in [
        "dev", "etc", "home", "proc", "root", "run", "sys", "tmp", "var",
    ] {
        io("create", &root, fs::create_dir_all(root.join(dir)))?;
    }
    symlink(
        format!("{STORE}/{}", system.id),
        &root.join("vug/profiles/system-1"),
    )?;
    symlink("system-1", &root.join("vug/profiles/system"))?;
    for name in ["index", "index-outputs"] {
        let index = settings.data_dir.join(name);
        if index.exists() {
            copy_tree(&index, &root.join("vug").join(name))?;
        }
    }
    let tree = root.join("vug/tree");
    io("create", &tree, fs::create_dir_all(&tree))?;
    for part in ["seed.toml", "packages", "services", "systems"] {
        if let Ok(from) = fs::canonicalize(settings.root.join(part)) {
            copy_tree(&from, &tree.join(part))?;
        }
    }
    if image.symlink_metadata().is_ok() {
        io("remove", image, fs::remove_file(image))?;
    }
    let mut options = format!("root_owner=0:0,hash_seed={IMAGE_UUID}");
    if system.efi {
        io("create", &root, fs::create_dir_all(root.join("boot")))?;
        let layout = system.disk.as_ref();
        let esp_bytes = layout.map_or(ESP_BYTES, |layout| layout.esp_bytes);
        let root_bytes = size_in_bytes(size)?;
        let root_offset = MIB + esp_bytes;
        let image_bytes = root_offset
            .checked_add(root_bytes)
            .and_then(|size| size.checked_add(MIB))
            .ok_or("disk image size overflows")?;
        let disk = io("create", image, fs::File::create(image))?;
        io("resize", image, disk.set_len(image_bytes))?;
        drop(disk);
        write_efi_parts(settings, &system, &work, image, root_bytes, builder)?;
        options.push_str(&format!(",offset={root_offset}"));
    }
    let status = build::store_command(settings, true)?
        .env("E2FSPROGS_FAKE_TIME", "315532800")
        .arg(format!("{STORE}/{}/sbin/mke2fs", tools.id))
        .args(["-q", "-F", "-t", "ext4", "-L", "vug"])
        .args(["-U", IMAGE_UUID])
        .args(["-E", &options])
        .arg("-d")
        .arg(&root)
        .arg(image)
        .arg(size)
        .status()
        .map_err(|e| format!("bwrap: {e}"))?;
    if !status.success() {
        return Err(format!("mke2fs: {status}"));
    }
    remove_tree(&work)?;
    Ok(system)
}

/// Writes a cpio archive of the store paths of the system that the system
/// `base` (an id or a store path, present in this store) does not hold.
/// Returns the system and the number of store paths written.
pub fn export(
    settings: &Settings,
    name: &str,
    archive: &Path,
    base: Option<&str>,
) -> Result<(BuiltSystem, usize), String> {
    let system = build(settings, name)?;
    let mut ids = system.closure.clone();
    if let Some(base) = base {
        let id = base.strip_prefix(&format!("{STORE}/")).unwrap_or(base);
        let list = settings
            .data_dir
            .join("store")
            .join(id)
            .join("manifest/closure");
        for line in io("read", &list, fs::read_to_string(&list))?.lines() {
            ids.remove(line.strip_prefix(&format!("{STORE}/")).unwrap_or(line));
        }
    }
    let work = work_dir(settings, "export")?;
    copy_store(settings, &ids, &work)?;
    let mut out = std::io::BufWriter::new(io("create", archive, fs::File::create(archive))?);
    io("write", archive, cpio::write_newc(&work, &mut out))?;
    drop(out);
    remove_tree(&work)?;
    Ok((system, ids.len()))
}

/// Builds the system and replaces this process by its `switch-to-system`.
/// Returns only when that fails, with the reason.
pub fn switch(settings: &Settings, name: &str) -> String {
    use std::os::unix::process::CommandExt;

    let system = match build(settings, name) {
        Ok(system) => system,
        Err(error) => return error,
    };
    let program = settings
        .data_dir
        .join("store")
        .join(&system.id)
        .join("bin/switch-to-system");
    format!(
        "{}: {}",
        program.display(),
        std::process::Command::new(&program).exec()
    )
}
