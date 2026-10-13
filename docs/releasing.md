# Releasing

This is the maintainer runbook for cutting a crossmatrix release. A tag
`vX.Y.Z` on `main` runs [`.github/workflows/release.yml`](../.github/workflows/release.yml),
which builds and publishes the GitHub release, the container image and the MCP
registry entry, and attaches the npm launcher tarball. The npm package is then
published with [`scripts/npm-release.sh`](../scripts/npm-release.sh) (or by the
workflow's npm job once trusted publishing is set up), and crates.io by hand.

Throughout, `X.Y.Z` is the new version and `vX.Y.Z` its tag.

## 1. Bump the version

On a branch off `dev`, set the same version in every file that carries it:

- `Cargo.toml`: `[workspace.package] version`.
- `crates/crossmatrix-mcp/Cargo.toml`: the `version` of the `crossmatrix`
  dependency (`crossmatrix = { version = "X.Y.Z", path = "../crossmatrix" }`).
- `Cargo.lock`: run `cargo check` so the lock file records the new
  `crossmatrix` and `crossmatrix-mcp` versions.
- `server.json`: the top-level `version` and the tag of the OCI package
  identifier (`ghcr.io/praxec/crossmatrix:X.Y.Z`). The OCI package has no
  `version` field: the MCP registry rejects one.

`Dockerfile` labels the image `io.modelcontextprotocol.server.name` with
`server.json`'s `name`; the MCP registry rejects an image without it (a test
in `scripts/release.test.mjs` keeps them equal).
- `npm/package.json`: `version` (the launcher downloads the GitHub release with
  this version, so it must match).

Then check them:

```sh
sh scripts/check-version-sync.sh
```

It fails, naming each file, when any of them differs from `Cargo.toml`. The
release workflow repeats the check against the tag (`--tag vX.Y.Z`) and
refuses to build on a mismatch.

## 2. Update the CHANGELOG

In [CHANGELOG.md](../CHANGELOG.md), rename `## [Unreleased]` to
`## [X.Y.Z] - YYYY-MM-DD` (or add that section above the newest release) and
leave a fresh, empty `## [Unreleased]` above it. Group entries under Added,
Changed, Fixed and so on (Keep a Changelog). The date is the planned release
date; step 3 corrects it on the day.

Open the bump as a pull request into `dev` and merge it once CI is green.

## 3. Release PR from `dev` to `main`

On release day, set the CHANGELOG date to today if it differs (a one-line pull
request into `dev`). Then open a pull request with base `main` and head `dev`.
The gitflow guard (`gitflow-guard.yml`) rejects any other head branch. Wait for
every check, then merge it with a **merge commit**, not squash or rebase, so
`main` and `dev` keep a shared history.

## 4. Tag `main`

```sh
git fetch origin
git checkout main
git pull --ff-only origin main
git tag -a vX.Y.Z -m "vX.Y.Z"
git push origin vX.Y.Z
```

Tag only commits on `main`: the workflow triggers on any `v*` tag. The tag must
match `vMAJOR.MINOR.PATCH` (an optional pre-release or build suffix is allowed)
and equal every manifest version.

The workflow then runs these jobs:

1. **validate + draft release**: `check-version-sync.sh --tag vX.Y.Z`, then a
   draft GitHub release with generated notes.
2. **build**: six targets (Linux, macOS and Windows on x86_64 and ARM64). Each
   archive is smoke-tested with `scripts/mcp-smoke.mjs` (MCP `initialize` and
   `tools/list`) and uploaded to the draft.
3. **aggregate metadata + publish**: writes `checksums.sha256` and
   `release-manifest.json` (it fails if the matrix is incomplete), uploads them
   with `install.sh` and `install.ps1`, packs the npm launcher **with the agent
   skills** and attaches `matthew-cochran-crossmatrix-X.Y.Z.tgz` with its digest
   in `npm-package.sha256`, and publishes the release.
4. **npm publish**: skipped unless npm trusted publishing is set up (see
   below); otherwise publish with `scripts/npm-release.sh` (step 5).
5. **container image**: pushes `ghcr.io/praxec/crossmatrix` for linux/amd64 and
   linux/arm64 with the tags `X.Y.Z`, `X.Y` and `latest`.
6. **MCP registry**: publishes `server.json` as `io.github.praxec/crossmatrix`
   over GitHub OIDC. Best-effort (`continue-on-error`): check its log.

## 5. Publish to npm

The npm launcher (`npm/`, package `@matthew-cochran/crossmatrix`) downloads the
GitHub release whose version equals its own, so publish it only after the
release is public (step 4 ends with the release published). Until then
`npx -y @matthew-cochran/crossmatrix` fails.

Logged in to npm (`npm login`) as an owner of the `@matthew-cochran` scope and
to GitHub (`gh auth login`):

```sh
git checkout vX.Y.Z
bash scripts/npm-release.sh            # version from npm/package.json; or: bash scripts/npm-release.sh X.Y.Z
```

The script:

1. refuses to run unless you are logged in to npm and the GitHub release
   `vX.Y.Z` exists and is not a draft (a version already on npm is skipped);
2. downloads the `matthew-cochran-crossmatrix-X.Y.Z.tgz` that release.yml
   attached, checks it against `npm-package.sha256`, checks that it holds only
   `LICENSE`, `NOTICE`, `README.md`, `package.json`, `bin/`, `lib/` and
   `skills/`, and that the agent skills are bundled;
3. publishes that file with `scripts/npm-publish.sh --local`, so npm prompts
   for your 2FA one-time password (pre-releases `X.Y.Z-rc.N` go to the `next`
   dist-tag);
4. waits (up to `CROSSMATRIX_RELEASE_WAIT_SECS`, default 180) for the registry
   to serve the version, then checks that
   `npx -y @matthew-cochran/crossmatrix@X.Y.Z --version` prints
   `crossmatrix-mcp X.Y.Z`.

It never tags, pushes or edits the release. No npm token is stored, and a
manual publish carries no provenance attestation (only a trusted-publishing run
can create one).

### Optional: npm trusted publishing

release.yml's **npm publish** job publishes the same tarball over GitHub OIDC
with provenance. It is off until you set it up:

1. On npmjs.com, the package's **Settings → Trusted publishing** names GitHub
   Actions, the repository `praxec/crossmatrix`, the workflow **`release.yml`**
   (exactly; `release.yaml` makes the publish 404) and the environment `npm`.
2. On GitHub, the `npm` environment has required reviewers and allows only
   `v*` tags.
3. Set the repository variable `NPM_TRUSTED_PUBLISHING` to `true`.

The job then waits for approval in the `npm` environment after each release;
approving it is the whole npm step. It skips a version already on npm, so it is
safe after a manual publish.

## 6. Verify the release

```sh
gh release view vX.Y.Z --json assets --jq '.assets[].name'
# six archives, checksums.sha256, release-manifest.json, install.sh, install.ps1,
# matthew-cochran-crossmatrix-X.Y.Z.tgz, npm-package.sha256
gh release download vX.Y.Z --pattern checksums.sha256 --pattern '*linux-gnu.tar.gz'
sha256sum -c --ignore-missing checksums.sha256
```

Installer, on a clean shell (repeat with `install.ps1` on Windows):

```sh
curl -fsSL https://github.com/praxec/crossmatrix/releases/latest/download/install.sh | sh -s -- --version vX.Y.Z
node scripts/mcp-smoke.mjs ~/.local/bin/crossmatrix-mcp
```

npm and skills:

```sh
npx -y @matthew-cochran/crossmatrix@X.Y.Z --version
npx -y @matthew-cochran/crossmatrix@X.Y.Z skills install --target claude --project "$(mktemp -d)" --dry-run
```

Container image:

```sh
docker pull ghcr.io/praxec/crossmatrix:X.Y.Z
# mcp-smoke.mjs takes a single executable, so wrap `docker run`:
printf '#!/bin/sh\nexec docker run -i --rm ghcr.io/praxec/crossmatrix:X.Y.Z "$@"\n' > /tmp/cmx-docker && chmod +x /tmp/cmx-docker
node scripts/mcp-smoke.mjs /tmp/cmx-docker
```

The package must be public (praxec → Packages → crossmatrix → Package settings →
Change visibility) for anonymous pulls and the MCP registry entry to work.

MCP registry: confirm the registry job succeeded, or look the server up in the
registry and check its version.

## 7. Publish to crates.io (manual, optional)

The workflow does not publish the crates. From the tagged commit, with a
crates.io token configured (`cargo login`), publish `crossmatrix` first, then
`crossmatrix-mcp` (which depends on it):

```sh
git checkout vX.Y.Z
cargo publish -p crossmatrix --dry-run --locked
cargo publish -p crossmatrix --locked
```

Wait for `crossmatrix X.Y.Z` to appear on crates.io. `crossmatrix-mcp` embeds
the agent skills at build time from `crates/crossmatrix-mcp/skills` when it
exists (else `../../skills`), so copy them in for packaging. Paste this block
whole: it stops at the first failure and refuses to publish when the tree has
other changes:

```sh
(
  set -euo pipefail
  rm -rf crates/crossmatrix-mcp/skills && cp -R skills crates/crossmatrix-mcp/skills
  dirty="$(git status --porcelain --untracked-files=all | grep -v ' crates/crossmatrix-mcp/skills/' || true)"
  if [ -n "$dirty" ]; then
    printf '%s\n' "$dirty"
    echo "the working tree has changes besides crates/crossmatrix-mcp/skills/; not publishing" >&2
    exit 1
  fi
  cargo publish -p crossmatrix-mcp --dry-run --locked --allow-dirty
  cargo publish -p crossmatrix-mcp --locked --allow-dirty
  rm -rf crates/crossmatrix-mcp/skills
)
```

## Rollback

- **A build failed before publishing.** The release is still a draft. Fix the
  problem on `dev`, delete the draft and the tag, and re-release (the next patch
  version, or the same tag if nothing was published):

  ```sh
  gh release delete vX.Y.Z --yes
  git push origin :refs/tags/vX.Y.Z
  git tag -d vX.Y.Z
  ```

  Re-running the workflow on an existing draft is also safe: assets are
  uploaded with `--clobber`.
- **A bad release was published.** Do not reuse its version. Mark it as a
  pre-release or warn in its notes, then ship `X.Y.(Z+1)`. The installers
  resolve `releases/latest`, so they pick up the fix as soon as it is published.
- **A bad npm version was published.** npm versions cannot be reused. Deprecate
  it and publish the fix:

  ```sh
  npm deprecate @matthew-cochran/crossmatrix@X.Y.Z "broken release, use X.Y.(Z+1)"
  ```

  `npm unpublish` works only within 72 hours and blocks the version number for
  good, so prefer deprecation.
- **A bad crate was published.** crates.io versions cannot be deleted. Yank it
  (`cargo yank --version X.Y.Z crossmatrix-mcp`) and publish a fix.
- **A bad image was pushed.** Push the fixed release, which moves `latest` and
  `X.Y`. Delete the bad `X.Y.Z` tag in the GHCR package settings only if it is
  harmful to keep.
- **A bad MCP registry entry.** Publish the fixed version; the registry lists
  the newest version.
