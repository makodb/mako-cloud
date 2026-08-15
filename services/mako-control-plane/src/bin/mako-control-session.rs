//! Issues a short-lived, file-only developer session for deployment operations.

#![forbid(unsafe_code)]

use std::{
    collections::BTreeMap,
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

const DEVELOPER_KEY_DOMAIN: &str = "mako/control-plane/developer-session-signing/v1";
const DEVELOPER_AUDIENCE: &str = "mako-management";
const MINIMUM_TTL_SECONDS: u64 = 60;
const MAXIMUM_TTL_SECONDS: u64 = 3_600;

fn main() -> ExitCode {
    match run(env::args().skip(1)) {
        Ok(output) => {
            println!(
                "wrote short-lived developer session to {}",
                output.display()
            );
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("developer session was not issued: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run(arguments: impl IntoIterator<Item = String>) -> Result<PathBuf, String> {
    let options = parse_options(arguments)?;
    let secret_path = required_path(&options, "secret-file")?;
    let output_path = required_path(&options, "output")?;
    let issuer = required(&options, "issuer")?;
    let identity_id = required(&options, "identity-id")?;
    let email = required(&options, "email")?;
    let display_name = required(&options, "display-name")?;
    let authorization_epoch = required(&options, "authorization-epoch")?
        .parse::<u64>()
        .map_err(|_| "authorization-epoch must be a positive integer".to_owned())?;
    let ttl_seconds = required(&options, "ttl-seconds")?
        .parse::<u64>()
        .map_err(|_| "ttl-seconds must be an integer".to_owned())?;

    validate_claim("issuer", issuer, 8, 2_048)?;
    validate_prefixed_identifier("identity-id", identity_id, "dev_")?;
    validate_claim("email", email, 3, 320)?;
    validate_claim("display-name", display_name, 1, 200)?;
    if authorization_epoch == 0 {
        return Err("authorization-epoch must be a positive integer".to_owned());
    }
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
    let session_digest =
        blake3::hash(format!("{identity_id}:{now}:{}", std::process::id()).as_bytes())
            .to_hex()
            .to_string();
    let token = sign(
        &secret,
        json!({
            "iss": issuer,
            "sub": email,
            "aud": [DEVELOPER_AUDIENCE],
            "email": email,
            "emailVerified": true,
            "name": display_name,
            "sid": format!("session_{}", &session_digest[..20]),
            "developerIdentityId": identity_id,
            "status": "active",
            "authorizationEpoch": authorization_epoch,
            "iat": now,
            "exp": expires,
        }),
    )?;
    write_private_new(&output_path, token.as_bytes())?;
    Ok(output_path)
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
        "authorization-epoch",
        "display-name",
        "email",
        "identity-id",
        "issuer",
        "output",
        "secret-file",
        "ttl-seconds",
    ];
    if options.keys().any(|key| !expected.contains(&key.as_str())) {
        return Err("an unsupported option was provided".to_owned());
    }
    Ok(options)
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
    let mut file = File::open(path).map_err(|_| "secret file could not be opened".to_owned())?;
    let metadata = file
        .metadata()
        .map_err(|_| "secret file metadata is unavailable".to_owned())?;
    if !metadata.is_file() || metadata.mode() & 0o777 != 0o600 {
        return Err("secret file must be regular and mode 0600".to_owned());
    }
    let mut source = String::new();
    file.read_to_string(&mut source)
        .map_err(|_| "secret file could not be read".to_owned())?;
    let secret = source.trim_end_matches(['\r', '\n']);
    if secret.len() != 64 || !secret.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("secret file must contain exactly 64 hexadecimal characters".to_owned());
    }
    Ok(secret.to_owned())
}

fn sign(secret: &str, claims: serde_json::Value) -> Result<String, String> {
    let signing =
        SigningKey::from_bytes(&blake3::derive_key(DEVELOPER_KEY_DOMAIN, secret.as_bytes()));
    let digest = blake3::hash(signing.verifying_key().as_bytes())
        .to_hex()
        .to_string();
    let header = json!({
        "alg": "EdDSA",
        "typ": "JWT",
        "kid": format!("devkid_{}", &digest[..16]),
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
        .map_err(|_| "session output could not be written".to_owned())
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
    if !(8..=64).contains(&suffix.len())
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
    fn option_parser_rejects_duplicates_and_unknowns() {
        assert!(parse_options(["--email".to_owned(), "a@b.test".to_owned()]).is_ok());
        assert!(
            parse_options([
                "--email".to_owned(),
                "a@b.test".to_owned(),
                "--email".to_owned(),
                "b@b.test".to_owned(),
            ])
            .is_err()
        );
        assert!(parse_options(["--token".to_owned(), "secret".to_owned()]).is_err());
    }

    #[test]
    fn identifiers_and_ttls_are_bounded() {
        assert!(validate_prefixed_identifier("identity", "dev_publicbeta", "dev_").is_ok());
        assert!(validate_prefixed_identifier("identity", "dev_short", "dev_").is_err());
        assert!(!(MINIMUM_TTL_SECONDS..=MAXIMUM_TTL_SECONDS).contains(&59));
        assert!((MINIMUM_TTL_SECONDS..=MAXIMUM_TTL_SECONDS).contains(&3_600));
    }
}
