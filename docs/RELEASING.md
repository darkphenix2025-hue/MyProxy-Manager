# Release and auto-update

## One-time repository setup

1. Generate a Tauri updater signing key pair on a trusted machine. Keep the private key and its password outside the repository.
2. Replace `REPLACE_WITH_TAURI_UPDATER_PUBLIC_KEY` in `src-tauri/tauri.conf.json` with the generated public key and commit that public key.
3. Add GitHub Actions secrets `TAURI_SIGNING_PRIVATE_KEY` and `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`.
4. Protect `main`: require pull requests, require the `frontend` and `rust` CI jobs, require branches to be up to date, and prevent bypass/deletion where appropriate.

The private key is only read by the Release workflow. Do not commit it or print it in workflow logs.

## Version and release flow

1. Update the version in `package.json`, `src-tauri/Cargo.toml`, and `src-tauri/tauri.conf.json`.
2. Run `npm ci`, `npm run test:version`, `npm run check:version`, `npm run build`, and the Rust checks documented in `CLAUDE.md`.
3. Open a pull request and wait for both required CI jobs.
4. Merge to `main`.
5. Run the **Release** workflow from `main` and enter the manifest version without `v` (for example `4.1.32`). Alternatively, push a matching `v*` tag.
6. The workflow builds macOS Intel and Apple Silicon, Windows x64, and Linux x64 installers, signs updater artifacts, creates `latest.json`, and opens a draft GitHub Release.
7. Inspect the assets and `latest.json`, download and smoke-test at least one installer, then publish the draft Release.
8. Launch the previous signed build. It checks `latest.json`, verifies the signature, asks for confirmation, downloads and installs the update, and relaunches.

Formal releases are never triggered by an ordinary merge to `main`.
