# Proxy account pool & auto-disable behavior

## What we wanted
- Keep the proxy “always-on” even when some Google OAuth accounts become invalid.
- Avoid repeatedly attempting to refresh a revoked `refresh_token` (noise + wasted requests).
- Make failures actionable by surfacing account state clearly in the UI.

## What we got
### 1) Disabled accounts are skipped by the proxy pool
Account files can be marked as disabled on disk (`accounts/<id>.json`):
- `disabled: true`
- `disabled_at: <unix_ts>`
- `disabled_reason: <string>`

The proxy token pool loader skips such accounts:
- `TokenManager::load_single_account(...)` in [`src-tauri/src/proxy/token_manager.rs`](../../src-tauri/src/proxy/token_manager.rs)

### 2) Automatic disable on OAuth `invalid_grant`
If an account refresh fails with `invalid_grant` during token refresh, the proxy marks it disabled and removes it from the in-memory pool:
- Refresh/disable logic: `TokenManager::get_token(...)` in [`src-tauri/src/proxy/token_manager.rs`](../../src-tauri/src/proxy/token_manager.rs)
- Persist disable flags to disk: `TokenManager::disable_account(...)` in [`src-tauri/src/proxy/token_manager.rs`](../../src-tauri/src/proxy/token_manager.rs)

This prevents endless rotation attempts against a dead account.

### 3) Batch quota refresh skips disabled accounts
When refreshing quotas for all accounts, disabled ones are skipped immediately:
- `refresh_all_quotas(...)` in [`src-tauri/src/commands/mod.rs`](../../src-tauri/src/commands/mod.rs)

### 4) UI surfaces disabled state and blocks actions
The accounts UI reads `disabled` fields and shows a “Disabled” badge and tooltip, and disables “switch / refresh” controls:
- Account type includes `disabled*` fields: [`src/types/account.ts`](../../src/types/account.ts)
- Card view: [`src/components/accounts/AccountCard.tsx`](../../src/components/accounts/AccountCard.tsx)
- Table row view: [`src/components/accounts/AccountRow.tsx`](../../src/components/accounts/AccountRow.tsx)
- Filters: “Available” excludes disabled accounts: [`src/pages/Accounts.tsx`](../../src/pages/Accounts.tsx)

Translations:
- [`src/locales/en.json`](../../src/locales/en.json)
- [`src/locales/zh.json`](../../src/locales/zh.json)

### 5) API errors avoid leaking user emails
Token refresh failures returned to API clients no longer include account emails:
- Error message construction: `TokenManager::get_token(...)` in [`src-tauri/src/proxy/token_manager.rs`](../../src-tauri/src/proxy/token_manager.rs)
- Proxy error mapping: `handle_messages(...)` in [`src-tauri/src/proxy/handlers/claude.rs`](../../src-tauri/src/proxy/handlers/claude.rs)

## Operational guidance
- If an account becomes disabled due to `invalid_grant`, it usually means the `refresh_token` was revoked or expired.
- Re-authorize the account (or update the stored token) to restore it.

### Legacy Google OAuth client configuration

The legacy Antigravity/Google OAuth client secret is no longer embedded in the binary. OAuth login and refresh require one of these external configurations before the OAuth registry is first used:

- `ANTIGRAVITY_OAUTH_CLIENT_ID` and `ANTIGRAVITY_OAUTH_CLIENT_SECRET` together; or
- `ANTIGRAVITY_OAUTH_CLIENTS` with `key|client_id|client_secret|optional_label` entries separated by `;`.

`ANTIGRAVITY_OAUTH_CLIENT_KEY` optionally selects the active configured entry. If no complete client is configured, legacy Google OAuth is disabled with an explicit error; existing non-OAuth provider connections are unaffected. Never commit these values or include them in diagnostics.

## Validation
1) Ensure at least one account file has `disabled: true`.
2) Start the proxy and verify:
   - The disabled account is not selected for requests.
   - Batch quota refresh logs show “Skipping … (Disabled)”.
   - The UI shows the Disabled badge and blocks actions.
