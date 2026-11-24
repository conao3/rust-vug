# Binary cache

A binary cache is a directory, served over HTTP, from which `vug` takes
outputs instead of building them.

## Layout

| Path | Contents |
|---|---|
| `keys/<key>` | For a single-output build, the store ID on the first line and the hex Ed25519 signature of `<key> <id>` on the second, when signed. |
| `output-keys/<key>` | For a split build, a line of canonical JSON containing `format` (currently 1), `key`, and `outputs`. Each output name maps to an `id` and a sorted `run` list of direct runtime dependency IDs. When signed, the second line contains the hex Ed25519 signature of the JSON bytes excluding the newline. |
| `paths/<id>.cpio.zst` | The contents of `/vug/store/<id>` as a cpio archive in the "newc" format, compressed with zstd. |

The key is the build key from the corresponding local index. It includes
the content digests of the patches used by the build. See
[Building packages](build.md).

```console
$ vug store generate-key --output text > cache.key
$ vug store export-cache --directory cache --secret-key-file cache.key
```

`generate-key` prints the secret key and public key as 64 hex characters
each. `--secret-key-file` reads the secret key from the first line.

`export-cache` reads `index/` and `index-outputs/` in the selected
`--data-dir`. It writes one archive per available store ID and reuses
archives already in the cache. It publishes a split manifest after every
output it lists has an archive. `Keys` in the result counts key entries
written across both formats. `Written` counts new archives.

## Use

Pass `--binary-cache URL --trusted-key PUBLIC` to use a signed cache with
a command such as `vug package build-package --root . --name example:lib`. On a
machine that runs a vug system, `/etc/vug.toml` supplies either setting
when omitted from the command line.

```toml
binary-cache = "http://cache.example"
trusted-key = "…64 hex characters…"
```

A build with no matching local index entry looks up its key in
`keys/` or `output-keys/`, depending on the record. A trusted key is
required for these lookups. For split builds, vug also checks that the
manifest contains the expected build key, every declared output, and the
direct runtime dependencies resolved from the record.

For a package selected by a runtime command, the cache supplies the
selected output and its runtime closure. Build dependencies and exposed
outputs are materialized when a build needs them. vug can use manifest
IDs to compute build keys before fetching archives. Each downloaded tree
must hash to its store ID after normalizing self-references. A restored
split tree must also have the expected output name in `.vug/output`.

HTTP 401 or 404 on a lookup means the cache has no entry. When a required archive
is missing, a command that builds can rebuild from the record. A rebuilt
split manifest must agree with the known manifest, including the IDs of
all outputs. A cache-only command reports a missing required archive.
Invalid signatures, manifest contents, and archive contents are errors.

Archive downloads stream through the zstd decoder into the temporary
extraction directory. To migrate a cache containing uncompressed
`paths/<id>.cpio` archives, re-export it from the store before switching
consumers to this format.

## Inventory and source builds

```console
$ vug --output json store list-cache --root . --binary-cache URL --trusted-key PUBLIC
```

This command lists object names reachable from the current version of each
package in the repository. It resolves records and verifies signed cache
metadata without downloading archives, preparing the seed, or writing to
the store. A missing key also excludes packages whose build keys need the
missing output. A split manifest retains every output it names. Packages
referenced by system records use the same keys as packages built on their
own. A composed system generation is assembled locally from its package
outputs and has no key in the binary cache.

Pass `--build-from-source NAME` to `package build-package` for each package
that must be built in an independent store even when a matching remote
cache entry exists. vug still uses the cache for other packages and can
reuse local outputs built earlier in the same store.

## Fetch a Nix reference closure

```console
$ stage_parent=$(mktemp -d)
$ vug nix-cache fetch-closure --store-path /nix/store/jv5kdlkwx0l8a7jdpg21g934ff30s319-which-2.25 --directory "$stage_parent"
```

Supply one or more absolute Nix store paths after `--store-path`. The command resolves their complete `References` closure over HTTPS from `cache.nixos.org`. It verifies each narinfo with the built-in `cache.nixos.org-1` Ed25519 key and requires each `StorePath` to match the requested path. The cache URLs must be relative paths, and any HTTP redirect is an error. [Nix narinfo format](https://releases.nixos.org/nix/nix-2.36.0/manual/protocols/binary-cache/narinfo.html)

The command verifies `FileHash` and `FileSize` when present, then verifies `NarHash` and `NarSize` on the decoded NAR before extraction. It accepts SHA-256 hashes in Nix32 and supports uncompressed, Zstandard, XZ, and bzip2 archives. Extraction supports directories, regular files, and symlinks. Staged directories and executables use mode `0755`, other regular files use `0644`, and symlink targets are preserved. It accepts a nesting depth of 128 and at most 1,000,000 nodes per archive. [NAR format](https://nix.dev/manual/nix/2.34/store/file-system-object/content-address.html#serial-nix-archive)

The CLI allows up to 4,096 store paths, 64 KiB per narinfo, 4 GiB per archive transfer, and 8 GiB per decoded NAR. The closure totals are limited to 16 GiB transferred and 32 GiB decoded. Transfer limits apply to bytes read, including when the narinfo omits `FileSize`.

Use an existing staging parent outside `/nix/store`. On success, JSON gives the new private `Directory` and the verified `Paths` with their hashes, sizes, and references. Text gives just the directory. Every staged store path is a direct child named by its Nix store basename. Absolute symlink targets still refer to their original paths. The caller owns the staging directory on success. Failed fetches clean up staging.

Library callers use `vug::nix_cache::Cache::resolve` with their chosen `Limits` to inspect verified metadata and `Cache::fetch` to stage the closure. Staging is returned as a `tempfile::TempDir` and is removed when dropped unless the caller keeps it.
