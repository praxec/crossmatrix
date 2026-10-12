// Verifies the published file list: every repository skill is bundled, and
// nothing outside the declared `files` set leaks into the package.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const HERE = path.dirname(fileURLToPath(import.meta.url));
const NPM_DIR = path.join(HERE, '..');
const SKILLS_DIR = path.join(NPM_DIR, '..', 'skills');

// Build the bundled skills and inspect the resulting tarball once.
const sync = spawnSync(process.execPath, ['lib/sync-skills.js'], { cwd: NPM_DIR, encoding: 'utf8' });
if (sync.status !== 0) throw new Error(`sync-skills failed: ${sync.stderr}`);
const packed = spawnSync('npm', ['pack', '--dry-run', '--json', '--ignore-scripts'], { cwd: NPM_DIR, encoding: 'utf8' });
if (packed.status !== 0) throw new Error(`npm pack failed: ${packed.stderr}`);
const files = JSON.parse(packed.stdout)[0].files.map((f) => f.path);

const skillNames = fs
  .readdirSync(SKILLS_DIR, { withFileTypes: true })
  .filter((entry) => entry.isDirectory())
  .map((entry) => entry.name);

for (const name of skillNames) {
  test(`the published tarball includes skills/${name}/SKILL.md`, () => {
    assert.ok(files.includes(`skills/${name}/SKILL.md`));
  });
}

test('the published tarball contains no file outside the allowed set', () => {
  const allowed = (p) =>
    p === 'package.json' ||
    p === 'README.md' ||
    p === 'LICENSE' ||
    p === 'NOTICE' ||
    p.startsWith('bin/') ||
    p.startsWith('lib/') ||
    p.startsWith('skills/');
  assert.deepEqual(files.filter((p) => !allowed(p)), []);
});
