# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

**Antigravity Tools** — a Tauri v2 desktop application that acts as a local AI gateway. It converts web-based AI sessions (Google/Anthropic) into standardized API endpoints (OpenAI, Anthropic, Gemini formats) with account rotation, protocol translation, and smart retry.

- **Version:** 4.1.31
- **License:** CC-BY-NC-SA-4.0
- **Repo:** https://github.com/lbjlaq/Antigravity-Manager

## Tech Stack

| Layer | Tech |
|---|---|
| Frontend | React 19 + TypeScript, Vite 7, React Router 7, Ant Design 5, Zustand 5, Tailwind CSS 3 + DaisyUI 5, i18next (12 locales) |
| Backend | Rust, Tauri v2, Axum 0.7, SQLite (rusqlite), tokio, reqwest/rquest |
| CI | GitHub Actions (build + lint + test on Ubuntu/Windows/macOS) |

## Directory Structure

```
Antigravity-Manager/
├── src/                          # React frontend
│   ├── main.tsx                  # Entry point
│   ├── App.tsx                   # Router + global state + Tauri events
│   ├── components/               # UI components (layout, accounts, proxy, etc.)
│   ├── pages/                    # 8 page components (Dashboard, Accounts, ApiProxy, Monitor, etc.)
│   ├── stores/                   # Zustand stores (account, config, view, debug)
│   ├── services/                 # API service wrappers
│   ├── utils/request.ts          # HTTP request utility (maps to localhost:8045 proxy)
│   └── locales/                  # 12 i18n translation files
├── src-tauri/                    # Rust backend
│   ├── src/
│   │   ├── lib.rs                # Tauri app setup (windows, tray, commands)
│   │   ├── commands/             # Tauri command handlers (account, config, quota, token)
│   │   ├── modules/              # Business logic (account, oauth, proxy_db, scheduler, tray)
│   │   ├── proxy/                # Core proxy server
│   │   │   ├── server.rs         # Main Axum server (~124KB, all routes)
│   │   │   ├── token_manager.rs  # Token management (~146KB)
│   │   │   ├── mappers/          # Protocol mappers (OpenAI, Claude, Gemini)
│   │   │   ├── handlers/         # Route handlers
│   │   │   ├── middleware/       # Auth, logging, CORS
│   │   │   └── common/           # Client adapters, rate limiters, schemas
│   │   └── models/               # Data models
│   └── tauri.conf.json           # Tauri configuration
└── package.json
```

## Key Entry Points

| File | Purpose |
|---|---|
| `src/main.tsx` | React entry point |
| `src/App.tsx` | Router definition, Zustand stores, Tauri event listeners |
| `src-tauri/src/lib.rs` | Tauri app builder (window, tray, plugins, commands) |
| `src-tauri/src/proxy/server.rs` | Axum proxy server — the core API gateway |

## Essential Commands

### Frontend (NPM)
```bash
npm run dev          # Start Vite dev server (port 1420)
npm run build        # TypeScript check + production build
npm run preview      # Preview production build
npm run tauri        # Run Tauri CLI
npm run tauri:debug  # Run Tauri with RUST_LOG=debug
```

### Rust Backend
```bash
cd src-tauri
cargo fmt -- --check                                    # Check formatting
cargo clippy --all-targets --all-features -- -D warnings # Lint
cargo check                                             # Compilation check
cargo test                                              # Run all tests
cargo test --package antigravity_tools <test_name>       # Run specific test
cargo build --release                                   # Release build
```

### CI Pipeline
Three parallel jobs on push/PR to main:
1. **Build Frontend**: `npx tsc --noEmit` + `npm run build`
2. **Check Rust**: `cargo fmt` + `cargo clippy` + `cargo check` (Ubuntu/Windows/macOS)
3. **Build Tauri**: `npm run tauri build -- --debug` (Ubuntu/Windows/macOS)

## Architecture Notes

### Frontend-to-Backend Communication
The frontend communicates with the Rust backend via HTTP proxy at `http://127.0.0.1:8045`. The `src/utils/request.ts` utility maps Tauri command names to REST endpoints (e.g., `list_accounts` → `GET /api/accounts`). In Tauri mode it can also use `invoke`, but HTTP is the primary path.

### State Management
Five Zustand stores manage frontend state:
- `useAccountStore` — account list and operations
- `useConfigStore` — application configuration
- `useViewStore` — UI view state
- `useDebugConsole` — debug console state
- `networkMonitorStore` — network monitoring

### Core Proxy Architecture
The proxy (`src-tauri/src/proxy/`) is the core value proposition:
- Accepts requests in OpenAI (`/v1/chat/completions`), Anthropic (`/v1/messages`), and Gemini formats
- Handles protocol translation, account rotation, model mapping, rate limiting, retry with backoff, and session binding
- Uses SQLite for persistent storage (accounts, config, security logs, user tokens)

### Tauri System Tray
The app provides a system tray with account switching and refresh controls, defined in `src-tauri/src/modules/tray.rs`.

## Testing

- **Rust tests** are the primary test suite — use `cargo test` in `src-tauri/`
- **No frontend test framework** is installed (no Jest/Vitest, no `*.test.ts`/`*.spec.ts` files)
- Tests cover protocol mappers, security, rate limiting, retry strategies, quota protection, and integration flows

## 文档地图 / Docs

| Path | Description |
|---|---|
| `docs/` | Project documentation root |
| `docs/API_REFERENCE.md` | API 参考文档 |
| `docs/proxy/` | 代理相关技术文档 |
| `docs/zai/` | Z.AI (Google) 相关文档 |
| `docs/testing/` | 测试相关文档 |
| `docs/images/` | 文档附图 |

## Important Conventions

- TypeScript strict mode: `noUnusedLocals`, `noUnusedParameters`, `noFallthroughCasesInSwitch` are all `true`
- No ESLint is configured
- i18n uses i18next with 12 locales (en, zh, zh-TW, ja, tr, vi, pt, ru, ko, ar, es, my)
- All irreversible/destructive actions require explicit user confirmation
