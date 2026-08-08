import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const SEMVER_TAG = /^v(\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?)$/;

export function normalizeTag(tag) {
  const match = SEMVER_TAG.exec(tag);
  if (!match) throw new Error(`Expected a tag like v1.2.3, received ${tag}`);
  return match[1];
}

export function validateVersions(versions, releaseTag) {
  const errors = [];
  const expected = versions.packageJson;
  if (versions.cargoToml !== expected) {
    errors.push(`src-tauri/Cargo.toml version ${versions.cargoToml} does not match package.json version ${expected}`);
  }
  if (versions.tauriConfig !== expected) {
    errors.push(`src-tauri/tauri.conf.json version ${versions.tauriConfig} does not match package.json version ${expected}`);
  }
  if (releaseTag && normalizeTag(releaseTag) !== expected) {
    errors.push(`Release tag ${releaseTag} does not match manifest version ${expected}`);
  }
  return errors;
}

function readVersions(rootDirectory) {
  const packageJson = JSON.parse(fs.readFileSync(path.join(rootDirectory, 'package.json'), 'utf8'));
  const tauriConfig = JSON.parse(fs.readFileSync(path.join(rootDirectory, 'src-tauri/tauri.conf.json'), 'utf8'));
  const cargoToml = fs.readFileSync(path.join(rootDirectory, 'src-tauri/Cargo.toml'), 'utf8');
  const cargoVersion = cargoToml.match(/^version\s*=\s*"([^"]+)"/m)?.[1];
  if (!cargoVersion) throw new Error('Could not read package version from src-tauri/Cargo.toml');
  return { packageJson: packageJson.version, cargoToml: cargoVersion, tauriConfig: tauriConfig.version };
}

function main() {
  const rootDirectory = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
  const tagIndex = process.argv.indexOf('--tag');
  const releaseTag = tagIndex === -1 ? undefined : process.argv[tagIndex + 1];
  if (tagIndex !== -1 && !releaseTag) throw new Error('--tag requires a value');
  const versions = readVersions(rootDirectory);
  const errors = validateVersions(versions, releaseTag);
  if (errors.length > 0) throw new Error(errors.join('\n'));
  console.log(`Version ${versions.packageJson} is consistent${releaseTag ? ` with ${releaseTag}` : ''}.`);
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try { main(); } catch (error) {
    console.error(error instanceof Error ? error.message : error);
    process.exitCode = 1;
  }
}
