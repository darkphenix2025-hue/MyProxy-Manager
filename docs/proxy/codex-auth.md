# Codex authentication

## Current implementation status

Phase 0A now includes the provider-neutral Codex OAuth session core in `src-tauri/src/modules/codex_auth.rs`. This is an internal foundation and is **not yet a complete user-facing login flow**.

Implemented:

- OAuth authorization URL generation from explicit configuration;
- RFC 7636 S256 PKCE with a cryptographically random verifier;
- independent high-entropy OAuth `state`;
- strict redirect validation for `http://localhost`, `http://127.0.0.1`, or `http://[::1]` with the exact `/auth/callback` path;
- five-minute-style configurable session lifetime with exact-boundary expiry;
- session identifier matching on status/cancel/complete, state validation, cancellation, replacement, and single-use callback consumption;
- redacted `Debug` implementations for authorization URLs, state, verifier, and authorization codes;
- automatic in-memory zeroization for state, verifier, internal authorization URL, and consumed authorization code containers;
- public start/status DTOs that do not expose verifier or a separate state field.

Not implemented yet:

- binding and shutting down the dedicated loopback callback listener;
- browser launch and callback HTML;
- authorization-code token exchange and refresh de-duplication;
- token response parsing and account identity extraction;
- OS keyring/encrypted Secret Store persistence;
- Tauri and HTTP account-management endpoints;
- account-page UI and migration from legacy account files.

## Security contract

The callback redirect must remain an HTTP loopback URL. Do not reuse the main proxy listener because it may bind to a LAN address. A callback with an incorrect state, empty authorization code, expired session, wrong session identifier, or replayed session must not produce token-exchange material.

Only `CodexTokenExchangeMaterial` may carry the authorization code and verifier into the future token adapter. It must not be serialized or logged. Management APIs may return `CodexAuthStart` and `CodexAuthSessionStatus`; the authorization URL is intentionally returned to the initiating client but redacted from `Debug` output.

OpenAI's current authentication documentation describes browser-based ChatGPT sign-in as the default Codex CLI path and API keys as the other supported local sign-in mode: <https://learn.chatgpt.com/docs/auth>.

The local `CLIProxyAPI` Codex implementation was used as a behavioral reference for PKCE parameters and the callback lifecycle. Its listener implementation is not copied because MyProxy Manager requires an explicit loopback-only binding.

## Next slice

1. Bind a dedicated listener to a validated loopback redirect and add timeout/cancel/port-conflict integration tests.
2. Exchange the code using the consumed verifier and sanitize every non-success response.
3. Persist the complete token set through a Secret Store port and retain only a `SecretRef` in schema v3.
4. Expose start/status/cancel operations through the account application service.
