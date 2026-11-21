# Systems

A system record names a kernel, packages, files below `/etc`, users, the
program to start, and the service records to add. `vug system build-system` turns it into one store path, a
*system*. A disk holds a store, any number of systems, and a numbered link
per system that was switched to (a *generation*).

## Record

`systems/<name>.toml` at the root of the package tree:

```toml
name = "demo"
kernel = "linux"
init = "{pkg:systemd}/lib/systemd/systemd"
sh = "{pkg:bash}/bin/bash"
packages = [
  "bash",
  "coreutils",
  "sudo",
  "systemd",
]

[[file]]
path = "/etc/sudoers"
content = """
%wheel ALL=(ALL:ALL) NOPASSWD: ALL
"""

[[file]]
path = "/etc/hostname"
kind = "seed"
content = "demo\n"

[[group]]
name = "root"
gid = 0

[[group]]
name = "wheel"
gid = 1

[[user]]
name = "root"
uid = 0
group = "root"
groups = ["wheel"]
home = "/root"
shell = "/bin/bash"

[[setuid]]
package = "sudo"
program = "bin/sudo"

[service.serial-autologin]
user = "root"
```

| Key | Type | Required | Meaning |
|---|---|---|---|
| `name` | string | yes | Must equal the file name without `.toml`. |
| `kernel` | string | yes | A package whose output holds `boot/bzImage` (build kind `linux`). |
| `init` | string | yes | The command the system starts as process 1, split at white space. |
| `sh` | string | no | The program `/bin/sh` runs. Without it `/bin/sh` exists only when a listed package has a program `sh`. |
| `packages` | array of strings | yes | Packages whose programs appear in `/bin`. |
| `file.path` | string | yes | A path below `/etc`. No listed path equals or lies inside another. |
| `file.kind` | `"link"` or `"seed"` | no (default `"link"`) | How the file is managed, see below. |
| `file.mode` | string of octal digits | no (default `"644"`) | Permission bits. A link file takes `"644"` or `"755"`. |
| `file.content` | string | yes, unless `file.target` | Contents. |
| `file.target` | string | no | For a link file: the absolute path the file is a link to, in place of content (`/etc/localtime` to a zone file, for example). |
| `group.name`, `group.gid` | string, integer | per group | A group. Names and ids are unique. |
| `user.name`, `user.uid` | string, integer | per user | A user. Names and ids are unique. |
| `user.group` | string | per user | Primary group, one of the groups of the record. |
| `user.groups` | array of strings | no | Further groups of the record. |
| `user.home`, `user.shell` | string | per user | Home directory (`"/"` for none) and login shell. |
| `user.password` | string | no (default `"!"`, no login by password) | The password field of the first `shadow` entry of the user. |
| `setuid.package`, `setuid.program` | string | per entry | A program of a package of the system that runs as root. |
| `service.<name>` | table of strings | no | Adds the service record `<name>`, with these values for its parameters. |

`user.password` goes into a store path that every user of the machine can
read, and a package tree is usually public: it is the place for `""` (no
password) or `"!"`, not for the hash of a real password. A password set on
the machine with `passwd` lives only in `/etc/shadow`.

`{pkg:NAME}` in `init`, `sh`, `file.content`, and `file.target` is replaced by the store path of
the package `NAME` of the system. Unknown keys are errors.

## Service records

`services/<name>.toml` holds a part that several systems share:

```toml
name = "serial-autologin"
packages = [
  "util-linux",
]

[parameter.user]

[parameter.tty]
default = "ttyS0"

[[file]]
path = "/etc/systemd/system/serial-getty@{param:tty}.service.d/autologin.conf"
content = """
[Service]
ExecStart=
ExecStart=-/bin/agetty --autologin {param:user} - vt220
"""
```

| Key | Type | Required | Meaning |
|---|---|---|---|
| `name` | string | yes | Must equal the file name without `.toml`. |
| `parameter.<name>` | table | no | A parameter. `default` (a string) is its value when the system gives none; without `default` the system must give one. |
| `packages`, `file`, `group`, `user`, `setuid` | as in a system record | no | What the service adds. |

`{param:NAME}` in `file.path`, `file.content`, and `file.target` is replaced by the value of
the parameter. `{pkg:NAME}` names a package of the merged system.

A system is the union of its own record and its service records, taken in
the order of the service names. Packages keep the order of first mention.
A file path, a group, or a user that two records list must be the same in
both; anything else is an error. A record cannot replace what another one
says.

## The system

`/vug/store/<id>` of a system holds:

| Path | Contents |
|---|---|
| `init` | What the kernel starts: it mounts `/proc`, `/dev`, and `/run`, runs `activate`, and replaces itself by the `init` command of the record. |
| `activate` | Makes this system the running one. |
| `bin/switch-to-system` | Switches a running system to this one. |
| `bin/busybox` | The busybox of the seed. The three scripts above use nothing else. |
| `kernel` | A link to the kernel image. |
| `sw/bin`, `sw/lib`, `sw/share` | One link per entry of the `bin` and `sbin`, `lib`, and `share` directories of the listed packages. A directory that two packages have is merged entry by entry, at every depth; for anything else of the same name, the package listed later wins. `sw/sbin` is a link to `bin`, and `sw/bin/sh` a link to the `sh` program of the record. |
| `etc/` | The link files. |
| `seed/` | The seed files. |
| `manifest/` | The paths and kinds of the files, the modes of the seed files, `passwd`, `group`, `shadow`, the home directories, the setuid programs, and `closure`: the store paths of the system, its kernel, its packages, and everything they need at run time. |
| `.vug/system.toml`, `.vug/services/<name>.toml` | Copies of the system record and of its service records. |

Owners and permission bits other than the execute bit are not part of a store
path (`docs/build.md`), so every mode that matters is written in `manifest/`.

## The root file system

The root is an ordinary writable file system:

| Path | Contents |
|---|---|
| `/vug/store` | Store paths. |
| `/vug/profiles/system-<N>` | A link to the system of generation `N`. |
| `/vug/profiles/system` | A link to the default generation (`system-<N>`). |
| `/run/current-system` | A link to the running system, made by `activate`. |
| `/usr` | A link to `/run/current-system/sw`. `/bin` and `/sbin` link to `usr/bin`, `/lib` to `usr/lib`. |
| `/run/wrappers/bin` | Copies of the setuid programs, owned by root with mode 4755, on a file system mounted without `nosuid`. A login `PATH` names it before `/bin`. |
| `/etc` | A writable directory. vug manages only the listed paths. |

The kernel is started with `root=<device> rw init=/vug/profiles/system/init`.
`init=/vug/profiles/system-<N>/init` starts generation `N` instead.

## Files below `/etc`

| Kind | What `activate` puts at the path | On a later activation |
|---|---|---|
| `link` | A link to `/run/current-system/etc/<path>`. | The content changes with `/run/current-system`. A path the new system does not list is removed; a path it adds is linked. |
| `seed` | A regular file with the content and mode of the record, when nothing is at the path. | Left alone. |
| users | `/etc/passwd`, `/etc/group`, and `/etc/shadow` as regular files, when the record lists a user. | `passwd` and `group` are written again from the record. A user that already has an entry in `shadow` keeps it. A missing home directory is created. |

`activate` first looks at every link path. Where something other than its
own link is at the path, `switch-to-system` prints the path and stops before
it changes anything; at boot the path is reported and left as it is.
Paths the record does not list are never touched.

## Switching

`<system>/bin/switch-to-system`, run as root on the running machine:

1. Checks the link paths (above).
2. Finds the generation that points to this system, or adds the next number,
   and points `/vug/profiles/system` to it.
3. Runs `activate`: `/run/current-system` is replaced in one rename, then
   the links below `/etc`, the seed files, the users, and the setuid copies
   follow.
4. With systemd running: stops the `.service`, `.socket`, `.timer`, and
   `.path` units the new system no longer has, reloads systemd, restarts the
   active ones whose unit file or drop-ins changed, and starts the active
   targets again so that units they newly want come up.

Going back is switching to the earlier generation:
`/vug/profiles/system-<N>/bin/switch-to-system`.

## Commands

```console
$ vug system build-system --root . --name demo
{
  "Kernel": "/home/user/.local/share/vug/store/bhnpszoabsqfxuuygzh7q43iux5u7fxq/boot/bzImage",
  "Name": "demo",
  "Path": "/vug/store/457dsludg37onlyikmipl5lfoey3wzuu",
  "StorePaths": 18
}
$ vug system build-image --root . --name demo --image demo.img --size 2g
$ qemu-system-x86_64 -enable-kvm -m 1024 -nographic \
    -kernel ~/.local/share/vug/store/bhnpszoabsqfxuuygzh7q43iux5u7fxq/boot/bzImage \
    -drive file=demo.img,if=virtio,format=raw \
    -append "console=ttyS0 root=/dev/vda rw init=/vug/profiles/system/init"
```

| Command | Result |
|---|---|
| `build-system` | The system in the store. |
| `build-image --image FILE [--size SIZE]` | Also an ext4 image (default size `4g`) whose root holds the store paths of the system, generation 1 pointing to it, the index of the data directory at `/vug/index`, the records (`seed.toml`, `packages`, `services`, `systems`) at `/vug/tree`, and empty directories. The image is made by `mke2fs` of the package `e2fsprogs`; every file belongs to root. The kernel needs the drivers of the disk and ext4 built in, since there is no initramfs. |
| `switch-system` | The system in the store, then its `bin/switch-to-system` runs. For a machine that runs a vug system. |
| `export-system --archive FILE [--base SYSTEM]` | Also a cpio archive ("newc") of the store paths of the system, without those of the system `SYSTEM` (an id or a store path in the same store). |

All four take the parameters of `vug package build-package` and build what
is missing. `StorePaths` is the number of store paths of the system.

On a machine that runs a vug system, the data directory defaults to `/vug`
and the cache directory to `/var/cache/vug`. With the package `vug` in the
system, a change to the records takes effect with

```console
# vug system switch-system --root /vug/tree --name demo --binary-cache http://cache.example
```

which builds only the system itself when the index knows every package, and
takes from the binary cache (`docs/cache.md`) what the store lacks. The
busybox of the new system is the one of the running system when the seed
names the same file, so a change that adds no package needs no network.

Without `vug` on the machine: export the new system with the running
system as the base, unpack the archive at `/` as root
(`/run/current-system/bin/busybox cpio -id`), and run its
`bin/switch-to-system`.
