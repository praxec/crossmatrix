#!/usr/bin/env bash
#
# Manual npm release of the launcher `@matthew-cochran/crossmatrix` (see
# docs/releasing.md step 5). It publishes exactly the tarball that release.yml
# attached to the GitHub release vX.Y.Z, after checking its digest and
# contents, so npm gets the file CI built and tested.
#
# Why manual: until npm trusted publishing (GitHub OIDC) is set up for this
# package, release.yml's npm job is skipped, and no long-lived NPM_TOKEN is
# stored (by decision). npm prompts for your 2FA one-time password instead.
#
# Afterwards it waits (bounded) until the registry serves the version and
# checks that `npx -y @matthew-cochran/crossmatrix@X.Y.Z --version` runs the
# release binary. It never tags, pushes or edits the GitHub release.
#
# Prerequisites:
#   - `npm login` as an owner of the @matthew-cochran scope
#   - `gh auth login` with read access to praxec/crossmatrix
#   - the GitHub release vX.Y.Z is published (not a draft): the launcher
#     downloads its binaries, so publishing earlier ships a broken package
#   - your authenticator app for the OTP prompt
#
# Usage:  bash scripts/npm-release.sh [X.Y.Z]     (default: npm/package.json version)
#   CROSSMATRIX_RELEASE_WAIT_SECS  how long to wait for npm to serve the version (default 180)
set -euo pipefail
cd "$(dirname "$0")/.."

pkg="@matthew-cochran/crossmatrix"
repo="praxec/crossmatrix"
bin="crossmatrix-mcp"

case "${1:-}" in
  -h|--help) sed -n '2,25p' "$0"; exit 0 ;;
esac
[ "$#" -le 1 ] || { echo "✗ usage: $0 [X.Y.Z]" >&2; exit 64; }
version=${1:-$(node -p "require('./npm/package.json').version")}
case "$version" in
  [0-9]*.[0-9]*.[0-9]*) ;;
  *) echo "✗ not a version: $version" >&2; exit 64 ;;
esac
tag="v${version}"
tarball="matthew-cochran-crossmatrix-${version}.tgz"
wait_secs=${CROSSMATRIX_RELEASE_WAIT_SECS:-180}

if ! npm whoami >/dev/null 2>&1; then
  echo "✗ not logged in to npm — run 'npm login' first." >&2
  exit 1
fi
echo "→ npm user: $(npm whoami)"

if [ "$(npm view "${pkg}@${version}" version 2>/dev/null || true)" = "$version" ]; then
  echo "✓ ${pkg}@${version} is already on npm; nothing to publish."
  exit 0
fi

draft=$(gh release view "$tag" --repo "$repo" --json isDraft --jq .isDraft 2>/dev/null) \
  || { echo "✗ GitHub release $tag not found in $repo; tag and release first (docs/releasing.md step 4)." >&2; exit 1; }
[ "$draft" = "false" ] \
  || { echo "✗ GitHub release $tag is still a draft; the launcher could not download its binaries." >&2; exit 1; }

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
echo "→ downloading $tarball and npm-package.sha256 from $tag"
gh release download "$tag" --repo "$repo" --dir "$work" \
  --pattern "$tarball" --pattern npm-package.sha256

sha256_check() {
  if command -v sha256sum >/dev/null 2>&1; then sha256sum -c npm-package.sha256
  else shasum -a 256 -c npm-package.sha256; fi
}
(cd "$work" && sha256_check) \
  || { echo "✗ digest mismatch for $tarball; stop." >&2; exit 1; }

# The package holds the launcher and the bundled agent skills only.
unexpected=$(tar -tzf "$work/$tarball" \
  | grep -vE '^package/(LICENSE|NOTICE|README\.md|package\.json|bin/.*|lib/.*|skills/.*)$' || true)
[ -z "$unexpected" ] \
  || { echo "✗ unexpected files in $tarball:" >&2; echo "$unexpected" >&2; exit 1; }
tar -tzf "$work/$tarball" | grep -qx 'package/skills/crossmatrix/SKILL.md' \
  || { echo "✗ $tarball does not bundle the agent skills (package/skills/crossmatrix/SKILL.md)." >&2; exit 1; }

echo "→ publishing ${pkg}@${version} (enter your OTP when prompted)"
bash scripts/npm-publish.sh "$work/$tarball" "$version" --local

echo "→ waiting up to ${wait_secs}s for the registry to serve ${version}"
deadline=$(( $(date +%s) + wait_secs ))
until [ "$(npm view "${pkg}@${version}" version 2>/dev/null || true)" = "$version" ]; do
  [ "$(date +%s)" -lt "$deadline" ] \
    || { echo "✗ npm does not serve ${pkg}@${version} yet; check later with: npm view ${pkg}@${version}" >&2; exit 1; }
  sleep 10
done

echo "→ verifying the published launcher (one-time binary download to the npm cache)"
out=$(cd "$work" && npx -y "${pkg}@${version}" --version 2>/dev/null) || true
if [ "$out" != "${bin} ${version}" ]; then
  echo "✗ npx ${pkg}@${version} --version printed '${out}', expected '${bin} ${version}'" >&2
  exit 1
fi
echo "✓ ${out}"
echo "✓ published ${pkg}@${version}"
