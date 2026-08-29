//! A bounded HTTPS client for the few outbound calls the platform makes to
//! the outside world: an identity provider's token and identity endpoints.
//!
//! One request, one connection, `Connection: close`, no redirects followed,
//! a ceiling on the response, and TLS through rustls with the Mozilla roots.
//! Plain HTTP is admitted only to loopback and only when the caller says so,
//! which is how tests stand a provider in without certificates.
use std::{
    collections::BTreeMap,
    error::Error,
    fmt,
    io::{Read, Write},
    net::{IpAddr, TcpStream, ToSocketAddrs},
    sync::Arc,
    time::Duration,
};

use rustls::{ClientConfig, ClientConnection, RootCertStore, StreamOwned, pki_types::ServerName};
use url::Url;

const MAX_HEADER_BYTES: usize = 32 * 1024;

#[derive(Clone, Debug)]
pub struct HttpClientConfig {
    pub connect_timeout: Duration,
    pub io_timeout: Duration,
    pub maximum_response_bytes: usize,
    /// Admit `http://` to loopback addresses; refused in production.
    pub allow_plain_http_loopback: bool,
}

impl Default for HttpClientConfig {
    fn default() -> Self {
        Self {
            connect_timeout: Duration::from_secs(5),
            io_timeout: Duration::from_secs(15),
            maximum_response_bytes: 1024 * 1024,
            allow_plain_http_loopback: false,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum HttpClientError {
    /// The URL is not one this client will speak to.
    UnsupportedUrl,
    Connect,
    Tls,
    Io,
    /// The response was malformed or exceeded the ceiling.
    Response,
}

impl fmt::Display for HttpClientError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::UnsupportedUrl => "url is not supported",
            Self::Connect => "connection failed",
            Self::Tls => "tls handshake failed",
            Self::Io => "request failed",
            Self::Response => "response is invalid or too large",
        })
    }
}

impl Error for HttpClientError {}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HttpResponse {
    pub status: u16,
    /// Header names lowercased; repeated headers keep the last value.
    pub headers: BTreeMap<String, String>,
    pub body: Vec<u8>,
}

impl HttpResponse {
    #[must_use]
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .get(&name.to_ascii_lowercase())
            .map(String::as_str)
    }
}

#[derive(Clone)]
pub struct HttpClient {
    config: HttpClientConfig,
    tls: Arc<ClientConfig>,
}

impl fmt::Debug for HttpClient {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("HttpClient")
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

impl HttpClient {
    #[must_use]
    pub fn new(config: HttpClientConfig) -> Self {
        let mut roots = RootCertStore::empty();
        roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        let tls = ClientConfig::builder()
            .with_root_certificates(roots)
            .with_no_client_auth();
        Self {
            config,
            tls: Arc::new(tls),
        }
    }

    /// Sends one request and reads the whole response. `headers` are sent as
    /// given; `Host`, `Content-Length`, and `Connection` are the client's.
    pub fn request(
        &self,
        method: &str,
        url: &Url,
        headers: &[(&str, &str)],
        body: &[u8],
    ) -> Result<HttpResponse, HttpClientError> {
        let host = url.host_str().ok_or(HttpClientError::UnsupportedUrl)?;
        let secure = match url.scheme() {
            "https" => true,
            "http" if self.config.allow_plain_http_loopback && is_loopback_host(host) => false,
            _ => return Err(HttpClientError::UnsupportedUrl),
        };
        if !method.bytes().all(|byte| byte.is_ascii_uppercase()) || method.is_empty() {
            return Err(HttpClientError::UnsupportedUrl);
        }
        let port = url
            .port_or_known_default()
            .ok_or(HttpClientError::UnsupportedUrl)?;
        let mut target = url.path().to_owned();
        if let Some(query) = url.query() {
            target.push('?');
            target.push_str(query);
        }
        let host_header = match url.port() {
            Some(port) => format!("{host}:{port}"),
            None => host.to_owned(),
        };
        let mut request = format!(
            "{method} {target} HTTP/1.1\r\nHost: {host_header}\r\nConnection: close\r\nContent-Length: {}\r\n",
            body.len()
        );
        for (name, value) in headers {
            if name.is_empty()
                || name
                    .bytes()
                    .any(|byte| !byte.is_ascii_graphic() || byte == b':')
                || value.bytes().any(|byte| byte == b'\r' || byte == b'\n')
            {
                return Err(HttpClientError::UnsupportedUrl);
            }
            request.push_str(name);
            request.push_str(": ");
            request.push_str(value);
            request.push_str("\r\n");
        }
        request.push_str("\r\n");
        let stream = self.connect(host, port)?;
        if secure {
            let server_name = ServerName::try_from(host.to_owned())
                .map_err(|_| HttpClientError::UnsupportedUrl)?;
            let connection = ClientConnection::new(Arc::clone(&self.tls), server_name)
                .map_err(|_| HttpClientError::Tls)?;
            let mut tls = StreamOwned::new(connection, stream);
            self.exchange(&mut tls, request.as_bytes(), body)
                .map_err(|error| {
                    if error == HttpClientError::Io {
                        HttpClientError::Tls
                    } else {
                        error
                    }
                })
        } else {
            let mut plain = stream;
            self.exchange(&mut plain, request.as_bytes(), body)
        }
    }

    fn connect(&self, host: &str, port: u16) -> Result<TcpStream, HttpClientError> {
        let address = (host, port)
            .to_socket_addrs()
            .map_err(|_| HttpClientError::Connect)?
            .next()
            .ok_or(HttpClientError::Connect)?;
        let stream = TcpStream::connect_timeout(&address, self.config.connect_timeout)
            .map_err(|_| HttpClientError::Connect)?;
        stream
            .set_read_timeout(Some(self.config.io_timeout))
            .and_then(|()| stream.set_write_timeout(Some(self.config.io_timeout)))
            .map_err(|_| HttpClientError::Connect)?;
        Ok(stream)
    }

    fn exchange<S: Read + Write>(
        &self,
        stream: &mut S,
        head: &[u8],
        body: &[u8],
    ) -> Result<HttpResponse, HttpClientError> {
        stream
            .write_all(head)
            .and_then(|()| stream.write_all(body))
            .and_then(|()| stream.flush())
            .map_err(|_| HttpClientError::Io)?;
        let mut raw = Vec::new();
        let mut buffer = [0u8; 8192];
        let limit = self
            .config
            .maximum_response_bytes
            .saturating_add(MAX_HEADER_BYTES);
        loop {
            match stream.read(&mut buffer) {
                Ok(0) => break,
                Ok(read) => {
                    raw.extend_from_slice(&buffer[..read]);
                    if raw.len() > limit {
                        return Err(HttpClientError::Response);
                    }
                }
                // rustls reports a peer that closes without close_notify as an error;
                // with `Connection: close` and a complete body that is the normal end.
                Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => break,
                Err(_) => return Err(HttpClientError::Io),
            }
        }
        parse_response(&raw, self.config.maximum_response_bytes)
    }
}

fn is_loopback_host(host: &str) -> bool {
    host == "localhost"
        || host
            .trim_matches(['[', ']'])
            .parse::<IpAddr>()
            .is_ok_and(|address| address.is_loopback())
}

fn parse_response(raw: &[u8], maximum_body: usize) -> Result<HttpResponse, HttpClientError> {
    let split = raw
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .ok_or(HttpClientError::Response)?;
    if split > MAX_HEADER_BYTES {
        return Err(HttpClientError::Response);
    }
    let head = std::str::from_utf8(&raw[..split]).map_err(|_| HttpClientError::Response)?;
    let mut lines = head.split("\r\n");
    let status_line = lines.next().ok_or(HttpClientError::Response)?;
    let mut parts = status_line.split(' ');
    if !parts
        .next()
        .is_some_and(|version| version.starts_with("HTTP/1."))
    {
        return Err(HttpClientError::Response);
    }
    let status: u16 = parts
        .next()
        .and_then(|status| status.parse().ok())
        .filter(|status| (100..=599).contains(status))
        .ok_or(HttpClientError::Response)?;
    let mut headers = BTreeMap::new();
    for line in lines {
        let (name, value) = line.split_once(':').ok_or(HttpClientError::Response)?;
        headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_owned());
    }
    let remainder = &raw[split + 4..];
    let body = if headers
        .get("transfer-encoding")
        .is_some_and(|encoding| encoding.eq_ignore_ascii_case("chunked"))
    {
        decode_chunked(remainder, maximum_body)?
    } else {
        let declared = headers
            .get("content-length")
            .map(|value| {
                value
                    .parse::<usize>()
                    .map_err(|_| HttpClientError::Response)
            })
            .transpose()?;
        match declared {
            Some(length) if length > maximum_body => return Err(HttpClientError::Response),
            Some(length) => remainder
                .get(..length)
                .ok_or(HttpClientError::Response)?
                .to_vec(),
            None => remainder.to_vec(),
        }
    };
    if body.len() > maximum_body {
        return Err(HttpClientError::Response);
    }
    Ok(HttpResponse {
        status,
        headers,
        body,
    })
}

fn decode_chunked(mut input: &[u8], maximum_body: usize) -> Result<Vec<u8>, HttpClientError> {
    let mut body = Vec::new();
    loop {
        let line_end = input
            .windows(2)
            .position(|window| window == b"\r\n")
            .ok_or(HttpClientError::Response)?;
        let size_text =
            std::str::from_utf8(&input[..line_end]).map_err(|_| HttpClientError::Response)?;
        let size_text = size_text.split(';').next().unwrap_or("").trim();
        let size = usize::from_str_radix(size_text, 16).map_err(|_| HttpClientError::Response)?;
        input = &input[line_end + 2..];
        if size == 0 {
            return Ok(body);
        }
        let chunk = input.get(..size).ok_or(HttpClientError::Response)?;
        body.extend_from_slice(chunk);
        if body.len() > maximum_body {
            return Err(HttpClientError::Response);
        }
        input = input.get(size + 2..).ok_or(HttpClientError::Response)?;
    }
}

#[cfg(test)]
mod tests {
    use std::{
        io::{Read, Write},
        net::TcpListener,
        thread,
    };

    use super::*;

    fn serve_once(response: &'static [u8]) -> (String, thread::JoinHandle<Vec<u8>>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
        let port = listener.local_addr().expect("address").port();
        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept");
            let mut received = Vec::new();
            let mut buffer = [0u8; 4096];
            loop {
                let read = stream.read(&mut buffer).expect("read");
                received.extend_from_slice(&buffer[..read]);
                if received.windows(4).any(|window| window == b"\r\n\r\n") {
                    let head_end = received.windows(4).position(|w| w == b"\r\n\r\n").unwrap() + 4;
                    let head = String::from_utf8_lossy(&received[..head_end]).to_string();
                    let length: usize = head
                        .lines()
                        .find_map(|line| {
                            line.strip_prefix("Content-Length: ")
                                .and_then(|v| v.parse().ok())
                        })
                        .unwrap_or(0);
                    if received.len() >= head_end + length {
                        break;
                    }
                }
            }
            stream.write_all(response).expect("write");
            received
        });
        (format!("http://127.0.0.1:{port}"), handle)
    }

    #[test]
    fn plain_http_to_loopback_works_only_when_admitted() {
        let (endpoint, server) = serve_once(
            b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 13\r\n\r\n{\"ok\":true}\r\n",
        );
        let client = HttpClient::new(HttpClientConfig {
            allow_plain_http_loopback: true,
            ..HttpClientConfig::default()
        });
        let url = Url::parse(&format!("{endpoint}/token?grant_type=code")).expect("url");
        let response = client
            .request(
                "POST",
                &url,
                &[("content-type", "application/x-www-form-urlencoded")],
                b"code=abc",
            )
            .expect("response");
        assert_eq!(response.status, 200);
        assert_eq!(response.header("content-type"), Some("application/json"));
        assert_eq!(&response.body[..11], b"{\"ok\":true}");
        let sent = String::from_utf8(server.join().expect("server")).expect("utf8");
        assert!(
            sent.starts_with("POST /token?grant_type=code HTTP/1.1\r\n"),
            "{sent}"
        );
        assert!(sent.contains("Host: 127.0.0.1:"));
        assert!(sent.contains("content-type: application/x-www-form-urlencoded\r\n"));
        assert!(sent.ends_with("\r\n\r\ncode=abc"));

        let strict = HttpClient::new(HttpClientConfig::default());
        assert_eq!(
            strict.request("GET", &url, &[], b""),
            Err(HttpClientError::UnsupportedUrl)
        );
        let remote = Url::parse("http://example.com/token").expect("url");
        assert_eq!(
            client.request("GET", &remote, &[], b""),
            Err(HttpClientError::UnsupportedUrl)
        );
        let ftp = Url::parse("ftp://127.0.0.1/x").expect("url");
        assert_eq!(
            client.request("GET", &ftp, &[], b""),
            Err(HttpClientError::UnsupportedUrl)
        );
    }

    #[test]
    fn chunked_responses_are_decoded_and_ceilings_hold() {
        let (endpoint, server) = serve_once(
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n6\r\n world\r\n0\r\n\r\n",
        );
        let client = HttpClient::new(HttpClientConfig {
            allow_plain_http_loopback: true,
            ..HttpClientConfig::default()
        });
        let url = Url::parse(&format!("{endpoint}/userinfo")).expect("url");
        let response = client.request("GET", &url, &[], b"").expect("response");
        assert_eq!(response.body, b"hello world");
        server.join().expect("server");

        assert_eq!(
            parse_response(b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\n\r\nabcd", 3),
            Err(HttpClientError::Response)
        );
        assert_eq!(
            decode_chunked(b"3\r\nabc\r\n0\r\n\r\n", 2),
            Err(HttpClientError::Response)
        );
        assert!(parse_response(b"garbage", 10).is_err());
        assert!(parse_response(b"HTTP/1.1 999 Nope\r\n\r\n", 10).is_err());
    }

    #[test]
    fn header_injection_is_refused() {
        let client = HttpClient::new(HttpClientConfig {
            allow_plain_http_loopback: true,
            ..HttpClientConfig::default()
        });
        let url = Url::parse("http://127.0.0.1:1/x").expect("url");
        assert_eq!(
            client.request(
                "GET",
                &url,
                &[("authorization", "Bearer x\r\nevil: y")],
                b""
            ),
            Err(HttpClientError::UnsupportedUrl)
        );
        assert_eq!(
            client.request("get", &url, &[], b""),
            Err(HttpClientError::UnsupportedUrl)
        );
    }
}
