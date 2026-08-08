import assert from 'node:assert/strict';
import { describe, it } from 'node:test';

import { normalizeTag, validateVersions } from './check-version.mjs';

describe('release version validation', () => {
  it('accepts matching manifest versions and a matching v-prefixed tag', () => {
    assert.deepEqual(
      validateVersions(
        { packageJson: '4.1.31', cargoToml: '4.1.31', tauriConfig: '4.1.31' },
        'v4.1.31',
      ),
      [],
    );
  });

  it('reports every manifest that differs from package.json', () => {
    assert.deepEqual(
      validateVersions(
        { packageJson: '4.1.31', cargoToml: '4.1.30', tauriConfig: '4.2.0' },
        undefined,
      ),
      [
        'src-tauri/Cargo.toml version 4.1.30 does not match package.json version 4.1.31',
        'src-tauri/tauri.conf.json version 4.2.0 does not match package.json version 4.1.31',
      ],
    );
  });

  it('rejects a release tag that does not match the manifests', () => {
    assert.deepEqual(
      validateVersions(
        { packageJson: '4.1.31', cargoToml: '4.1.31', tauriConfig: '4.1.31' },
        'v4.1.32',
      ),
      ['Release tag v4.1.32 does not match manifest version 4.1.31'],
    );
  });

  it('only accepts semantic v* release tags', () => {
    assert.equal(normalizeTag('v4.1.31'), '4.1.31');
    assert.throws(() => normalizeTag('release-4.1.31'), /Expected a tag like v1.2.3/);
  });
});
