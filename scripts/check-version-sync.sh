#!/bin/sh
# check-version-sync.sh — fail unless every manifest version in this repository
# equals Cargo.toml's [workspace.package] version.
#
# Compares, against Cargo.toml:
#   - the `crossmatrix` dependency in crates/crossmatrix-mcp/Cargo.toml
#   - the `crossmatrix` and `crossmatrix-mcp` entries in Cargo.lock
#   - server.json's top-level `version`, the tag of every OCI package
#     `identifier` (`ghcr.io/owner/image:X.Y.Z`; the MCP registry rejects a
#     `version` field on OCI packages, so one is an error), and the `version`
#     of every non-OCI package
#   - npm/package.json's `version`
# With `--tag vX.Y.Z` the tag (minus the leading `v`) must equal that version.
#
# Offline and self-contained; no network, no jq. Errors are GitHub annotations:
# one `::error::` line per mismatching file (name, found, expected).
#
# Usage: scripts/check-version-sync.sh [--root DIR] [--tag vX.Y.Z]
# Exit:  0 everything agrees, 1 a version disagrees, 2 a usage/read error.
set -eu

root=$(cd "$(dirname "$0")/.." && pwd)
tag=""

while [ "$#" -gt 0 ]; do
  case "$1" in
    --root)
      [ "$#" -ge 2 ] || { echo "check-version-sync: --root needs a directory" >&2; exit 2; }
      root=$2
      shift 2
      ;;
    --tag)
      [ "$#" -ge 2 ] || { echo "check-version-sync: --tag needs a value" >&2; exit 2; }
      tag=$2
      shift 2
      ;;
    -h|--help)
      sed -n '2,16p' "$0"
      exit 0
      ;;
    *)
      echo "check-version-sync: unknown argument: $1" >&2
      exit 2
      ;;
  esac
done

[ -d "$root" ] || { echo "check-version-sync: root not found: $root" >&2; exit 2; }

cargo_toml="$root/Cargo.toml"
mcp_toml="$root/crates/crossmatrix-mcp/Cargo.toml"
cargo_lock="$root/Cargo.lock"
server_json="$root/server.json"
npm_package="$root/npm/package.json"

for file in "$cargo_toml" "$mcp_toml" "$cargo_lock" "$server_json" "$npm_package"; do
  [ -f "$file" ] || { echo "check-version-sync: missing manifest: $file" >&2; exit 2; }
done

# The canonical version: the first `version = "..."` inside [workspace.package].
expected=$(awk '
  /^\[/ { in_pkg = ($0 == "[workspace.package]") }
  in_pkg && /^version[[:space:]]*=/ { sub(/^[^=]*=[[:space:]]*/, ""); gsub(/["[:space:]]/, ""); print; exit }
' "$cargo_toml")
[ -n "$expected" ] || { echo "check-version-sync: could not read [workspace.package] version from $cargo_toml" >&2; exit 2; }

status=0
fail() {
  echo "::error::$1" >&2
  status=1
}

# JSON scalar/array queries run on python3, which release.yml already requires.
json_scalar() {
  python3 -c 'import json, sys; print(json.load(open(sys.argv[1])).get(sys.argv[2], "<missing>"))' "$1" "$2"
}
# Prints one problem per line for server.json's packages (nothing when they
# all carry the expected version).
server_package_problems() {
  python3 -c '
import json, sys
expected = sys.argv[2]
for i, pkg in enumerate(json.load(open(sys.argv[1])).get("packages") or []):
    if pkg.get("registryType") == "oci":
        ident = pkg.get("identifier", "")
        name, sep, tag = ident.rpartition(":")
        if not sep or "/" in tag:
            tag = "<untagged>"
        if tag != expected:
            print(f"packages[{i}].identifier tag {tag} != expected {expected}")
        if "version" in pkg:
            print(f"packages[{i}] has a version field (OCI packages must carry the version in the identifier tag)")
    else:
        version = pkg.get("version", "<missing>")
        if version != expected:
            print(f"packages[{i}].version {version} != expected {expected}")
' "$1" "$2"
}

# crates/crossmatrix-mcp/Cargo.toml: the intra-workspace `crossmatrix` dependency.
dep_version=$(sed -n 's/^crossmatrix[[:space:]]*=.*version[[:space:]]*=[[:space:]]*"\([^"]*\)".*/\1/p' "$mcp_toml" | head -n 1)
[ -n "$dep_version" ] || { echo "check-version-sync: could not read the crossmatrix dependency from $mcp_toml" >&2; exit 2; }
if [ "$dep_version" != "$expected" ]; then
  fail "crates/crossmatrix-mcp/Cargo.toml crossmatrix dependency version $dep_version != expected $expected"
fi

# Cargo.lock: both workspace crates must be locked to the same version.
lock_bad=""
for name in crossmatrix crossmatrix-mcp; do
  locked=$(awk -v pkg="$name" '
    $0 == "name = \"" pkg "\"" { want = 1; next }
    want && /^version[[:space:]]*=/ { sub(/^[^=]*=[[:space:]]*/, ""); gsub(/["[:space:]]/, ""); print; exit }
  ' "$cargo_lock")
  [ -n "$locked" ] || { echo "check-version-sync: could not read the $name version from $cargo_lock" >&2; exit 2; }
  if [ "$locked" != "$expected" ] && [ -z "$lock_bad" ]; then
    lock_bad=$locked
  fi
done
if [ -n "$lock_bad" ]; then
  fail "Cargo.lock version $lock_bad != expected $expected"
fi

# server.json: top-level version, then each package (one line for the file,
# even when several fields disagree).
server_top=$(json_scalar "$server_json" version)
server_bad=""
if [ "$server_top" != "$expected" ]; then
  server_bad="version $server_top != expected $expected"
else
  problems=$(server_package_problems "$server_json" "$expected") \
    || { echo "check-version-sync: could not read the packages in $server_json" >&2; exit 2; }
  server_bad=$(printf '%s\n' "$problems" | head -n 1)
fi
if [ -n "$server_bad" ]; then
  fail "server.json $server_bad"
fi

# npm/package.json: the launcher manifest.
npm_version=$(json_scalar "$npm_package" version)
if [ "$npm_version" != "$expected" ]; then
  fail "npm/package.json version $npm_version != expected $expected"
fi

# Optional tag guard: vX.Y.Z must hold exactly the shared version.
if [ -n "$tag" ]; then
  case "$tag" in
    v[0-9]*) tag_version=${tag#v} ;;
    *) echo "check-version-sync: --tag must look like vMAJOR.MINOR.PATCH: $tag" >&2; exit 2 ;;
  esac
  if [ "$tag_version" != "$expected" ]; then
    fail "tag $tag != expected $expected"
  fi
fi

if [ "$status" -eq 0 ]; then
  echo "version sync ok: $expected"
fi
exit "$status"
