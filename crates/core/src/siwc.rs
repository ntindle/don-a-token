//! Sign in with ChatGPT (SIWC) token-sharing flow for open-source clients.
//!
//! Source: <https://developers.openai.com/siwc/token-sharing-open-source>
//! Full export: <https://developers.openai.com/siwc/llms-full.txt>
//!
//! This module holds the flow's constants and pure helpers. Network I/O
//! (loopback listener, token exchange, JWKS validation) lives in the Tauri
//! shell and is the subject of the auth spike.

use base64::Engine as _;
use rand::RngCore;
use sha2::{Digest, Sha256};
use url::Url;

use crate::CoreError;

/// First-time registration entrypoint. Never save or send this as an issued id.
pub const DYNAMIC_CLIENT_ID: &str = "dynamic_agent_client";

/// Stable app name sent as `agent_name_hint` on initial registration only.
/// Must match Codex app-server `clientInfo.name`.
pub const AGENT_NAME: &str = "don-a-token";

pub const AUTHORIZE_URL: &str = "https://auth.openai.com/api/accounts/authorize";
pub const TOKEN_URL: &str = "https://auth.openai.com/api/accounts/oauth/token";
pub const OIDC_DISCOVERY_URL: &str = "https://auth.openai.com/.well-known/openid-configuration";
pub const MODELS_URL: &str = "https://api.openai.com/v1/models";
pub const RESPONSES_URL: &str = "https://api.openai.com/v1/responses";
pub const TOKEN_ISSUER: &str = "https://auth.openai.com";
pub const TOKEN_AUDIENCE: &str = "https://api.openai.com/v1";

/// Resource indicator sent on authorize and token requests.
pub const RESOURCE: &str = "https://api.openai.com/v1";

/// Scope that gates ChatGPT plan usage. A valid ID token alone is not enough.
pub const PLAN_SCOPE: &str = "chatgpt.tokens.use.direct";

/// Full requested scope set, in the docs' order.
pub const SCOPES: &[&str] = &[
    "openid",
    "profile",
    "email",
    "offline_access",
    "resource.invoke",
    PLAN_SCOPE,
];

/// Loopback callback host. Never `localhost`.
pub const CALLBACK_HOST: &str = "127.0.0.1";

/// Loopback callback path. Scheme, host, and path are fixed; only the port
/// may vary between attempts.
pub const CALLBACK_PATH: &str = "/auth/callback";

/// Space-joined scope string for the authorize request.
pub fn scope_string() -> String {
    SCOPES.join(" ")
}

/// Build a loopback redirect URI for the given port.
pub fn redirect_uri(port: u16) -> String {
    format!("http://{CALLBACK_HOST}:{port}{CALLBACK_PATH}")
}

pub fn new_state() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}

pub fn new_nonce() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}

/// 43-char PKCE verifier: 32 random bytes, base64url-encoded without padding.
pub fn new_code_verifier() -> String {
    let mut bytes = [0u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// S256 code challenge for a verifier.
pub fn code_challenge(verifier: &str) -> String {
    let digest = Sha256::digest(verifier.as_bytes());
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest)
}

/// Parameters for one authorization attempt. Generate fresh `state`, `nonce`,
/// and PKCE values per attempt and keep them until the callback completes.
pub struct AuthorizeParams {
    /// Issued client id, or [`DYNAMIC_CLIENT_ID`] for first registration.
    pub client_id: String,
    /// [`AGENT_NAME`] on initial registration; `None` on reauthorization.
    pub agent_name_hint: Option<String>,
    pub host_id: String,
    pub redirect_uri: String,
    pub state: String,
    pub nonce: String,
    pub code_challenge: String,
    /// Retained ID token from the previous sign-in (reauth only).
    pub id_token_hint: Option<String>,
    /// Saved email from the validated ID token (reauth only).
    pub login_hint: Option<String>,
}

/// Build the system-browser authorize URL. Values are URL-encoded.
pub fn build_authorize_url(p: &AuthorizeParams) -> Result<Url, CoreError> {
    let mut url = Url::parse(AUTHORIZE_URL)?;
    {
        let mut q = url.query_pairs_mut();
        q.append_pair("client_id", &p.client_id);
        if let Some(hint) = &p.agent_name_hint {
            q.append_pair("agent_name_hint", hint);
        }
        q.append_pair("ext_agent_host_id", &p.host_id);
        if let Some(hint) = &p.id_token_hint {
            q.append_pair("id_token_hint", hint);
        }
        if let Some(hint) = &p.login_hint {
            q.append_pair("login_hint", hint);
        }
        q.append_pair("response_type", "code");
        q.append_pair("redirect_uri", &p.redirect_uri);
        q.append_pair("scope", &scope_string());
        q.append_pair("resource", RESOURCE);
        q.append_pair("state", &p.state);
        q.append_pair("nonce", &p.nonce);
        q.append_pair("code_challenge_method", "S256");
        q.append_pair("code_challenge", &p.code_challenge);
    }
    Ok(url)
}

/// Parsed loopback callback query.
#[derive(Debug, PartialEq, Eq)]
pub struct CallbackResult {
    pub code: Option<String>,
    pub state: Option<String>,
    /// Issued client id. Present on new registration; may be omitted on
    /// reauthorization (retain the pending id then).
    pub client_id: Option<String>,
    pub scope: Option<String>,
    pub error: Option<String>,
}

/// Parse the query string of the loopback callback request.
pub fn parse_callback(query: &str) -> Result<CallbackResult, CoreError> {
    let mut out = CallbackResult {
        code: None,
        state: None,
        client_id: None,
        scope: None,
        error: None,
    };
    for (k, v) in url::form_urlencoded::parse(query.as_bytes()) {
        match k.as_ref() {
            "code" => out.code = Some(v.into_owned()),
            "state" => out.state = Some(v.into_owned()),
            "client_id" => out.client_id = Some(v.into_owned()),
            "scope" => out.scope = Some(v.into_owned()),
            "error" => out.error = Some(v.into_owned()),
            _ => {}
        }
    }
    if out.error.is_none() && out.code.is_none() {
        return Err(CoreError::InvalidCallback(
            "neither code nor error present".to_string(),
        ));
    }
    Ok(out)
}

/// Resolve which client id the token exchange must use.
///
/// - New registration: the callback must supply the issued id, and it must
///   not be the dynamic entrypoint.
/// - Reauthorization: keep the pending issued id. If the callback supplies
///   a *different* id, reject rather than replace the registration.
pub fn resolve_client_id(
    pending_client_id: &str,
    callback_client_id: Option<&str>,
    is_new_registration: bool,
) -> Result<String, CoreError> {
    if is_new_registration {
        match callback_client_id {
            None | Some("") => Err(CoreError::MissingClientId),
            Some(id) if id == DYNAMIC_CLIENT_ID => {
                Err(CoreError::DynamicClientIdNotIssuable)
            }
            Some(id) => Ok(id.to_string()),
        }
    } else {
        match callback_client_id {
            None | Some("") => Ok(pending_client_id.to_string()),
            Some(id) if id == pending_client_id => Ok(pending_client_id.to_string()),
            Some(_) => Err(CoreError::ClientIdMismatch),
        }
    }
}

/// Form-encoded `authorization_code` grant body for the token endpoint.
/// No client secret is used in this public-client flow.
pub fn token_exchange_form(
    client_id: &str,
    code: &str,
    code_verifier: &str,
    redirect_uri: &str,
) -> String {
    url::form_urlencoded::Serializer::new(String::new())
        .append_pair("grant_type", "authorization_code")
        .append_pair("client_id", client_id)
        .append_pair("code", code)
        .append_pair("code_verifier", code_verifier)
        .append_pair("redirect_uri", redirect_uri)
        .append_pair("resource", RESOURCE)
        .finish()
}

/// Form-encoded `refresh_token` grant body.
pub fn token_refresh_form(client_id: &str, refresh_token: &str) -> String {
    url::form_urlencoded::Serializer::new(String::new())
        .append_pair("grant_type", "refresh_token")
        .append_pair("client_id", client_id)
        .append_pair("refresh_token", refresh_token)
        .finish()
}

/// Split a space-separated granted-scope string into a sorted, deduped list.
pub fn parse_granted_scopes(scope: &str) -> Vec<String> {
    let mut scopes: Vec<String> = scope.split_whitespace().map(str::to_string).collect();
    scopes.sort();
    scopes.dedup();
    scopes
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_params() -> AuthorizeParams {
        AuthorizeParams {
            client_id: DYNAMIC_CLIENT_ID.to_string(),
            agent_name_hint: Some(AGENT_NAME.to_string()),
            host_id: "urn:uuid:12345678-1234-1234-1234-1234567890ab".to_string(),
            redirect_uri: redirect_uri(1455),
            state: "teststate".to_string(),
            nonce: "testnonce".to_string(),
            code_challenge: "testchallenge".to_string(),
            id_token_hint: None,
            login_hint: None,
        }
    }

    #[test]
    fn scope_string_matches_docs_order() {
        assert_eq!(
            scope_string(),
            "openid profile email offline_access resource.invoke chatgpt.tokens.use.direct"
        );
    }

    #[test]
    fn authorize_url_has_required_params() {
        let url = build_authorize_url(&sample_params()).unwrap();
        let q: std::collections::HashMap<_, _> = url.query_pairs().into_owned().collect();
        assert_eq!(q["client_id"], DYNAMIC_CLIENT_ID);
        assert_eq!(q["agent_name_hint"], AGENT_NAME);
        assert!(q["ext_agent_host_id"].starts_with("urn:uuid:"));
        assert_eq!(q["response_type"], "code");
        assert_eq!(q["redirect_uri"], "http://127.0.0.1:1455/auth/callback");
        assert_eq!(q["resource"], RESOURCE);
        assert_eq!(q["code_challenge_method"], "S256");
        assert!(q["scope"].contains(PLAN_SCOPE));
    }

    #[test]
    fn reauth_omits_agent_name_hint_and_keeps_hints() {
        let mut p = sample_params();
        p.client_id = "oaiapp_test123".to_string();
        p.agent_name_hint = None;
        p.id_token_hint = Some("old-id-token".to_string());
        p.login_hint = Some("user@example.com".to_string());
        let url = build_authorize_url(&p).unwrap();
        let text = url.as_str();
        assert!(!text.contains("agent_name_hint"));
        assert!(text.contains("id_token_hint=old-id-token"));
        assert!(text.contains("login_hint=user%40example.com"));
    }

    #[test]
    fn pkce_challenge_matches_rfc7636_vector() {
        // RFC 7636 Appendix B test vector.
        assert_eq!(
            code_challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }

    #[test]
    fn new_verifier_is_43_unreserved_chars() {
        let v = new_code_verifier();
        assert_eq!(v.len(), 43);
        assert!(v
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' || c == '~'));
    }

    #[test]
    fn state_and_nonce_are_unique() {
        assert_ne!(new_state(), new_state());
        assert_ne!(new_nonce(), new_nonce());
    }

    #[test]
    fn parse_new_registration_callback() {
        let cb = parse_callback(
            "code=abc123&scope=openid%20profile&state=xyz&client_id=oaiapp_1",
        )
        .unwrap();
        assert_eq!(cb.code.as_deref(), Some("abc123"));
        assert_eq!(cb.state.as_deref(), Some("xyz"));
        assert_eq!(cb.client_id.as_deref(), Some("oaiapp_1"));
        assert_eq!(cb.scope.as_deref(), Some("openid profile"));
        assert_eq!(cb.error, None);
    }

    #[test]
    fn parse_denied_callback() {
        let cb = parse_callback("error=access_denied&state=xyz").unwrap();
        assert_eq!(cb.error.as_deref(), Some("access_denied"));
        assert_eq!(cb.code, None);
    }

    #[test]
    fn parse_empty_callback_fails() {
        assert!(parse_callback("state=xyz").is_err());
    }

    #[test]
    fn resolve_new_registration_requires_issued_id() {
        assert_eq!(
            resolve_client_id(DYNAMIC_CLIENT_ID, Some("oaiapp_1"), true).unwrap(),
            "oaiapp_1"
        );
        assert!(matches!(
            resolve_client_id(DYNAMIC_CLIENT_ID, None, true),
            Err(CoreError::MissingClientId)
        ));
        assert!(matches!(
            resolve_client_id(DYNAMIC_CLIENT_ID, Some(DYNAMIC_CLIENT_ID), true),
            Err(CoreError::DynamicClientIdNotIssuable)
        ));
    }

    #[test]
    fn resolve_reauth_keeps_pending_and_rejects_mismatch() {
        assert_eq!(
            resolve_client_id("oaiapp_1", None, false).unwrap(),
            "oaiapp_1"
        );
        assert_eq!(
            resolve_client_id("oaiapp_1", Some("oaiapp_1"), false).unwrap(),
            "oaiapp_1"
        );
        assert!(matches!(
            resolve_client_id("oaiapp_1", Some("oaiapp_2"), false),
            Err(CoreError::ClientIdMismatch)
        ));
    }

    #[test]
    fn granted_scopes_sorted_and_deduped() {
        assert_eq!(
            parse_granted_scopes("openid openid profile"),
            vec!["openid".to_string(), "profile".to_string()]
        );
    }
}
