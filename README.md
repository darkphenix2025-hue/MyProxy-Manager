# MyProxy Manager

MyProxy Manager 是一个基于 Tauri v2 的本地 AI 网关。它统一管理账号、模型路由和请求重试，并提供兼容 OpenAI、Anthropic 与 Gemini 的 API。

> 当前版本：1.0.1。English documentation: [README_EN.md](README_EN.md)

## 功能

- 多账号管理、健康检查、配额监控和自动轮换
- OpenAI `/v1/chat/completions`、Anthropic `/v1/messages` 与 Gemini 协议兼容
- 模型映射、供应商路由、重试、限流和会话绑定
- 请求日志、令牌统计、IP 安全策略和代理池
- 桌面端与无界面代理模式

## 开发

要求：Node.js 22、Rust stable，以及 [Tauri v2 的系统依赖](https://v2.tauri.app/start/prerequisites/)。

```bash
npm ci
npm run dev
```

常用检查：

```bash
npm run preflight
cd src-tauri
cargo fmt -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test
```

运行桌面应用：

```bash
npm run tauri dev
```

运行无界面代理：

```bash
cargo run --manifest-path src-tauri/Cargo.toml --bin myproxy-proxy -- --headless
```

## 数据与兼容性

- 默认数据目录：`~/.proxy_manager`
- 默认管理地址：`http://127.0.0.1:8045`
- MyProxy Manager 会与 Antigravity 上游应用及其本地会话格式交互；源码中的相关名称、数据库键和请求标识属于协议兼容要求，不代表本项目品牌。
- OpenCode 同步使用 `myproxy-manager` provider，并会迁移或清理由早期版本创建的旧 provider。

API、架构与安全说明位于 [`docs/`](docs/README.md)。

## 发布

发布制品由 GitHub Actions 构建。版本号必须在 `package.json`、`src-tauri/Cargo.toml` 与 `src-tauri/tauri.conf.json` 中保持一致。

```bash
npm run check:version
npm run test:release-assets
```

发布地址：[darkphenix2025-hue/MyProxy-Manager](https://github.com/darkphenix2025-hue/MyProxy-Manager/releases)

## 许可与来源

本项目采用 [CC-BY-NC-SA-4.0](LICENSE) 许可，并保留对原始项目及贡献者的署名。当前维护版本由 MyProxy Manager contributors 继续开发。
