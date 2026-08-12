import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { describe, it } from 'node:test';

import { PLATFORM_CONFIG, mergeReleaseAssets, prepareReleaseAssets } from './release-assets.mjs';

const VERSION = '1.0.1';

function createBundleFixture(rootDirectory, platform, { omitUpdateSignature = false } = {}) {
  const config = PLATFORM_CONFIG[platform];
  const descriptors = [config.update, ...config.installers];
  for (const [index, descriptor] of descriptors.entries()) {
    const directory = path.join(rootDirectory, descriptor.directory);
    fs.mkdirSync(directory, { recursive: true });
    const sourceName =
      platform === 'windows-x86_64' && index === 0
        ? 'test_x64-setup.exe'
        : descriptor.directory === 'macos'
          ? 'test.app.tar.gz'
          : descriptor.directory === 'dmg'
            ? 'test.dmg'
            : `test.${descriptor.directory === 'appimage' ? 'AppImage' : descriptor.directory}`;
    const sourcePath = path.join(directory, sourceName);
    fs.writeFileSync(sourcePath, `${platform}-${descriptor.directory}`);
    if (!(omitUpdateSignature && index === 0)) {
      fs.writeFileSync(`${sourcePath}.sig`, `${platform}-${descriptor.directory}-signature\n`);
    }
  }
}

describe('release asset preparation and manifest generation', () => {
  it('normalizes every supported platform and preserves updater signatures', () => {
    const temporaryDirectory = fs.mkdtempSync(path.join(os.tmpdir(), 'myproxy-release-assets-'));
    const inputDirectory = path.join(temporaryDirectory, 'input');
    const payloadDirectory = path.join(temporaryDirectory, 'payload');
    fs.mkdirSync(inputDirectory);

    for (const platform of Object.keys(PLATFORM_CONFIG)) {
      const bundleDirectory = path.join(temporaryDirectory, `bundle-${platform}`);
      const outputDirectory = path.join(temporaryDirectory, `output-${platform}`);
      createBundleFixture(bundleDirectory, platform);
      const fragment = prepareReleaseAssets({
        bundleRoot: bundleDirectory,
        outputDirectory,
        platform,
        version: VERSION,
      });
      assert.equal(fragment.update.signature, `${platform}-${PLATFORM_CONFIG[platform].update.directory}-signature`);
      for (const fileName of fs.readdirSync(outputDirectory)) {
        fs.copyFileSync(path.join(outputDirectory, fileName), path.join(inputDirectory, fileName));
      }
    }

    const latest = mergeReleaseAssets({
      inputDirectory,
      outputDirectory: payloadDirectory,
      repository: 'darkphenix2025-hue/MyProxy-Manager',
      tag: `v${VERSION}`,
      version: VERSION,
    });

    assert.deepEqual(Object.keys(latest.platforms).sort(), Object.keys(PLATFORM_CONFIG).sort());
    assert.match(latest.platforms['darwin-aarch64'].url, /MyProxy\.Manager_1\.0\.1_aarch64\.app\.tar\.gz$/);
    assert.equal(fs.existsSync(path.join(payloadDirectory, 'latest.json')), true);
    assert.equal(fs.existsSync(path.join(payloadDirectory, 'MyProxy.Manager_1.0.1_x64-setup.exe.sig')), true);
  });

  it('fails when an updater artifact has no signature', () => {
    const temporaryDirectory = fs.mkdtempSync(path.join(os.tmpdir(), 'myproxy-release-assets-'));
    const bundleDirectory = path.join(temporaryDirectory, 'bundle');
    createBundleFixture(bundleDirectory, 'linux-x86_64', { omitUpdateSignature: true });

    assert.throws(
      () =>
        prepareReleaseAssets({
          bundleRoot: bundleDirectory,
          outputDirectory: path.join(temporaryDirectory, 'output'),
          platform: 'linux-x86_64',
          version: VERSION,
        }),
      /Missing required signature/,
    );
  });

  it('rejects a release with an incomplete platform matrix', () => {
    const temporaryDirectory = fs.mkdtempSync(path.join(os.tmpdir(), 'myproxy-release-assets-'));
    const inputDirectory = path.join(temporaryDirectory, 'input');
    const bundleDirectory = path.join(temporaryDirectory, 'bundle');
    createBundleFixture(bundleDirectory, 'linux-x86_64');
    prepareReleaseAssets({
      bundleRoot: bundleDirectory,
      outputDirectory: inputDirectory,
      platform: 'linux-x86_64',
      version: VERSION,
    });

    assert.throws(
      () =>
        mergeReleaseAssets({
          inputDirectory,
          outputDirectory: path.join(temporaryDirectory, 'payload'),
          repository: 'darkphenix2025-hue/MyProxy-Manager',
          tag: `v${VERSION}`,
          version: VERSION,
        }),
      /Expected 4 release manifest fragments, found 1/,
    );
  });
});
