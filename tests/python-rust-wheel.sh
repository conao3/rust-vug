#!/usr/bin/env bash
set -euo pipefail

vug=$(realpath "$1")
packages=$(realpath "$2")
store=$(realpath "$3")
cache=$(realpath "$4")
fixture=$(dirname "$(realpath "$0")")/fixtures/python-rust-wheel
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
root="$work/root"
name=zz-vug-rust-wheel
mkdir -p "$root/packages/zz/$name/versions" "$work/source" "$work/verified"
ln -s "$packages/seed.toml" "$root/seed.toml"
for shard in "$packages"/packages/*; do
    [[ ${shard##*/} == zz ]] || ln -s "$shard" "$root/packages/"
done
cp -r "$fixture" "$work/source/"
record="$root/packages/zz/$name"
url=https://example.invalid/python-rust-wheel.tar.gz
cat > "$record/package.toml" <<RECORD
name = "$name"
license = "MIT"

[upstream]
sources = ["$url"]
RECORD

write_record() {
    tar -czf "$work/source.tar.gz" -C "$work/source" python-rust-wheel
    digest=$(sha256sum "$work/source.tar.gz")
    digest=${digest%% *}
    cat > "$record/versions/0.1.0.toml" <<RECORD
version = "0.1.0"

[source]
url = "$url"
sha256 = "$digest"

[build]
kind = "python"
cargo-lock = true
imports = ["vug_rust_wheel"]

[dependencies]
build = ["binutils", "make", "python-build", "python-installer", "python-maturin", "rust"]
run = ["musl", "python", "rust-libunwind"]

[check]
enabled = true
skip = []
RECORD
}

args=(--root "$root" --name "$name" --jobs 3 --data-dir "$store" --cache-dir "$cache")
write_record
mkdir -p "$cache/sources"
cp -n "$work/source.tar.gz" "$cache/sources/$digest"
"$vug" package build-package "${args[@]}"
test "$("$vug" package run-package "${args[@]}" --program bin/vug-rust-wheel)" = 1

lock="$work/source/python-rust-wheel/Cargo.lock"
checksum=$(sed -n 's/^checksum = "\(.*\)"/\1/p' "$lock")
wrong="0${checksum:1}"
sed -i "s/$checksum/$wrong/" "$lock"
write_record
cp "$work/source.tar.gz" "$work/verified/$digest"
cp "$cache/sources/$checksum" "$work/verified/$wrong"
if "$vug" package build-package "${args[@]}" --source-cache-dir "$work/verified" >"$work/output" 2>&1; then
    exit 1
fi
cat "$work/output"
grep -F "expected $wrong, got $checksum" "$work/output"
if grep -F 'Building wheel' "$work/output"; then
    exit 1
fi
