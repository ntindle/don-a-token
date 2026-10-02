//! Stable per-host `ext_agent_host_id` management.
//!
//! One host = one installation environment (laptop install, self-hosted VM).
//! The id is chosen and persisted *before* the first sign-in and reused
//! across restarts and re-sign-ins. It is opaque (never an email or user id)
//! and is not an authentication credential.
//!
//! Accepted formats (docs recommend JWK thumbprint; UUID is supported):
//! - `urn:ietf:params:oauth:jwk-thumbprint:...`
//! - `urn:uuid:<uuidv4>`
//! - `did:key:<key>`

use std::fs;
use std::io;
use std::path::Path;

use crate::CoreError;

pub const JWK_THUMBPRINT_PREFIX: &str = "urn:ietf:params:oauth:jwk-thumbprint:";
pub const UUID_PREFIX: &str = "urn:uuid:";
pub const DID_KEY_PREFIX: &str = "did:key:";

#[derive(Debug, PartialEq, Eq)]
pub enum HostIdFormat {
    JwkThumbprint,
    Uuid,
    DidKey,
}

/// Generate a fresh host id (UUID-based). A future upgrade may derive
/// `urn:ietf:params:oauth:jwk-thumbprint:` ids from a host keypair instead.
pub fn generate() -> String {
    format!("{}{}", UUID_PREFIX, uuid::Uuid::new_v4().hyphenated())
}

/// Validate a host id and report its format.
pub fn parse(id: &str) -> Result<HostIdFormat, CoreError> {
    if let Some(rest) = id.strip_prefix(UUID_PREFIX) {
        rest.parse::<uuid::Uuid>()
            .map_err(|_| CoreError::InvalidHostId(id.to_string()))?;
        return Ok(HostIdFormat::Uuid);
    }
    if let Some(rest) = id.strip_prefix(JWK_THUMBPRINT_PREFIX) {
        if rest.is_empty() {
            return Err(CoreError::InvalidHostId(id.to_string()));
        }
        return Ok(HostIdFormat::JwkThumbprint);
    }
    if let Some(rest) = id.strip_prefix(DID_KEY_PREFIX) {
        if rest.is_empty() {
            return Err(CoreError::InvalidHostId(id.to_string()));
        }
        return Ok(HostIdFormat::DidKey);
    }
    Err(CoreError::InvalidHostId(id.to_string()))
}

/// Load the persisted host id, or generate + persist one atomically.
/// Parent directory must exist. File is owner-only on Unix.
pub fn load_or_generate(path: &Path) -> io::Result<String> {
    if let Ok(text) = fs::read_to_string(path) {
        let id = text.trim().to_string();
        if !id.is_empty() {
            return Ok(id);
        }
    }
    let id = generate();
    write_private_file(path, id.as_bytes())?;
    Ok(id)
}

pub(crate) fn write_private_file(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, bytes)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&tmp, fs::Permissions::from_mode(0o600))?;
    }
    fs::rename(&tmp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_id_is_valid_uuid_urn() {
        let id = generate();
        assert_eq!(parse(&id).unwrap(), HostIdFormat::Uuid);
    }

    #[test]
    fn parse_accepts_all_documented_formats() {
        assert_eq!(
            parse("urn:uuid:12345678-1234-1234-1234-1234567890ab").unwrap(),
            HostIdFormat::Uuid
        );
        assert_eq!(
            parse("urn:ietf:params:oauth:jwk-thumbprint:abc123").unwrap(),
            HostIdFormat::JwkThumbprint
        );
        assert_eq!(
            parse("did:key:z6Mkabc").unwrap(),
            HostIdFormat::DidKey
        );
    }

    #[test]
    fn parse_rejects_garbage() {
        assert!(parse("user@example.com").is_err());
        assert!(parse("urn:uuid:not-a-uuid").is_err());
        assert!(parse("urn:uuid:").is_err());
        assert!(parse("").is_err());
    }

    #[test]
    fn load_or_generate_roundtrip() {
        let dir = std::env::temp_dir().join(format!("dat-host-test-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("host_id");
        let first = load_or_generate(&path).unwrap();
        let second = load_or_generate(&path).unwrap();
        assert_eq!(first, second);
        assert_eq!(parse(&first).unwrap(), HostIdFormat::Uuid);
        fs::remove_dir_all(&dir).unwrap();
    }
}
