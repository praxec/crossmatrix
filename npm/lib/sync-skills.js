'use strict';
// Copies the repository's ../skills tree into npm/skills/ so the bundled
// skills ship inside the published package. Runs on `prepack`; the copy is a
// build product and is git-ignored.
//
// Only SKILL.md and any *.md/*.json/*.toml files beside them are copied, and
// symlinks are never followed (a symlinked directory is skipped rather than
// walked). Stale copies are removed first so a deleted skill does not linger.

const fs = require('node:fs');
const path = require('node:path');

const SRC = path.resolve(__dirname, '..', '..', 'skills');
const DEST = path.resolve(__dirname, '..', 'skills');

function isCopyable(name) {
  return /\.(md|json|toml)$/i.test(name);
}

function copyTree(rel, copied) {
  const abs = path.join(SRC, rel);
  for (const entry of fs.readdirSync(abs, { withFileTypes: true })) {
    const entryRel = rel ? path.join(rel, entry.name) : entry.name;
    const entryAbs = path.join(SRC, entryRel);
    const st = fs.lstatSync(entryAbs);
    if (st.isSymbolicLink()) continue; // never follow links
    if (st.isDirectory()) {
      copyTree(entryRel, copied);
      continue;
    }
    if (!st.isFile() || !isCopyable(entry.name)) continue;
    const destFile = path.join(DEST, entryRel);
    fs.mkdirSync(path.dirname(destFile), { recursive: true });
    fs.copyFileSync(entryAbs, destFile);
    copied.push(entryRel);
  }
}

// Rebuilds npm/skills/ from ../skills. Returns the relative paths copied.
function sync() {
  fs.rmSync(DEST, { recursive: true, force: true });
  fs.mkdirSync(DEST, { recursive: true });
  const copied = [];
  if (fs.existsSync(SRC)) copyTree('', copied);
  return copied;
}

if (require.main === module) sync();

module.exports = { sync };
