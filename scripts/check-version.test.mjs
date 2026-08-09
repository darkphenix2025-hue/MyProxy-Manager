import assert from 'node:assert/strict';
import { describe, it } from 'node:test';

import { normalizeTag, validateVersions } from './check-version.mjs';

describe('release version validation', () => {
  it('accepts matching manifest versions and a matching v-prefixed tag', () => {
    assert.deepEqual(
      validateVersions(
        { packageJson: '1.0.0', cargoToml: '1.0.0', tauriConfig: '1.0.0' },
        'v1.0.0',
      ),
      [],
    );
  });

  it('reports every manifest that differs from package.json', () => {
    assert.deepEqual(
      validateVersions(
        { packageJson: '1.0.0', cargoToml: '0.9.0', tauriConfig: '1.1.0' },
        undefined,
      ),
      [
        'src-tauri/Cargo.toml version 0.9.0 does not match package.json version 1.0.0',
        'src-tauri/tauri.conf.json version 1.1.0 does not match package.json version 1.0.0',
      ],
    );
  });

  it('rejects a release tag that does not match the manifests', () => {
    assert.deepEqual(
      validateVersions(
        { packageJson: '1.0.0', cargoToml: '1.0.0', tauriConfig: '1.0.0' },
        'v1.0.1',
      ),
      ['Release tag v1.0.1 does not match manifest version 1.0.0'],
    );
  });

  it('only accepts semantic v* release tags', () => {
    assert.equal(normalizeTag('v1.0.0'), '1.0.0');
    assert.throws(() => normalizeTag('release-1.0.0'), /Expected a tag like v1.2.3/);
  });
});
