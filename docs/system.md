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
| `kernel` | string | yes | The package output that holds `boot/bzImage` (build kind `linux`). |
| `init` | string | yes | The command the system starts as process 1, split at white space. |
| `sh` | string | no | The program `/bin/sh` runs. Without it `/bin/sh` exists only when a listed package has a program `sh`. |
| `packages` | array of strings | yes | Package outputs to merge into the system. |
| `firmware-packages` | array of strings | no | Package outputs that contribute to `/lib/firmware`. |
| `file.path` | string | yes | A path below `/etc`. No listed path equals or lies inside another. |
| `file.kind` | `"link"` or `"seed"` | no (default `"link"`) | How the file is managed, see below. |
| `file.mode` | string of octal digits | no (default `"644"`) | Permission bits. A link file takes `"644"` or `"755"`. |
| `file.content` | string | yes, unless `file.target` | Contents. |
| `file.target` | string | no | For a link file: the absolute path the file is a direct link to, in place of content (`/etc/localtime` to `/usr/share/zoneinfo/<zone>`, whose name programs read from the link). |
| `group.name`, `group.gid` | string, integer | per group | A group. Names and ids are unique. |
| `user.name`, `user.uid` | string, integer | per user | A user. Names and ids are unique. |
| `user.group` | string | per user | Primary group, one of the groups of the record. |
| `user.groups` | array of strings | no | Further groups of the record. |
| `user.home`, `user.shell` | string | per user | Home directory (`"/"` for none) and login shell. |
| `user.password` | string | no (default `"!"`, no login by password) | The password field of the first `shadow` entry of the user. |
| `setuid.package`, `setuid.program` | string | per entry | A selected package output and the path within it to a program that runs as root. |
| `service.<name>` | table of strings | no | Adds the service record `<name>`, with these values for its parameters. |
| `boot.loader` | string | with `[boot]` | The EFI program of a limine boot loader, usually `{pkg:limine}/share/limine/BOOTX64.EFI`. |
| `boot.root` | string | no | Root selector. The default is `PARTLABEL=vug-root` for direct boot and the image root partition’s GPT PARTUUID with initrd. An explicit initrd selector must be `PARTUUID=<uuid>`. |
| `boot.arguments` | array of strings | no | Further kernel arguments. |
| `boot.initrd` | table | no | Build a plain ext4 root initrd for the selected kernel. |
| `boot.initrd.available-modules` | array of strings | no | Include these modules and their dependencies for udev coldplug. |
| `boot.initrd.force-modules` | array of strings | no | Include these modules and their dependencies, then load the listed modules in order before coldplug. |
| `boot.initrd.firmware-files` | array of strings | no | Include these firmware paths and their link targets in the initrd. |

`user.password` goes into a store path that every user of the machine can
read, and a package tree is usually public: it is the place for `""` (no
password) or `"!"`, not for the hash of a real password. A password set on
the machine with `passwd` lives only in `/etc/shadow`.

`kernel`, `packages`, and `setuid.package` accept selectors of the form `NAME:OUTPUT`.
A bare `NAME` selects `NAME:out`. A selector in `setuid.package` must appear in the merged package list.

`{pkg:NAME}` and `{pkg:NAME:OUTPUT}` in `init`, `sh`, `boot.loader`, `file.content`, and `file.target`
expand to the selected store path. A reference must select an output in the merged package list.

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
| `bin/busybox` | The seed’s BusyBox used by the boot, activation, and switch scripts. |
| `kernel` | A link to the kernel image. |
| `boot/entry`, `boot/loader`, `bin/install-boot` | With `[boot]`: the boot entry of this system (kernel, `root=`, `init=` of this system, and the arguments), a link to the loader, and the script that writes the boot entries. |
| `boot/initrd.gz` | With `[boot.initrd]`: a reproducible gzip-compressed newc archive. |
| `sw/bin`, `sw/lib`, `sw/share` | Links to the entries in `bin`, `sbin`, `lib`, and `share` of the outputs selected in `packages`. Matching directories merge recursively. Other name conflicts go to the later entry. `sw/sbin` links to `bin`, and `sw/bin/sh` links to the `sh` program of the record. |
| `etc/` | The link files. |
| `seed/` | The seed files. |
| `manifest/` | The managed files, seed modes, users, groups, home directories, and setuid programs. `closure` lists the system's own store path and the deduplicated store paths of the selected kernel and package outputs and their transitive runtime dependencies. |
| `manifest/nix-closure.json` | When the generation uses Nix outputs, the deduplicated Nix store paths, signed NAR hashes and sizes, and references. |
| `.vug/system.toml`, `.vug/services/<name>.toml` | Copies of the system record and of its service records. |

Owners and permission bits other than the execute bit are not part of a store
path (`docs/build.md`), so every mode that matters is written in `manifest/`.

When the kernel output contains `lib/modules`, the system exposes its
module tree at `/lib/modules`. Boot selects the module tree together with
the kernel. Switching between systems with modular kernels keeps the running
kernel's module tree available until the next boot.

### Boot with an initrd

The archive contains the selected kernel modules and their depmod indexes, the
seed BusyBox, kmod, systemd-udevd and its driver rule, and blkid with the loader
and libraries they use. Kernel `root=` and the generation's `init=` are passed
through the Limine entry. Set additional arguments in `boot.arguments`, which
reserves `root=`, `init=`, and `rdinit=` in this mode.

The first init mounts proc, sysfs, devtmpfs, and a temporary runtime directory.
It starts udev, loads the forced modules and triggers coldplug. It allows 30
seconds for a block partition whose PARTUUID matches the entry, reports
duplicate matches or a timeout on the console, and mounts the selected ext4
partition. It checks the selected generation's init, stops early udev, and moves
the mounts into the real root before `switch_root`.

When installing the boot menu, the kernel and initrd are copied before the menu
is replaced. Boot files referenced by retained generations remain on the ESP.

### Firmware

Set the system's top-level `firmware-packages` array to package or output
selectors that provide `lib/firmware`. The selected outputs are part of the
system closure. During activation, `/lib/firmware` resolves to their merged
tree. Identical files may share a path. Conflicting files and directory
collisions stop the build. Relative symbolic links resolve within that tree.

In `[boot.initrd]`, set `firmware-files` to paths relative to
`/lib/firmware`. Use the firmware names reported by `modinfo -F firmware`
for the storage drivers needed during early boot. The initrd includes these
files and the targets of their relative symbolic links. A missing file
stops the build.

## The root file system
The build commands use umask `022` when creating package outputs, system
trees, and disk images.

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

## Booting from the disk

A system with `[boot]` starts through an EFI boot loader instead of a kernel
given to the virtual machine. `bin/install-boot`, which `switch-to-system`
runs, needs the EFI system partition mounted at `/boot` and writes there:

| Path | Contents |
|---|---|
| `EFI/BOOT/BOOTX64.EFI` | The loader of the system being switched to. |
| `vug/<id>.bzImage` | The kernel of every generation that has a boot entry; kernels of no generation are removed. |
| `limine.conf` | One entry per generation, newest first, and the default generation as the default entry. |

The root partition is found by its GPT name `vug-root`; a service record
mounts the partition named `vug-esp` at `/boot`. The kernel has to hold the
drivers of the disk, its file system, and the console, since there is no
initramfs.

## Files below `/etc`

| Kind | What `activate` puts at the path | On a later activation |
|---|---|---|
| `link` | A link to `/run/current-system/etc/<path>`. | The content changes with `/run/current-system`. A path the new system does not list is removed; a path it adds is linked. |
| `link` with `target` | A link to the target itself. | The link is replaced when the new system names another target, and removed when it no longer lists the path. |
| `seed` | A regular file with the content and mode of the record, when nothing is at the path. | Left alone. |
| users | `/etc/passwd`, `/etc/group`, and `/etc/shadow` as regular files, when the record lists a user. | `passwd` and `group` are written again from the record. A user that already has an entry in `shadow` keeps it. A missing home directory is created. |

`activate` first looks at every link path. Where something other than its
own link is at the path, `switch-to-system` prints the path and stops before
it changes anything; at boot the path is reported and left as it is.
Paths the record does not list are never touched.

## Switching

`<system>/bin/switch-to-system`, run as root on the running machine:

1. Checks that the store paths in `manifest/closure` exist, validates the
   link paths, and checks that `/boot` is mounted for a system with boot
   entries. A failed check ends the switch before any changes.
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

## Cleaning up

```console
# vug system delete-generations --keep 3
# vug store collect-garbage
```

`delete-generations` retains the newest `--keep` links, the default generation, and the running generation. Use `--keep 2` to retain the two newest generations for rollback. The command refreshes the boot entries when the running system has them.

`collect-garbage` uses the closures of the retained generations, the running system, and active program leases. It waits for builds and switches, and retains the old closure while changed services restart. If the restart is interrupted or fails, the previous closure remains protected until reboot. Along with unreferenced vug outputs, GC reclaims Nix paths marked as owned by vug when it published them. Use `--dry-run` to list the paths that GC would remove. GC requires a generation in `--profiles` (default `/vug/profiles`) or a running generation in the selected system store. Entries in the build index allow a later build to look up removed vug outputs in the binary cache.

## Declaring a GPT layout

EFI systems with `[boot.initrd]` can declare the ESP and root filesystem in the system record:

```toml
[disk]
table = "gpt"

[[disk.partition]]
name = "esp"
type = "esp"
size-mib = 1024

[[disk.partition]]
name = "root"
type = "linux-filesystem"
grow = true

[[filesystem]]
partition = "root"
mount = "/"
type = "ext4"

[[filesystem]]
partition = "esp"
mount = "/boot"
type = "vfat"
options = ["umask=0077"]
```

The ESP comes first and is at least 64 MiB. The root takes the size requested by `build-image --size`. The generated image assigns fresh GPT IDs and saves the layout and PARTUUIDs in `/vug/disk`. The root PARTUUID goes in each Limine entry and the root and ESP PARTUUIDs go in `/etc/fstab`. Partition names are GPT labels.

This layout supplies the root boot argument and the ESP mount. Omit `boot.root`, `service.esp`, and separate fstab or boot mount definitions for these systems. The system switch checks the requested layout against the installed layout before changing generations. It reuses the installed root PARTUUID for the next boot menu and leaves the partition table in place.

### Encrypting the root

Add `[disk.partition.luks]` below the growing root partition and set
`name = "vug-root"`. Set the root filesystem's `partition` to that mapper
name. The ESP remains FAT, and the root stays ext4 inside LUKS2. The kernel
needs device mapper, dm-crypt, and AES-XTS support. The initial environment
includes cryptsetup and loads dm-crypt for this layout.

Run the installer with `--apply` from a local terminal to set and confirm the
passphrase, then enter it again to populate the encrypted partition. The
installer needs loop and device mapper support on the running kernel.
`build-image` with an encrypted layout likewise requires root and a terminal.
Each boot resolves the outer partition by PARTUUID and prompts once on the
active local console. A failed unlock stops stage 1. The generation layout
check includes the mapper name, and the root fstab entry refers to that mapper.

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
| `build-image --image FILE [--size SIZE]` | For a system with `[boot]`: also a GPT disk image with an EFI system partition of 256 MiB by default (the loader, the kernel, and the entry of generation 1) and a root partition of `SIZE`, made with `sfdisk`, `mkfs.fat`, `mcopy`, and `mke2fs` of the packages `util-linux`, `dosfstools`, `mtools`, and `e2fsprogs`. Written to a disk, it boots a machine with EFI firmware. Otherwise: also an ext4 image (default size `4g`) whose root holds the store paths listed in `manifest/closure`, generation 1 pointing to the system, the data directory indexes at `/vug/index` and `/vug/index-outputs` when present, the records (`seed.toml`, `packages`, `services`, `systems`) at `/vug/tree`, and empty directories. The image is made by `mke2fs` of the package `e2fsprogs`; every file belongs to root. The kernel needs the drivers of the disk and ext4 built in, since there is no initramfs. |
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

### Installing onto a disk

Run `vug system install --root DIR --name NAME --disk /dev/disk/by-id/ID` to
inspect the target and print its capacity, current partition table signature,
and planned ESP and root filesystem. The system record must define a GPT
layout and an EFI boot loader. The target must be an exclusively accessible
whole disk with writable 512-byte logical sectors and a capacity in whole KiB.

Pass `--apply` as root to overwrite the disk. Set `--binary-cache URL` and
`--trusted-key KEY`, or configure the same values in the running Vug system.
The installer resolves the package closure from the signed cache in an empty
temporary store, then stages a bootable image sized for the target. The
staging filesystem must have space for the substituted outputs and the
filesystem image. The target stays untouched if staging fails. Once staged,
the installer opens the target exclusively, checks its identity and partition
header again, writes the image, and synchronizes the disk. Generation 1 and
the chosen partition identifiers are stored on the target. Boot it through
the UEFI fallback loader after installation.

## System service credentials

A system or service record can declare a logical secret, its filesystem owner UID, and the systemd service that consumes it.

```toml
[[secret]]
name = "example-key"
uid = 1000
unit = "example.service"
```

Include `vug` and provide SOPS at `/bin/sops` in the running system. Provision a root-owned directory at `/etc/vug/secrets` with mode `0700`. Place the system age identity in `age.key` and the source map in `sources.toml`, both root-owned regular files with mode `0600`. Keep these files and the encrypted documents outside the store. Store the encrypted documents in a directory controlled by the administrator.

```toml
[example-key]
document = "/etc/vug/secrets/example.yaml"
field = ["example", "key"]
```

Systemd starts a root preparation service before starting the consumer. The preparation service decrypts the selected field to `/run/vug/secrets/example-key/value` with mode `0600` and the declared UID. The consumer can use the named file inside `$CREDENTIALS_DIRECTORY`. A preparation failure prevents the consumer from starting. Restart `vug-secret-example-key.service` to refresh the value and restart its running consumer. Systemd removes that runtime directory when the preparation service stops.
