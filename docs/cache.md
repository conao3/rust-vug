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

A Git over SSH source makes every output of its build private. Privacy propagates through build, runtime, and exposed dependencies, including dependencies of split outputs. Private output IDs and build keys stay local. `export-cache` skips their index entries and archives, and `list-cache` excludes these builds from the signed cache inventory. Use the exported directory as the input to the upload step.

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
