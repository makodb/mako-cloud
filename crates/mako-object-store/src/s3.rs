use std::{
    collections::BTreeMap,
    fmt,
    io::{Read, Write},
    net::{IpAddr, SocketAddr, TcpStream},
    sync::Arc,
    time::Duration,
};

use async_trait::async_trait;
use hmac::{Hmac, Mac};
use mako_api::TenantScope;
use sha2::{Digest, Sha256};
use time::OffsetDateTime;
use url::Url;

use crate::{MAX_OBJECT_BYTES, ObjectAddress, ObjectStore, ObjectStoreError, PutImmutableOutcome};

const MAX_RESPONSE_HEADER_BYTES: usize = 32 * 1024;
const MAX_RESPONSE_HEADERS: usize = 128;
const MAX_ERROR_BODY_BYTES: usize = 64 * 1024;

#[derive(Clone, Eq, PartialEq)]
pub struct S3Credentials {
    access_key: Box<str>,
    secret_key: Box<str>,
}

impl S3Credentials {
    pub fn new(
        access_key: impl Into<String>,
        secret_key: impl Into<String>,
    ) -> Result<Self, ObjectStoreError> {
        let access_key = access_key.into();
        let secret_key = secret_key.into();
        if !(3..=128).contains(&access_key.len())
            || access_key.chars().any(char::is_control)
            || !(16..=1024).contains(&secret_key.len())
            || secret_key.chars().any(char::is_control)
        {
            return Err(ObjectStoreError::InvalidConfiguration);
        }
        Ok(Self {
            access_key: access_key.into_boxed_str(),
            secret_key: secret_key.into_boxed_str(),
        })
    }
}

impl fmt::Debug for S3Credentials {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("S3Credentials([REDACTED])")
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct S3ObjectStoreConfig {
    pub endpoint: Url,
    pub bucket: String,
    pub region: String,
    pub connect_timeout: Duration,
    pub io_timeout: Duration,
    pub maximum_object_bytes: usize,
}

impl S3ObjectStoreConfig {
    #[must_use]
    pub fn loopback(endpoint: Url, region: impl Into<String>) -> Self {
        Self {
            endpoint,
            bucket: "mako-function-bundles-v1".to_owned(),
            region: region.into(),
            connect_timeout: Duration::from_secs(2),
            io_timeout: Duration::from_secs(10),
            maximum_object_bytes: MAX_OBJECT_BYTES,
        }
    }
}

#[derive(Clone)]
pub struct S3ObjectStore {
    config: S3ObjectStoreConfig,
    credentials: S3Credentials,
    endpoint: SocketAddr,
    host_header: String,
}

impl S3ObjectStore {
    pub fn new(
        config: S3ObjectStoreConfig,
        credentials: S3Credentials,
    ) -> Result<Self, ObjectStoreError> {
        let endpoint = validate_config(&config)?;
        let host_header = match endpoint.ip() {
            IpAddr::V4(ip) => format!("{ip}:{}", endpoint.port()),
            IpAddr::V6(ip) => format!("[{ip}]:{}", endpoint.port()),
        };
        Ok(Self {
            config,
            credentials,
            endpoint,
            host_header,
        })
    }

    pub fn ensure_bucket(&self) -> Result<(), ObjectStoreError> {
        let response = self.send(
            "PUT",
            &self.bucket_path(),
            BTreeMap::new(),
            &[],
            MAX_ERROR_BODY_BYTES,
        )?;
        if matches!(response.status, 200 | 201 | 204) {
            return Ok(());
        }
        if matches!(response.status, 409 | 412) && self.dependency_ready() {
            return Ok(());
        }
        Err(ObjectStoreError::Unavailable)
    }

    #[must_use]
    pub fn dependency_ready(&self) -> bool {
        self.send("HEAD", &self.bucket_path(), BTreeMap::new(), &[], 0)
            .is_ok_and(|response| response.status == 200)
    }

    fn bucket_path(&self) -> String {
        format!("/{}", self.config.bucket)
    }

    fn object_path(&self, address: &ObjectAddress) -> String {
        uri_encode_path(&format!("/{}/{}", self.config.bucket, address.path()))
    }

    fn send(
        &self,
        method: &str,
        canonical_uri: &str,
        mut headers: BTreeMap<String, String>,
        body: &[u8],
        maximum_response_bytes: usize,
    ) -> Result<S3Response, ObjectStoreError> {
        let now = OffsetDateTime::now_utc();
        let amz_date = aws_timestamp(now);
        let date = aws_date(now);
        let payload_hash = sha256_hex(body);
        headers.insert("host".to_owned(), self.host_header.clone());
        headers.insert("x-amz-content-sha256".to_owned(), payload_hash.clone());
        headers.insert("x-amz-date".to_owned(), amz_date.clone());
        let authorization = authorization(
            method,
            canonical_uri,
            &headers,
            &payload_hash,
            &amz_date,
            &date,
            &self.config.region,
            &self.credentials,
        )?;
        headers.insert("authorization".to_owned(), authorization);

        let mut stream = TcpStream::connect_timeout(&self.endpoint, self.config.connect_timeout)
            .map_err(|_| ObjectStoreError::Unavailable)?;
        stream
            .set_read_timeout(Some(self.config.io_timeout))
            .and_then(|()| stream.set_write_timeout(Some(self.config.io_timeout)))
            .map_err(|_| ObjectStoreError::Unavailable)?;
        write!(
            stream,
            "{method} {canonical_uri} HTTP/1.1\r\nConnection: close\r\nContent-Length: {}\r\n",
            body.len()
        )
        .map_err(|_| ObjectStoreError::Unavailable)?;
        for (name, value) in &headers {
            write!(stream, "{name}: {value}\r\n").map_err(|_| ObjectStoreError::Unavailable)?;
        }
        stream
            .write_all(b"\r\n")
            .and_then(|()| stream.write_all(body))
            .and_then(|()| stream.flush())
            .map_err(|_| ObjectStoreError::Unavailable)?;
        read_response(&mut stream, method == "HEAD", maximum_response_bytes)
    }
}

impl fmt::Debug for S3ObjectStore {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("S3ObjectStore")
            .field("endpoint", &self.config.endpoint)
            .field("bucket", &self.config.bucket)
            .field("region", &self.config.region)
            .field("credentials", &"[REDACTED]")
            .finish_non_exhaustive()
    }
}

#[async_trait]
impl ObjectStore for S3ObjectStore {
    async fn put_immutable(
        &self,
        tenant: &TenantScope,
        address: &ObjectAddress,
        bytes: Arc<[u8]>,
    ) -> Result<PutImmutableOutcome, ObjectStoreError> {
        address.require_tenant(tenant)?;
        if bytes.is_empty() || bytes.len() > self.config.maximum_object_bytes {
            return Err(ObjectStoreError::InvalidObject);
        }
        if !address.content_matches(&bytes) {
            return match self.get(tenant, address).await? {
                Some(_) => Err(ObjectStoreError::ImmutableConflict),
                None => Err(ObjectStoreError::InvalidObject),
            };
        }
        let headers = BTreeMap::from([
            (
                "content-type".to_owned(),
                "application/octet-stream".to_owned(),
            ),
            ("if-none-match".to_owned(), "*".to_owned()),
        ]);
        let response = self.send(
            "PUT",
            &self.object_path(address),
            headers,
            &bytes,
            MAX_ERROR_BODY_BYTES,
        )?;
        match response.status {
            200 | 201 | 204 => Ok(PutImmutableOutcome::Created),
            409 | 412 => match self.get(tenant, address).await? {
                Some(existing) if existing.as_ref() == bytes.as_ref() => {
                    Ok(PutImmutableOutcome::AlreadyPresent)
                }
                Some(_) => Err(ObjectStoreError::ImmutableConflict),
                None => Err(ObjectStoreError::Unavailable),
            },
            _ => Err(ObjectStoreError::Unavailable),
        }
    }

    async fn get(
        &self,
        tenant: &TenantScope,
        address: &ObjectAddress,
    ) -> Result<Option<Arc<[u8]>>, ObjectStoreError> {
        address.require_tenant(tenant)?;
        let response = self.send(
            "GET",
            &self.object_path(address),
            BTreeMap::new(),
            &[],
            self.config.maximum_object_bytes,
        )?;
        match response.status {
            200 if address.content_matches(&response.body) => Ok(Some(Arc::from(response.body))),
            200 => Err(ObjectStoreError::Integrity),
            404 => Ok(None),
            _ => Err(ObjectStoreError::Unavailable),
        }
    }

    async fn delete(
        &self,
        tenant: &TenantScope,
        address: &ObjectAddress,
    ) -> Result<(), ObjectStoreError> {
        address.require_tenant(tenant)?;
        let response = self.send(
            "DELETE",
            &self.object_path(address),
            BTreeMap::new(),
            &[],
            MAX_ERROR_BODY_BYTES,
        )?;
        match response.status {
            200 | 202 | 204 | 404 => Ok(()),
            _ => Err(ObjectStoreError::Unavailable),
        }
    }
}

fn validate_config(config: &S3ObjectStoreConfig) -> Result<SocketAddr, ObjectStoreError> {
    if config.endpoint.scheme() != "http"
        || config.endpoint.username() != ""
        || config.endpoint.password().is_some()
        || config.endpoint.path() != "/"
        || config.endpoint.query().is_some()
        || config.endpoint.fragment().is_some()
        || !valid_bucket(&config.bucket)
        || !valid_region(&config.region)
        || config.connect_timeout.is_zero()
        || config.io_timeout.is_zero()
        || config.maximum_object_bytes == 0
        || config.maximum_object_bytes > MAX_OBJECT_BYTES
    {
        return Err(ObjectStoreError::InvalidConfiguration);
    }
    let ip = config
        .endpoint
        .host_str()
        .and_then(|host| host.parse::<IpAddr>().ok())
        .filter(IpAddr::is_loopback)
        .ok_or(ObjectStoreError::InvalidConfiguration)?;
    let port = config
        .endpoint
        .port_or_known_default()
        .ok_or(ObjectStoreError::InvalidConfiguration)?;
    Ok(SocketAddr::new(ip, port))
}

fn valid_bucket(value: &str) -> bool {
    (3..=63).contains(&value.len())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        && !value.starts_with('-')
        && !value.ends_with('-')
        && !value.contains("--")
}

fn valid_region(value: &str) -> bool {
    (3..=64).contains(&value.len())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

#[allow(clippy::too_many_arguments)]
fn authorization(
    method: &str,
    canonical_uri: &str,
    headers: &BTreeMap<String, String>,
    payload_hash: &str,
    amz_date: &str,
    date: &str,
    region: &str,
    credentials: &S3Credentials,
) -> Result<String, ObjectStoreError> {
    let signed_headers = headers
        .keys()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join(";");
    let canonical_headers = headers
        .iter()
        .map(|(name, value)| format!("{name}:{}\n", value.trim()))
        .collect::<String>();
    let canonical_request = format!(
        "{method}\n{canonical_uri}\n\n{canonical_headers}\n{signed_headers}\n{payload_hash}"
    );
    let scope = format!("{date}/{region}/s3/aws4_request");
    let string_to_sign = format!(
        "AWS4-HMAC-SHA256\n{amz_date}\n{scope}\n{}",
        sha256_hex(canonical_request.as_bytes())
    );
    let date_key = hmac_sha256(
        format!("AWS4{}", credentials.secret_key).as_bytes(),
        date.as_bytes(),
    )?;
    let region_key = hmac_sha256(&date_key, region.as_bytes())?;
    let service_key = hmac_sha256(&region_key, b"s3")?;
    let signing_key = hmac_sha256(&service_key, b"aws4_request")?;
    let signature = hex(&hmac_sha256(&signing_key, string_to_sign.as_bytes())?);
    Ok(format!(
        "AWS4-HMAC-SHA256 Credential={}/{scope}, SignedHeaders={signed_headers}, Signature={signature}",
        credentials.access_key
    ))
}

fn hmac_sha256(key: &[u8], value: &[u8]) -> Result<[u8; 32], ObjectStoreError> {
    let mut mac =
        Hmac::<Sha256>::new_from_slice(key).map_err(|_| ObjectStoreError::InvalidConfiguration)?;
    mac.update(value);
    Ok(mac.finalize().into_bytes().into())
}

fn sha256_hex(value: &[u8]) -> String {
    hex(&Sha256::digest(value))
}

fn hex(value: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(value.len() * 2);
    for byte in value {
        encoded.push(char::from(DIGITS[usize::from(byte >> 4)]));
        encoded.push(char::from(DIGITS[usize::from(byte & 0x0f)]));
    }
    encoded
}

fn aws_date(now: OffsetDateTime) -> String {
    format!(
        "{:04}{:02}{:02}",
        now.year(),
        u8::from(now.month()),
        now.day()
    )
}

fn aws_timestamp(now: OffsetDateTime) -> String {
    format!(
        "{}T{:02}{:02}{:02}Z",
        aws_date(now),
        now.hour(),
        now.minute(),
        now.second()
    )
}

fn uri_encode_path(value: &str) -> String {
    const DIGITS: &[u8; 16] = b"0123456789ABCDEF";
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~' | b'/') {
            encoded.push(char::from(byte));
        } else {
            encoded.push('%');
            encoded.push(char::from(DIGITS[usize::from(byte >> 4)]));
            encoded.push(char::from(DIGITS[usize::from(byte & 0x0f)]));
        }
    }
    encoded
}

#[derive(Debug, Eq, PartialEq)]
struct S3Response {
    status: u16,
    body: Vec<u8>,
}

fn read_response(
    stream: &mut TcpStream,
    head_request: bool,
    maximum_body_bytes: usize,
) -> Result<S3Response, ObjectStoreError> {
    let mut received = Vec::new();
    let header_end = loop {
        if let Some(index) = received.windows(4).position(|window| window == b"\r\n\r\n") {
            break index + 4;
        }
        if received.len() > MAX_RESPONSE_HEADER_BYTES {
            return Err(ObjectStoreError::Unavailable);
        }
        let mut chunk = [0_u8; 4096];
        let size = stream
            .read(&mut chunk)
            .map_err(|_| ObjectStoreError::Unavailable)?;
        if size == 0 {
            return Err(ObjectStoreError::Unavailable);
        }
        received.extend_from_slice(&chunk[..size]);
    };
    if header_end > MAX_RESPONSE_HEADER_BYTES {
        return Err(ObjectStoreError::Unavailable);
    }
    let header_text = std::str::from_utf8(&received[..header_end - 4])
        .map_err(|_| ObjectStoreError::Unavailable)?;
    let mut lines = header_text.split("\r\n");
    let status_line = lines.next().ok_or(ObjectStoreError::Unavailable)?;
    let mut status_parts = status_line.split_whitespace();
    if status_parts.next() != Some("HTTP/1.1") {
        return Err(ObjectStoreError::Unavailable);
    }
    let status = status_parts
        .next()
        .and_then(|value| value.parse::<u16>().ok())
        .filter(|status| (100..=599).contains(status))
        .ok_or(ObjectStoreError::Unavailable)?;
    let mut content_length = None;
    let mut header_count = 0_usize;
    for line in lines {
        header_count += 1;
        if header_count > MAX_RESPONSE_HEADERS {
            return Err(ObjectStoreError::Unavailable);
        }
        let (name, value) = line.split_once(':').ok_or(ObjectStoreError::Unavailable)?;
        if name.eq_ignore_ascii_case("transfer-encoding") {
            return Err(ObjectStoreError::Unavailable);
        }
        if name.eq_ignore_ascii_case("content-length") {
            if content_length.is_some() {
                return Err(ObjectStoreError::Unavailable);
            }
            content_length = Some(
                value
                    .trim()
                    .parse::<usize>()
                    .map_err(|_| ObjectStoreError::Unavailable)?,
            );
        }
    }
    if head_request {
        return Ok(S3Response {
            status,
            body: Vec::new(),
        });
    }
    let mut body = received.split_off(header_end);
    if let Some(content_length) = content_length {
        if content_length > maximum_body_bytes || body.len() > content_length {
            return Err(ObjectStoreError::Unavailable);
        }
        while body.len() < content_length {
            let remaining = content_length - body.len();
            let mut chunk = [0_u8; 8192];
            let limit = remaining.min(chunk.len());
            let size = stream
                .read(&mut chunk[..limit])
                .map_err(|_| ObjectStoreError::Unavailable)?;
            if size == 0 {
                return Err(ObjectStoreError::Unavailable);
            }
            body.extend_from_slice(&chunk[..size]);
        }
    } else {
        loop {
            if body.len() > maximum_body_bytes {
                return Err(ObjectStoreError::Unavailable);
            }
            let mut chunk = [0_u8; 8192];
            let size = stream
                .read(&mut chunk)
                .map_err(|_| ObjectStoreError::Unavailable)?;
            if size == 0 {
                break;
            }
            if body.len().saturating_add(size) > maximum_body_bytes {
                return Err(ObjectStoreError::Unavailable);
            }
            body.extend_from_slice(&chunk[..size]);
        }
    }
    Ok(S3Response { status, body })
}

#[cfg(test)]
mod tests {
    use std::{
        net::TcpListener,
        sync::mpsc::{self, Receiver},
        thread,
    };

    use futures::executor::block_on;
    use mako_api::{EnvironmentId, ProjectId};

    use super::*;

    const ACCESS_KEY: &str = "test-access-key";
    const SECRET_KEY: &str = "test-secret-key-that-is-long-enough";

    #[test]
    fn configuration_requires_authenticated_loopback_http_and_redacts_credentials() {
        let credentials = credentials();
        assert_eq!(format!("{credentials:?}"), "S3Credentials([REDACTED])");
        assert!(!format!("{credentials:?}").contains(SECRET_KEY));

        for endpoint in [
            "https://127.0.0.1:8333/",
            "http://192.0.2.1:8333/",
            "http://localhost:8333/",
            "http://127.0.0.1:8333/path",
        ] {
            assert_eq!(
                S3ObjectStore::new(config(endpoint), credentials.clone()).unwrap_err(),
                ObjectStoreError::InvalidConfiguration,
            );
        }
        assert_eq!(
            S3Credentials::new("ok", "too-short"),
            Err(ObjectStoreError::InvalidConfiguration),
        );
    }

    #[test]
    fn sigv4_canonical_headers_include_the_required_separator_line() {
        let payload_hash = sha256_hex(b"");
        let headers = BTreeMap::from([
            ("host".to_owned(), "127.0.0.1:8333".to_owned()),
            ("x-amz-content-sha256".to_owned(), payload_hash.clone()),
            ("x-amz-date".to_owned(), "20260809T005000Z".to_owned()),
        ]);
        let signed = authorization(
            "HEAD",
            "/mako-function-bundles-v1",
            &headers,
            &payload_hash,
            "20260809T005000Z",
            "20260809",
            "us-east-1-beta",
            &credentials(),
        )
        .expect("signature");
        assert_eq!(
            signed,
            "AWS4-HMAC-SHA256 Credential=test-access-key/20260809/us-east-1-beta/s3/aws4_request, SignedHeaders=host;x-amz-content-sha256;x-amz-date, Signature=8118140c23c5d9c17caa3eafec9e4874bbcf55168a2dec75a2927c34d6ae6c8c"
        );
    }

    #[test]
    fn immutable_put_is_signed_conditional_tenant_scoped_and_bounded() {
        let (endpoint, requests) = mock_s3(vec![response(201, b"")]);
        let store = S3ObjectStore::new(config(&endpoint), credentials()).expect("store");
        let tenant = tenant();
        let bytes: Arc<[u8]> = Arc::from(b"bundle".as_slice());
        let address = address(&tenant, &bytes);

        assert_eq!(
            block_on(store.put_immutable(&tenant, &address, bytes)),
            Ok(PutImmutableOutcome::Created),
        );
        let request = requests.recv().expect("request");
        assert!(request.starts_with("PUT /mako-function-bundles-v1/projects/"));
        assert!(request.contains("/function-bundles/sha256%3A"));
        assert!(request.contains("if-none-match: *\r\n"));
        assert!(request.contains("authorization: AWS4-HMAC-SHA256 Credential=test-access-key/"));
        assert!(request.contains(
            "SignedHeaders=content-type;host;if-none-match;x-amz-content-sha256;x-amz-date"
        ));
        assert!(request.ends_with("\r\n\r\nbundle"));
        assert!(!request.contains(SECRET_KEY));
    }

    #[test]
    fn conditional_retry_only_succeeds_when_existing_bytes_are_identical() {
        let (endpoint, requests) = mock_s3(vec![response(412, b""), response(200, b"bundle")]);
        let store = S3ObjectStore::new(config(&endpoint), credentials()).expect("store");
        let tenant = tenant();
        let bytes: Arc<[u8]> = Arc::from(b"bundle".as_slice());
        let address = address(&tenant, &bytes);

        assert_eq!(
            block_on(store.put_immutable(&tenant, &address, bytes)),
            Ok(PutImmutableOutcome::AlreadyPresent),
        );
        assert!(requests.recv().expect("put").starts_with("PUT "));
        assert!(requests.recv().expect("get").starts_with("GET "));
    }

    #[test]
    fn bucket_initialization_accepts_an_existing_authenticated_bucket() {
        let (endpoint, requests) = mock_s3(vec![response(409, b""), response(200, b"")]);
        let store = S3ObjectStore::new(config(&endpoint), credentials()).expect("store");
        assert_eq!(store.ensure_bucket(), Ok(()));
        assert!(
            requests
                .recv()
                .expect("create bucket")
                .starts_with("PUT /mako-function-bundles-v1 ")
        );
        assert!(
            requests
                .recv()
                .expect("head bucket")
                .starts_with("HEAD /mako-function-bundles-v1 ")
        );
    }

    #[test]
    fn digest_mismatch_and_oversized_responses_fail_closed() {
        let tenant = tenant();
        let address = address(&tenant, b"expected");
        let (endpoint, _requests) = mock_s3(vec![response(200, b"tampered")]);
        let store = S3ObjectStore::new(config(&endpoint), credentials()).expect("store");
        assert_eq!(
            block_on(store.get(&tenant, &address)),
            Err(ObjectStoreError::Integrity),
        );

        let oversized = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            MAX_OBJECT_BYTES + 1
        );
        let (endpoint, _requests) = mock_s3(vec![oversized.into_bytes()]);
        let store = S3ObjectStore::new(config(&endpoint), credentials()).expect("store");
        assert_eq!(
            block_on(store.get(&tenant, &address)),
            Err(ObjectStoreError::Unavailable),
        );
    }

    #[test]
    fn tenant_mismatch_never_reaches_the_dependency() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
        let endpoint = format!("http://{}/", listener.local_addr().expect("address"));
        let mut store_config = config(&endpoint);
        store_config.connect_timeout = Duration::from_millis(25);
        let store = S3ObjectStore::new(store_config, credentials()).expect("store");
        let owner = tenant();
        let other = TenantScope::new(
            ProjectId::parse("prj_other000").expect("project"),
            EnvironmentId::parse("env_other000").expect("environment"),
        );
        let address = address(&owner, b"bundle");
        assert_eq!(
            block_on(store.get(&other, &address)),
            Err(ObjectStoreError::TenantMismatch),
        );
        listener
            .set_nonblocking(true)
            .expect("nonblocking listener");
        assert!(listener.accept().is_err());
    }

    fn config(endpoint: &str) -> S3ObjectStoreConfig {
        S3ObjectStoreConfig::loopback(Url::parse(endpoint).expect("endpoint"), "us-east-1")
    }

    fn credentials() -> S3Credentials {
        S3Credentials::new(ACCESS_KEY, SECRET_KEY).expect("credentials")
    }

    fn tenant() -> TenantScope {
        TenantScope::new(
            ProjectId::parse("prj_abcdefgh").expect("project"),
            EnvironmentId::parse("env_abcdefgh").expect("environment"),
        )
    }

    fn address(tenant: &TenantScope, bytes: &[u8]) -> ObjectAddress {
        ObjectAddress::function_bundle(tenant.clone(), &format!("sha256:{}", sha256_hex(bytes)))
            .expect("address")
    }

    fn response(status: u16, body: &[u8]) -> Vec<u8> {
        let reason = match status {
            200 => "OK",
            201 => "Created",
            412 => "Precondition Failed",
            _ => "Response",
        };
        let mut response = format!(
            "HTTP/1.1 {status} {reason}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        )
        .into_bytes();
        response.extend_from_slice(body);
        response
    }

    fn mock_s3(responses: Vec<Vec<u8>>) -> (String, Receiver<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
        let address = listener.local_addr().expect("address");
        let (sender, receiver) = mpsc::channel();
        thread::spawn(move || {
            for response in responses {
                let (mut stream, _) = listener.accept().expect("connection");
                let request = read_request(&mut stream);
                sender.send(request).expect("request receiver");
                stream.write_all(&response).expect("response");
            }
        });
        (format!("http://{address}/"), receiver)
    }

    fn read_request(stream: &mut TcpStream) -> String {
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .expect("timeout");
        let mut request = Vec::new();
        let header_end = loop {
            if let Some(index) = request.windows(4).position(|window| window == b"\r\n\r\n") {
                break index + 4;
            }
            let mut chunk = [0_u8; 1024];
            let size = stream.read(&mut chunk).expect("request header");
            assert!(size > 0);
            request.extend_from_slice(&chunk[..size]);
        };
        let headers = std::str::from_utf8(&request[..header_end]).expect("header text");
        let content_length = headers
            .split("\r\n")
            .filter_map(|line| line.split_once(':'))
            .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
            .map_or(0, |(_, value)| {
                value.trim().parse().expect("content length")
            });
        while request.len() < header_end + content_length {
            let mut chunk = [0_u8; 1024];
            let size = stream.read(&mut chunk).expect("request body");
            assert!(size > 0);
            request.extend_from_slice(&chunk[..size]);
        }
        String::from_utf8(request).expect("request UTF-8")
    }
}
