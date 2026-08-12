# MyProxy Manager

MyProxy Manager is a Tauri v2 desktop application and local AI gateway. It combines account management, model routing, protocol translation, and resilient request handling behind OpenAI-, Anthropic-, and Gemini-compatible APIs.

> Current version: 1.0.1. 中文文档：[README.md](README.md)

## Features

- Multi-account management, health checks, quota monitoring, and rotation
- OpenAI `/v1/chat/completions`, Anthropic `/v1/messages`, and Gemini compatibility
- Model mapping, provider routing, retry, rate limiting, and session binding
- Request logs, token statistics, IP security controls, and proxy pools
- Desktop and headless proxy modes

## Development

Install Node.js 22, stable Rust, and the [Tauri v2 system prerequisites](https://v2.tauri.app/start/prerequisites/).

```bash
npm ci
npm run dev
```

Run the project checks:

```bash
npm run preflight
cd src-tauri
cargo fmt -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test
```

Run the desktop application:

```bash
npm run tauri dev
```

Run the headless proxy:

```bash
cargo run --manifest-path src-tauri/Cargo.toml --bin myproxy-proxy -- --headless
```

## Data and compatibility

- Default data directory: `~/.proxy_manager`
- Default administration endpoint: `http://127.0.0.1:8045`
- MyProxy Manager interoperates with the Antigravity upstream application and its local session formats. Related process names, database keys, and request identifiers in the source are protocol compatibility requirements, not project branding.
- OpenCode synchronization uses the `myproxy-manager` provider and migrates or removes the provider identifier created by earlier releases.

API, architecture, and security documentation is available under [`docs/`](docs/README.md).

## Releases

GitHub Actions builds the release artifacts. Versions in `package.json`, `src-tauri/Cargo.toml`, and `src-tauri/tauri.conf.json` must match.

```bash
npm run check:version
npm run test:release-assets
```

Releases: [darkphenix2025-hue/MyProxy-Manager](https://github.com/darkphenix2025-hue/MyProxy-Manager/releases)

## License and attribution

This project is licensed under [CC-BY-NC-SA-4.0](LICENSE) and retains attribution to the original project and its contributors. The current maintained version is developed by MyProxy Manager contributors.
