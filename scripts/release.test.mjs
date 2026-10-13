// release.test.mjs — fast, offline behavioral tests for the Praxec release
// installers and packaging metadata. Each test makes exactly one behavioral
// assertion about a public interface (install.sh, install.ps1,
// release-manifest.sh, mcp-smoke.mjs, check-version-sync.sh, npm-publish.sh,
// npm-release.sh). No network, no machine-config changes.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { spawnSync, execFileSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';

const ROOT = import.meta.dirname;
const TAG = 'v1.2.3';
const SIX_TARGETS = [
  ['x86_64-unknown-linux-gnu', 'tar.gz'],
  ['aarch64-unknown-linux-gnu', 'tar.gz'],
  ['x86_64-apple-darwin', 'tar.gz'],
  ['aarch64-apple-darwin', 'tar.gz'],
  ['x86_64-pc-windows-msvc', 'zip'],
  ['aarch64-pc-windows-msvc', 'zip'],
];
const REPOS = [{ repo: 'crossmatrix', bin: 'crossmatrix-mcp' }];

function scriptPath(repo, name) {
  return path.join(ROOT, name);
}

function run(cmd, args, opts = {}) {
  const res = spawnSync(cmd, args, {
    encoding: 'utf8',
    timeout: 60000,
    cwd: opts.cwd ?? ROOT,
    env: { ...process.env, ...(opts.env ?? {}) },
  });
  return { status: res.status, stdout: res.stdout ?? '', stderr: res.stderr ?? '' };
}

function sha256(file) {
  return createHash('sha256').update(fs.readFileSync(file)).digest('hex');
}

function rewriteChecksums(releaseDir) {
  const lines = fs.readdirSync(releaseDir)
    .filter((f) => f.endsWith('.tar.gz'))
    .sort()
    .map((f) => `${sha256(path.join(releaseDir, f))}  ${f}`);
  fs.writeFileSync(path.join(releaseDir, 'checksums.sha256'), lines.join('\n') + '\n');
}

// Builds a directory tree shaped like a GitHub release download page, served
// to the installer over file:// so tests stay fully offline.
function makeReleaseFixture(bin, assets, mutate) {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'praxec-release-'));
  const releaseDir = path.join(root, 'releases', 'download', TAG);
  fs.mkdirSync(releaseDir, { recursive: true });
  for (const asset of assets) {
    const stage = path.join(root, 'stage', asset.target);
    fs.mkdirSync(stage, { recursive: true });
    const binary = path.join(stage, bin);
    fs.writeFileSync(binary, asset.payload, { mode: 0o755 });
    execFileSync('tar', ['-czf', path.join(releaseDir, `${bin}-${asset.target}.tar.gz`), '-C', stage, bin]);
  }
  if (mutate) mutate({ root, releaseDir });
  rewriteChecksums(releaseDir);
  return { root, releaseDir, baseUrl: `file://${path.join(root, 'releases')}` };
}

function tempDir(t) {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'praxec-dest-'));
  t.after(() => fs.rmSync(dir, { recursive: true, force: true }));
  return dir;
}

function installArgs(fixture, dest) {
  return [
    '--base-url', fixture.baseUrl,
    '--version', TAG,
    '--install-dir', dest,
  ];
}

// ---------------------------------------------------------------------------
// install.sh: target resolution across all three repositories
// ---------------------------------------------------------------------------
for (const { repo, bin } of REPOS) {
  test(`${repo}/install.sh resolves the native linux/x86_64 release target`, () => {
    const r = run('sh', [scriptPath(repo, 'install.sh'), '--print-target'], {
      env: { PRAXEC_OS: 'linux', PRAXEC_ARCH: 'x86_64' },
    });
    assert.equal(r.stdout.trim(), 'x86_64-unknown-linux-gnu');
  });
}

// ---------------------------------------------------------------------------
// install.sh: end-to-end install behavior
// ---------------------------------------------------------------------------
for (const { repo, bin } of REPOS) {
  test(`${repo}/install.sh installs the linux/x86_64 asset into the managed directory`, (t) => {
    const fixture = makeReleaseFixture(bin, [
      { target: 'x86_64-unknown-linux-gnu', payload: 'NATIVE-X64\n' },
      { target: 'aarch64-unknown-linux-gnu', payload: 'NATIVE-ARM\n' },
    ]);
    t.after(() => fs.rmSync(fixture.root, { recursive: true, force: true }));
    const dest = tempDir(t);
    run('sh', [scriptPath(repo, 'install.sh'), ...installArgs(fixture, dest)], {
      env: { PRAXEC_OS: 'linux', PRAXEC_ARCH: 'x86_64' },
    });
    assert.equal(fs.readFileSync(path.join(dest, bin), 'utf8'), 'NATIVE-X64\n');
  });
}

test('crossmatrix/install.sh selects the aarch64 asset for an arm64 host', (t) => {
  const fixture = makeReleaseFixture('crossmatrix-mcp', [
    { target: 'x86_64-unknown-linux-gnu', payload: 'NATIVE-X64\n' },
    { target: 'aarch64-unknown-linux-gnu', payload: 'NATIVE-ARM\n' },
  ]);
  t.after(() => fs.rmSync(fixture.root, { recursive: true, force: true }));
  const dest = tempDir(t);
  run('sh', [scriptPath('crossmatrix', 'install.sh'), ...installArgs(fixture, dest)], {
    env: { PRAXEC_OS: 'linux', PRAXEC_ARCH: 'aarch64' },
  });
  assert.equal(fs.readFileSync(path.join(dest, 'crossmatrix-mcp'), 'utf8'), 'NATIVE-ARM\n');
});

test('crossmatrix/install.sh atomically replaces an existing installed binary', (t) => {
  const fixture = makeReleaseFixture('crossmatrix-mcp', [
    { target: 'x86_64-unknown-linux-gnu', payload: 'NEW-BUILD\n' },
  ]);
  t.after(() => fs.rmSync(fixture.root, { recursive: true, force: true }));
  const dest = tempDir(t);
  fs.writeFileSync(path.join(dest, 'crossmatrix-mcp'), 'OLD-BUILD\n', { mode: 0o755 });
  run('sh', [scriptPath('crossmatrix', 'install.sh'), ...installArgs(fixture, dest)], {
    env: { PRAXEC_OS: 'linux', PRAXEC_ARCH: 'x86_64' },
  });
  assert.equal(fs.readFileSync(path.join(dest, 'crossmatrix-mcp'), 'utf8'), 'NEW-BUILD\n');
});

test('crossmatrix/install.sh refuses an asset whose checksum does not match', (t) => {
  const fixture = makeReleaseFixture('crossmatrix-mcp', [
    { target: 'x86_64-unknown-linux-gnu', payload: 'PAYLOAD\n' },
  ]);
  t.after(() => fs.rmSync(fixture.root, { recursive: true, force: true }));
  fs.writeFileSync(
    path.join(fixture.releaseDir, 'checksums.sha256'),
    `${'0'.repeat(64)}  crossmatrix-mcp-x86_64-unknown-linux-gnu.tar.gz\n`,
  );
  const dest = tempDir(t);
  const r = run('sh', [scriptPath('crossmatrix', 'install.sh'), ...installArgs(fixture, dest)], {
    env: { PRAXEC_OS: 'linux', PRAXEC_ARCH: 'x86_64' },
  });
  assert.notEqual(r.status, 0);
});

test('crossmatrix/install.sh fails on an unsupported operating system instead of compiling', () => {
  const r = run('sh', [scriptPath('crossmatrix', 'install.sh'), '--print-target'], {
    env: { PRAXEC_OS: 'freebsd', PRAXEC_ARCH: 'x86_64' },
  });
  assert.notEqual(r.status, 0);
});

test('crossmatrix/install.sh fails on an unsupported CPU architecture instead of compiling', () => {
  const r = run('sh', [scriptPath('crossmatrix', 'install.sh'), '--print-target'], {
    env: { PRAXEC_OS: 'linux', PRAXEC_ARCH: 'ppc64le' },
  });
  assert.notEqual(r.status, 0);
});

test('crossmatrix/install.sh rejects a tar archive containing parent-directory traversal', (t) => {
  const fixture = makeReleaseFixture('crossmatrix-mcp', [
    { target: 'x86_64-unknown-linux-gnu', payload: 'X\n' },
  ], ({ releaseDir }) => {
    execFileSync('python3', ['-c', `
import io, sys, tarfile
with tarfile.open(sys.argv[1], 'w:gz') as t:
    data = b'escaped'
    info = tarfile.TarInfo('../escape.txt')
    info.size = len(data)
    t.addfile(info, io.BytesIO(data))
`, path.join(releaseDir, 'crossmatrix-mcp-x86_64-unknown-linux-gnu.tar.gz')]);
  });
  t.after(() => fs.rmSync(fixture.root, { recursive: true, force: true }));
  const dest = tempDir(t);
  const r = run('sh', [scriptPath('crossmatrix', 'install.sh'), ...installArgs(fixture, dest)], {
    env: { PRAXEC_OS: 'linux', PRAXEC_ARCH: 'x86_64' },
  });
  assert.notEqual(r.status, 0);
});

test('crossmatrix/install.sh rejects a tar archive containing a symlink entry', (t) => {
  const fixture = makeReleaseFixture('crossmatrix-mcp', [
    { target: 'x86_64-unknown-linux-gnu', payload: 'X\n' },
  ], ({ releaseDir }) => {
    execFileSync('python3', ['-c', `
import sys, tarfile
with tarfile.open(sys.argv[1], 'w:gz') as t:
    info = tarfile.TarInfo('link')
    info.type = tarfile.SYMTYPE
    info.linkname = '/etc/passwd'
    t.addfile(info)
`, path.join(releaseDir, 'crossmatrix-mcp-x86_64-unknown-linux-gnu.tar.gz')]);
  });
  t.after(() => fs.rmSync(fixture.root, { recursive: true, force: true }));
  const dest = tempDir(t);
  const r = run('sh', [scriptPath('crossmatrix', 'install.sh'), ...installArgs(fixture, dest)], {
    env: { PRAXEC_OS: 'linux', PRAXEC_ARCH: 'x86_64' },
  });
  assert.notEqual(r.status, 0);
});

test('crossmatrix/install.sh resolves native arm64 when macOS runs under Rosetta', (t) => {
  const fakeBin = tempDir(t);
  fs.writeFileSync(path.join(fakeBin, 'uname'), '#!/bin/sh\ncase "$1" in -m) echo x86_64;; *) echo Darwin;; esac\n', { mode: 0o755 });
  fs.writeFileSync(path.join(fakeBin, 'sysctl'), '#!/bin/sh\necho 1\n', { mode: 0o755 });
  const r = run('sh', [scriptPath('crossmatrix', 'install.sh'), '--print-target'], {
    env: { PATH: `${fakeBin}:${process.env.PATH}`, PRAXEC_OS: '', PRAXEC_ARCH: '' },
  });
  assert.equal(r.stdout.trim(), 'aarch64-apple-darwin');
});

// ---------------------------------------------------------------------------
// release-manifest.sh: completeness and metadata
// ---------------------------------------------------------------------------
function makeManifestFixture(bin, outName = 'out') {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'praxec-manifest-'));
  const dist = path.join(root, 'dist');
  const out = path.join(root, outName);
  fs.mkdirSync(dist, { recursive: true });
  for (const [target, ext] of SIX_TARGETS) {
    fs.writeFileSync(path.join(dist, `${bin}-${target}.${ext}`), `payload ${target}\n`);
  }
  return { root, dist, out };
}

function runManifest(fixture, bin) {
  return run('sh', [
    path.join(ROOT, 'release-manifest.sh'),
    '--assets-dir', fixture.dist,
    '--out-dir', fixture.out,
    '--version', '1.2.3',
    '--source-sha', 'abcdef0123456789',
    '--binary', bin,
  ]);
}

test('release-manifest.sh emits one manifest entry for every native target', (t) => {
  const fixture = makeManifestFixture('crossmatrix-mcp');
  t.after(() => fs.rmSync(fixture.root, { recursive: true, force: true }));
  runManifest(fixture, 'crossmatrix-mcp');
  const manifest = JSON.parse(fs.readFileSync(path.join(fixture.out, 'release-manifest.json'), 'utf8'));
  assert.equal(manifest.targets.length, 6);
});

test('release-manifest.sh lists every packaged asset in checksums.sha256', (t) => {
  const fixture = makeManifestFixture('crossmatrix-mcp');
  t.after(() => fs.rmSync(fixture.root, { recursive: true, force: true }));
  runManifest(fixture, 'crossmatrix-mcp');
  const lines = fs.readFileSync(path.join(fixture.out, 'checksums.sha256'), 'utf8').trim().split('\n');
  assert.equal(lines.length, 6);
});

test('release-manifest.sh records the requested version', (t) => {
  const fixture = makeManifestFixture('crossmatrix-mcp');
  t.after(() => fs.rmSync(fixture.root, { recursive: true, force: true }));
  runManifest(fixture, 'crossmatrix-mcp');
  const manifest = JSON.parse(fs.readFileSync(path.join(fixture.out, 'release-manifest.json'), 'utf8'));
  assert.equal(manifest.version, '1.2.3');
});

test('release-manifest.sh records the source commit SHA', (t) => {
  const fixture = makeManifestFixture('crossmatrix-mcp');
  t.after(() => fs.rmSync(fixture.root, { recursive: true, force: true }));
  runManifest(fixture, 'crossmatrix-mcp');
  const manifest = JSON.parse(fs.readFileSync(path.join(fixture.out, 'release-manifest.json'), 'utf8'));
  assert.equal(manifest.sourceSha, 'abcdef0123456789');
});

test('release-manifest.sh records a sha256 digest for every target', (t) => {
  const fixture = makeManifestFixture('crossmatrix-mcp');
  t.after(() => fs.rmSync(fixture.root, { recursive: true, force: true }));
  runManifest(fixture, 'crossmatrix-mcp');
  const manifest = JSON.parse(fs.readFileSync(path.join(fixture.out, 'release-manifest.json'), 'utf8'));
  assert.ok(manifest.targets.every((entry) => /^sha256:[0-9a-f]{64}$/.test(entry.digest)));
});

test('release-manifest.sh refuses to emit metadata for an incomplete matrix', (t) => {
  const fixture = makeManifestFixture('crossmatrix-mcp');
  t.after(() => fs.rmSync(fixture.root, { recursive: true, force: true }));
  fs.rmSync(path.join(fixture.dist, 'crossmatrix-mcp-aarch64-pc-windows-msvc.zip'));
  const r = runManifest(fixture, 'crossmatrix-mcp');
  assert.notEqual(r.status, 0);
});

// ---------------------------------------------------------------------------
// mcp-smoke.mjs: MCP protocol handshake over stdio
// ---------------------------------------------------------------------------
const GOOD_SERVER = `#!/usr/bin/env node
import process from 'node:process';
let buf = '';
const reply = (m) => {
  if (m.method === 'initialize') {
    process.stdout.write(JSON.stringify({ jsonrpc: '2.0', id: m.id, result: { protocolVersion: '2024-11-05', capabilities: { tools: {} }, serverInfo: { name: 'fake', version: '1' } } }) + '\\n');
  } else if (m.method === 'tools/list') {
    process.stdout.write(JSON.stringify({ jsonrpc: '2.0', id: m.id, result: { tools: [{ name: 'ping', description: 'ping', inputSchema: { type: 'object' } }] } }) + '\\n');
  }
};
process.stdin.on('data', (d) => {
  buf += d.toString();
  let i;
  while ((i = buf.indexOf('\\n')) >= 0) {
    const line = buf.slice(0, i);
    buf = buf.slice(i + 1);
    if (!line.trim()) continue;
    try { const m = JSON.parse(line); if (m.id != null) reply(m); } catch {}
  }
});
`;

const EMPTY_SERVER = GOOD_SERVER.replace(
  "result: { tools: [{ name: 'ping', description: 'ping', inputSchema: { type: 'object' } }] }",
  'result: { tools: [] }',
);

function writeServer(t, source) {
  const dir = tempDir(t);
  const file = path.join(dir, 'fake-mcp.mjs');
  fs.writeFileSync(file, source, { mode: 0o755 });
  return file;
}

test('mcp-smoke.mjs accepts a packaged server that answers initialize and tools/list', (t) => {
  const server = writeServer(t, GOOD_SERVER);
  const r = run('node', [scriptPath('crossmatrix', 'mcp-smoke.mjs'), server]);
  assert.equal(r.status, 0);
});

test('mcp-smoke.mjs rejects a packaged server that advertises no tools', (t) => {
  const server = writeServer(t, EMPTY_SERVER);
  const r = run('node', [scriptPath('crossmatrix', 'mcp-smoke.mjs'), server]);
  assert.notEqual(r.status, 0);
});

// ---------------------------------------------------------------------------
// install.ps1: Windows target resolution (only when PowerShell is available)
// ---------------------------------------------------------------------------
const hasPwsh = spawnSync('pwsh', ['-NoProfile', '-Command', 'exit 0']).status === 0;

test('crossmatrix/install.ps1 resolves the native windows/arm64 target', { skip: !hasPwsh }, () => {
  const r = run('pwsh', ['-NoProfile', '-File', scriptPath('crossmatrix', 'install.ps1'), '-PrintTarget'], {
    env: { PRAXEC_OS: 'windows', PRAXEC_ARCH: 'aarch64' },
  });
  assert.equal(r.stdout.trim(), 'aarch64-pc-windows-msvc');
});

// ---------------------------------------------------------------------------
// check-version-sync.sh: every manifest agrees with Cargo.toml
// ---------------------------------------------------------------------------
const REPO_ROOT = path.resolve(ROOT, '..');
const VERSION_FILES = [
  'Cargo.toml',
  'Cargo.lock',
  'server.json',
  'npm/package.json',
  'crates/crossmatrix-mcp/Cargo.toml',
];

// Copies the manifests into a throwaway root so a test can mutate one without
// touching the repository; the script reads them via its public --root option.
function versionRoot(t) {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'praxec-version-'));
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  for (const rel of VERSION_FILES) {
    fs.mkdirSync(path.dirname(path.join(root, rel)), { recursive: true });
    fs.copyFileSync(path.join(REPO_ROOT, rel), path.join(root, rel));
  }
  return root;
}

function runVersionSync(args) {
  return run('sh', [path.join(ROOT, 'check-version-sync.sh'), ...args]);
}

function currentVersion() {
  return JSON.parse(fs.readFileSync(path.join(REPO_ROOT, 'npm/package.json'), 'utf8')).version;
}

test('check-version-sync.sh exits 0 when every manifest agrees', () => {
  const r = runVersionSync([]);
  assert.equal(r.status, 0);
});

test('check-version-sync.sh exits 0 for the release tag matching the manifest version', () => {
  const r = runVersionSync(['--tag', `v${currentVersion()}`]);
  assert.equal(r.status, 0);
});

test('check-version-sync.sh exits 1 when npm/package.json disagrees', (t) => {
  const root = versionRoot(t);
  const pkg = path.join(root, 'npm/package.json');
  const data = JSON.parse(fs.readFileSync(pkg, 'utf8'));
  data.version = '9.9.9';
  fs.writeFileSync(pkg, JSON.stringify(data, null, 2));
  const r = runVersionSync(['--root', root]);
  assert.equal(r.status, 1);
});

test('check-version-sync.sh names npm/package.json when it disagrees', (t) => {
  const root = versionRoot(t);
  const pkg = path.join(root, 'npm/package.json');
  const data = JSON.parse(fs.readFileSync(pkg, 'utf8'));
  data.version = '9.9.9';
  fs.writeFileSync(pkg, JSON.stringify(data, null, 2));
  const r = runVersionSync(['--root', root]);
  assert.match(`${r.stdout}${r.stderr}`, /npm\/package\.json/);
});

test('check-version-sync.sh reports the mismatching file with a ::error:: line', (t) => {
  const root = versionRoot(t);
  const pkg = path.join(root, 'npm/package.json');
  const data = JSON.parse(fs.readFileSync(pkg, 'utf8'));
  data.version = '9.9.9';
  fs.writeFileSync(pkg, JSON.stringify(data, null, 2));
  const r = runVersionSync(['--root', root]);
  assert.match(`${r.stdout}${r.stderr}`, /::error::.*npm\/package\.json/);
});

test('check-version-sync.sh exits 1 when server.json disagrees', (t) => {
  const root = versionRoot(t);
  const server = path.join(root, 'server.json');
  const data = JSON.parse(fs.readFileSync(server, 'utf8'));
  data.version = '9.9.9';
  fs.writeFileSync(server, JSON.stringify(data, null, 2));
  const r = runVersionSync(['--root', root]);
  assert.equal(r.status, 1);
});

test('check-version-sync.sh exits 1 when the server.json OCI identifier tag disagrees', (t) => {
  const root = versionRoot(t);
  const server = path.join(root, 'server.json');
  const data = JSON.parse(fs.readFileSync(server, 'utf8'));
  data.packages[0].identifier = 'ghcr.io/praxec/crossmatrix:9.9.9';
  fs.writeFileSync(server, JSON.stringify(data, null, 2));
  const r = runVersionSync(['--root', root]);
  assert.equal(r.status, 1);
});

test('check-version-sync.sh rejects a version field on a server.json OCI package', (t) => {
  const root = versionRoot(t);
  const server = path.join(root, 'server.json');
  const data = JSON.parse(fs.readFileSync(server, 'utf8'));
  data.packages[0].version = currentVersion();
  fs.writeFileSync(server, JSON.stringify(data, null, 2));
  const r = runVersionSync(['--root', root]);
  assert.match(r.stderr, /must carry the version in the identifier tag/);
});

test('check-version-sync.sh exits 1 for an untagged server.json OCI identifier', (t) => {
  const root = versionRoot(t);
  const server = path.join(root, 'server.json');
  const data = JSON.parse(fs.readFileSync(server, 'utf8'));
  data.packages[0].identifier = 'ghcr.io/praxec/crossmatrix';
  fs.writeFileSync(server, JSON.stringify(data, null, 2));
  const r = runVersionSync(['--root', root]);
  assert.equal(r.status, 1);
});

test('check-version-sync.sh exits 1 when the release tag disagrees', () => {
  const r = runVersionSync(['--tag', 'v9.9.9']);
  assert.equal(r.status, 1);
});

// ---------------------------------------------------------------------------
// npm-publish.sh: the tarball must hold the version being published
// ---------------------------------------------------------------------------
function fakeTarball(t, version) {
  const work = tempDir(t);
  const pkgDir = path.join(work, 'package');
  fs.mkdirSync(pkgDir, { recursive: true });
  fs.writeFileSync(path.join(pkgDir, 'package.json'), JSON.stringify({
    name: '@matthew-cochran/crossmatrix',
    version,
  }));
  const tarball = path.join(work, 'fake.tgz');
  execFileSync('tar', ['-czf', tarball, '-C', work, 'package']);
  return tarball;
}

test('npm-publish.sh --dry-run refuses a tarball whose internal version differs from the argument', (t) => {
  const tarball = fakeTarball(t, '1.2.3');
  const r = run('bash', [path.join(ROOT, 'npm-publish.sh'), tarball, '9.9.9', '--dry-run']);
  assert.equal(r.status, 1);
});

// ---------------------------------------------------------------------------
// npm-release.sh: the maintainer's manual publish of the release tarball.
// Fake `npm`, `gh` and `npx` on PATH keep every test offline.
// ---------------------------------------------------------------------------
const RELEASE_VERSION = '9.8.7';

function writeExe(file, body) {
  fs.writeFileSync(file, `#!/usr/bin/env bash\n${body}\n`, { mode: 0o755 });
}

// Builds a release asset dir holding the launcher tarball (with `files`) and
// its npm-package.sha256, as release.yml's finalize job attaches them.
function releaseAssets(t, { files, corruptDigest = false } = {}) {
  const work = tempDir(t);
  const pkgDir = path.join(work, 'package');
  const entries = files ?? {
    'package.json': JSON.stringify({ name: '@matthew-cochran/crossmatrix', version: RELEASE_VERSION }),
    'LICENSE': 'Apache-2.0',
    'bin/crossmatrix-mcp.js': '',
    'skills/crossmatrix/SKILL.md': '---\nname: crossmatrix\n---\n',
  };
  for (const [rel, content] of Object.entries(entries)) {
    fs.mkdirSync(path.dirname(path.join(pkgDir, rel)), { recursive: true });
    fs.writeFileSync(path.join(pkgDir, rel), content);
  }
  const assets = path.join(work, 'assets');
  fs.mkdirSync(assets);
  const name = `matthew-cochran-crossmatrix-${RELEASE_VERSION}.tgz`;
  // List files only: `npm pack` tarballs carry no directory entries.
  const members = Object.keys(entries).map((rel) => `package/${rel}`);
  execFileSync('tar', ['-czf', path.join(assets, name), '-C', work, ...members]);
  const digest = corruptDigest ? '0'.repeat(64) : sha256(path.join(assets, name));
  fs.writeFileSync(path.join(assets, 'npm-package.sha256'), `${digest}  ${name}\n`);
  return assets;
}

// Fake CLIs. State lives in `state/`: `published` appears once `npm publish`
// ran, and `publish-args` records its arguments.
function fakeTools(t, { loggedIn = true, alreadyPublished = false, draft = 'false', assets }) {
  const dir = tempDir(t);
  const state = path.join(dir, 'state');
  fs.mkdirSync(state);
  if (alreadyPublished) fs.writeFileSync(path.join(state, 'published'), '');
  writeExe(path.join(dir, 'npm'), `
case "$1" in
  whoami) ${loggedIn ? 'echo maintainer' : 'exit 1'} ;;
  view) [ -f "${state}/published" ] && echo "${RELEASE_VERSION}"; exit 0 ;;
  publish) shift; echo "$@" > "${state}/publish-args"; touch "${state}/published" ;;
  *) exit 0 ;;
esac`);
  writeExe(path.join(dir, 'gh'), `
case "$1 $2" in
  "release view") echo "${draft}" ;;
  "release download")
    while [ "$#" -gt 0 ]; do [ "$1" = "--dir" ] && dest="$2"; shift; done
    cp "${assets ?? ''}"/* "$dest"/ ;;
esac`);
  writeExe(path.join(dir, 'npx'), `echo "crossmatrix-mcp ${RELEASE_VERSION}"`);
  return { env: { PATH: `${dir}:${process.env.PATH}`, CROSSMATRIX_RELEASE_WAIT_SECS: '5' }, state };
}

function runRelease(args, env) {
  return run('bash', [path.join(ROOT, 'npm-release.sh'), ...args], { env });
}

test('npm-release.sh --help exits 0', () => {
  assert.equal(runRelease(['--help']).status, 0);
});

test('npm-release.sh rejects an argument that is not a version with exit 64', () => {
  assert.equal(runRelease(['latest']).status, 64);
});

test('npm-release.sh refuses to run when not logged in to npm', (t) => {
  const tools = fakeTools(t, { loggedIn: false });
  assert.equal(runRelease([RELEASE_VERSION], tools.env).status, 1);
});

test('npm-release.sh skips a version that is already on npm', (t) => {
  const tools = fakeTools(t, { alreadyPublished: true });
  assert.match(runRelease([RELEASE_VERSION], tools.env).stdout, /already on npm; nothing to publish/);
});

test('npm-release.sh refuses a GitHub release that is still a draft', (t) => {
  const tools = fakeTools(t, { draft: 'true', assets: releaseAssets(t) });
  assert.match(runRelease([RELEASE_VERSION], tools.env).stderr, /still a draft/);
});

test('npm-release.sh refuses a tarball whose digest does not match', (t) => {
  const tools = fakeTools(t, { assets: releaseAssets(t, { corruptDigest: true }) });
  assert.match(runRelease([RELEASE_VERSION], tools.env).stderr, /digest mismatch/);
});

test('npm-release.sh refuses a tarball holding an unexpected file', (t) => {
  const assets = releaseAssets(t, {
    files: {
      'package.json': JSON.stringify({ version: RELEASE_VERSION }),
      'skills/crossmatrix/SKILL.md': '',
      'test/secret.test.mjs': '',
    },
  });
  const tools = fakeTools(t, { assets });
  assert.match(runRelease([RELEASE_VERSION], tools.env).stderr, /unexpected files/);
});

test('npm-release.sh refuses a tarball without the bundled agent skills', (t) => {
  const assets = releaseAssets(t, {
    files: { 'package.json': JSON.stringify({ version: RELEASE_VERSION }), 'LICENSE': '' },
  });
  const tools = fakeTools(t, { assets });
  assert.match(runRelease([RELEASE_VERSION], tools.env).stderr, /does not bundle the agent skills/);
});

test('npm-release.sh publishes the verified release tarball publicly under latest', (t) => {
  const tools = fakeTools(t, { assets: releaseAssets(t) });
  runRelease([RELEASE_VERSION], tools.env);
  const args = fs.readFileSync(path.join(tools.state, 'publish-args'), 'utf8');
  assert.match(args, new RegExp(`matthew-cochran-crossmatrix-${RELEASE_VERSION}\\.tgz --access public --tag latest\\s*$`));
});

test('npm-release.sh exits 0 after the published launcher reports the release version', (t) => {
  const tools = fakeTools(t, { assets: releaseAssets(t) });
  assert.equal(runRelease([RELEASE_VERSION], tools.env).status, 0);
});
