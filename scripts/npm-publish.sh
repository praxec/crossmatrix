#!/usr/bin/env bash
# npm-publish.sh — publish the npm launcher tarball for a release.
# Usage: scripts/npm-publish.sh <tarball> <version> [--dry-run | --local]
#
# The tarball is the exact file the release attached (release.yml's finalize
# job); this script first checks that the version inside it matches <version>,
# so a mismatched file can never be published. A version already on npm is
# skipped, so re-running a release is safe. Pre-releases (X.Y.Z-rc.N) go to the
# `next` dist-tag, never `latest`.
#
# --dry-run runs `npm publish --dry-run` (used by tests) and never contacts the
# registry for an upload. --local is the maintainer's publish path and skips the
# provenance attestation. Otherwise `--provenance` is attached only when the
# process holds a GitHub Actions OIDC token (trusted publishing); no npm token
# is ever read from the environment or disk.
set -euo pipefail

pkg="@matthew-cochran/crossmatrix"
[ "$#" -ge 2 ] || { echo "usage: $0 <tarball> <version> [--dry-run]" >&2; exit 2; }
tarball=$1
version=$2
dry_run=false
local_publish=false
case "${3:-}" in
  "") ;;
  --dry-run) dry_run=true ;;
  --local) local_publish=true ;;
  *) echo "unknown option: $3" >&2; exit 2 ;;
esac

[ -f "$tarball" ] || { echo "tarball not found: $tarball" >&2; exit 1; }
# npm reads a bare `dir/file.tgz` as GitHub shorthand (owner/repo) and tries to
# clone it, so always hand it an explicit path.
case "$tarball" in
  /* | ./* | ../*) ;;
  *) tarball="./$tarball" ;;
esac

packed_version=$(tar -xzOf "$tarball" package/package.json | node -e \
  'let s="";process.stdin.on("data",d=>s+=d).on("end",()=>console.log(JSON.parse(s).version))')
[ "$packed_version" = "$version" ] \
  || { echo "tarball holds version $packed_version, expected $version" >&2; exit 1; }

# Provenance requires the GitHub Actions OIDC token; --dry-run and --local do
# not attest. Never fall back to a stored npm token.
provenance=false
if [ "$dry_run" = false ] && [ "$local_publish" = false ] \
  && [ -n "${ACTIONS_ID_TOKEN_REQUEST_URL:-}" ] && [ -n "${ACTIONS_ID_TOKEN_REQUEST_TOKEN:-}" ]; then
  provenance=true
fi

if [ "$(npm view "${pkg}@${version}" version 2>/dev/null || true)" = "$version" ]; then
  echo "${pkg}@${version} is already on npm; nothing to publish."
  exit 0
fi

dist_tag=latest
case "$version" in *-*) dist_tag=next ;; esac

args=(publish "$tarball" --access public --tag "$dist_tag")
if [ "$dry_run" = true ]; then
  args+=(--dry-run)
fi
if [ "$provenance" = true ]; then
  args+=(--provenance)
fi
npm "${args[@]}"
