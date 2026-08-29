//! Loopback stand-ins for what an application sign-in flow reaches outside
//! the platform: an OpenID Connect provider and an SMTP relay.
use std::{
    io::{BufRead, BufReader, Read, Write},
    net::TcpListener,
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use ring::{
    rand::SystemRandom,
    signature::{ECDSA_P256_SHA256_FIXED_SIGNING, EcdsaKeyPair, KeyPair},
};
use serde_json::json;

/// A stand-in OpenID Connect provider: discovery, token, JWKS and userinfo
/// on loopback, signing ES256 id tokens for one fixed subject. The nonce it
/// puts in the token is whatever the test last announced with
/// [`OidcProviderStub::expect_nonce`] — a real provider echoes the nonce the
/// authorization request carried, and the test plays the browser's part.
pub struct OidcProviderStub {
    pub endpoint: String,
    pub client_id: String,
    pub subject: String,
    pub email: String,
    nonce: Arc<Mutex<Option<String>>>,
    token_requests: Arc<Mutex<Vec<String>>>,
}

impl OidcProviderStub {
    pub fn start(client_id: &str, subject: &str, email: &str) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("provider listener");
        let port = listener.local_addr().expect("provider address").port();
        let endpoint = format!("http://127.0.0.1:{port}");
        let random = SystemRandom::new();
        let pkcs8 =
            EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &random).expect("key");
        let key =
            EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, pkcs8.as_ref(), &random)
                .expect("key pair");
        let public = key.public_key().as_ref().to_vec();
        let jwks = json!({ "keys": [{
            "kty": "EC", "crv": "P-256", "kid": "smoke-1", "alg": "ES256", "use": "sig",
            "x": URL_SAFE_NO_PAD.encode(&public[1..33]),
            "y": URL_SAFE_NO_PAD.encode(&public[33..65]),
        }]});
        let nonce = Arc::new(Mutex::new(None));
        let token_requests = Arc::new(Mutex::new(Vec::new()));
        let stub = Self {
            endpoint: endpoint.clone(),
            client_id: client_id.to_owned(),
            subject: subject.to_owned(),
            email: email.to_owned(),
            nonce: Arc::clone(&nonce),
            token_requests: Arc::clone(&token_requests),
        };
        let (client_id, subject, email) =
            (client_id.to_owned(), subject.to_owned(), email.to_owned());
        thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { break };
                let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
                let Some((path, body)) = read_http_request(&mut stream) else {
                    continue;
                };
                let (status, payload) = match path.as_str() {
                    "/.well-known/openid-configuration" => (
                        200,
                        json!({
                            "issuer": endpoint,
                            "authorization_endpoint": format!("{endpoint}/authorize"),
                            "token_endpoint": format!("{endpoint}/token"),
                            "jwks_uri": format!("{endpoint}/jwks"),
                            "userinfo_endpoint": format!("{endpoint}/userinfo"),
                            "id_token_signing_alg_values_supported": ["ES256"],
                        }),
                    ),
                    "/jwks" => (200, jwks.clone()),
                    "/token" => {
                        token_requests.lock().expect("token requests").push(body);
                        let now = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map_or(0, |elapsed| elapsed.as_secs());
                        let claims = json!({
                            "iss": endpoint,
                            "aud": client_id,
                            "sub": subject,
                            "iat": now,
                            "exp": now + 300,
                            "nonce": nonce.lock().expect("nonce").clone().unwrap_or_default(),
                            "email": email,
                            "email_verified": true,
                            "name": "Smoke Subject",
                        });
                        let header = URL_SAFE_NO_PAD
                            .encode(br#"{"alg":"ES256","kid":"smoke-1","typ":"JWT"}"#);
                        let payload =
                            URL_SAFE_NO_PAD.encode(serde_json::to_vec(&claims).expect("claims"));
                        let signed = format!("{header}.{payload}");
                        let signature = key
                            .sign(&SystemRandom::new(), signed.as_bytes())
                            .expect("sign id token");
                        let id_token =
                            format!("{signed}.{}", URL_SAFE_NO_PAD.encode(signature.as_ref()));
                        (
                            200,
                            json!({ "access_token": "smoke-access-token", "id_token": id_token, "token_type": "Bearer", "expires_in": 300 }),
                        )
                    }
                    "/userinfo" => (
                        200,
                        json!({ "sub": subject, "email": email, "email_verified": true }),
                    ),
                    _ => (404, json!({ "error": "not_found" })),
                };
                let payload = payload.to_string();
                let _ = write!(
                    stream,
                    "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{payload}",
                    payload.len()
                );
                let _ = stream.flush();
            }
        });
        stub
    }

    /// The nonce the next id token carries: the one the platform put in the
    /// authorization URL the test is about to "visit".
    pub fn expect_nonce(&self, nonce: &str) {
        *self.nonce.lock().expect("nonce") = Some(nonce.to_owned());
    }

    /// Bodies of the token requests the platform made, oldest first.
    pub fn token_requests(&self) -> Vec<String> {
        self.token_requests.lock().expect("token requests").clone()
    }
}

fn read_http_request(stream: &mut std::net::TcpStream) -> Option<(String, String)> {
    let mut raw = Vec::new();
    let mut buffer = [0u8; 8192];
    loop {
        let read = stream.read(&mut buffer).unwrap_or(0);
        if read == 0 {
            break;
        }
        raw.extend_from_slice(&buffer[..read]);
        if let Some(end) = raw.windows(4).position(|window| window == b"\r\n\r\n") {
            let head = String::from_utf8_lossy(&raw[..end]).to_string();
            let length: usize = head
                .lines()
                .find_map(|line| {
                    line.to_ascii_lowercase()
                        .strip_prefix("content-length:")
                        .and_then(|value| value.trim().parse().ok())
                })
                .unwrap_or(0);
            if raw.len() >= end + 4 + length {
                break;
            }
        }
    }
    if raw.is_empty() {
        return None;
    }
    let text = String::from_utf8_lossy(&raw).to_string();
    let path = text
        .lines()
        .next()
        .and_then(|line| line.split(' ').nth(1))
        .unwrap_or("/")
        .split('?')
        .next()
        .unwrap_or("/")
        .to_owned();
    let body = text.split("\r\n\r\n").nth(1).unwrap_or("").to_owned();
    Some((path, body))
}

/// One message an SMTP client handed to [`SmtpCaptureStub`].
#[derive(Clone, Debug)]
pub struct CapturedMail {
    pub sender: String,
    pub recipients: Vec<String>,
    /// The raw RFC 5322 message, dot-unstuffed.
    pub raw: String,
}

impl CapturedMail {
    /// The first `text/plain` body, transfer-decoded, so a link inside it can
    /// be read regardless of how the sender wrapped or escaped it.
    pub fn text(&self) -> String {
        let (headers, body) = split_message(&self.raw);
        let content_type = header(&headers, "content-type").unwrap_or_default();
        if let Some(boundary) = content_type
            .split(';')
            .find_map(|parameter| parameter.trim().strip_prefix("boundary="))
            .map(|value| value.trim_matches('"').to_owned())
        {
            let delimiter = format!("--{boundary}");
            for part in body.split(&delimiter).skip(1) {
                let part = part.trim_start_matches("\r\n").trim_start_matches('\n');
                let (part_headers, part_body) = split_message(part);
                let part_type = header(&part_headers, "content-type").unwrap_or_default();
                if part_type.to_ascii_lowercase().starts_with("text/plain") {
                    return decode_transfer(
                        &part_body,
                        &header(&part_headers, "content-transfer-encoding").unwrap_or_default(),
                    );
                }
            }
        }
        decode_transfer(
            &body,
            &header(&headers, "content-transfer-encoding").unwrap_or_default(),
        )
    }
}

fn split_message(raw: &str) -> (String, String) {
    let normalized = raw.replace("\r\n", "\n");
    match normalized.split_once("\n\n") {
        Some((headers, body)) => (headers.to_owned(), body.to_owned()),
        None => (normalized, String::new()),
    }
}

fn header(headers: &str, name: &str) -> Option<String> {
    let mut unfolded: Vec<String> = Vec::new();
    for line in headers.lines() {
        if line.starts_with([' ', '\t'])
            && let Some(last) = unfolded.last_mut()
        {
            last.push(' ');
            last.push_str(line.trim());
        } else {
            unfolded.push(line.to_owned());
        }
    }
    unfolded.iter().find_map(|line| {
        let (candidate, value) = line.split_once(':')?;
        candidate
            .trim()
            .eq_ignore_ascii_case(name)
            .then(|| value.trim().to_owned())
    })
}

fn decode_transfer(body: &str, encoding: &str) -> String {
    match encoding.trim().to_ascii_lowercase().as_str() {
        "quoted-printable" => {
            let joined = body.replace("=\n", "");
            let mut bytes = Vec::with_capacity(joined.len());
            let mut characters = joined.bytes();
            while let Some(byte) = characters.next() {
                if byte == b'=' {
                    let high = characters.next();
                    let low = characters.next();
                    if let (Some(high), Some(low)) = (high, low)
                        && let Ok(value) =
                            u8::from_str_radix(&String::from_utf8_lossy(&[high, low]), 16)
                    {
                        bytes.push(value);
                        continue;
                    }
                    bytes.push(byte);
                    bytes.extend(high);
                    bytes.extend(low);
                } else {
                    bytes.push(byte);
                }
            }
            String::from_utf8_lossy(&bytes).into_owned()
        }
        "base64" => {
            let compact: String = body.chars().filter(|c| !c.is_whitespace()).collect();
            base64::engine::general_purpose::STANDARD
                .decode(compact.as_bytes())
                .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
                .unwrap_or_else(|_| body.to_owned())
        }
        _ => body.to_owned(),
    }
}

/// A plaintext SMTP relay on loopback that keeps what it is handed. The
/// control plane's relay client speaks the same few verbs to mailpit.
pub struct SmtpCaptureStub {
    pub port: u16,
    messages: Arc<Mutex<Vec<CapturedMail>>>,
}

impl SmtpCaptureStub {
    pub fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("smtp listener");
        let port = listener.local_addr().expect("smtp address").port();
        let messages = Arc::new(Mutex::new(Vec::new()));
        let captured = Arc::clone(&messages);
        thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { break };
                let captured = Arc::clone(&captured);
                thread::spawn(move || serve_smtp(stream, &captured));
            }
        });
        Self { port, messages }
    }

    pub fn messages(&self) -> Vec<CapturedMail> {
        self.messages.lock().expect("messages").clone()
    }

    /// The first captured message addressed to `recipient`, waiting up to
    /// `timeout` for the relay client to hand it over.
    pub fn wait_for(&self, recipient: &str, timeout: Duration) -> Option<CapturedMail> {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(found) = self.messages().into_iter().find(|message| {
                message
                    .recipients
                    .iter()
                    .any(|candidate| candidate.eq_ignore_ascii_case(recipient))
            }) {
                return Some(found);
            }
            if Instant::now() >= deadline {
                return None;
            }
            thread::sleep(Duration::from_millis(200));
        }
    }
}

fn serve_smtp(stream: std::net::TcpStream, captured: &Mutex<Vec<CapturedMail>>) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(30)));
    let mut writer = match stream.try_clone() {
        Ok(writer) => writer,
        Err(_) => return,
    };
    let mut reader = BufReader::new(stream);
    if writer.write_all(b"220 smoke.local ESMTP\r\n").is_err() {
        return;
    }
    let mut sender = String::new();
    let mut recipients = Vec::new();
    loop {
        let mut line = String::new();
        match reader.read_line(&mut line) {
            Ok(0) | Err(_) => return,
            Ok(_) => {}
        }
        let command = line.trim_end();
        let upper = command.to_ascii_uppercase();
        let reply: &[u8] = if upper.starts_with("EHLO") {
            b"250-smoke.local\r\n250-8BITMIME\r\n250 SIZE 1048576\r\n"
        } else if upper.starts_with("HELO") {
            b"250 smoke.local\r\n"
        } else if upper.starts_with("MAIL FROM:") {
            sender = address(&command["MAIL FROM:".len()..]);
            recipients.clear();
            b"250 OK\r\n"
        } else if upper.starts_with("RCPT TO:") {
            recipients.push(address(&command["RCPT TO:".len()..]));
            b"250 OK\r\n"
        } else if upper == "DATA" {
            if writer.write_all(b"354 go ahead\r\n").is_err() {
                return;
            }
            let mut raw = String::new();
            loop {
                let mut data_line = String::new();
                match reader.read_line(&mut data_line) {
                    Ok(0) | Err(_) => return,
                    Ok(_) => {}
                }
                if data_line == ".\r\n" || data_line == ".\n" {
                    break;
                }
                raw.push_str(data_line.strip_prefix('.').unwrap_or(&data_line));
            }
            captured.lock().expect("messages").push(CapturedMail {
                sender: sender.clone(),
                recipients: recipients.clone(),
                raw,
            });
            b"250 queued\r\n"
        } else if upper == "QUIT" {
            let _ = writer.write_all(b"221 bye\r\n");
            return;
        } else if upper == "RSET" || upper == "NOOP" {
            b"250 OK\r\n"
        } else if upper.starts_with("STARTTLS") || upper.starts_with("AUTH") {
            b"502 not here\r\n"
        } else {
            b"500 unknown\r\n"
        };
        if writer.write_all(reply).is_err() {
            return;
        }
    }
}

fn address(argument: &str) -> String {
    let argument = argument.trim();
    let inner = argument
        .split_once('<')
        .and_then(|(_, rest)| rest.split_once('>'))
        .map_or(argument, |(inside, _)| inside);
    inner.trim().to_owned()
}
