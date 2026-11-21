# Systems

A system record names a kernel, the packages of a root file system, and the
files to put on top. `vug system build-system` turns it into a kernel image
and an initramfs that boot together.

## Record

`systems/<name>.toml` at the root of the package tree:

```toml
name = "minimal"
kernel = "linux"
packages = [
  "bash",
  "coreutils",
]

[[file]]
path = "/init"
mode = "755"
content = """
#!/bin/bash
mount -t proc proc /proc
exec bash
"""

[[file]]
path = "/etc/hostname"
content = "vug-minimal\n"
```

| Key | Type | Required | Meaning |
|---|---|---|---|
| `name` | string | yes | Must equal the file name without `.toml`. |
| `kernel` | string | yes | A package whose output holds `boot/bzImage` (build kind `linux`). |
| `packages` | array of strings | yes | Packages whose programs appear in `/bin`. |
| `file.path` | string | yes | Absolute path in the root. A path is listed once. |
| `file.mode` | string of octal digits | no (default `644`) | Permission bits. |
| `file.content` | string | yes | Contents. `{pkg:NAME}` is replaced by the store path of the package `NAME` of this system. |

Unknown keys are errors.

## The root file system

- `/vug/store/<id>` for every listed package and everything it needs at run
  time.
- `/bin` holds one link per program in the `bin` and `sbin` directories of
  the listed packages; `/sbin`, `/usr/bin`, and `/usr/sbin` are links to it.
  When two packages have a program of the same name, the package listed
  later wins.
- Empty `/dev`, `/etc`, `/proc`, `/root`, `/run`, `/sys`, `/tmp` (mode 1777),
  `/usr`, and `/var`.
- The files of the record, written last.

The kernel runs `/init`, so a system needs a file at that path.

## The initramfs

The root is written as an uncompressed cpio archive in the "newc" format.
The archive is a function of the root alone: entries are in ascending byte
order of their names, every entry belongs to user and group 0, and every time
stamp is 0.

## Command

```console
$ vug system build-system --root . --name minimal
{
  "Initrd": "/home/user/.local/share/vug/systems/minimal/initrd",
  "Kernel": "/home/user/.local/share/vug/systems/minimal/bzImage",
  "Name": "minimal",
  "Packages": 9
}
$ qemu-system-x86_64 -enable-kvm -m 1024 -nographic \
    -kernel ~/.local/share/vug/systems/minimal/bzImage \
    -initrd ~/.local/share/vug/systems/minimal/initrd \
    -append console=ttyS0
```

`build-system` takes the parameters of `vug package build-package` and
builds what is missing. `Packages` is the number of store paths in the root.
