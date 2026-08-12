import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { describe, it } from 'node:test';
import { fileURLToPath } from 'node:url';

const repositoryRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const packageJson = JSON.parse(fs.readFileSync(path.join(repositoryRoot, 'package.json'), 'utf8'));

function readHook(name) {
  return fs.readFileSync(path.join(repositoryRoot, '.githooks', name), 'utf8');
}

describe('local Git hooks', () => {
  it('enables hooks after dependency installation', () => {
    assert.equal(packageJson.scripts.prepare, 'node scripts/setup-git-hooks.mjs');
  });

  it('runs fast checks before commits', () => {
    const hook = readHook('pre-commit');
    assert.match(hook, /npm run test:version/);
    assert.match(hook, /npm run test:release-assets/);
    assert.match(hook, /SKIP_LOCAL_CI=1/);
  });

  it('runs the full local CI and Tauri checks before pushes', () => {
    const hook = readHook('pre-push');
    assert.match(hook, /npm run ci:local:tauri/);
    assert.match(hook, /SKIP_LOCAL_CI=1/);
  });
});
