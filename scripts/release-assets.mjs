import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const PLATFORM_CONFIG = Object.freeze({
  'darwin-aarch64': {
    update: {
      directory: 'macos',
      pattern: /\.app\.tar\.gz$/i,
      name: (version) => `MyProxy.Manager_${version}_aarch64.app.tar.gz`,
    },
    installers: [
      {
        directory: 'dmg',
        pattern: /\.dmg$/i,
        name: (version) => `MyProxy.Manager_${version}_aarch64.dmg`,
      },
    ],
  },
  'darwin-x86_64': {
    update: {
      directory: 'macos',
      pattern: /\.app\.tar\.gz$/i,
      name: (version) => `MyProxy.Manager_${version}_x64.app.tar.gz`,
    },
    installers: [
      {
        directory: 'dmg',
        pattern: /\.dmg$/i,
        name: (version) => `MyProxy.Manager_${version}_x64.dmg`,
      },
    ],
  },
  'windows-x86_64': {
    update: {
      directory: 'nsis',
      pattern: /-setup\.exe$/i,
      name: (version) => `MyProxy.Manager_${version}_x64-setup.exe`,
    },
    installers: [
      {
        directory: 'msi',
        pattern: /\.msi$/i,
        name: (version) => `MyProxy.Manager_${version}_x64.msi`,
      },
    ],
  },
  'linux-x86_64': {
    update: {
      directory: 'appimage',
      pattern: /\.AppImage$/i,
      name: (version) => `MyProxy.Manager_${version}_x86_64.AppImage`,
    },
    installers: [
      {
        directory: 'deb',
        pattern: /\.deb$/i,
        name: (version) => `MyProxy.Manager_${version}_amd64.deb`,
      },
      {
        directory: 'rpm',
        pattern: /\.rpm$/i,
        name: (version) => `MyProxy.Manager_${version}_x86_64.rpm`,
      },
    ],
  },
});

const VERSION_PATTERN = /^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?$/;

function assertVersion(version) {
  if (!VERSION_PATTERN.test(version)) {
    throw new Error(`Expected a semantic version without the v prefix, received ${version}`);
  }
}

function assertPlatform(platform) {
  if (!Object.hasOwn(PLATFORM_CONFIG, platform)) {
    throw new Error(`Unsupported release platform ${platform}`);
  }
}

function ensureDirectory(directory) {
  fs.mkdirSync(directory, { recursive: true });
}

function findSingleArtifact(bundleRoot, descriptor) {
  const directory = path.join(bundleRoot, descriptor.directory);
  if (!fs.existsSync(directory)) {
    throw new Error(`Missing bundle directory: ${directory}`);
  }

  const matches = fs
    .readdirSync(directory, { withFileTypes: true })
    .filter((entry) => entry.isFile() && !entry.name.endsWith('.sig') && descriptor.pattern.test(entry.name))
    .map((entry) => path.join(directory, entry.name));

  if (matches.length !== 1) {
    throw new Error(
      `Expected exactly one ${descriptor.directory} artifact matching ${descriptor.pattern}, found ${matches.length}`,
    );
  }

  return matches[0];
}

function copyArtifact(sourcePath, destinationDirectory, destinationName, { requireSignature = false } = {}) {
  ensureDirectory(destinationDirectory);
  const destinationPath = path.join(destinationDirectory, destinationName);
  fs.copyFileSync(sourcePath, destinationPath);

  const sourceSignaturePath = `${sourcePath}.sig`;
  const destinationSignaturePath = `${destinationPath}.sig`;
  let signature;
  if (fs.existsSync(sourceSignaturePath)) {
    signature = fs.readFileSync(sourceSignaturePath, 'utf8').trim();
    if (!signature) {
      throw new Error(`Signature file is empty: ${sourceSignaturePath}`);
    }
    fs.copyFileSync(sourceSignaturePath, destinationSignaturePath);
  } else if (requireSignature) {
    throw new Error(`Missing required signature: ${sourceSignaturePath}`);
  }

  return {
    name: destinationName,
    signature,
  };
}

export function prepareReleaseAssets({ bundleRoot, outputDirectory, platform, version }) {
  assertVersion(version);
  assertPlatform(platform);
  ensureDirectory(outputDirectory);

  const config = PLATFORM_CONFIG[platform];
  const updateSource = findSingleArtifact(bundleRoot, config.update);
  const update = copyArtifact(updateSource, outputDirectory, config.update.name(version), {
    requireSignature: true,
  });

  const installers = config.installers.map((descriptor) => {
    const source = findSingleArtifact(bundleRoot, descriptor);
    return copyArtifact(source, outputDirectory, descriptor.name(version));
  });

  const fragment = {
    version,
    platform,
    update,
    assets: [update, ...installers],
  };
  fs.writeFileSync(
    path.join(outputDirectory, `manifest-${platform}.json`),
    `${JSON.stringify(fragment, null, 2)}\n`,
  );
  return fragment;
}

function collectFiles(directory) {
  if (!fs.existsSync(directory)) return [];
  return fs.readdirSync(directory, { withFileTypes: true }).flatMap((entry) => {
    const entryPath = path.join(directory, entry.name);
    return entry.isDirectory() ? collectFiles(entryPath) : [entryPath];
  });
}

function readManifestFragments(inputDirectory) {
  const fragments = collectFiles(inputDirectory)
    .filter((filePath) => path.basename(filePath).startsWith('manifest-') && filePath.endsWith('.json'))
    .map((filePath) => JSON.parse(fs.readFileSync(filePath, 'utf8')));

  const expectedPlatforms = Object.keys(PLATFORM_CONFIG);
  if (fragments.length !== expectedPlatforms.length) {
    throw new Error(
      `Expected ${expectedPlatforms.length} release manifest fragments, found ${fragments.length}`,
    );
  }

  const seenPlatforms = new Set();
  for (const fragment of fragments) {
    assertVersion(fragment.version);
    assertPlatform(fragment.platform);
    if (seenPlatforms.has(fragment.platform)) {
      throw new Error(`Duplicate release manifest fragment for ${fragment.platform}`);
    }
    seenPlatforms.add(fragment.platform);
  }

  for (const platform of expectedPlatforms) {
    if (!seenPlatforms.has(platform)) {
      throw new Error(`Missing release manifest fragment for ${platform}`);
    }
  }

  return fragments.sort((left, right) => left.platform.localeCompare(right.platform));
}

function copyReleaseAssets(inputDirectory, outputDirectory, fragments) {
  ensureDirectory(outputDirectory);
  const copiedNames = new Set();
  for (const fragment of fragments) {
    for (const asset of fragment.assets) {
      if (copiedNames.has(asset.name)) {
        throw new Error(`Duplicate release asset name: ${asset.name}`);
      }
      const sourcePath = collectFiles(inputDirectory).find((filePath) => path.basename(filePath) === asset.name);
      if (!sourcePath) {
        throw new Error(`Release asset listed by ${fragment.platform} is missing: ${asset.name}`);
      }
      fs.copyFileSync(sourcePath, path.join(outputDirectory, asset.name));
      if (asset.signature) {
        const signaturePath = `${sourcePath}.sig`;
        if (!fs.existsSync(signaturePath)) {
          throw new Error(`Release asset signature is missing: ${asset.name}.sig`);
        }
        fs.copyFileSync(signaturePath, path.join(outputDirectory, `${asset.name}.sig`));
      }
      copiedNames.add(asset.name);
    }
  }
  return copiedNames;
}

export function mergeReleaseAssets({ inputDirectory, outputDirectory, repository, tag, version }) {
  assertVersion(version);
  if (!repository || repository.includes(' ')) throw new Error('A GitHub repository is required');
  if (!/^v\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?$/.test(tag)) {
    throw new Error(`Expected a v-prefixed release tag, received ${tag}`);
  }

  const fragments = readManifestFragments(inputDirectory);
  if (fragments.some((fragment) => fragment.version !== version)) {
    throw new Error('Release manifest fragment version does not match the requested release version');
  }

  const copiedNames = copyReleaseAssets(inputDirectory, outputDirectory, fragments);
  const releaseBaseUrl = `https://github.com/${repository}/releases/download/${tag}`;
  const platforms = Object.fromEntries(
    fragments.map((fragment) => [
      fragment.platform,
      {
        url: `${releaseBaseUrl}/${encodeURIComponent(fragment.update.name)}`,
        signature: fragment.update.signature,
      },
    ]),
  );

  for (const fragment of fragments) {
    if (!fragment.update.signature) {
      throw new Error(`Updater signature is missing for ${fragment.platform}`);
    }
  }

  const latest = {
    version,
    notes: 'See the assets below to download and install this version.',
    pub_date: new Date().toISOString(),
    platforms,
  };
  fs.writeFileSync(path.join(outputDirectory, 'latest.json'), `${JSON.stringify(latest, null, 2)}\n`);
  copiedNames.add('latest.json');
  return latest;
}

function parseArguments(argv) {
  const args = {};
  for (let index = 0; index < argv.length; index += 1) {
    const argument = argv[index];
    if (!argument.startsWith('--')) throw new Error(`Unexpected argument ${argument}`);
    const key = argument.slice(2);
    const value = argv[index + 1];
    if (!value || value.startsWith('--')) throw new Error(`Argument --${key} requires a value`);
    args[key] = value;
    index += 1;
  }
  return args;
}

function main() {
  const [mode, ...rawArguments] = process.argv.slice(2);
  const args = parseArguments(rawArguments);
  if (mode === 'prepare') {
    prepareReleaseAssets({
      bundleRoot: args['bundle-root'],
      outputDirectory: args.output,
      platform: args.platform,
      version: args.version,
    });
    return;
  }
  if (mode === 'merge') {
    mergeReleaseAssets({
      inputDirectory: args.input,
      outputDirectory: args.output,
      repository: args.repository,
      tag: args.tag,
      version: args.version,
    });
    return;
  }
  throw new Error('Usage: release-assets.mjs <prepare|merge> --...');
}

const isMain = process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url);
if (isMain) {
  try {
    main();
  } catch (error) {
    console.error(error instanceof Error ? error.message : error);
    process.exitCode = 1;
  }
}

export { PLATFORM_CONFIG };
