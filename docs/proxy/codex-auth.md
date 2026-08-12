# Codex authentication

## Current implementation status

Phase 0B includes the provider-neutral Codex OAuth session and backend browser
flow. This is an internal backend foundation and is **not yet exposed as a
complete user-facing login flow**.

Implemented:

- OAuth authorization URL generation from explicit configuration;
- RFC 7636 S256 PKCE with a cryptographically random verifier;
- independent high-entropy OAuth `state`;
- strict redirect validation for `http://localhost`, `http://127.0.0.1`, or
  `http://[::1]` with the exact `/auth/callback` path;
- five-minute-style configurable session lifetime with exact-boundary expiry;
- session identifier matching on status, cancel, and complete, plus state
  validation, cancellation, replacement, and single-use callback consumption;
- redacted `Debug` implementations for authorization URLs, state, verifier,
  and authorization codes;
- automatic in-memory zeroization for state, verifier, internal authorization
  URL, and consumed authorization code containers;
- public start/status DTOs that do not expose verifier or a separate state
  field;
- a dedicated, single-use callback listener that binds only to an explicit
  loopback address and enforces the exact callback path;
- bounded callback headers, bounded token responses, no redirect following,
  and generic provider errors that never include response bodies;
- authorization-code exchange with the consumed PKCE verifier;
- refresh-token reuse when the provider does not rotate the refresh token;
- per-credential refresh de-duplication with a second expiry check under lock;
- a replaceable `SecretStore` port and a cross-platform system-keyring adapter;
- a `CodexTokenVault` that serializes the complete token set only inside the
  Secret Store boundary and returns a `SecretRef`;
- a composed `CodexLoginService` covering listener creation, callback
  validation, token exchange, and secret persistence.

Not implemented yet:

- Tauri browser launch and ownership of the pending flow across commands;
- verified account identity and workspace extraction;
- Tauri and HTTP account-management endpoints;
- account-page UI and migration from legacy account files.

## Security contract

The callback redirect must remain an HTTP loopback URL. Do not reuse the main
proxy listener because it may bind to a LAN address. A callback with an
incorrect state, empty authorization code, expired session, wrong session
identifier, or replayed session must not produce token-exchange material.

Only `CodexTokenExchangeMaterial` may carry the authorization code and verifier
into the token adapter. It cannot be serialized or logged. Runtime
`CodexTokenSet` also cannot be serialized directly. A private persistence DTO
exists only inside `CodexTokenVault`, and its encoded value is passed directly
to `SecretStore`.

Management APIs may return `CodexAuthStart` and `CodexAuthSessionStatus`. The
authorization URL is intentionally returned to the initiating client but is
redacted from `Debug` output. Provider error bodies, authorization codes,
verifiers, and token values must never enter logs or public DTOs.

OpenAI's current authentication documentation describes browser-based ChatGPT
sign-in as the default Codex CLI path and API keys as the other supported local
sign-in mode: <https://learn.chatgpt.com/docs/auth>.

The local `CLIProxyAPI` Codex implementation was used as a behavioral reference
for PKCE parameters and the callback lifecycle. Its listener implementation is
not copied because MyProxy Manager requires an explicit loopback-only binding.

## Next slice

1. Own pending `CodexLoginFlow` values in the application runtime and expose
   start, status, and cancel through shared Tauri and HTTP use cases.
2. Launch the system browser from the Tauri adapter and keep manual URL copy as
   a fallback.
3. Validate identity and workspace metadata without treating unverified JWT
   claims as authorization facts.
4. Create schema v3 Identity, Credential, and ProviderConnection records from
   the resulting `SecretRef`.
