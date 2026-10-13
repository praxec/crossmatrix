import { test } from 'node:test';
import assert from 'node:assert/strict';
import { launcher } from './helpers.mjs';

const { resolveTarget } = launcher;

const CASES = [
  ['linux', 'x64', 'crossmatrix-mcp-x86_64-unknown-linux-gnu.tar.gz'],
  ['linux', 'arm64', 'crossmatrix-mcp-aarch64-unknown-linux-gnu.tar.gz'],
  ['darwin', 'x64', 'crossmatrix-mcp-x86_64-apple-darwin.tar.gz'],
  ['darwin', 'arm64', 'crossmatrix-mcp-aarch64-apple-darwin.tar.gz'],
  ['win32', 'x64', 'crossmatrix-mcp-x86_64-pc-windows-msvc.zip'],
  ['win32', 'arm64', 'crossmatrix-mcp-aarch64-pc-windows-msvc.zip'],
];

for (const [platform, arch, asset] of CASES) {
  test(`${platform}/${arch} maps to the release asset ${asset}`, () => {
    assert.equal(resolveTarget(platform, arch).asset, asset);
  });
}

test('windows targets run crossmatrix-mcp.exe from the archive', () => {
  assert.equal(resolveTarget('win32', 'arm64').binary, 'crossmatrix-mcp.exe');
});

test('an unsupported platform is refused with a clear error', () => {
  assert.throws(() => resolveTarget('freebsd', 'x64'), /unsupported platform freebsd\/x64/);
});

test('an unsupported CPU architecture is refused with a clear error', () => {
  assert.throws(() => resolveTarget('linux', 'ia32'), /unsupported platform linux\/ia32/);
});

test('an x64 Node under Rosetta selects the native Apple Silicon asset', () => {
  assert.equal(resolveTarget('darwin', 'x64', { translated: true }).target, 'aarch64-apple-darwin');
});
