//! SIWC sign-in implementation: loopback callback, code exchange, ID-token
//! validation, token refresh, and sign-out.
//!
//! Pure flow helpers live in `don_a_token_core`; this module owns network
//! I/O and the pending-attempt state. Tokens never leave the Rust shell:
//! commands return only safe summaries (email, client id, plan flag).

use std::path::PathBuf;
use std::time::Duration;

use chrono::Utc;
use don_a_token_core::credentials::{self, CredentialRecord, TokenResponse};
use don_a_token_core::siwc;
use jsonwebtoken::jwk::JwkSet;
use jsonwebtoken::{decode, decode_header, DecodingKey, Validation};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager, State};
use tauri_plugin_opener::OpenerExt;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::Mutex;
use tokio::time::timeout;

/// How long `start_sign_in` waits for the browser to hit the callback.
const CALLBACK_TIMEOUT: Duration = Duration::from_secs(300);
const HTTP_TIMEOUT: Duration = Duration::from_secs(30);
const REFRESH_INTERVAL: Duration = Duration::from_secs(300);

/// At most one sign-in attempt at a time per app process.
#[derive(Default)]
pub struct PendingState(pub Mutex<Option<PendingAttempt>>);

pub struct PendingAttempt {
    pending_client_id: String,
    is_new_registration: bool,
    expected_subject: Option<String>,
    state: String,
    nonce: String,
    verifier: String,
    redirect_uri: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct SignInResult {
    pub email: String,
    pub client_id: String,
    pub has_plan_usage: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct AccountSummary {
    pub client_id: String,
    pub email: String,
    pub has_plan_usage: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct SignOutResult {
    pub revoked_remote: bool,
}

#[derive(Debug, Deserialize)]
struct DiscoveryDoc {
    jwks_uri: String,
    revocation_endpoint: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Claims {
    sub: String,
    iss: String,
    aud: Aud,
    #[allow(dead_code)]
    exp: usize, // enforced by jsonwebtoken validation
    nonce: Option<String>,
    email: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum Aud {
    One(String),
    Many(Vec<String>),
}

impl Aud {
    fn contains(&self, id: &str) -> bool {
        match self {
            Aud::One(a) => a == id,
            Aud::Many(list) => list.iter().any(|a| a == id),
        }
    }
}

fn creds_dir(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir)
}

fn host_id(app: &AppHandle) -> Result<String, String> {
    let dir = creds_dir(app)?;
    don_a_token_core::host::load_or_generate(&dir.join("host_id")).map_err(|e| e.to_string())
}

fn http_client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .timeout(HTTP_TIMEOUT)
        .build()
        .map_err(|e| format!("http client: {e}"))
}

/// Parse the query string out of a raw HTTP callback request.
/// Returns `None` unless it is a GET for the exact callback path.
fn parse_callback_request(req: &str) -> Option<String> {
    let line = req.lines().next()?;
    let mut parts = line.split_whitespace();
    if parts.next()? != "GET" {
        return None;
    }
    let target = parts.next()?;
    let (path, query) = target.split_once('?')?;
    if path != siwc::CALLBACK_PATH {
        return None;
    }
    Some(query.to_string())
}

const CALLBACK_HTML: &str = concat!(
    "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\n",
    "Connection: close\r\n\r\n",
    "<!doctype html><html><body style=\"font-family:sans-serif\">",
    "<h1>Signed in to Don-a-Token</h1>",
    "<p>You can close this window and return to the app.</p>",
    "</body></html>",
);

/// Serve exactly one loopback callback request, then shut down.
async fn await_callback(listener: TcpListener) -> Result<String, String> {
    let (mut socket, _) = timeout(CALLBACK_TIMEOUT, listener.accept())
        .await
        .map_err(|_| "sign-in timed out waiting for the browser callback".to_string())
        .and_then(|r| r.map_err(|e| format!("callback accept: {e}")))?;
    let mut buf = vec![0u8; 16384];
    let mut len = 0usize;
    loop {
        if len >= buf.len() {
            return Err("callback request too large".to_string());
        }
        let n = timeout(CALLBACK_TIMEOUT, socket.read(&mut buf[len..]))
            .await
            .map_err(|_| "sign-in timed out reading the callback".to_string())
            .and_then(|r| r.map_err(|e| format!("callback read: {e}")))?;
        if n == 0 {
            break;
        }
        len += n;
        if buf[..len].windows(4).any(|w| w == b"\r\n\r\n") {
            break;
        }
    }
    let _ = socket.write_all(CALLBACK_HTML.as_bytes()).await;
    let req = String::from_utf8_lossy(&buf[..len]).into_owned();
    parse_callback_request(&req).ok_or_else(|| "unexpected callback request".to_string())
}

async fn discovery(client: &reqwest::Client) -> Result<DiscoveryDoc, String> {
    client
        .get(siwc::OIDC_DISCOVERY_URL)
        .send()
        .await
        .map_err(|e| format!("discovery request: {e}"))?
        .error_for_status()
        .map_err(|e| format!("discovery status: {e}"))?
        .json::<DiscoveryDoc>()
        .await
        .map_err(|e| format!("discovery body: {e}"))
}

async fn exchange_code(
    client: &reqwest::Client,
    client_id: &str,
    code: &str,
    verifier: &str,
    redirect_uri: &str,
) -> Result<TokenResponse, String> {
    let form = siwc::token_exchange_form(client_id, code, verifier, redirect_uri);
    let res = client
        .post(siwc::TOKEN_URL)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(form)
        .send()
        .await
        .map_err(|e| format!("token request: {e}"))?;
    if res.status() == reqwest::StatusCode::BAD_REQUEST {
        let body = res.text().await.unwrap_or_default();
        if body.contains("invalid_grant") {
            return Err(
                "authorization code expired; please sign in again".to_string(),
            );
        }
        return Err(format!("token exchange rejected: {body}"));
    }
    res.error_for_status()
        .map_err(|e| format!("token status: {e}"))?
        .json::<TokenResponse>()
        .await
        .map_err(|e| format!("token body: {e}"))
}

/// Validate the ID token and return its claims. Checks signature (JWKS),
/// issuer, audience (issued client id), expiry, and the attempt nonce.
async fn validate_id_token(
    client: &reqwest::Client,
    id_token: &str,
    issued_client_id: &str,
    nonce: &str,
) -> Result<Claims, String> {
    let doc = discovery(client).await?;
    let jwks = client
        .get(&doc.jwks_uri)
        .send()
        .await
        .map_err(|e| format!("jwks request: {e}"))?
        .error_for_status()
        .map_err(|e| format!("jwks status: {e}"))?
        .json::<JwkSet>()
        .await
        .map_err(|e| format!("jwks body: {e}"))?;
    let header = decode_header(id_token).map_err(|e| format!("id token header: {e}"))?;
    let kid = header.kid.ok_or_else(|| "id token has no key id".to_string())?;
    let jwk = jwks
        .find(&kid)
        .ok_or_else(|| "id token key not in JWKS".to_string())?;
    let key = DecodingKey::from_jwk(jwk).map_err(|e| format!("jwk key: {e}"))?;
    let mut validation = Validation::new(header.alg);
    validation.validate_aud = false; // checked manually below (string-or-list)
    let data = decode::<Claims>(id_token, &key, &validation)
        .map_err(|e| format!("id token invalid: {e}"))?;
    let claims = data.claims;
    if claims.iss != siwc::TOKEN_ISSUER {
        return Err("id token issuer mismatch".to_string());
    }
    if !claims.aud.contains(issued_client_id) {
        return Err("id token audience mismatch".to_string());
    }
    if claims.nonce.as_deref() != Some(nonce) {
        return Err("id token nonce mismatch".to_string());
    }
    Ok(claims)
}

/// Run one refresh grant against the current record, rotating tokens.
async fn refresh_record(
    client: &reqwest::Client,
    record: &mut CredentialRecord,
) -> Result<(), String> {
    let refresh_token = record
        .refresh_token
        .clone()
        .ok_or_else(|| "no refresh token; please sign in again".to_string())?;
    let form = siwc::token_refresh_form(&record.client_id, &refresh_token);
    let res = client
        .post(siwc::TOKEN_URL)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(form)
        .send()
        .await
        .map_err(|e| format!("refresh request: {e}"))?;
    if res.status() == reqwest::StatusCode::BAD_REQUEST {
        let body = res.text().await.unwrap_or_default();
        if body.contains("invalid_grant") {
            return Err("session expired; please sign in again".to_string());
        }
        return Err(format!("refresh rejected: {body}"));
    }
    let token = res
        .error_for_status()
        .map_err(|e| format!("refresh status: {e}"))?
        .json::<TokenResponse>()
        .await
        .map_err(|e| format!("refresh body: {e}"))?;
    record.apply_refresh(&token, Utc::now());
    Ok(())
}

fn summarize(record: &CredentialRecord) -> AccountSummary {
    AccountSummary {
        client_id: record.client_id.clone(),
        email: record.email.clone(),
        has_plan_usage: record.has_plan_usage(),
    }
}

/// Start a SIWC sign-in. `client_id=None` registers a new ChatGPT account;
/// `Some(id)` reauthorizes a saved one. Opens the system browser and waits
/// for the loopback callback.
#[tauri::command]
pub async fn start_sign_in(
    app: AppHandle,
    pending: State<'_, PendingState>,
    client_id: Option<String>,
) -> Result<SignInResult, String> {
    {
        let guard = pending.0.lock().await;
        if guard.is_some() {
            return Err("a sign-in is already in progress".to_string());
        }
    }
    let is_new = client_id.is_none();
    let pending_client_id = client_id.unwrap_or_else(|| siwc::DYNAMIC_CLIENT_ID.to_string());

    let dir = creds_dir(&app)?;
    let host = host_id(&app)?;
    let attempt_state = siwc::new_state();
    let nonce = siwc::new_nonce();
    let verifier = siwc::new_code_verifier();
    let challenge = siwc::code_challenge(&verifier);

    // Reauthorization hints from the saved registration, if any.
    let (id_token_hint, login_hint, expected_subject) = if is_new {
        (None, None, None)
    } else {
        match credentials::load_record(&dir, &pending_client_id) {
            Ok(rec) => (
                Some(rec.id_token.clone()),
                Some(rec.email.clone()),
                Some(rec.subject.clone()),
            ),
            Err(_) => {
                return Err("unknown account; register it as new instead".to_string());
            }
        }
    };

    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(|e| format!("callback listener: {e}"))?;
    let port = listener
        .local_addr()
        .map_err(|e| format!("callback port: {e}"))?
        .port();
    let redirect_uri = siwc::redirect_uri(port);

    let params = siwc::AuthorizeParams {
        client_id: pending_client_id.clone(),
        agent_name_hint: is_new.then(|| siwc::AGENT_NAME.to_string()),
        host_id: host.clone(),
        redirect_uri: redirect_uri.clone(),
        state: attempt_state.clone(),
        nonce: nonce.clone(),
        code_challenge: challenge,
        id_token_hint,
        login_hint,
    };
    let url = siwc::build_authorize_url(&params).map_err(|e| e.to_string())?;

    *pending.0.lock().await = Some(PendingAttempt {
        pending_client_id: pending_client_id.clone(),
        is_new_registration: is_new,
        expected_subject,
        state: attempt_state,
        nonce,
        verifier,
        redirect_uri,
    });

    let outcome = run_attempt(&app, &dir, &host, listener, url.as_str(), &pending).await;
    *pending.0.lock().await = None;
    outcome
}

async fn run_attempt(
    app: &AppHandle,
    dir: &PathBuf,
    host: &str,
    listener: TcpListener,
    auth_url: &str,
    pending: &State<'_, PendingState>,
) -> Result<SignInResult, String> {
    app.opener()
        .open_url(auth_url, None::<&str>)
        .map_err(|e| format!("open browser: {e}"))?;
    let query = await_callback(listener).await?;
    let attempt = pending
        .0
        .lock()
        .await
        .take()
        .ok_or_else(|| "sign-in attempt expired".to_string())?;

    let cb = siwc::parse_callback(&query).map_err(|e| e.to_string())?;
    if let Some(error) = cb.error {
        if error == "access_denied" {
            return Err(
                "authorization declined; you can enable ChatGPT plan usage later from settings"
                    .to_string(),
            );
        }
        return Err(format!("authorization failed: {error}"));
    }
    if cb.state.as_deref() != Some(attempt.state.as_str()) {
        return Err("callback state mismatch".to_string());
    }
    let code = cb.code.ok_or_else(|| "callback missing code".to_string())?;
    let issued = siwc::resolve_client_id(
        &attempt.pending_client_id,
        cb.client_id.as_deref(),
        attempt.is_new_registration,
    )
    .map_err(|e| e.to_string())?;

    let client = http_client()?;
    let token = exchange_code(&client, &issued, &code, &attempt.verifier, &attempt.redirect_uri).await?;
    let claims = validate_id_token(&client, &token.id_token, &issued, &attempt.nonce).await?;
    if let Some(expected) = attempt.expected_subject {
        if claims.sub != expected {
            return Err("signed in as a different account; keeping the saved one".to_string());
        }
    }
    let email = claims
        .email
        .ok_or_else(|| "id token has no email".to_string())?;
    let record = CredentialRecord::from_token_response(
        email.clone(),
        claims.iss,
        claims.sub,
        issued.clone(),
        host.to_string(),
        &token,
        Utc::now(),
    );
    credentials::save_record(dir, &record).map_err(|e| format!("save credentials: {e}"))?;

    Ok(SignInResult {
        email,
        client_id: issued,
        has_plan_usage: record.has_plan_usage(),
    })
}

/// Refresh one account's tokens when due. Returns the current summary.
#[tauri::command]
pub async fn refresh_account(
    app: AppHandle,
    client_id: String,
) -> Result<AccountSummary, String> {
    let dir = creds_dir(&app)?;
    let mut record =
        credentials::load_record(&dir, &client_id).map_err(|e| format!("load account: {e}"))?;
    if record.should_refresh(Utc::now()) {
        let client = http_client()?;
        refresh_record(&client, &mut record).await?;
        credentials::save_record(&dir, &record).map_err(|e| format!("save account: {e}"))?;
    }
    Ok(summarize(&record))
}

/// List saved ChatGPT accounts (no tokens leave the shell).
#[tauri::command]
pub async fn list_accounts(app: AppHandle) -> Result<Vec<AccountSummary>, String> {
    let dir = creds_dir(&app)?;
    let mut out = Vec::new();
    for id in credentials::list_client_ids(&dir).map_err(|e| format!("list accounts: {e}"))? {
        match credentials::load_record(&dir, &id) {
            Ok(rec) => out.push(summarize(&rec)),
            Err(e) => eprintln!("skip unreadable credential record {id}: {e}"),
        }
    }
    Ok(out)
}

/// Sign out of one account: revoke the renewable session remotely (best
/// effort), then delete local tokens. The host id is retained.
#[tauri::command]
pub async fn sign_out(app: AppHandle, client_id: String) -> Result<SignOutResult, String> {
    let dir = creds_dir(&app)?;
    let record =
        credentials::load_record(&dir, &client_id).map_err(|e| format!("load account: {e}"))?;
    let revoked_remote = revoke_remote(&record).await;
    credentials::delete_record(&dir, &client_id).map_err(|e| format!("clear tokens: {e}"))?;
    Ok(SignOutResult { revoked_remote })
}

async fn revoke_remote(record: &CredentialRecord) -> bool {
    let client = match http_client() {
        Ok(c) => c,
        Err(_) => return false,
    };
    let endpoint = match discovery(&client).await {
        Ok(doc) => match doc.revocation_endpoint {
            Some(url) => url,
            None => return false,
        },
        Err(_) => return false,
    };
    let Some(refresh_token) = &record.refresh_token else {
        return true; // nothing renewable to revoke
    };
    let form = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("token", refresh_token)
        .append_pair("token_type_hint", "refresh_token")
        .append_pair("client_id", &record.client_id)
        .finish();
    match client
        .post(endpoint)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(form)
        .send()
        .await
    {
        Ok(res) => res.status().is_success(),
        Err(_) => false,
    }
}

/// Background loop: refresh every account whose access token is due.
/// Failures are logged; the next tick retries.
pub fn spawn_refresh_loop(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let mut interval = tokio::time::interval(REFRESH_INTERVAL);
        loop {
            interval.tick().await;
            refresh_all_due(&app).await;
        }
    });
}

async fn refresh_all_due(app: &AppHandle) {
    let dir = match creds_dir(app) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("refresh loop: {e}");
            return;
        }
    };
    let ids = match credentials::list_client_ids(&dir) {
        Ok(ids) => ids,
        Err(e) => {
            eprintln!("refresh loop: {e}");
            return;
        }
    };
    let client = match http_client() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("refresh loop: {e}");
            return;
        }
    };
    for id in ids {
        let mut record = match credentials::load_record(&dir, &id) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("refresh loop: skip {id}: {e}");
                continue;
            }
        };
        if !record.should_refresh(Utc::now()) {
            continue;
        }
        match refresh_record(&client, &mut record).await {
            Ok(()) => {
                if let Err(e) = credentials::save_record(&dir, &record) {
                    eprintln!("refresh loop: save {id}: {e}");
                }
            }
            Err(e) => eprintln!("refresh loop: {id}: {e}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn callback_request_parses_query() {
        let req = "GET /auth/callback?code=abc&state=xyz HTTP/1.1\r\nHost: x\r\n\r\n";
        assert_eq!(
            parse_callback_request(req).as_deref(),
            Some("code=abc&state=xyz")
        );
    }

    #[test]
    fn callback_request_rejects_wrong_method_path_or_shape() {
        assert_eq!(
            parse_callback_request("POST /auth/callback?code=abc HTTP/1.1\r\n\r\n"),
            None
        );
        assert_eq!(
            parse_callback_request("GET /other?code=abc HTTP/1.1\r\n\r\n"),
            None
        );
        assert_eq!(
            parse_callback_request("GET /auth/callback HTTP/1.1\r\n\r\n"),
            None
        );
        assert_eq!(parse_callback_request(""), None);
    }
}
