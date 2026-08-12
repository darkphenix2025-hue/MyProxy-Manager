import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { spawnSync } from 'node:child_process';

const repositoryRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const uninstall = process.argv.includes('--uninstall');

const args = uninstall
  ? ['config', '--unset', 'core.hooksPath']
  : ['config', 'core.hooksPath', '.githooks'];
const result = spawnSync('git', args, {
  cwd: repositoryRoot,
  stdio: 'inherit',
});

if (result.error) throw result.error;
if (result.status !== 0 && !uninstall) {
  process.exit(result.status ?? 1);
}

console.log(uninstall ? 'Local Git hooks disabled.' : 'Local Git hooks enabled from .githooks/.');

