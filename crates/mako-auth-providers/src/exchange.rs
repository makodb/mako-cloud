use std::{error::Error, fmt, time::Duration};

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use mako_http_client::{HttpClient, HttpClientConfig, HttpClientError};
use ring::signature;
use serde::Deserialize;
use serde_json::Value;
use url::Url;

use crate::{ProviderConfig, ProviderKind};

const GITHUB_AUTHORIZE: &str = "https://github.com/login/oauth/authorize";
const GITHUB_TOKEN: &str = "https://github.com/login/oauth/access_token";
const GITHUB_USER: &str = "https://api.github.com/user";
const GITHUB_EMAILS: &str = "https://api.github.com/user/emails";
const ID_TOKEN_CLOCK_SKEW_SECONDS: u64 = 120;

/// What a completed flow proves about the person: a stable subject at the
/// provider and, when the provider vouches for it, an email address.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProviderIdentity {
    pub provider: String,
    pub subject: String,
    pub email: Option<String>,
    pub email_verified: bool,
    pub display_name: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProviderExchangeError {
    /// The provider's discovery document or endpoints are unusable.
    Discovery,
    /// The provider refused the code or the request could not be made.
    Exchange,
    /// The id token or user record did not verify.
    Identity(&'static str),
    Unavailable,
}

impl fmt::Display for ProviderExchangeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Discovery => formatter.write_str("provider discovery failed"),
            Self::Exchange => formatter.write_str("provider refused the authorization code"),
            Self::Identity(reason) => {
                write!(formatter, "provider identity did not verify: {reason}")
            }
            Self::Unavailable => formatter.write_str("provider is unavailable"),
        }
    }
}

impl Error for ProviderExchangeError {}

#[derive(Clone, Debug)]
pub struct ProviderClientConfig {
    /// Admit plain HTTP to loopback providers: stubs outside production.
    pub allow_plain_http_loopback: bool,
    pub timeout: Duration,
}

impl Default for ProviderClientConfig {
    fn default() -> Self {
        Self {
            allow_plain_http_loopback: false,
            timeout: Duration::from_secs(15),
        }
    }
}

/// Talks to providers: discovery, the authorization URL, the code exchange,
/// and whatever identity call the protocol needs.
#[derive(Clone, Debug)]
pub struct ProviderClient {
    http: HttpClient,
}

#[derive(Clone, Debug, Deserialize)]
struct OidcDiscovery {
    issuer: String,
    authorization_endpoint: String,
    token_endpoint: String,
    jwks_uri: String,
    #[serde(default)]
    userinfo_endpoint: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
struct TokenResponse {
    #[serde(default)]
    access_token: Option<String>,
    #[serde(default)]
    id_token: Option<String>,
    #[serde(default)]
    error: Option<String>,
}

impl ProviderClient {
    #[must_use]
    pub fn new(config: ProviderClientConfig) -> Self {
        Self {
            http: HttpClient::new(HttpClientConfig {
                connect_timeout: Duration::from_secs(5),
                io_timeout: config.timeout,
                maximum_response_bytes: 256 * 1024,
                allow_plain_http_loopback: config.allow_plain_http_loopback,
            }),
        }
    }

    /// Where the browser goes to sign in with `provider`.
    pub fn authorization_url(
        &self,
        provider: &ProviderConfig,
        redirect_uri: &str,
        state: &str,
        nonce: &str,
    ) -> Result<String, ProviderExchangeError> {
        let mut url = match &provider.kind {
            ProviderKind::Oidc { issuer } => {
                Url::parse(&self.discover(issuer)?.authorization_endpoint)
                    .map_err(|_| ProviderExchangeError::Discovery)?
            }
            ProviderKind::GitHub => {
                Url::parse(GITHUB_AUTHORIZE).map_err(|_| ProviderExchangeError::Discovery)?
            }
        };
        let scopes = match &provider.kind {
            ProviderKind::Oidc { .. } => {
                let mut scopes = vec!["openid", "email", "profile"];
                scopes.extend(provider.scopes.iter().map(String::as_str));
                scopes.join(" ")
            }
            ProviderKind::GitHub => "read:user user:email".to_owned(),
        };
        {
            let mut query = url.query_pairs_mut();
            query.append_pair("response_type", "code");
            query.append_pair("client_id", &provider.client_id);
            query.append_pair("redirect_uri", redirect_uri);
            query.append_pair("scope", &scopes);
            query.append_pair("state", state);
            if matches!(provider.kind, ProviderKind::Oidc { .. }) {
                query.append_pair("nonce", nonce);
            }
        }
        Ok(url.into())
    }

    /// Redeems the callback's code and verifies who the provider says it was.
    pub fn complete(
        &self,
        provider: &ProviderConfig,
        client_secret: &str,
        code: &str,
        redirect_uri: &str,
        expected_nonce: &str,
        now_unix_seconds: u64,
    ) -> Result<ProviderIdentity, ProviderExchangeError> {
        if code.is_empty() || code.len() > 2048 || !code.bytes().all(|byte| byte.is_ascii_graphic())
        {
            return Err(ProviderExchangeError::Exchange);
        }
        match &provider.kind {
            ProviderKind::Oidc { issuer } => self.complete_oidc(
                provider,
                issuer,
                client_secret,
                code,
                redirect_uri,
                expected_nonce,
                now_unix_seconds,
            ),
            ProviderKind::GitHub => {
                self.complete_github(provider, client_secret, code, redirect_uri)
            }
        }
    }

    /// GitHub's endpoints are fixed; tests point them at a stub.
    #[cfg(test)]
    fn complete_github_at(
        &self,
        base: &str,
        client_id: &str,
        client_secret: &str,
        code: &str,
        redirect_uri: &str,
    ) -> Result<ProviderIdentity, ProviderExchangeError> {
        let provider = ProviderConfig {
            name: "github".to_owned(),
            kind: ProviderKind::GitHub,
            client_id: client_id.to_owned(),
            client_secret: crate::SealedSecret {
                nonce: String::new(),
                ciphertext: String::new(),
            },
            scopes: Vec::new(),
            enabled: true,
        };
        self.github_flow(
            &provider,
            client_secret,
            code,
            redirect_uri,
            &format!("{base}/login/oauth/access_token"),
            &format!("{base}/user"),
            &format!("{base}/user/emails"),
        )
    }

    fn discover(&self, issuer: &str) -> Result<OidcDiscovery, ProviderExchangeError> {
        let mut url = Url::parse(issuer).map_err(|_| ProviderExchangeError::Discovery)?;
        let base = url.path().trim_end_matches('/').to_owned();
        url.set_path(&format!("{base}/.well-known/openid-configuration"));
        let response = self
            .http
            .request("GET", &url, &[("accept", "application/json")], b"")
            .map_err(map_http)?;
        if response.status != 200 {
            return Err(ProviderExchangeError::Discovery);
        }
        let discovery: OidcDiscovery =
            serde_json::from_slice(&response.body).map_err(|_| ProviderExchangeError::Discovery)?;
        if discovery.issuer.trim_end_matches('/') != issuer.trim_end_matches('/') {
            return Err(ProviderExchangeError::Discovery);
        }
        Ok(discovery)
    }

    #[allow(clippy::too_many_arguments)]
    fn complete_oidc(
        &self,
        provider: &ProviderConfig,
        issuer: &str,
        client_secret: &str,
        code: &str,
        redirect_uri: &str,
        expected_nonce: &str,
        now_unix_seconds: u64,
    ) -> Result<ProviderIdentity, ProviderExchangeError> {
        let discovery = self.discover(issuer)?;
        let token_url =
            Url::parse(&discovery.token_endpoint).map_err(|_| ProviderExchangeError::Discovery)?;
        let body = form(&[
            ("grant_type", "authorization_code"),
            ("code", code),
            ("redirect_uri", redirect_uri),
            ("client_id", &provider.client_id),
            ("client_secret", client_secret),
        ]);
        let response = self
            .http
            .request(
                "POST",
                &token_url,
                &[
                    ("content-type", "application/x-www-form-urlencoded"),
                    ("accept", "application/json"),
                ],
                body.as_bytes(),
            )
            .map_err(map_http)?;
        let tokens: TokenResponse =
            serde_json::from_slice(&response.body).map_err(|_| ProviderExchangeError::Exchange)?;
        if response.status != 200 || tokens.error.is_some() {
            return Err(ProviderExchangeError::Exchange);
        }
        let id_token = tokens
            .id_token
            .ok_or(ProviderExchangeError::Identity("no id token"))?;
        let jwks_url =
            Url::parse(&discovery.jwks_uri).map_err(|_| ProviderExchangeError::Discovery)?;
        let jwks = self
            .http
            .request("GET", &jwks_url, &[("accept", "application/json")], b"")
            .map_err(map_http)?;
        if jwks.status != 200 {
            return Err(ProviderExchangeError::Discovery);
        }
        let jwks: Value =
            serde_json::from_slice(&jwks.body).map_err(|_| ProviderExchangeError::Discovery)?;
        let claims = verify_id_token(
            &id_token,
            &jwks,
            &discovery.issuer,
            &provider.client_id,
            expected_nonce,
            now_unix_seconds,
        )?;
        let mut identity = ProviderIdentity {
            provider: provider.name.clone(),
            subject: claims
                .get("sub")
                .and_then(Value::as_str)
                .filter(|sub| !sub.is_empty() && sub.len() <= 1024)
                .ok_or(ProviderExchangeError::Identity("no subject"))?
                .to_owned(),
            email: claims
                .get("email")
                .and_then(Value::as_str)
                .map(str::to_owned),
            email_verified: claims
                .get("email_verified")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            display_name: claims
                .get("name")
                .and_then(Value::as_str)
                .map(str::to_owned),
        };
        // Some providers put the email on userinfo rather than the id token.
        if identity.email.is_none()
            && let (Some(endpoint), Some(access_token)) =
                (&discovery.userinfo_endpoint, &tokens.access_token)
            && let Ok(userinfo_url) = Url::parse(endpoint)
            && let Ok(userinfo) = self.http.request(
                "GET",
                &userinfo_url,
                &[
                    ("authorization", &format!("Bearer {access_token}")),
                    ("accept", "application/json"),
                ],
                b"",
            )
            && userinfo.status == 200
            && let Ok(info) = serde_json::from_slice::<Value>(&userinfo.body)
            && info.get("sub").and_then(Value::as_str) == Some(identity.subject.as_str())
        {
            identity.email = info.get("email").and_then(Value::as_str).map(str::to_owned);
            identity.email_verified = info
                .get("email_verified")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            if identity.display_name.is_none() {
                identity.display_name = info.get("name").and_then(Value::as_str).map(str::to_owned);
            }
        }
        Ok(identity)
    }

    fn complete_github(
        &self,
        provider: &ProviderConfig,
        client_secret: &str,
        code: &str,
        redirect_uri: &str,
    ) -> Result<ProviderIdentity, ProviderExchangeError> {
        self.github_flow(
            provider,
            client_secret,
            code,
            redirect_uri,
            GITHUB_TOKEN,
            GITHUB_USER,
            GITHUB_EMAILS,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn github_flow(
        &self,
        provider: &ProviderConfig,
        client_secret: &str,
        code: &str,
        redirect_uri: &str,
        token_endpoint: &str,
        user_endpoint: &str,
        emails_endpoint: &str,
    ) -> Result<ProviderIdentity, ProviderExchangeError> {
        let token_url = Url::parse(token_endpoint).map_err(|_| ProviderExchangeError::Discovery)?;
        let body = form(&[
            ("client_id", &provider.client_id),
            ("client_secret", client_secret),
            ("code", code),
            ("redirect_uri", redirect_uri),
        ]);
        let response = self
            .http
            .request(
                "POST",
                &token_url,
                &[
                    ("content-type", "application/x-www-form-urlencoded"),
                    ("accept", "application/json"),
                ],
                body.as_bytes(),
            )
            .map_err(map_http)?;
        let tokens: TokenResponse =
            serde_json::from_slice(&response.body).map_err(|_| ProviderExchangeError::Exchange)?;
        let access_token = match (response.status, tokens.error, tokens.access_token) {
            (200, None, Some(token)) => token,
            _ => return Err(ProviderExchangeError::Exchange),
        };
        let headers = [
            ("authorization", format!("Bearer {access_token}")),
            ("accept", "application/vnd.github+json".to_owned()),
            ("user-agent", "mako-cloud".to_owned()),
        ];
        let header_refs: Vec<(&str, &str)> =
            headers.iter().map(|(n, v)| (*n, v.as_str())).collect();
        let user_url = Url::parse(user_endpoint).map_err(|_| ProviderExchangeError::Discovery)?;
        let user = self
            .http
            .request("GET", &user_url, &header_refs, b"")
            .map_err(map_http)?;
        if user.status != 200 {
            return Err(ProviderExchangeError::Identity("user lookup failed"));
        }
        let user: Value = serde_json::from_slice(&user.body)
            .map_err(|_| ProviderExchangeError::Identity("user record"))?;
        let subject = user
            .get("id")
            .and_then(|id| {
                id.as_u64()
                    .map(|n| n.to_string())
                    .or_else(|| id.as_str().map(str::to_owned))
            })
            .ok_or(ProviderExchangeError::Identity("no subject"))?;
        let emails_url =
            Url::parse(emails_endpoint).map_err(|_| ProviderExchangeError::Discovery)?;
        let emails = self
            .http
            .request("GET", &emails_url, &header_refs, b"")
            .map_err(map_http)?;
        let mut email = None;
        let mut email_verified = false;
        if emails.status == 200
            && let Ok(Value::Array(entries)) = serde_json::from_slice::<Value>(&emails.body)
        {
            let primary = entries
                .iter()
                .find(|entry| entry.get("primary").and_then(Value::as_bool) == Some(true))
                .or_else(|| entries.first());
            if let Some(entry) = primary {
                email = entry
                    .get("email")
                    .and_then(Value::as_str)
                    .map(str::to_owned);
                email_verified = entry
                    .get("verified")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
            }
        }
        Ok(ProviderIdentity {
            provider: provider.name.clone(),
            subject,
            email,
            email_verified,
            display_name: user
                .get("name")
                .and_then(Value::as_str)
                .or_else(|| user.get("login").and_then(Value::as_str))
                .map(str::to_owned),
        })
    }
}

/// Verifies a compact JWS id token against the provider's JWKS and the OIDC
/// core claims: issuer, audience, expiry, and the nonce this flow issued.
pub(crate) fn verify_id_token(
    token: &str,
    jwks: &Value,
    issuer: &str,
    client_id: &str,
    expected_nonce: &str,
    now_unix_seconds: u64,
) -> Result<serde_json::Map<String, Value>, ProviderExchangeError> {
    let mut parts = token.split('.');
    let (header, payload, signature) =
        match (parts.next(), parts.next(), parts.next(), parts.next()) {
            (Some(header), Some(payload), Some(signature), None) => (header, payload, signature),
            _ => {
                return Err(ProviderExchangeError::Identity(
                    "id token is not a compact JWS",
                ));
            }
        };
    let header_json: Value = serde_json::from_slice(
        &URL_SAFE_NO_PAD
            .decode(header)
            .map_err(|_| ProviderExchangeError::Identity("header"))?,
    )
    .map_err(|_| ProviderExchangeError::Identity("header"))?;
    let alg = header_json.get("alg").and_then(Value::as_str).unwrap_or("");
    let kid = header_json.get("kid").and_then(Value::as_str);
    let keys = jwks
        .get("keys")
        .and_then(Value::as_array)
        .ok_or(ProviderExchangeError::Identity("jwks"))?;
    let key = keys
        .iter()
        .find(|key| kid.is_none_or(|kid| key.get("kid").and_then(Value::as_str) == Some(kid)))
        .ok_or(ProviderExchangeError::Identity("no matching key"))?;
    let signed = format!("{header}.{payload}");
    let signature = URL_SAFE_NO_PAD
        .decode(signature)
        .map_err(|_| ProviderExchangeError::Identity("signature"))?;
    match (alg, key.get("kty").and_then(Value::as_str)) {
        ("RS256", Some("RSA")) => {
            let n = jwk_bytes(key, "n")?;
            let e = jwk_bytes(key, "e")?;
            signature::RsaPublicKeyComponents { n: &n, e: &e }
                .verify(
                    &signature::RSA_PKCS1_2048_8192_SHA256,
                    signed.as_bytes(),
                    &signature,
                )
                .map_err(|_| ProviderExchangeError::Identity("signature did not verify"))?;
        }
        ("ES256", Some("EC")) => {
            if key.get("crv").and_then(Value::as_str) != Some("P-256") {
                return Err(ProviderExchangeError::Identity("unsupported curve"));
            }
            let x = jwk_bytes(key, "x")?;
            let y = jwk_bytes(key, "y")?;
            let mut point = Vec::with_capacity(65);
            point.push(0x04);
            point.extend_from_slice(&x);
            point.extend_from_slice(&y);
            signature::UnparsedPublicKey::new(&signature::ECDSA_P256_SHA256_FIXED, point)
                .verify(signed.as_bytes(), &signature)
                .map_err(|_| ProviderExchangeError::Identity("signature did not verify"))?;
        }
        _ => return Err(ProviderExchangeError::Identity("unsupported algorithm")),
    }
    let claims: serde_json::Map<String, Value> = serde_json::from_slice(
        &URL_SAFE_NO_PAD
            .decode(payload)
            .map_err(|_| ProviderExchangeError::Identity("claims"))?,
    )
    .map_err(|_| ProviderExchangeError::Identity("claims"))?;
    if claims
        .get("iss")
        .and_then(Value::as_str)
        .map(|iss| iss.trim_end_matches('/'))
        != Some(issuer.trim_end_matches('/'))
    {
        return Err(ProviderExchangeError::Identity("issuer"));
    }
    let audience_ok = match claims.get("aud") {
        Some(Value::String(aud)) => aud == client_id,
        Some(Value::Array(auds)) => auds.iter().any(|aud| aud.as_str() == Some(client_id)),
        _ => false,
    };
    if !audience_ok {
        return Err(ProviderExchangeError::Identity("audience"));
    }
    let exp = claims
        .get("exp")
        .and_then(Value::as_u64)
        .ok_or(ProviderExchangeError::Identity("expiry"))?;
    if exp.saturating_add(ID_TOKEN_CLOCK_SKEW_SECONDS) < now_unix_seconds {
        return Err(ProviderExchangeError::Identity("expired"));
    }
    if let Some(iat) = claims.get("iat").and_then(Value::as_u64)
        && iat > now_unix_seconds.saturating_add(ID_TOKEN_CLOCK_SKEW_SECONDS)
    {
        return Err(ProviderExchangeError::Identity("issued in the future"));
    }
    if claims.get("nonce").and_then(Value::as_str) != Some(expected_nonce) {
        return Err(ProviderExchangeError::Identity("nonce"));
    }
    Ok(claims)
}

fn jwk_bytes(key: &Value, field: &str) -> Result<Vec<u8>, ProviderExchangeError> {
    key.get(field)
        .and_then(Value::as_str)
        .and_then(|value| URL_SAFE_NO_PAD.decode(value).ok())
        .ok_or(ProviderExchangeError::Identity("jwk"))
}

fn form(pairs: &[(&str, &str)]) -> String {
    let mut serializer = url::form_urlencoded::Serializer::new(String::new());
    for (name, value) in pairs {
        serializer.append_pair(name, value);
    }
    serializer.finish()
}

fn map_http(error: HttpClientError) -> ProviderExchangeError {
    match error {
        HttpClientError::UnsupportedUrl => ProviderExchangeError::Discovery,
        HttpClientError::Response => ProviderExchangeError::Exchange,
        HttpClientError::Connect | HttpClientError::Tls | HttpClientError::Io => {
            ProviderExchangeError::Unavailable
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{
        collections::BTreeMap,
        io::{Read, Write},
        net::TcpListener,
        sync::{Arc, Mutex},
        thread,
    };

    use mako_api::{EnvironmentId, ProjectId, TenantScope};
    use ring::{
        rand::SystemRandom,
        signature::{ECDSA_P256_SHA256_FIXED_SIGNING, EcdsaKeyPair, KeyPair},
    };
    use serde_json::json;

    use super::*;
    use crate::{FlowStateError, FlowStateKey, FlowStateVerifier, ProviderSecretKey, SealedSecret};

    /// A stand-in identity provider on loopback: discovery, token, JWKS,
    /// userinfo for OIDC; token, user, and emails for the GitHub shape.
    struct ProviderStub {
        endpoint: String,
        requests: Arc<Mutex<Vec<(String, String)>>>,
        routes: Arc<Mutex<BTreeMap<&'static str, (u16, String)>>>,
    }

    impl ProviderStub {
        fn set_routes(&self, routes: BTreeMap<&'static str, (u16, String)>) {
            *self.routes.lock().expect("routes") = routes;
        }
    }

    fn serve(initial: BTreeMap<&'static str, (u16, String)>) -> ProviderStub {
        let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
        let port = listener.local_addr().expect("address").port();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let seen = Arc::clone(&requests);
        let routes = Arc::new(Mutex::new(initial));
        let table = Arc::clone(&routes);
        thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { break };
                let mut raw = Vec::new();
                let mut buffer = [0u8; 8192];
                loop {
                    let read = stream.read(&mut buffer).unwrap_or(0);
                    if read == 0 {
                        break;
                    }
                    raw.extend_from_slice(&buffer[..read]);
                    if let Some(end) = raw.windows(4).position(|w| w == b"\r\n\r\n") {
                        let head = String::from_utf8_lossy(&raw[..end]).to_string();
                        let length: usize = head
                            .lines()
                            .find_map(|line| {
                                line.to_ascii_lowercase()
                                    .strip_prefix("content-length: ")
                                    .and_then(|v| v.trim().parse().ok())
                            })
                            .unwrap_or(0);
                        if raw.len() >= end + 4 + length {
                            break;
                        }
                    }
                }
                let text = String::from_utf8_lossy(&raw).to_string();
                let path = text
                    .lines()
                    .next()
                    .and_then(|l| l.split(' ').nth(1))
                    .unwrap_or("/")
                    .split('?')
                    .next()
                    .unwrap_or("/")
                    .to_owned();
                let body = text.split("\r\n\r\n").nth(1).unwrap_or("").to_owned();
                seen.lock().expect("lock").push((path.clone(), body));
                let (status, payload) = table
                    .lock()
                    .expect("routes")
                    .get(path.as_str())
                    .cloned()
                    .unwrap_or((404, "{}".to_owned()));
                let _ = write!(
                    stream,
                    "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{payload}",
                    payload.len()
                );
            }
        });
        ProviderStub {
            endpoint: format!("http://127.0.0.1:{port}"),
            requests,
            routes,
        }
    }

    fn tenant() -> TenantScope {
        TenantScope::new(
            ProjectId::parse("prj_providers0").expect("project"),
            EnvironmentId::parse("env_providers0").expect("environment"),
        )
    }

    fn client() -> ProviderClient {
        ProviderClient::new(ProviderClientConfig {
            allow_plain_http_loopback: true,
            ..ProviderClientConfig::default()
        })
    }

    fn es256_id_token(key: &EcdsaKeyPair, claims: &Value) -> (String, Value) {
        let header = URL_SAFE_NO_PAD.encode(br#"{"alg":"ES256","kid":"stub-1","typ":"JWT"}"#);
        let payload = URL_SAFE_NO_PAD.encode(serde_json::to_vec(claims).expect("claims"));
        let signed = format!("{header}.{payload}");
        let signature = key
            .sign(&SystemRandom::new(), signed.as_bytes())
            .expect("sign");
        let public = key.public_key().as_ref();
        let jwks = json!({ "keys": [{
            "kty": "EC", "crv": "P-256", "kid": "stub-1", "alg": "ES256",
            "x": URL_SAFE_NO_PAD.encode(&public[1..33]),
            "y": URL_SAFE_NO_PAD.encode(&public[33..65]),
        }]});
        (
            format!("{signed}.{}", URL_SAFE_NO_PAD.encode(signature.as_ref())),
            jwks,
        )
    }

    #[test]
    fn an_oidc_flow_verifies_the_id_token_and_yields_the_verified_email() {
        let random = SystemRandom::new();
        let pkcs8 =
            EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &random).expect("key");
        let key =
            EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, pkcs8.as_ref(), &random)
                .expect("pair");
        // The issuer is the stub itself, so its URL is known only once it
        // listens; the routes are set afterwards.
        let stub = serve(BTreeMap::new());
        let issuer = stub.endpoint.clone();
        let (id_token, jwks) = es256_id_token(
            &key,
            &json!({ "iss": issuer, "aud": "client-1", "sub": "user-42", "exp": 2_000_000_000u64, "iat": 1_700_000_000u64, "nonce": "nonce-abc", "email": "alice@example.test", "email_verified": true, "name": "Alice" }),
        );
        stub.set_routes(BTreeMap::from([
            ("/.well-known/openid-configuration", (200, json!({ "issuer": issuer, "authorization_endpoint": format!("{issuer}/authorize"), "token_endpoint": format!("{issuer}/token"), "jwks_uri": format!("{issuer}/jwks"), "userinfo_endpoint": format!("{issuer}/userinfo") }).to_string())),
            ("/token", (200, json!({ "access_token": "at-1", "id_token": id_token, "token_type": "Bearer" }).to_string())),
            ("/jwks", (200, jwks.to_string())),
        ]));
        let secret_key = ProviderSecretKey::derive(b"internal-secret-for-tests-0123456789");
        let provider = ProviderConfig {
            name: "stub".to_owned(),
            kind: ProviderKind::Oidc {
                issuer: issuer.clone(),
            },
            client_id: "client-1".to_owned(),
            client_secret: secret_key.seal(&tenant(), "stub", "s3cret").expect("seal"),
            scopes: Vec::new(),
            enabled: true,
        };
        let client = client();
        let url = client
            .authorization_url(
                &provider,
                "https://app.example/callback",
                "state-1",
                "nonce-abc",
            )
            .expect("authorization url");
        assert!(
            url.starts_with(&format!(
                "{issuer}/authorize?response_type=code&client_id=client-1"
            )),
            "{url}"
        );
        assert!(url.contains("scope=openid+email+profile") && url.contains("nonce=nonce-abc"));
        let secret = secret_key
            .open(&tenant(), "stub", &provider.client_secret)
            .expect("open");
        let identity = client
            .complete(
                &provider,
                &secret,
                "code-xyz",
                "https://app.example/callback",
                "nonce-abc",
                1_800_000_000,
            )
            .expect("identity");
        assert_eq!(identity.subject, "user-42");
        assert_eq!(identity.email.as_deref(), Some("alice@example.test"));
        assert!(identity.email_verified);
        let requests = stub.requests.lock().expect("lock");
        let token_request = requests
            .iter()
            .find(|(path, _)| path == "/token")
            .expect("token call");
        assert!(
            token_request.1.contains("code=code-xyz")
                && token_request.1.contains("client_secret=s3cret")
        );
        drop(requests);
        assert!(matches!(
            client.complete(
                &provider,
                &secret,
                "code-xyz",
                "https://app.example/callback",
                "other-nonce",
                1_800_000_000
            ),
            Err(ProviderExchangeError::Identity("nonce"))
        ));
        assert!(matches!(
            client.complete(
                &provider,
                &secret,
                "code-xyz",
                "https://app.example/callback",
                "nonce-abc",
                2_100_000_000
            ),
            Err(ProviderExchangeError::Identity("expired"))
        ));
        let mut wrong_audience = provider.clone();
        wrong_audience.client_id = "client-2".to_owned();
        assert!(matches!(
            client.complete(
                &wrong_audience,
                &secret,
                "code-xyz",
                "https://app.example/callback",
                "nonce-abc",
                1_800_000_000
            ),
            Err(ProviderExchangeError::Identity("audience"))
        ));
        // A sealed secret opens only for its tenant and provider.
        let other = TenantScope::new(
            ProjectId::parse("prj_providers0").expect("p"),
            EnvironmentId::parse("env_elsewhere0").expect("e"),
        );
        assert!(
            secret_key
                .open(&other, "stub", &provider.client_secret)
                .is_err()
        );
        assert!(
            secret_key
                .open(&tenant(), "other", &provider.client_secret)
                .is_err()
        );
        assert!(
            ProviderSecretKey::derive(b"another-secret-0123456789abcdefgh")
                .open(&tenant(), "stub", &provider.client_secret)
                .is_err()
        );
    }

    #[test]
    fn a_github_style_flow_reads_the_primary_verified_email() {
        let stub = serve(BTreeMap::from([
            ("/login/oauth/access_token", (200, json!({ "access_token": "gh-token", "token_type": "bearer" }).to_string())),
            ("/user", (200, json!({ "id": 4242, "login": "octo", "name": "Octo Cat" }).to_string())),
            ("/user/emails", (200, json!([{ "email": "old@example.test", "primary": false, "verified": true }, { "email": "octo@example.test", "primary": true, "verified": true }]).to_string())),
        ]));
        // The GitHub client talks to fixed public hosts; point them at the stub
        // through the test-only override.
        let client = client();
        let identity = client
            .complete_github_at(
                &stub.endpoint,
                "client-gh",
                "gh-secret",
                "code-1",
                "https://app.example/cb",
            )
            .expect("identity");
        assert_eq!(identity.subject, "4242");
        assert_eq!(identity.email.as_deref(), Some("octo@example.test"));
        assert!(identity.email_verified);
        assert_eq!(identity.display_name.as_deref(), Some("Octo Cat"));
        let requests = stub.requests.lock().expect("lock");
        assert!(
            requests
                .iter()
                .any(|(path, body)| path == "/login/oauth/access_token"
                    && body.contains("client_secret=gh-secret"))
        );
        assert!(!stub.endpoint.is_empty());
    }

    #[test]
    fn flow_state_round_trips_and_refuses_tampering() {
        let key = FlowStateKey::derive(b"internal-secret-for-tests-0123456789");
        let verifier = FlowStateVerifier::new(key);
        let (state, token) = verifier
            .issue(&tenant(), "google", "https://app.example/cb", 1_000)
            .expect("issue");
        let back = verifier.verify(&tenant(), &token, 1_100).expect("verify");
        assert_eq!(back, state);
        assert!(matches!(
            verifier.verify(&tenant(), &token, 1_000 + 601),
            Err(FlowStateError::Expired)
        ));
        let other = TenantScope::new(
            ProjectId::parse("prj_providers0").expect("p"),
            EnvironmentId::parse("env_elsewhere0").expect("e"),
        );
        assert!(matches!(
            verifier.verify(&other, &token, 1_100),
            Err(FlowStateError::ScopeMismatch)
        ));
        let mut tampered = token.clone();
        tampered.replace_range(0..1, if token.starts_with('A') { "B" } else { "A" });
        assert!(matches!(
            verifier.verify(&tenant(), &tampered, 1_100),
            Err(FlowStateError::BadSignature | FlowStateError::Malformed)
        ));
        assert!(verifier.verify(&tenant(), "nonsense", 1_100).is_err());
        let sealed = SealedSecret {
            nonce: "x".to_owned(),
            ciphertext: "y".to_owned(),
        };
        assert!(
            ProviderSecretKey::derive(b"k0123456789012345678901234567890")
                .open(&tenant(), "p", &sealed)
                .is_err()
        );
    }
}
