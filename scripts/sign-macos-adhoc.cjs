const fs = require('node:fs');
const path = require('node:path');
const { spawnSync } = require('node:child_process');

// Tauri's macOS bundler validates nested executables before it signs the app.
// Pre-sign the Rust binaries when using the free ad-hoc identity so Intel
// runners do not fail on an unsigned sidecar. Developer ID builds are left to
// Tauri's normal signing flow.
if (process.platform !== 'darwin' || process.env.APPLE_SIGNING_IDENTITY !== '-') {
  process.exit(0);
}

const targetTriple = process.env.TAURI_TARGET_TRIPLE;
const targetArch = process.env.TAURI_ENV_ARCH || process.arch;
const targetRootCandidates = [
  path.resolve(process.cwd(), 'src-tauri', 'target'),
  path.resolve(process.cwd(), 'target'),
  path.resolve(__dirname, '..', 'src-tauri', 'target'),
];

const targetDirectoryMatches = (name) => {
  if (targetTriple) {
    return name === targetTriple;
  }

  if (!name.endsWith('-apple-darwin')) {
    return false;
  }

  if (targetArch === 'aarch64' || targetArch === 'arm64') {
    return name.startsWith('aarch64-');
  }

  if (targetArch === 'x86_64' || targetArch === 'x64') {
    return name.startsWith('x86_64-');
  }

  return true;
};

const releaseDirectories = [];
for (const targetRoot of targetRootCandidates) {
  if (!fs.existsSync(targetRoot)) {
    continue;
  }

  const targetDirectories = fs
    .readdirSync(targetRoot, { withFileTypes: true })
    .filter((entry) => entry.isDirectory() && targetDirectoryMatches(entry.name))
    .map((entry) => path.join(targetRoot, entry.name, 'release'));

  if (targetDirectories.length > 0) {
    releaseDirectories.push(...targetDirectories);
    break;
  }

  const defaultReleaseDirectory = path.join(targetRoot, 'release');
  if (fs.existsSync(defaultReleaseDirectory)) {
    releaseDirectories.push(defaultReleaseDirectory);
    break;
  }
}

const binaries = ['myproxy_manager', 'antigravity-proxy'];
const binaryPaths = releaseDirectories.flatMap((releaseDirectory) =>
  binaries
    .map((binary) => path.join(releaseDirectory, binary))
    .filter((binaryPath) => fs.existsSync(binaryPath)),
);

if (binaryPaths.length === 0) {
  throw new Error(
    `Ad-hoc signing was requested, but no macOS release binaries were found (target: ${targetTriple || targetArch})`,
  );
}

for (const binaryPath of binaryPaths) {
  const result = spawnSync('codesign', ['--force', '--sign', '-', binaryPath], {
    stdio: 'inherit',
  });

  if (result.error) {
    throw result.error;
  }

  if (result.status !== 0) {
    process.exit(result.status ?? 1);
  }
}
