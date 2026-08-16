# Codex authentication

## Current implementation status

Phase 0C connects the provider-neutral Codex OAuth flow to the application
runtime and both management adapters. The account-page UI does not use these
interfaces yet.

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
  validation, token exchange, and secret persistence;
- application ownership of pending flows with `pending`, `completed`, `failed`,
  and `cancelled` status values;
- system-browser launch from the Tauri adapter, with the authorization URL
  returned for manual copy;
- an authenticated HTTPS `userinfo` request for identity, workspace, and plan
  metadata instead of trusting an unverified JWT payload;
- atomic schema v3 metadata persistence in `accounts-v3.json` with `0600`
  permissions on Unix and an inter-process file lock;
- an onboarding journal that reconciles an interrupted Secret Store/metadata
  commit before the next login;
- runtime cancellation that remains effective through identity verification
  and is serialized against the final metadata commit;
- creation of linked Identity, Credential, and ProviderConnection records;
- Tauri commands and HTTP management routes that call the same runtime use
  cases.

Not implemented yet:

- account-page UI;
- schema v2 migration from legacy account files;
- Codex connection selection and request execution in the proxy runtime;
- API key, Codex access token, device-code, and external CLI credential flows.

## Management interfaces

The Tauri adapter exposes these commands:

- `start_codex_login`;
- `get_codex_login_status`;
- `cancel_codex_login`;
- `list_account_connections`.

`start_codex_login` attempts to open the system browser and returns both the
authorization URL and `browser_opened`. A browser-launch failure therefore
keeps the flow active and allows the UI to offer a manual-copy fallback.

The authenticated HTTP admin adapter exposes equivalent use cases:

- `POST /api/accounts/codex/login/start`;
- `GET /api/accounts/codex/login/{sessionId}`;
- `DELETE /api/accounts/codex/login/{sessionId}`;
- `GET /api/connections`.

The HTTP start route doesn't open a browser on the client machine. Its caller
must open the returned authorization URL.

HTTP management routes always require the admin password (or API-key fallback),
even when proxy traffic authentication is disabled. Browser requests must also
come from the same HTTP origin or a recognized Tauri origin. Tauri and HTTP
failures use stable error codes and do not return provider response bodies.

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

Management APIs return a start DTO and a sanitized session status. The
authorization URL is intentionally returned to the initiating client but is
redacted from `Debug` output. Provider error bodies, authorization codes,
verifiers, raw identity subjects, SecretRef keys, and token values must never
enter logs or public DTOs.

The runtime binds to `127.0.0.1:0`, which asks the operating system for an
available loopback port. The generated redirect URI carries the selected port.
This avoids conflicts with Codex CLI or another local login flow.

OpenAI's current authentication documentation describes browser-based ChatGPT
sign-in as the default Codex CLI path and API keys as the other supported local
sign-in mode: <https://learn.chatgpt.com/docs/auth>.

The local `CLIProxyAPI` Codex implementation was used as a behavioral reference
for PKCE parameters and the callback lifecycle. Its listener implementation is
not copied because MyProxy Manager requires an explicit loopback-only binding.

## Next slice

1. Add the Codex-first account-page flow and typed frontend gateway methods.
2. Add connection deletion, reauthentication, and lifecycle operations.
3. Route one Responses request through the new ProviderConnection and refresh
   its token on demand.
4. Add the reversible schema v2 migration adapter.
