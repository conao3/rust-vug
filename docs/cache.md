# Binary cache

A binary cache is a directory, served over HTTP, from which `vug` takes
outputs instead of building them.

## Layout

| Path | Contents |
|---|---|
| `keys/<key>` | The id of the output of the build `<key>`, followed by a newline. `<key>` is the key of the index (`docs/build.md`). |
| `paths/<id>.cpio` | The contents of `/vug/store/<id>` as a cpio archive in the "newc" format. |

```console
$ vug store export-cache --directory cache
{
  "Directory": "cache",
  "Keys": 54,
  "Written": 54
}
```

`export-cache` writes a key for every entry of the index of the data
directory (`--data-dir`, default as for `build-package`) and an archive for
every output of the index that the store holds and the cache does not.

## Use

`--binary-cache URL` on a command that builds:

1. A build whose key is not in the index asks `URL/keys/<key>`. An answer is
   written to the index.
2. An output that is needed in the store and is not there is fetched from
   `URL/paths/<id>.cpio`, unpacked, and checked: its tree hash, with its own
   path replaced by the placeholder, must be `<id>`. A status 404 means the
   cache does not have it, and the output is built.

An id from the index or a cache is enough while nothing needs the files: the
output is fetched or built only when it is a dependency of a build that
runs, or part of what the command returns (a package and what it needs at
run time).

The check in step 2 covers the content of a path. Nothing checks that a key
belongs to an id: a cache is trusted for `keys/`.
