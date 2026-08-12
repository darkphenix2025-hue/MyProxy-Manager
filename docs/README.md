# Documentation index

This folder contains developer-focused documentation (architecture, implementation details, and validation steps).

## Architecture

- [`docs/architecture/design-myproxy-manager.md`](architecture/design-myproxy-manager.md) — full repository structure assessment, target modular-monolith design, and phased optimization plan.

## Requirements

- [`docs/requirements/requirements-account-platform.md`](requirements/requirements-account-platform.md) — Codex-first multi-auth account platform requirements, security boundaries, legacy migration, and acceptance criteria.

## Proxy

- [`docs/proxy/auth.md`](proxy/auth.md) — proxy authorization modes, expected client behavior, and implementation pointers.
- [`docs/proxy/accounts.md`](proxy/accounts.md) — account lifecycle in the proxy pool (including auto-disable on `invalid_grant`) and UI behavior.
- [`docs/proxy/codex-auth.md`](proxy/codex-auth.md) — Codex-first authentication implementation status, PKCE/AuthSession security contract, and remaining browser/token-storage work.

## z.ai (GLM) integration

- [`docs/zai/implementation.md`](zai/implementation.md) — end-to-end “what’s implemented” and how to validate it.
- [`docs/zai/mcp.md`](zai/mcp.md) — MCP endpoints exposed by the proxy (Search / Reader / Vision) and upstream behavior.
- [`docs/zai/provider.md`](zai/provider.md) — Anthropic-compatible passthrough provider details and dispatch modes.
- [`docs/zai/vision-mcp.md`](zai/vision-mcp.md) — built-in Vision MCP server protocol and tool implementations.
- [`docs/zai/notes.md`](zai/notes.md) — research notes, constraints, and future follow-ups (budget/usage, additional endpoints).
