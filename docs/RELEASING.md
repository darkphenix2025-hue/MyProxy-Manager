# Release and auto-update

## One-time repository setup

1. Generate a Tauri updater signing key pair on a trusted machine. Keep the private key and its password outside the repository.
2. Replace `REPLACE_WITH_TAURI_UPDATER_PUBLIC_KEY` in `src-tauri/tauri.conf.json` with the generated public key and commit that public key.
3. Add GitHub Actions secrets `TAURI_SIGNING_PRIVATE_KEY` and `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`.
4. Protect `main`: require pull requests, require the `frontend`, `rust`, and `tauri-build` CI checks, require branches to be up to date, and prevent bypass/deletion where appropriate.

The private key is only read by the Release workflow. Do not commit it or print it in workflow logs.

## Local CI and Git hooks

The local checks are integrated with native Git hooks rather than a Codex skill.
A skill only runs when Codex is driving the work; a Git hook also runs when you
commit or push from the terminal, an IDE, or another Git client.

`npm ci`/`npm install` enables the hooks automatically through the package
`prepare` script. If dependencies are already installed, enable them once per
checkout explicitly:

```sh
npm run setup:hooks
git config --local --get core.hooksPath  # should print .githooks
```

After that, the normal development flow is:

1. `git commit` runs the fast version and release-asset tests.
2. `git push` runs the full local CI: frontend preflight, Rust format check,
   Clippy correctness checks, Rust tests, and a Tauri debug compile with
   packaging and signing disabled.
3. Only after those checks pass should the branch be opened as a PR.

The same checks can be run manually with `npm run ci:local` (without the Tauri
compile) or `npm run ci:local:tauri` (the complete local gate). The hook scripts
are `.githooks/pre-commit` and `.githooks/pre-push`.

`SKIP_LOCAL_CI=1 git commit` and `SKIP_LOCAL_CI=1 git push` are available as an
explicit emergency bypass, but should not be part of the normal flow. Local
hooks can be bypassed by a user, so they reduce unnecessary remote runs but do
not replace GitHub branch protection or remote status checks.

Release publication is intentionally not attached to `post-commit` or
`post-push`: tagging, signing, and publishing a GitHub Release are external
side effects. Keep that step explicit after the PR is merged and the release
version has been reviewed.

## macOS signing modes

The Tauri updater key above signs update metadata; it is separate from the
signature Gatekeeper checks on the macOS application. Windows and Linux jobs do
not need Apple credentials.

With no Apple secrets configured, the Release workflow uses Tauri's **ad-hoc**
signature (`APPLE_SIGNING_IDENTITY=-`). This is the usable mode for a free Apple
account: it avoids the “damaged” failure that commonly affects downloaded Apple
Silicon apps, but it is not notarized. Users must still choose **Open** once or
allow the app under **System Settings → Privacy & Security**. Tauri documents
this limitation explicitly in [macOS code signing](https://tauri.app/distribute/sign/macos/).

If any Apple secret is present, the workflow requires all six Apple secrets and
switches to Developer ID signing plus notarization. A partially configured set
fails early with the missing secret names instead of publishing a mixed release.

### Optional: paid Developer ID signing and notarization

This requires a paid [Apple Developer Program](https://developer.apple.com/programs/)
membership, a **Developer ID Application** certificate, and an App Store Connect
Team API key. Apple documents the certificate and Tauri's CI setup in the same
[macOS code signing guide](https://tauri.app/distribute/sign/macos/).

### 1. Export the Developer ID certificate

On a Mac that has the certificate and its private key in Keychain Access:

1. Open **Keychain Access → My Certificates** and select **Developer ID Application**.
2. Export the certificate and private key as a `.p12` file. Set a strong export password.
3. Convert the file to one-line base64 without printing it:

   ```sh
   openssl base64 -A -in MyProxyDeveloperID.p12 -out MyProxyDeveloperID.p12.b64
   ```

The workflow imports this certificate into an ephemeral CI keychain and derives
the exact signing identity from it. No `APPLE_SIGNING_IDENTITY` secret is needed.

### 2. Create an App Store Connect API key

In **App Store Connect → Users and Access → Integrations → App Store Connect API**,
open the **Team Keys** tab and create a key with a role that can submit builds
for notarization (Tauri documents **Developer** access as sufficient). Do not
use an **Individual Key**: Apple's API documentation states that individual keys
cannot use `notaryTool`. Download the `.p8` file and record the **Issuer ID** and
**Key ID**; the private key can only be downloaded once. Convert it to one-line
base64:

```sh
openssl base64 -A -in AuthKey_<KEY_ID>.p8 -out AuthKey_<KEY_ID>.p8.b64
```

### 3. Add the repository Actions secrets

Open **Settings → Secrets and variables → Actions → New repository secret** and
add the following names. Paste the contents of the `.b64` files as single-line
values; do not commit either source file.

| Secret | Value |
| --- | --- |
| `APPLE_CERTIFICATE` | Contents of `MyProxyDeveloperID.p12.b64` |
| `APPLE_CERTIFICATE_PASSWORD` | Password used when exporting the `.p12` |
| `KEYCHAIN_PASSWORD` | A new random password used only for the CI keychain |
| `APPLE_API_ISSUER` | App Store Connect Issuer ID |
| `APPLE_API_KEY` | App Store Connect Key ID (not the `.p8` contents) |
| `APPLE_API_PRIVATE_KEY` | Contents of `AuthKey_<KEY_ID>.p8.b64` |

Keep the existing `TAURI_SIGNING_PRIVATE_KEY` and
`TAURI_SIGNING_PRIVATE_KEY_PASSWORD` secrets as well. `KEYCHAIN_PASSWORD` is
not an Apple account password, and it may be generated locally with a password
manager. Never put any of these values in workflow files or logs.

### 4. Run a release

Never overwrite a published tag; when signing or workflow configuration changes,
publish the next version instead:

1. Merge the CI-checked configuration change into `main`.
2. Run **Actions → Release → Run workflow** from `main` and enter the next
   manifest version (for example `1.0.1`), or push a matching `v1.0.1` tag.
3. Inspect the draft release. In ad-hoc mode, macOS may require the one-time
   **Open**/**Privacy & Security** approval described above. In Developer ID
   mode, the DMGs should be notarized; `spctl --assess --type execute` and
   `xcrun stapler validate` can be used for local verification.
4. Publish the draft only after the macOS, Windows, Linux, and `latest.json`
   assets have been smoke-tested.

If you intend to use ad-hoc signing, leave all six Apple secrets absent. If you
intend to upgrade to notarized Developer ID releases, add all six secrets and
rerun the workflow. The updater signing key and macOS application signing are
independent chains.

## Version and release flow

1. Update the version in `package.json`, `src-tauri/Cargo.toml`, and `src-tauri/tauri.conf.json`.
2. Run `npm ci`, `npm run test:version`, `npm run test:release-assets`, `npm run check:version`, `npm run build`, and the Rust checks documented in `CLAUDE.md`.
3. Open a pull request and wait for the required frontend, Rust, and Tauri build checks.
4. Merge to `main`.
5. Run the **Release** workflow from `main` and enter the manifest version without `v` (for example `4.1.32`). Alternatively, push a matching `v*` tag.
6. The workflow builds macOS Intel and Apple Silicon, Windows x64, and Linux x64 installers. Each matrix job normalizes and verifies its signed updater artifact, then a final publish job combines all four outputs, creates `latest.json`, and opens a draft GitHub Release.
7. Inspect the assets and `latest.json`, download and smoke-test at least one installer, then publish the draft Release.
8. Launch the previous signed build. It checks `latest.json`, verifies the signature, asks for confirmation, downloads and installs the update, and relaunches.

Formal releases are never triggered by an ordinary merge to `main`.
