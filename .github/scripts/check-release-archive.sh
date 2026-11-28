#!/usr/bin/env bash
set -euo pipefail

archive=$1
version=$2
dir=$(mktemp -d)
trap 'rm -rf "$dir"' EXIT

tar -xzf "$archive" -C "$dir"
members=$(tar -tzf "$archive" | sort | tr '\n' ' ')
if [[ "$members" != "vug vug-bwrap " ]]; then
  echo "unexpected archive members: $members" >&2
  exit 1
fi
[[ "$("$dir/vug" --version)" == "vug $version" ]]
[[ "$("$dir/vug-bwrap" --version)" == "bubblewrap "* ]]

unshare=$(command -v unshare)
"$dir/vug-bwrap" --unshare-all --uid 1000 --gid 100 --die-with-parent --clearenv \
  --ro-bind / / --proc /proc --dev /dev --tmpfs /tmp -- \
  "$unshare" --user --net true
