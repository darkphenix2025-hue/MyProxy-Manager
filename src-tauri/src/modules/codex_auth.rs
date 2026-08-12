use base64::Engine;
use rand::RngCore;
use sha2::{Digest, Sha256};
use std::fmt;

pub const CODEX_AUTH_SESSION_LIFETIME_SECONDS: i64 = 300;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CodexAuthError {
    #[error("invalid Codex OAuth configuration: {0}")]
    InvalidConfiguration(String),
    #[error("redirect URI must be an HTTP loopback callback")]
    InvalidRedirectUri,
    #[error("Codex OAuth state mismatch")]
    StateMismatch,
    #[error("Codex OAuth session has expired")]
    Expired,
    #[error("Codex OAuth session was already consumed")]
    AlreadyConsumed,
    #[error("no active Codex OAuth session")]
    NoActiveSession,
    #[error("Codex OAuth session identifier mismatch")]
    SessionMismatch,
    #[error("authorization code is missing")]
    MissingAuthorizationCode,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodexOAuthConfig {
    pub client_id: String,
    pub authorization_endpoint: String,
    pub token_endpoint: String,
    pub scopes: Vec<String>,
}

impl CodexOAuthConfig {
    fn validate(&self) -> Result<(), CodexAuthError> {
        if self.client_id.trim().is_empty() {
            return Err(CodexAuthError::InvalidConfiguration(
                "client_id is required".to_string(),
            ));
        }
        validate_https_endpoint(&self.authorization_endpoint, "authorization_endpoint")?;
        validate_https_endpoint(&self.token_endpoint, "token_endpoint")?;
        if self.scopes.is_empty() || self.scopes.iter().any(|scope| scope.trim().is_empty()) {
            return Err(CodexAuthError::InvalidConfiguration(
                "at least one non-empty scope is required".to_string(),
            ));
        }
        Ok(())
    }
}

fn validate_https_endpoint(value: &str, name: &str) -> Result<(), CodexAuthError> {
    let parsed = url::Url::parse(value)
        .map_err(|_| CodexAuthError::InvalidConfiguration(format!("{name} must be a valid URL")))?;
    if parsed.scheme() != "https"
        || parsed.host_str().is_none()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.query().is_some()
        || parsed.fragment().is_some()
    {
        return Err(CodexAuthError::InvalidConfiguration(format!(
            "{name} must be a safe HTTPS endpoint without userinfo, query parameters, or fragments"
        )));
    }
    Ok(())
}

pub fn validate_loopback_redirect(value: &str) -> Result<url::Url, CodexAuthError> {
    let parsed = url::Url::parse(value).map_err(|_| CodexAuthError::InvalidRedirectUri)?;
    let is_loopback = match parsed.host() {
        Some(url::Host::Domain(host)) => host.eq_ignore_ascii_case("localhost"),
        Some(url::Host::Ipv4(address)) => address == std::net::Ipv4Addr::LOCALHOST,
        Some(url::Host::Ipv6(address)) => address.is_loopback(),
        None => false,
    };
    if parsed.scheme() != "http"
        || !is_loopback
        || parsed.port().is_none()
        || parsed.path() != "/auth/callback"
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.query().is_some()
        || parsed.fragment().is_some()
    {
        return Err(CodexAuthError::InvalidRedirectUri);
    }
    Ok(parsed)
}

fn pkce_challenge(verifier: &str) -> String {
    let digest = Sha256::digest(verifier.as_bytes());
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest)
}

fn random_url_safe(byte_count: usize) -> String {
    let mut bytes = vec![0u8; byte_count];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

#[derive(PartialEq, Eq)]
pub struct CodexTokenExchangeMaterial {
    authorization_code: zeroize::Zeroizing<String>,
    redirect_uri: String,
    code_verifier: zeroize::Zeroizing<String>,
}

impl CodexTokenExchangeMaterial {
    pub fn authorization_code(&self) -> &str {
        self.authorization_code.as_str()
    }

    pub fn redirect_uri(&self) -> &str {
        &self.redirect_uri
    }

    pub fn code_verifier(&self) -> &str {
        self.code_verifier.as_str()
    }
}

impl fmt::Debug for CodexTokenExchangeMaterial {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CodexTokenExchangeMaterial")
            .field("authorization_code", &"[REDACTED]")
            .field("redirect_uri", &self.redirect_uri)
            .field("code_verifier", &"[REDACTED]")
            .finish()
    }
}

#[derive(PartialEq, Eq)]
struct CodexAuthSession {
    id: uuid::Uuid,
    state: zeroize::Zeroizing<String>,
    pkce_verifier: zeroize::Zeroizing<String>,
    redirect_uri: String,
    authorization_url: zeroize::Zeroizing<String>,
    created_at: i64,
    expires_at: i64,
    consumed: bool,
}

impl CodexAuthSession {
    fn begin(
        config: &CodexOAuthConfig,
        redirect_uri: &str,
        now: i64,
        lifetime_seconds: i64,
    ) -> Result<Self, CodexAuthError> {
        config.validate()?;
        let redirect_uri = validate_loopback_redirect(redirect_uri)?.to_string();
        if lifetime_seconds <= 0 {
            return Err(CodexAuthError::InvalidConfiguration(
                "session lifetime must be positive".to_string(),
            ));
        }

        let state = random_url_safe(32);
        let pkce_verifier = random_url_safe(64);
        let pkce_challenge = pkce_challenge(&pkce_verifier);
        let mut authorization_url =
            url::Url::parse(&config.authorization_endpoint).map_err(|_| {
                CodexAuthError::InvalidConfiguration(
                    "authorization_endpoint must be a valid URL".to_string(),
                )
            })?;
        authorization_url
            .query_pairs_mut()
            .append_pair("client_id", config.client_id.trim())
            .append_pair("response_type", "code")
            .append_pair("redirect_uri", &redirect_uri)
            .append_pair("scope", &config.scopes.join(" "))
            .append_pair("state", &state)
            .append_pair("code_challenge", &pkce_challenge)
            .append_pair("code_challenge_method", "S256")
            .append_pair("prompt", "login")
            .append_pair("id_token_add_organizations", "true")
            .append_pair("codex_cli_simplified_flow", "true");

        Ok(Self {
            id: uuid::Uuid::new_v4(),
            state: state.into(),
            pkce_verifier: pkce_verifier.into(),
            redirect_uri,
            authorization_url: authorization_url.to_string().into(),
            created_at: now,
            expires_at: now.saturating_add(lifetime_seconds),
            consumed: false,
        })
    }

    fn id(&self) -> uuid::Uuid {
        self.id
    }

    #[cfg(test)]
    fn state(&self) -> &str {
        self.state.as_str()
    }

    #[cfg(test)]
    fn pkce_verifier(&self) -> &str {
        self.pkce_verifier.as_str()
    }

    #[cfg(test)]
    fn pkce_challenge(&self) -> String {
        pkce_challenge(&self.pkce_verifier)
    }

    fn authorization_url(&self) -> &str {
        self.authorization_url.as_str()
    }

    fn expires_at(&self) -> i64 {
        self.expires_at
    }

    fn is_consumed(&self) -> bool {
        self.consumed
    }

    fn consume_callback(
        &mut self,
        received_state: &str,
        authorization_code: &str,
        now: i64,
    ) -> Result<CodexTokenExchangeMaterial, CodexAuthError> {
        if self.consumed {
            return Err(CodexAuthError::AlreadyConsumed);
        }
        if now >= self.expires_at {
            return Err(CodexAuthError::Expired);
        }
        if !constant_time_eq(self.state.as_bytes(), received_state.as_bytes()) {
            return Err(CodexAuthError::StateMismatch);
        }
        if authorization_code.trim().is_empty() {
            return Err(CodexAuthError::MissingAuthorizationCode);
        }

        self.consumed = true;
        Ok(CodexTokenExchangeMaterial {
            authorization_code: authorization_code.to_string().into(),
            redirect_uri: self.redirect_uri.clone(),
            code_verifier: self.pkce_verifier.as_str().to_string().into(),
        })
    }
}

impl fmt::Debug for CodexAuthSession {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CodexAuthSession")
            .field("id", &self.id)
            .field("state", &"[REDACTED]")
            .field("pkce_verifier", &"[REDACTED]")
            .field("pkce_challenge", &"[DERIVED]")
            .field("redirect_uri", &self.redirect_uri)
            .field("authorization_url", &"[REDACTED]")
            .field("created_at", &self.created_at)
            .field("expires_at", &self.expires_at)
            .field("consumed", &self.consumed)
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq, serde::Serialize)]
pub struct CodexAuthStart {
    pub session_id: uuid::Uuid,
    pub authorization_url: String,
    pub expires_at: i64,
}

impl fmt::Debug for CodexAuthStart {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CodexAuthStart")
            .field("session_id", &self.session_id)
            .field("authorization_url", &"[REDACTED]")
            .field("expires_at", &self.expires_at)
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct CodexAuthSessionStatus {
    pub session_id: uuid::Uuid,
    pub expires_at: i64,
    pub consumed: bool,
}

#[derive(Default)]
pub struct CodexAuthSessionManager {
    active: parking_lot::Mutex<Option<CodexAuthSession>>,
}

impl CodexAuthSessionManager {
    pub fn start(
        &self,
        config: &CodexOAuthConfig,
        redirect_uri: &str,
    ) -> Result<CodexAuthStart, CodexAuthError> {
        self.start_at(
            config,
            redirect_uri,
            chrono::Utc::now().timestamp(),
            CODEX_AUTH_SESSION_LIFETIME_SECONDS,
        )
    }

    fn start_at(
        &self,
        config: &CodexOAuthConfig,
        redirect_uri: &str,
        now: i64,
        lifetime_seconds: i64,
    ) -> Result<CodexAuthStart, CodexAuthError> {
        let session = CodexAuthSession::begin(config, redirect_uri, now, lifetime_seconds)?;
        let start = CodexAuthStart {
            session_id: session.id(),
            authorization_url: session.authorization_url().to_string(),
            expires_at: session.expires_at(),
        };
        *self.active.lock() = Some(session);
        Ok(start)
    }

    pub fn status(&self, session_id: uuid::Uuid) -> Result<CodexAuthSessionStatus, CodexAuthError> {
        self.status_at(session_id, chrono::Utc::now().timestamp())
    }

    fn status_at(
        &self,
        session_id: uuid::Uuid,
        now: i64,
    ) -> Result<CodexAuthSessionStatus, CodexAuthError> {
        let mut guard = self.active.lock();
        let session = guard.as_ref().ok_or(CodexAuthError::NoActiveSession)?;
        if session.id() != session_id {
            return Err(CodexAuthError::SessionMismatch);
        }
        if now >= session.expires_at() {
            guard.take();
            return Err(CodexAuthError::Expired);
        }
        Ok(CodexAuthSessionStatus {
            session_id: session.id(),
            expires_at: session.expires_at(),
            consumed: session.is_consumed(),
        })
    }

    pub fn complete(
        &self,
        session_id: uuid::Uuid,
        state: &str,
        authorization_code: &str,
    ) -> Result<CodexTokenExchangeMaterial, CodexAuthError> {
        self.complete_at(
            session_id,
            state,
            authorization_code,
            chrono::Utc::now().timestamp(),
        )
    }

    fn complete_at(
        &self,
        session_id: uuid::Uuid,
        state: &str,
        authorization_code: &str,
        now: i64,
    ) -> Result<CodexTokenExchangeMaterial, CodexAuthError> {
        let mut guard = self.active.lock();
        let session = guard.as_mut().ok_or(CodexAuthError::NoActiveSession)?;
        if session.id() != session_id {
            return Err(CodexAuthError::SessionMismatch);
        }
        let result = session.consume_callback(state, authorization_code, now);
        if result.is_ok() || matches!(result, Err(CodexAuthError::Expired)) {
            guard.take();
        }
        result
    }

    pub fn cancel(&self, session_id: uuid::Uuid) -> Result<(), CodexAuthError> {
        let mut guard = self.active.lock();
        let session = guard.as_ref().ok_or(CodexAuthError::NoActiveSession)?;
        if session.id() != session_id {
            return Err(CodexAuthError::SessionMismatch);
        }
        guard.take();
        Ok(())
    }

    #[cfg(test)]
    fn state_for_callback_tests(&self) -> Option<String> {
        self.active
            .lock()
            .as_ref()
            .map(|session| session.state.as_str().to_string())
    }
}

impl fmt::Debug for CodexAuthSessionManager {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let has_active_session = self.active.lock().is_some();
        formatter
            .debug_struct("CodexAuthSessionManager")
            .field("has_active_session", &has_active_session)
            .finish()
    }
}

fn constant_time_eq(expected: &[u8], actual: &[u8]) -> bool {
    let mut difference = expected.len() ^ actual.len();
    let max_len = expected.len().max(actual.len());
    for index in 0..max_len {
        let left = expected.get(index).copied().unwrap_or(0);
        let right = actual.get(index).copied().unwrap_or(0);
        difference |= usize::from(left ^ right);
    }
    difference == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> CodexOAuthConfig {
        CodexOAuthConfig {
            client_id: "public-client-id".to_string(),
            authorization_endpoint: "https://auth.openai.com/oauth/authorize".to_string(),
            token_endpoint: "https://auth.openai.com/oauth/token".to_string(),
            scopes: vec![
                "openid".to_string(),
                "email".to_string(),
                "profile".to_string(),
                "offline_access".to_string(),
            ],
        }
    }

    #[test]
    fn pkce_matches_rfc_7636_s256_vector() {
        let challenge = pkce_challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk");
        assert_eq!(challenge, "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM");
    }

    #[test]
    fn generated_pkce_and_state_are_high_entropy_and_url_safe() {
        let session =
            CodexAuthSession::begin(&config(), "http://127.0.0.1:1455/auth/callback", 1_000, 300)
                .expect("valid loopback session");

        assert!((43..=128).contains(&session.pkce_verifier().len()));
        assert!(session
            .pkce_verifier()
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '~')));
        assert!(session.state().len() >= 43);
        assert_ne!(session.state(), session.pkce_verifier());
    }

    #[test]
    fn authorization_url_contains_pkce_and_codex_parameters() {
        let session =
            CodexAuthSession::begin(&config(), "http://localhost:1455/auth/callback", 1_000, 300)
                .unwrap();
        let url = url::Url::parse(session.authorization_url()).unwrap();
        let params = url
            .query_pairs()
            .collect::<std::collections::HashMap<_, _>>();

        assert_eq!(params["client_id"], "public-client-id");
        assert_eq!(params["response_type"], "code");
        assert_eq!(params["code_challenge_method"], "S256");
        assert_eq!(
            params["redirect_uri"],
            "http://localhost:1455/auth/callback"
        );
        assert_eq!(params["scope"], "openid email profile offline_access");
        assert_eq!(params["codex_cli_simplified_flow"], "true");
        assert_eq!(params["id_token_add_organizations"], "true");
        assert_eq!(params["state"], session.state());
        assert_eq!(params["code_challenge"], session.pkce_challenge());
    }

    #[test]
    fn callback_redirect_must_be_http_loopback_and_exact_path() {
        for accepted in [
            "http://localhost:1455/auth/callback",
            "http://127.0.0.1:1455/auth/callback",
            "http://[::1]:1455/auth/callback",
        ] {
            assert!(validate_loopback_redirect(accepted).is_ok(), "{accepted}");
        }

        for rejected in [
            "https://localhost:1455/auth/callback",
            "http://0.0.0.0:1455/auth/callback",
            "http://192.168.1.10:1455/auth/callback",
            "http://127.0.0.2:1455/auth/callback",
            "http://localhost:1455/other",
            "http://user@localhost:1455/auth/callback",
            "http://localhost:1455/auth/callback#fragment",
        ] {
            assert!(validate_loopback_redirect(rejected).is_err(), "{rejected}");
        }
    }

    #[test]
    fn oauth_endpoints_reject_userinfo_fragments_and_reserved_query_keys() {
        for endpoint in [
            "https://user:secret@auth.openai.com/oauth/authorize",
            "https://auth.openai.com/oauth/authorize#fragment",
            "https://auth.openai.com/oauth/authorize?state=attacker",
            "https://auth.openai.com/oauth/authorize?code_challenge=attacker",
            "https://auth.openai.com/oauth/authorize?redirect_uri=https://evil.example",
        ] {
            let mut invalid = config();
            invalid.authorization_endpoint = endpoint.to_string();
            assert!(
                CodexAuthSession::begin(
                    &invalid,
                    "http://127.0.0.1:1455/auth/callback",
                    1_000,
                    300
                )
                .is_err(),
                "{endpoint}"
            );
        }
    }

    #[test]
    fn mismatched_state_does_not_consume_session() {
        let mut session =
            CodexAuthSession::begin(&config(), "http://127.0.0.1:1455/auth/callback", 1_000, 300)
                .unwrap();

        assert_eq!(
            session.consume_callback("wrong-state", "code", 1_001),
            Err(CodexAuthError::StateMismatch)
        );
        assert!(!session.is_consumed());
    }

    #[test]
    fn callback_is_single_use_and_returns_token_exchange_material() {
        let mut session =
            CodexAuthSession::begin(&config(), "http://127.0.0.1:1455/auth/callback", 1_000, 300)
                .unwrap();
        let state = session.state().to_string();

        let material = session
            .consume_callback(&state, "auth-code", 1_001)
            .unwrap();
        assert_eq!(material.authorization_code(), "auth-code");
        assert_eq!(
            material.redirect_uri(),
            "http://127.0.0.1:1455/auth/callback"
        );
        assert_eq!(material.code_verifier(), session.pkce_verifier());
        assert!(session.is_consumed());
        assert_eq!(
            session.consume_callback(&state, "second-code", 1_002),
            Err(CodexAuthError::AlreadyConsumed)
        );
    }

    #[test]
    fn expired_session_cannot_be_consumed() {
        let mut session =
            CodexAuthSession::begin(&config(), "http://127.0.0.1:1455/auth/callback", 1_000, 300)
                .unwrap();
        let state = session.state().to_string();

        assert_eq!(
            session.consume_callback(&state, "code", 1_300),
            Err(CodexAuthError::Expired)
        );
        assert!(!session.is_consumed());
    }

    #[test]
    fn missing_code_does_not_consume_session() {
        let mut session =
            CodexAuthSession::begin(&config(), "http://127.0.0.1:1455/auth/callback", 1_000, 300)
                .unwrap();
        let state = session.state().to_string();

        assert_eq!(
            session.consume_callback(&state, "   ", 1_001),
            Err(CodexAuthError::MissingAuthorizationCode)
        );
        assert!(!session.is_consumed());
    }

    #[test]
    fn debug_output_redacts_state_and_verifier() {
        let session =
            CodexAuthSession::begin(&config(), "http://127.0.0.1:1455/auth/callback", 1_000, 300)
                .unwrap();
        let state = session.state().to_string();
        let verifier = session.pkce_verifier().to_string();
        let rendered = format!("{session:?}");

        assert!(!rendered.contains(&state));
        assert!(!rendered.contains(&verifier));
        assert!(rendered.contains("[REDACTED]"));
    }

    #[test]
    fn token_exchange_material_debug_redacts_code_and_verifier() {
        let material = CodexTokenExchangeMaterial {
            authorization_code: "authorization-canary".to_string().into(),
            redirect_uri: "http://127.0.0.1:1455/auth/callback".to_string(),
            code_verifier: "verifier-canary".to_string().into(),
        };
        let rendered = format!("{material:?}");

        assert!(!rendered.contains("authorization-canary"));
        assert!(!rendered.contains("verifier-canary"));
        assert!(rendered.contains("[REDACTED]"));
    }

    #[test]
    fn manager_start_dto_does_not_expose_verifier_or_state_field() {
        let manager = CodexAuthSessionManager::default();
        let start = manager
            .start_at(&config(), "http://127.0.0.1:1455/auth/callback", 1_000, 300)
            .unwrap();
        let encoded = serde_json::to_string(&start).unwrap();

        assert!(encoded.contains("authorization_url"));
        assert!(!encoded.contains("code_verifier"));
        assert!(!encoded.contains("pkce_verifier"));
        assert!(!encoded.contains("\"state\":"));
        assert_eq!(
            manager
                .status_at(start.session_id, 1_001)
                .unwrap()
                .session_id,
            start.session_id
        );
    }

    #[test]
    fn start_dto_debug_redacts_authorization_url() {
        let manager = CodexAuthSessionManager::default();
        let start = manager
            .start_at(&config(), "http://127.0.0.1:1455/auth/callback", 1_000, 300)
            .unwrap();
        let state = url::Url::parse(&start.authorization_url)
            .unwrap()
            .query_pairs()
            .find(|(key, _)| key == "state")
            .unwrap()
            .1
            .into_owned();
        let rendered = format!("{start:?}");

        assert!(!rendered.contains(&state));
        assert!(!rendered.contains("auth.openai.com"));
        assert!(rendered.contains("[REDACTED]"));
    }

    #[test]
    fn manager_discards_expired_session_after_status_check() {
        let manager = CodexAuthSessionManager::default();
        let start = manager
            .start_at(&config(), "http://127.0.0.1:1455/auth/callback", 1_000, 300)
            .unwrap();
        assert_eq!(
            manager.status_at(start.session_id, 1_300),
            Err(CodexAuthError::Expired)
        );
        assert_eq!(
            manager.status_at(start.session_id, 1_301),
            Err(CodexAuthError::NoActiveSession)
        );
    }

    #[test]
    fn manager_replaces_previous_pending_session() {
        let manager = CodexAuthSessionManager::default();
        let first = manager
            .start_at(&config(), "http://127.0.0.1:1455/auth/callback", 1_000, 300)
            .unwrap();
        let second = manager
            .start_at(&config(), "http://127.0.0.1:1455/auth/callback", 1_001, 300)
            .unwrap();

        assert_ne!(first.session_id, second.session_id);
        assert_eq!(
            manager.complete_at(first.session_id, "state", "code", 1_002),
            Err(CodexAuthError::SessionMismatch)
        );
        assert_eq!(
            manager.status_at(first.session_id, 1_002),
            Err(CodexAuthError::SessionMismatch)
        );
        assert_eq!(
            manager
                .status_at(second.session_id, 1_002)
                .unwrap()
                .session_id,
            second.session_id
        );
    }

    #[test]
    fn manager_keeps_session_after_state_mismatch_and_removes_it_after_success() {
        let manager = CodexAuthSessionManager::default();
        let start = manager
            .start_at(&config(), "http://127.0.0.1:1455/auth/callback", 1_000, 300)
            .unwrap();
        let state = manager.state_for_callback_tests().unwrap();

        assert_eq!(
            manager.complete_at(start.session_id, "wrong", "code", 1_001),
            Err(CodexAuthError::StateMismatch)
        );
        assert!(manager.status_at(start.session_id, 1_001).is_ok());
        assert!(manager
            .complete_at(start.session_id, &state, "code", 1_002)
            .is_ok());
        assert_eq!(
            manager.status_at(start.session_id, 1_003),
            Err(CodexAuthError::NoActiveSession)
        );
    }

    #[test]
    fn manager_cancel_requires_matching_session_id() {
        let manager = CodexAuthSessionManager::default();
        let start = manager
            .start_at(&config(), "http://127.0.0.1:1455/auth/callback", 1_000, 300)
            .unwrap();

        assert_eq!(
            manager.cancel(uuid::Uuid::new_v4()),
            Err(CodexAuthError::SessionMismatch)
        );
        manager.cancel(start.session_id).unwrap();
        assert_eq!(
            manager.status_at(start.session_id, 1_001),
            Err(CodexAuthError::NoActiveSession)
        );
    }
}
