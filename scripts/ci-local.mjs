import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';

const repositoryRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const npmCommand = process.platform === 'win32' ? 'npm.cmd' : 'npm';
const cargoCommand = process.platform === 'win32' ? 'cargo.exe' : 'cargo';
const includeTauri = process.argv.includes('--with-tauri');
const temporaryDataDirectory = fs.mkdtempSync(path.join(os.tmpdir(), 'myproxy-ci-local-'));

function run(command, args, extraEnvironment = {}) {
  const result = spawnSync(command, args, {
    cwd: repositoryRoot,
    env: {
      ...process.env,
      ABV_DATA_DIR: temporaryDataDirectory,
      ...extraEnvironment,
    },
    stdio: 'inherit',
  });

  if (result.error) throw result.error;
  if (result.status !== 0) {
    throw new Error(`${command} ${args.join(' ')} exited with status ${result.status ?? 1}`);
  }
}

function main() {
  console.log('Running local CI checks...');
  run(npmCommand, ['run', 'preflight']);
  run(cargoCommand, ['fmt', '--manifest-path', 'src-tauri/Cargo.toml', '--', '--check']);
  run(cargoCommand, [
    'clippy',
    '--manifest-path',
    'src-tauri/Cargo.toml',
    '--all-targets',
    '--all-features',
    '--',
    '-D',
    'clippy::correctness',
  ]);
  run(cargoCommand, [
    'test',
    '--manifest-path',
    'src-tauri/Cargo.toml',
    '--all-features',
    '--',
    '--test-threads=1',
  ]);

  if (includeTauri) {
    run(npmCommand, [
      'run',
      'tauri',
      'build',
      '--',
      '--debug',
      '--config',
      '{"bundle":{"active":false,"createUpdaterArtifacts":false}}',
    ]);
  }

  console.log(includeTauri ? 'Local CI and Tauri checks passed.' : 'Local CI checks passed.');
}

try {
  main();
} catch (error) {
  console.error(error instanceof Error ? error.message : error);
  process.exitCode = 1;
} finally {
  fs.rmSync(temporaryDataDirectory, { recursive: true, force: true });
}
