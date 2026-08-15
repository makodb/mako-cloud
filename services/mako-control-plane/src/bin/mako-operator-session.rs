//! Issues a short-lived, file-only operator session for protected beta administration.

#![forbid(unsafe_code)]

use std::{
    collections::{BTreeMap, BTreeSet},
    env,
    fs::{File, OpenOptions},
    io::{Read, Write},
    os::unix::fs::{MetadataExt as _, OpenOptionsExt as _},
    path::{Path, PathBuf},
    process::ExitCode,
    time::{SystemTime, UNIX_EPOCH},
};

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use ed25519_dalek::{Signer as _, SigningKey};
use serde_json::json;

const OPERATOR_KEY_DOMAIN: &str = "mako/control-plane/operator-session-signing/v1";
const OPERATOR_AUDIENCE: &str = "mako-operator";
const MINIMUM_TTL_SECONDS: u64 = 60;
const MAXIMUM_TTL_SECONDS: u64 = 3_600;
const ALLOWED_PERMISSIONS: [&str; 17] = [
    "tenant_read",
    "overview_read",
    "operations_read",
    "incident_read",
    "incident_manage",
    "backup_read",
    "recovery_manage",
    "fleet_read",
    "security_read",
    "security_manage",
    "activity_read",
    "activity_export",
    "provisioning_repair",
    "quota_override",
    "abuse_response",
    "support_access",
    "waitlist_review",
];

fn main() -> ExitCode {
    match run(env::args().skip(1)) {
        Ok((output, evidence)) => {
            println!(
                "wrote short-lived operator session to {} and redacted issuance evidence to {}",
                output.display(),
                evidence.display()
            );
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("operator session was not issued: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run(arguments: impl IntoIterator<Item = String>) -> Result<(PathBuf, PathBuf), String> {
    let options = parse_options(arguments)?;
    let secret_path = required_path(&options, "secret-file")?;
    let output_path = required_path(&options, "output")?;
    let evidence_path = required_path(&options, "evidence-output")?;
    let incident_reason_path = required_path(&options, "incident-reason-file")?;
    let issuer = required(&options, "issuer")?;
    let operator_id = required(&options, "operator-id")?;
    let permissions = parse_permissions(required(&options, "permissions")?)?;
    let ttl_seconds = required(&options, "ttl-seconds")?
        .parse::<u64>()
        .map_err(|_| "ttl-seconds must be an integer".to_owned())?;

    validate_claim("issuer", issuer, 8, 2_048)?;
    validate_prefixed_identifier("operator-id", operator_id, "opr_")?;
    let incident_reason = read_incident_reason(&incident_reason_path)?;
    if !(MINIMUM_TTL_SECONDS..=MAXIMUM_TTL_SECONDS).contains(&ttl_seconds) {
        return Err(format!(
            "ttl-seconds must be between {MINIMUM_TTL_SECONDS} and {MAXIMUM_TTL_SECONDS}"
        ));
    }

    let secret = read_secret(&secret_path)?;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| "system time is before the Unix epoch".to_owned())?
        .as_secs();
    let expires = now
        .checked_add(ttl_seconds)
        .ok_or_else(|| "session expiry overflows".to_owned())?;
    let token = sign(
        &secret,
        json!({
            "iss": issuer,
            "sub": operator_id,
            "aud": [OPERATOR_AUDIENCE],
            "permissions": permissions,
            "exp": expires,
        }),
    )?;
    write_private_new(&output_path, token.as_bytes())?;
    let permission_names = permissions.iter().copied().collect::<Vec<_>>().join(",");
    let evidence = serde_json::to_vec_pretty(&json!({
        "schemaVersion": 1,
        "action": "operator_break_glass_issued",
        "operatorIdDigest": blake3::hash(operator_id.as_bytes()).to_hex().to_string(),
        "permissionDigest": blake3::hash(permission_names.as_bytes()).to_hex().to_string(),
        "incidentReasonDigest": blake3::hash(incident_reason.as_bytes()).to_hex().to_string(),
        "credentialDigest": blake3::hash(token.as_bytes()).to_hex().to_string(),
        "issuedAtUnixSeconds": now,
        "expiresAtUnixSeconds": expires,
        "maximumTtlSeconds": MAXIMUM_TTL_SECONDS,
    }))
    .map_err(|_| "issuance evidence could not be encoded".to_owned())?;
    write_private_new(&evidence_path, &evidence)?;
    Ok((output_path, evidence_path))
}

fn parse_options(
    arguments: impl IntoIterator<Item = String>,
) -> Result<BTreeMap<String, String>, String> {
    let arguments = arguments.into_iter().collect::<Vec<_>>();
    let mut options = BTreeMap::new();
    let mut chunks = arguments.chunks_exact(2);
    for pair in &mut chunks {
        let name = pair[0]
            .strip_prefix("--")
            .ok_or_else(|| "options must use --name value pairs".to_owned())?;
        if name.is_empty() || options.insert(name.to_owned(), pair[1].clone()).is_some() {
            return Err("options must be unique --name value pairs".to_owned());
        }
    }
    if !chunks.remainder().is_empty() {
        return Err("options must use --name value pairs".to_owned());
    }
    let expected = [
        "evidence-output",
        "incident-reason-file",
        "issuer",
        "operator-id",
        "output",
        "permissions",
        "secret-file",
        "ttl-seconds",
    ];
    if options.keys().any(|key| !expected.contains(&key.as_str())) {
        return Err("an unsupported option was provided".to_owned());
    }
    Ok(options)
}

fn parse_permissions(value: &str) -> Result<BTreeSet<&str>, String> {
    let values = value.split(',').collect::<Vec<_>>();
    let permissions = values.iter().copied().collect::<BTreeSet<_>>();
    if permissions.is_empty()
        || permissions.len() != values.len()
        || permissions
            .iter()
            .any(|permission| permission.is_empty() || !ALLOWED_PERMISSIONS.contains(permission))
    {
        return Err(
            "permissions must be a non-empty unique comma-separated allowed set".to_owned(),
        );
    }
    Ok(permissions)
}

fn read_incident_reason(path: &Path) -> Result<String, String> {
    let reason = read_private_text(path, "incident reason")?;
    validate_claim("incident reason", &reason, 8, 1_024)?;
    Ok(reason)
}

fn required<'a>(options: &'a BTreeMap<String, String>, name: &str) -> Result<&'a str, String> {
    options
        .get(name)
        .map(String::as_str)
        .ok_or_else(|| format!("--{name} is required"))
}

fn required_path(options: &BTreeMap<String, String>, name: &str) -> Result<PathBuf, String> {
    let path = PathBuf::from(required(options, name)?);
    if !path.is_absolute() {
        return Err(format!("--{name} must be an absolute path"));
    }
    Ok(path)
}

fn read_secret(path: &Path) -> Result<String, String> {
    let secret = read_private_text(path, "secret")?;
    if secret.len() != 64 || !secret.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("secret file must contain exactly 64 hexadecimal characters".to_owned());
    }
    Ok(secret)
}

fn read_private_text(path: &Path, label: &str) -> Result<String, String> {
    let mut file = File::open(path).map_err(|_| "secret file could not be opened".to_owned())?;
    let metadata = file
        .metadata()
        .map_err(|_| "secret file metadata is unavailable".to_owned())?;
    if !metadata.is_file() || metadata.mode() & 0o777 != 0o600 {
        return Err(format!("{label} file must be regular and mode 0600"));
    }
    let mut source = String::new();
    file.read_to_string(&mut source)
        .map_err(|_| format!("{label} file could not be read"))?;
    Ok(source.trim_end_matches(['\r', '\n']).to_owned())
}

fn sign(secret: &str, claims: serde_json::Value) -> Result<String, String> {
    let signing =
        SigningKey::from_bytes(&blake3::derive_key(OPERATOR_KEY_DOMAIN, secret.as_bytes()));
    let digest = blake3::hash(signing.verifying_key().as_bytes())
        .to_hex()
        .to_string();
    let header = json!({
        "alg": "EdDSA",
        "typ": "JWT",
        "kid": format!("oprkid_{}", &digest[..16]),
    });
    let header = URL_SAFE_NO_PAD
        .encode(serde_json::to_vec(&header).map_err(|_| "session header is invalid".to_owned())?);
    let claims = URL_SAFE_NO_PAD
        .encode(serde_json::to_vec(&claims).map_err(|_| "session claims are invalid".to_owned())?);
    let signed = format!("{header}.{claims}");
    let signature = signing.sign(signed.as_bytes());
    Ok(format!(
        "{signed}.{}",
        URL_SAFE_NO_PAD.encode(signature.to_bytes())
    ))
}

fn write_private_new(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .map_err(|_| "output file must not already exist and must be creatable".to_owned())?;
    output
        .write_all(bytes)
        .and_then(|()| output.sync_all())
        .map_err(|_| "session output could not be written".to_owned())?;
    Ok(())
}

fn validate_claim(name: &str, value: &str, minimum: usize, maximum: usize) -> Result<(), String> {
    if !(minimum..=maximum).contains(&value.len())
        || value.trim() != value
        || value.chars().any(char::is_control)
    {
        return Err(format!("{name} is invalid"));
    }
    Ok(())
}

fn validate_prefixed_identifier(name: &str, value: &str, prefix: &str) -> Result<(), String> {
    let Some(suffix) = value.strip_prefix(prefix) else {
        return Err(format!("{name} is invalid"));
    };
    if !(8..=96).contains(&suffix.len())
        || !suffix
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(format!("{name} is invalid"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn permissions_are_least_privilege_and_reject_unknown_values() {
        assert!(parse_permissions("tenant_read,waitlist_review").is_ok());
        assert!(parse_permissions("tenant_read").is_ok());
        assert!(parse_permissions("").is_err());
        assert!(parse_permissions("waitlist_review,root").is_err());
        assert!(parse_permissions("waitlist_review,waitlist_review").is_err());
    }

    #[test]
    fn identifiers_and_ttls_are_bounded() {
        assert!(validate_prefixed_identifier("operator", "opr_publicbeta", "opr_").is_ok());
        assert!(validate_prefixed_identifier("operator", "opr_short", "opr_").is_err());
        assert!((MINIMUM_TTL_SECONDS..=MAXIMUM_TTL_SECONDS).contains(&3_600));
    }
}
