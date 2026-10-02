//! Protected local credential records, one per issued client id.
//!
//! Record shape follows the SIWC docs ("Store credentials in a local file").
//! Files are written atomically with owner-only permissions on Unix and must
//! never be committed or logged. Refresh rotates the access token, expiry,
//! granted scopes, and refresh token together.

use std::fs;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};

use crate::host::write_private_file;
use crate::siwc::{self, PLAN_SCOPE};

/// Raw token-endpoint response fields we persist.
#[derive(Debug, Clone, Deserialize)]
pub struct TokenResponse {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub id_token: String,
    pub token_type: String,
    pub expires_in: i64,
    pub scope: String,
    pub earliest_refresh_at: Option<i64>,
}

/// One account registration: validated identity + tokens + metadata.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CredentialRecord {
    pub email: String,
    pub issuer: String,
    pub subject: String,
    pub client_id: String,
    pub ext_agent_host_id: String,
    pub id_token: String,
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub token_type: String,
    pub expires_in: i64,
    pub scopes: Vec<String>,
    pub saved_at: DateTime<Utc>,
}

impl CredentialRecord {
    pub fn from_token_response(
        email: String,
        issuer: String,
        subject: String,
        client_id: String,
        host_id: String,
        token: &TokenResponse,
        saved_at: DateTime<Utc>,
    ) -> Self {
        Self {
            email,
            issuer,
            subject,
            client_id,
            ext_agent_host_id: host_id,
            id_token: token.id_token.clone(),
            access_token: token.access_token.clone(),
            refresh_token: token.refresh_token.clone(),
            token_type: token.token_type.clone(),
            expires_in: token.expires_in,
            scopes: siwc::parse_granted_scopes(&token.scope),
            saved_at,
        }
    }

    /// ChatGPT plan usage is enabled only when the granted scopes include it.
    pub fn has_plan_usage(&self) -> bool {
        self.scopes.iter().any(|s| s == PLAN_SCOPE)
    }

    pub fn expires_at(&self) -> DateTime<Utc> {
        self.saved_at + Duration::seconds(self.expires_in)
    }

    pub fn is_expired(&self, now: DateTime<Utc>) -> bool {
        now >= self.expires_at()
    }

    /// True when the access token is expired or expires within the skew
    /// window (5 minutes), and a refresh token is available.
    pub fn should_refresh(&self, now: DateTime<Utc>) -> bool {
        self.refresh_token.is_some() && now + Duration::minutes(5) >= self.expires_at()
    }

    /// Apply a successful refresh: replace access token, expiry, scopes, and
    /// the rotating refresh token together.
    pub fn apply_refresh(&mut self, token: &TokenResponse, saved_at: DateTime<Utc>) {
        self.access_token = token.access_token.clone();
        self.id_token = token.id_token.clone();
        self.token_type = token.token_type.clone();
        self.expires_in = token.expires_in;
        self.scopes = siwc::parse_granted_scopes(&token.scope);
        self.saved_at = saved_at;
        if token.refresh_token.is_some() {
            self.refresh_token = token.refresh_token.clone();
        }
    }
}

fn record_path(dir: &Path, client_id: &str) -> PathBuf {
    let safe: String = client_id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    dir.join(format!("{safe}.json"))
}

/// Save (or overwrite) the record for its client id. `dir` must exist.
pub fn save_record(dir: &Path, record: &CredentialRecord) -> std::io::Result<PathBuf> {
    let path = record_path(dir, &record.client_id);
    let bytes = serde_json::to_vec_pretty(record)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    write_private_file(&path, &bytes)?;
    Ok(path)
}

pub fn load_record(dir: &Path, client_id: &str) -> std::io::Result<CredentialRecord> {
    let bytes = fs::read(record_path(dir, client_id))?;
    serde_json::from_slice(&bytes)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
}

/// Delete the saved record for a client id (sign-out). Missing file is ok.
pub fn delete_record(dir: &Path, client_id: &str) -> std::io::Result<()> {
    match fs::remove_file(record_path(dir, client_id)) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

/// List saved client ids (filenames without extension).
pub fn list_client_ids(dir: &Path) -> std::io::Result<Vec<String>> {
    let mut ids = Vec::new();
    for entry in fs::read_dir(dir)? {
        let path = entry?.path();
        if path.extension().and_then(|e| e.to_str()) == Some("json") {
            if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                ids.push(stem.to_string());
            }
        }
    }
    ids.sort();
    Ok(ids)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn sample_token() -> TokenResponse {
        TokenResponse {
            access_token: "access-1".to_string(),
            refresh_token: Some("refresh-1".to_string()),
            id_token: "id-1".to_string(),
            token_type: "Bearer".to_string(),
            expires_in: 3600,
            scope: "chatgpt.tokens.use.direct email offline_access openid profile resource.invoke"
                .to_string(),
            earliest_refresh_at: None,
        }
    }

    fn sample_record() -> CredentialRecord {
        CredentialRecord::from_token_response(
            "user@example.com".to_string(),
            siwc::TOKEN_ISSUER.to_string(),
            "sub-1".to_string(),
            "oaiapp_1".to_string(),
            "urn:uuid:12345678-1234-1234-1234-1234567890ab".to_string(),
            &sample_token(),
            Utc.with_ymd_and_hms(2026, 10, 1, 12, 0, 0).unwrap(),
        )
    }

    #[test]
    fn record_matches_docs_shape() {
        let json = serde_json::to_value(sample_record()).unwrap();
        assert_eq!(json["email"], "user@example.com");
        assert_eq!(json["issuer"], "https://auth.openai.com");
        assert_eq!(json["token_type"], "Bearer");
        assert_eq!(json["expires_in"], 3600);
        assert!(json["scopes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s == PLAN_SCOPE));
        assert_eq!(json["saved_at"], "2026-10-01T12:00:00Z");
    }

    #[test]
    fn plan_usage_requires_scope() {
        assert!(sample_record().has_plan_usage());
        let mut rec = sample_record();
        rec.scopes = vec!["openid".to_string(), "profile".to_string()];
        assert!(!rec.has_plan_usage());
    }

    #[test]
    fn expiry_and_refresh_window() {
        let rec = sample_record();
        let saved = rec.saved_at;
        assert!(!rec.is_expired(saved));
        assert!(!rec.should_refresh(saved));
        // 56 minutes in: within the 5-minute skew window.
        assert!(rec.should_refresh(saved + Duration::minutes(56)));
        assert!(rec.is_expired(saved + Duration::seconds(3600)));
    }

    #[test]
    fn refresh_rotates_tokens_together() {
        let mut rec = sample_record();
        let next = TokenResponse {
            access_token: "access-2".to_string(),
            refresh_token: Some("refresh-2".to_string()),
            id_token: "id-2".to_string(),
            token_type: "Bearer".to_string(),
            expires_in: 3600,
            scope: "openid profile".to_string(),
            earliest_refresh_at: None,
        };
        let at = rec.saved_at + Duration::seconds(3600);
        rec.apply_refresh(&next, at);
        assert_eq!(rec.access_token, "access-2");
        assert_eq!(rec.refresh_token.as_deref(), Some("refresh-2"));
        assert_eq!(rec.saved_at, at);
        assert!(!rec.has_plan_usage());
    }

    #[test]
    fn save_load_list_roundtrip() {
        let dir =
            std::env::temp_dir().join(format!("dat-creds-test-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let rec = sample_record();
        save_record(&dir, &rec).unwrap();
        assert_eq!(load_record(&dir, "oaiapp_1").unwrap(), rec);
        assert_eq!(list_client_ids(&dir).unwrap(), vec!["oaiapp_1".to_string()]);
        delete_record(&dir, "oaiapp_1").unwrap();
        assert!(list_client_ids(&dir).unwrap().is_empty());
        delete_record(&dir, "oaiapp_1").unwrap(); // idempotent
        fs::remove_dir_all(&dir).unwrap();
    }
}
