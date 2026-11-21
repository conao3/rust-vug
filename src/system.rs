//! Building a system from a system record (`docs/system.md`).

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use vug::cpio;

use crate::build::{self, Builder, Settings};
use crate::naming::model;
use vug::steps::shell_quote;

const STORE: &str = "/vug/store";
const IMAGE_UUID: &str = "76756700-0000-4000-8000-000000000001";

const INIT: &str = r#"#!{self}/bin/busybox sh
b={self}/bin/busybox
$b mountpoint -q /proc || $b mount -t proc proc /proc
$b mount -o remount,rw /
for d in dev etc home root run sys tmp var; do
  if [ ! -d /$d ]; then $b mkdir /$d; fi
done
$b chmod 1777 /tmp
$b mountpoint -q /dev || $b mount -t devtmpfs devtmpfs /dev
$b mountpoint -q /run || $b mount -t tmpfs -o mode=755,nosuid,nodev tmpfs /run
{self}/activate boot
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
$self/activate check
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

#[derive(Clone, Copy, PartialEq)]
enum FileKind {
    Link,
    Seed,
}

pub struct SystemFile {
    /// Path below `/etc`, without a leading slash.
    path: String,
    kind: FileKind,
    mode: u32,
    content: String,
}

struct Group {
    name: String,
    gid: u32,
}

struct User {
    name: String,
    uid: u32,
    group: String,
    groups: Vec<String>,
    home: String,
    shell: String,
    password: String,
}

pub struct SystemRecord {
    text: String,
    kernel: String,
    init: String,
    sh: Option<String>,
    packages: Vec<String>,
    files: Vec<SystemFile>,
    groups: Vec<Group>,
    users: Vec<User>,
    /// (package, program path inside the package)
    setuid: Vec<(String, String)>,
}

pub struct BuiltSystem {
    pub name: String,
    pub id: String,
    /// The kernel image on the host.
    pub kernel: PathBuf,
    /// Ids of the system, its kernel, its packages, and what they need at
    /// run time.
    pub closure: BTreeSet<String>,
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

pub fn load(path: &Path, expected_name: &str) -> Result<SystemRecord, String> {
    let text = io("read", path, fs::read_to_string(path))?;
    let table = text
        .parse::<toml::Table>()
        .map_err(|e| format!("{}: {e}", path.display()))?;
    unknown_keys(
        &table,
        &[
            "name", "kernel", "init", "sh", "packages", "file", "group", "user", "setuid",
        ],
        "",
    )?;
    let name = string(&table, "name", "")?;
    if name != expected_name {
        return Err(format!("name: expected {expected_name:?}"));
    }
    let kernel = string(&table, "kernel", "")?;
    let init = string(&table, "init", "")?;
    if init.split_whitespace().next().is_none() {
        return Err("init: expected a command".to_owned());
    }
    let sh = match table.get("sh") {
        None => None,
        Some(_) => Some(string(&table, "sh", "")?),
    };
    let packages = strings(&table, "packages", "")?;
    for package in packages.iter().chain([&kernel]) {
        if !model::valid_name(package) {
            return Err(format!("{package:?}: not a valid package name"));
        }
    }

    let mut groups = Vec::new();
    for entry in tables(&table, "group")? {
        unknown_keys(entry, &["name", "gid"], "group.")?;
        let name = string(entry, "name", "group.")?;
        if !valid_account(&name) {
            return Err(format!("group.name: {name:?}: not a valid group name"));
        }
        let gid = number(entry, "gid", "group.")?;
        if groups
            .iter()
            .any(|g: &Group| g.name == name || g.gid == gid)
        {
            return Err(format!("group.name: {name:?}: name or gid listed twice"));
        }
        groups.push(Group { name, gid });
    }
    let mut users = Vec::new();
    for entry in tables(&table, "user")? {
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
        let uid = number(entry, "uid", "user.")?;
        if users.iter().any(|u: &User| u.name == name || u.uid == uid) {
            return Err(format!("user.name: {name:?}: name or uid listed twice"));
        }
        let group = string(entry, "group", "user.")?;
        let member_of = match entry.get("groups") {
            None => Vec::new(),
            Some(_) => strings(entry, "groups", "user.")?,
        };
        for group in member_of.iter().chain([&group]) {
            if !groups.iter().any(|g| &g.name == group) {
                return Err(format!(
                    "user.{name}: {group:?} is not a group of this system"
                ));
            }
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
            name,
            uid,
            group,
            groups: member_of,
            home,
            shell,
            password,
        });
    }

    let mut files: Vec<SystemFile> = Vec::new();
    for entry in tables(&table, "file")? {
        unknown_keys(entry, &["path", "kind", "mode", "content"], "file.")?;
        let path = string(entry, "path", "file.")?;
        let Some(relative) = path
            .strip_prefix("/etc/")
            .filter(|_| valid_image_path(&path))
        else {
            return Err(format!("file.path: {path:?}: expected a path below /etc"));
        };
        let reserved = relative == ".vug-links"
            || (!users.is_empty() && matches!(relative, "passwd" | "group" | "shadow"));
        if reserved {
            return Err(format!("file.path: {path:?}: written by vug itself"));
        }
        for other in &files {
            let nested = |a: &str, b: &str| a == b || a.starts_with(&format!("{b}/"));
            if nested(relative, &other.path) || nested(&other.path, relative) {
                return Err(format!(
                    "file.path: {path:?}: listed twice, or inside another listed path"
                ));
            }
        }
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
        files.push(SystemFile {
            path: relative.to_owned(),
            kind,
            mode,
            content: string(entry, "content", "file.")?,
        });
    }

    let mut setuid = Vec::new();
    for entry in tables(&table, "setuid")? {
        unknown_keys(entry, &["package", "program"], "setuid.")?;
        let package = string(entry, "package", "setuid.")?;
        let program = string(entry, "program", "setuid.")?;
        if !packages.contains(&package) {
            return Err(format!(
                "setuid.package: {package:?} is not a package of this system"
            ));
        }
        if !valid_image_path(&format!("/{program}")) {
            return Err(format!(
                "setuid.program: {program:?}: expected a path relative to the package"
            ));
        }
        setuid.push((package, program));
    }
    Ok(SystemRecord {
        text,
        kernel,
        init,
        sh,
        packages,
        files,
        groups,
        users,
        setuid,
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

/// Replaces `{pkg:NAME}` by the store path of the package NAME of the system.
fn expand(text: &str, paths: &BTreeMap<String, String>) -> Result<String, String> {
    let mut out = String::new();
    let mut rest = text;
    while let Some(start) = rest.find("{pkg:") {
        let (before, tail) = rest.split_at(start);
        let Some(end) = tail.find('}') else {
            break;
        };
        let name = &tail["{pkg:".len()..end];
        let path = paths
            .get(name)
            .ok_or_else(|| format!("{{pkg:{name}}}: {name:?} is not a package of this system"))?;
        out.push_str(before);
        out.push_str(path);
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
    if !model::valid_name(name) {
        return Err(format!("{name:?}: not a valid system name"));
    }
    let record = load(
        &settings.root.join("systems").join(format!("{name}.toml")),
        name,
    )?;
    let mut builder = Builder::new(settings)?;
    let kernel = builder.build(&record.kernel, None)?;
    let mut closure: BTreeSet<String> = kernel.closure().clone();
    let mut paths = BTreeMap::new();
    let mut roots = Vec::new();
    for package in &record.packages {
        let built = builder.build(package, None)?;
        closure.extend(built.closure().iter().cloned());
        paths.insert(package.clone(), format!("{STORE}/{}", built.id));
        roots.push(built.id);
    }

    let store_dir = settings.data_dir.join("store");
    let this = build::placeholder_path();
    let work = work_dir(settings, "system")?;
    let out = work.join("out");

    let busybox = builder.seed_dir().join("bin/busybox");
    write_file(
        &out.join("bin/busybox"),
        &io("read", &busybox, fs::read(&busybox))?,
        0o755,
    )?;
    let init = expand(&record.init, &paths)?
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
                symlink(
                    Path::new(STORE).join(id).join(from).join(&entry),
                    &out.join("sw").join(into).join(&entry),
                )?;
            }
        }
    }

    if let Some(sh) = &record.sh {
        let target = expand(sh, &paths)?;
        if !valid_image_path(&target) {
            return Err(format!("sh: {sh:?}: expected an absolute path"));
        }
        symlink(target, &out.join("sw/bin/sh"))?;
    }

    let mut links = String::new();
    let mut seeds = String::new();
    let mut files: Vec<&SystemFile> = record.files.iter().collect();
    files.sort_by(|a, b| a.path.cmp(&b.path));
    for file in files {
        let content = expand(&file.content, &paths)?;
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
            .groups
            .iter()
            .find(|g| g.name == group)
            .map(|g| g.gid)
            .ok_or_else(|| format!("{group:?} is not a group of this system"))
    };
    let mut passwd = String::new();
    let mut shadow = String::new();
    let mut homes = String::new();
    for user in &record.users {
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
    for entry in &record.groups {
        let members: Vec<&str> = record
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
    for (package, program) in &record.setuid {
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
    write_file(&out.join(".vug/system.toml"), record.text.as_bytes(), 0o644)?;

    let id = build::install_output(&store_dir, &out)?;
    remove_tree(&work)?;
    let kernel_image = store_dir.join(&kernel.id).join("boot/bzImage");
    closure.insert(id.clone());
    Ok(BuiltSystem {
        name: name.to_owned(),
        id,
        kernel: kernel_image,
        closure,
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

/// Writes an ext4 image whose root holds the system as generation 1.
pub fn build_image(
    settings: &Settings,
    name: &str,
    image: &Path,
    size: &str,
) -> Result<BuiltSystem, String> {
    let system = build(settings, name)?;
    let tools = Builder::new(settings)?.build("e2fsprogs", None)?;
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
    if image.symlink_metadata().is_ok() {
        io("remove", image, fs::remove_file(image))?;
    }
    let status = build::store_command(settings, true)?
        .env("E2FSPROGS_FAKE_TIME", "315532800")
        .arg(format!("{STORE}/{}/sbin/mke2fs", tools.id))
        .args(["-q", "-t", "ext4", "-L", "vug"])
        .args(["-U", IMAGE_UUID])
        .args(["-E", &format!("root_owner=0:0,hash_seed={IMAGE_UUID}")])
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
