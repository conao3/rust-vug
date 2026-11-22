# Binary cache

A binary cache is a directory, served over HTTP, from which `vug` takes
outputs instead of building them.

## Layout

| Path | Contents |
|---|---|
| `keys/<key>` | The id of the output of the build `<key>` on the first line, and on the second the Ed25519 signature of `<key> <id>` in hex, when the cache was written with a secret key. `<key>` is the key of the index (`docs/build.md`). |
| `paths/<id>.cpio` | The contents of `/vug/store/<id>` as a cpio archive in the "newc" format. |

```console
$ vug store export-cache --directory cache
{
  "Directory": "cache",
  "Keys": 54,
  "Written": 54
}
```

```console
$ vug store generate-key --output text > cache.key
$ vug store export-cache --directory cache --secret-key-file cache.key
```

`generate-key` prints a secret key and its public key (64 hex characters
each); `--secret-key-file` reads the secret key from the first line of the
file. `export-cache` writes a key for every entry of the index of the data
directory (`--data-dir`, default as for `build-package`) and an archive for
every output of the index that the store holds and the cache does not.

## Use

`--binary-cache URL` on a command that builds:

1. With `--trusted-key PUBLIC`, a build whose key is not in the index asks
   `URL/keys/<key>`. An answer signed by that key is written to the index;
   any other answer is an error. Without `--trusted-key` the cache is not
   asked for keys.
2. An output that is needed in the store and is not there is fetched from
   `URL/paths/<id>.cpio`, unpacked, and checked: its tree hash, with its own
   path replaced by the placeholder, must be `<id>`. A status 404 means the
   cache does not have it, and the output is built.

An id from the index or a cache is enough while nothing needs the files: the
output is fetched or built only when it is a dependency of a build that
runs, or part of what the command returns (a package and what it needs at
run time).

Unpacking writes nothing outside the new store path, whatever the archive
names. The check in step 2 covers the content of a path, so the files need no
trust in the cache. That a key belongs to an id is what the signature says:
whoever holds the secret key vouches for the builds.
