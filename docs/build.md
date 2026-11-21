# Building a package

`vug package build-package` turns one version record into one store path.
The build runs in an empty root that holds only the seed, the dependencies,
and the source.

## Store paths

- A store path is `/vug/store/<id>`, where `<id>` is 32 characters of
  lowercase RFC 4648 base32 (`a-z2-7`, no padding). Every store path is 43
  characters long.
- `<id>` encodes the first 20 bytes of the BLAKE3 hash of the output tree.
- The name and version of a package are not part of the path. They are read
  from `<path>/.vug/record.toml`, a copy of the version record that produced
  the output.

## Tree hash

The output tree is serialized to a byte stream and hashed. All integers are
u64 little-endian.

| Entry | Serialization |
|---|---|
| Regular file | `F`, then `x` when the owner-execute bit is set and `-` otherwise, the content length, the content |
| Symbolic link | `L`, the target length, the target |
| Directory | `D`, the number of entries, then for each entry in ascending byte order of its name: the name length, the name, the entry |

Timestamps, owners, and other mode bits do not affect the hash. Any other
file type is an error.

## Self-references

An output may contain its own path (an `rpath`, a script's interpreter line).
The build therefore installs into a placeholder path of the same length:

1. The output is built with the prefix `/vug/store/` followed by 32 `0`
   characters (`0` is not in the base32 alphabet, so a placeholder never
   equals a real id).
2. The tree hash of the output, placeholder included, gives `<id>`.
3. Every occurrence of the placeholder in file contents and link targets is
   replaced by `/vug/store/<id>`.

Checking a store path reverses step 3 and compares the hash with the id.

## Steps of a build

1. Fetch `source.url` into the cache directory, keyed by `source.sha256`. A
   file whose SHA-256 differs from the record is rejected.
2. Create the build root: the seed tools, each dependency at its store path,
   the source archive, the patches, and an empty output directory at the
   placeholder path.
3. Run the build inside `bwrap --unshare-all --clearenv`: no network, no
   host file system, and only the environment variables `PATH`, `HOME`,
   and `SOURCE_DATE_EPOCH`.
4. Unpack with `tar -xf <archive> --strip-components=<n>` and apply the
   patches with `patch -p1`.
5. Run the steps of the build kind (`docs/record.md`).
6. Copy the version record to `.vug/record.toml` in the output.
7. Hash the output, replace the placeholder, and move the output to its
   store path.

## Where the store lives

`/vug/store` exists only inside the build root. On the host the store is the
directory `$XDG_DATA_HOME/vug/store` (default `~/.local/share/vug/store`),
which is bound to `/vug/store` in the sandbox. The cache of fetched sources
is `$XDG_CACHE_HOME/vug/sources`.
