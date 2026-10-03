//! GitHub OAuth device flow (`gh auth login`-style) for contribution auth.
//!
//! The app requests a user code, the donor approves it in the browser, and
//! the app polls for a user token. The token is stored in the settings
//! store (OS keychain is the follow-up) and used host-side only, per
//! publish — it never enters a sandbox.

use std::time::Duration;

use serde::{Deserialize, Serialize};

/// Public client id (OAuth App or GitHub App — both support device
/// flow). Safe to ship in the binary: the device flow needs no secret.
pub const GITHUB_OAUTH_CLIENT_ID: &str = "Ov23libwvTLILpIhlikH";

const DEVICE_CODE_URL: &str = "https://github.com/login/device/code";
const ACCESS_TOKEN_URL: &str = "https://github.com/login/oauth/access_token";
/// `repo` covers branch push + PR open; `workflow` is required too
/// because a push whose history introduces workflow-file blobs (e.g.
/// a fork that drifted behind upstream) is rejected without it.
const SCOPE: &str = "repo workflow";
/// `urn:ietf:params:oauth:grant-type:device_code` per RFC 8628.
const DEVICE_GRANT: &str = "urn:ietf:params:oauth:grant-type:device_code";
const HTTP_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Serialize)]
pub struct DeviceStart {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    pub verification_uri_complete: Option<String>,
    pub expires_in: u64,
    pub interval_secs: u64,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum DevicePoll {
    Pending,
    SlowDown,
    Done { access_token: String },
    Expired,
    Denied,
}

#[derive(Debug, Deserialize)]
struct DeviceCodeResponse {
    device_code: String,
    user_code: String,
    verification_uri: String,
    verification_uri_complete: Option<String>,
    expires_in: u64,
    interval: u64,
}

#[derive(Debug, Deserialize)]
struct TokenPollResponse {
    access_token: Option<String>,
    error: Option<String>,
}

fn check_client_id() -> Result<(), String> {
    if GITHUB_OAUTH_CLIENT_ID.is_empty() || GITHUB_OAUTH_CLIENT_ID == "PASTE_CLIENT_ID_HERE" {
        return Err("GitHub OAuth client id is not configured".to_string());
    }
    Ok(())
}

fn parse_device_start(text: &str) -> Result<DeviceStart, String> {
    let r: DeviceCodeResponse =
        serde_json::from_str(text).map_err(|e| format!("device code response: {e}"))?;
    Ok(DeviceStart {
        device_code: r.device_code,
        user_code: r.user_code,
        verification_uri: r.verification_uri,
        verification_uri_complete: r.verification_uri_complete,
        expires_in: r.expires_in,
        interval_secs: r.interval,
    })
}

fn parse_poll(text: &str) -> Result<DevicePoll, String> {
    let r: TokenPollResponse =
        serde_json::from_str(text).map_err(|e| format!("device poll response: {e}"))?;
    if let Some(token) = r.access_token {
        return Ok(DevicePoll::Done {
            access_token: token,
        });
    }
    match r.error.as_deref() {
        Some("authorization_pending") => Ok(DevicePoll::Pending),
        Some("slow_down") => Ok(DevicePoll::SlowDown),
        Some("expired_token") | Some("incorrect_device_code") => Ok(DevicePoll::Expired),
        Some("access_denied") => Ok(DevicePoll::Denied),
        Some(other) => Err(format!("device authorization failed: {other}")),
        None => Err("device poll returned neither token nor error".to_string()),
    }
}

fn http_client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .timeout(HTTP_TIMEOUT)
        .build()
        .map_err(|e| format!("http client: {e}"))
}

/// Start a device flow: returns the user code + verification URI for the
/// frontend to display, plus the device code it polls with.
#[tauri::command]
pub async fn github_device_start() -> Result<DeviceStart, String> {
    check_client_id()?;
    let client = http_client()?;
    let text = client
        .post(DEVICE_CODE_URL)
        .header("Accept", "application/json")
        .json(&serde_json::json!({
            "client_id": GITHUB_OAUTH_CLIENT_ID,
            "scope": SCOPE,
        }))
        .send()
        .await
        .map_err(|e| format!("device code request: {e}"))?
        .error_for_status()
        .map_err(|e| format!("device code status: {e}"))?
        .text()
        .await
        .map_err(|e| format!("device code body: {e}"))?;
    parse_device_start(&text)
}

/// Poll once for the device flow result. The frontend paces calls by the
/// start response's interval.
#[tauri::command]
pub async fn github_device_poll(device_code: String) -> Result<DevicePoll, String> {
    check_client_id()?;
    let client = http_client()?;
    // Pending/error outcomes arrive as JSON bodies on either 200 or 4xx,
    // so read the body unconditionally and let the parser decide.
    let text = client
        .post(ACCESS_TOKEN_URL)
        .header("Accept", "application/json")
        .json(&serde_json::json!({
            "client_id": GITHUB_OAUTH_CLIENT_ID,
            "device_code": device_code,
            "grant_type": DEVICE_GRANT,
        }))
        .send()
        .await
        .map_err(|e| format!("device poll request: {e}"))?
        .text()
        .await
        .map_err(|e| format!("device poll body: {e}"))?;
    parse_poll(&text)
}

#[cfg(test)]
mod tests {
    use super::*;

    const START_FIXTURE: &str = r#"{
        "device_code": "dev-1",
        "user_code": "ABCD-1234",
        "verification_uri": "https://github.com/login/device",
        "verification_uri_complete": "https://github.com/login/device?user_code=ABCD-1234",
        "expires_in": 900,
        "interval": 5
    }"#;

    #[test]
    fn start_parses_github_response() {
        let s = parse_device_start(START_FIXTURE).unwrap();
        assert_eq!(s.user_code, "ABCD-1234");
        assert_eq!(s.interval_secs, 5);
        assert!(s
            .verification_uri_complete
            .unwrap()
            .contains("user_code="));
    }

    #[test]
    fn poll_maps_pending_slow_and_done() {
        assert_eq!(
            parse_poll(r#"{"error": "authorization_pending"}"#).unwrap(),
            DevicePoll::Pending
        );
        assert_eq!(
            parse_poll(r#"{"error": "slow_down"}"#).unwrap(),
            DevicePoll::SlowDown
        );
        assert_eq!(
            parse_poll(r#"{"access_token": "ghu_1", "token_type": "bearer", "scope": "repo"}"#)
                .unwrap(),
            DevicePoll::Done {
                access_token: "ghu_1".to_string()
            }
        );
    }

    #[test]
    fn poll_maps_fatal_and_unknown_errors() {
        assert_eq!(
            parse_poll(r#"{"error": "expired_token"}"#).unwrap(),
            DevicePoll::Expired
        );
        assert_eq!(
            parse_poll(r#"{"error": "access_denied"}"#).unwrap(),
            DevicePoll::Denied
        );
        assert!(parse_poll(r#"{"error": "weird_new_error"}"#).is_err());
        assert!(parse_poll(r#"{}"#).is_err());
    }
}
