use std::{
    io::{self, Read, Write},
    net::{SocketAddr, TcpStream},
    time::Duration,
};

use async_trait::async_trait;
use mako_edge_gateway::{
    FunctionRuntimeInvoker, RuntimeFunctionInvocation, RuntimeFunctionResponse,
    RuntimeInvocationError,
};
use mako_edge_runtime_protocol::{
    CALLER_AUTHORIZATION_HEADER, RUNTIME_RESPONSE_TIMEOUT_SECONDS, RuntimeErrorCode,
};

const MAX_RESPONSE_HEADER_BYTES: usize = 32 * 1024;
const STREAM_CHUNK_BYTES: usize = 16 * 1024;

#[derive(Clone, Copy)]
pub(crate) struct LoopbackRuntimeInvoker {
    endpoint: SocketAddr,
}

impl LoopbackRuntimeInvoker {
    pub(crate) const fn new(endpoint: SocketAddr) -> Self {
        Self { endpoint }
    }

    pub(crate) fn dependency_ready(&self) -> bool {
        self.endpoint.ip().is_loopback()
            && TcpStream::connect_timeout(&self.endpoint, Duration::from_millis(500)).is_ok()
    }
}

#[async_trait]
impl FunctionRuntimeInvoker for LoopbackRuntimeInvoker {
    async fn invoke(
        &self,
        request: RuntimeFunctionInvocation,
    ) -> Result<RuntimeFunctionResponse, RuntimeInvocationError> {
        let mut stream = TcpStream::connect_timeout(&self.endpoint, Duration::from_secs(2))
            .map_err(|_| runtime_unavailable())?;
        stream
            .set_read_timeout(Some(Duration::from_secs(RUNTIME_RESPONSE_TIMEOUT_SECONDS)))
            .and_then(|()| stream.set_write_timeout(Some(Duration::from_secs(5))))
            .map_err(|_| runtime_unavailable())?;
        let target = format!(
            "/{}/functions/v1/{}{}",
            request.tenant.project_id(),
            request.function_name,
            request.path_and_query
        );
        write!(
            stream,
            "{} {} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\nContent-Length: {}\r\nx-request-id: {}\r\nx-mako-runtime-environment-id: {}\r\nx-mako-runtime-deployment-version: {}\r\n",
            request.method.as_str(),
            target,
            self.endpoint,
            request.body.len(),
            request.request_id,
            request.tenant.environment_id(),
            request.version,
        )
        .map_err(|_| runtime_unavailable())?;
        // The credential of the caller this gateway verified, on the header
        // the protocol reserves for it and the SDK inside the worker reads.
        // A request that carried no verified caller sends nothing: a public
        // function is told there is no one rather than handed a token.
        if let Some(token) = &request.caller_token {
            write!(
                stream,
                "{CALLER_AUTHORIZATION_HEADER}: {}\r\n",
                token.expose_to_runtime_adapter()
            )
            .map_err(|_| runtime_unavailable())?;
        }
        for (name, value) in &request.headers {
            write!(stream, "{name}: {value}\r\n").map_err(|_| runtime_unavailable())?;
        }
        write!(stream, "\r\n").map_err(|_| runtime_unavailable())?;
        stream
            .write_all(&request.body)
            .and_then(|()| stream.flush())
            .map_err(|_| runtime_unavailable())?;

        let header = read_response_header(&mut stream).map_err(|_| runtime_unavailable())?;
        let parsed = parse_response_header(&header).ok_or_else(runtime_unavailable)?;
        let maximum =
            usize::try_from(request.response_limit_bytes).map_err(|_| response_too_large())?;
        if parsed.content_length.is_some_and(|length| length > maximum) {
            return Err(response_too_large());
        }
        let mode = if parsed.chunked {
            BodyMode::Chunked { remaining: 0 }
        } else if let Some(remaining) = parsed.content_length {
            BodyMode::ContentLength { remaining }
        } else {
            BodyMode::UntilClose
        };
        let body = futures::stream::try_unfold(
            BodyState {
                stream,
                mode,
                delivered: 0,
                maximum,
            },
            |mut state| async move {
                match state.next_chunk()? {
                    Some(chunk) => Ok(Some((chunk, state))),
                    None => Ok(None),
                }
            },
        );
        Ok(RuntimeFunctionResponse {
            status: parsed.status,
            headers: parsed.headers,
            body: Box::pin(body),
        })
    }
}

struct ParsedResponse {
    status: u16,
    headers: Vec<(String, String)>,
    content_length: Option<usize>,
    chunked: bool,
}

fn read_response_header(stream: &mut TcpStream) -> io::Result<Vec<u8>> {
    let mut header = Vec::new();
    let mut byte = [0_u8; 1];
    while header.len() < MAX_RESPONSE_HEADER_BYTES {
        if stream.read(&mut byte)? == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "response header",
            ));
        }
        header.push(byte[0]);
        if header.ends_with(b"\r\n\r\n") {
            return Ok(header);
        }
    }
    Err(io::Error::new(
        io::ErrorKind::InvalidData,
        "response header too large",
    ))
}

fn parse_response_header(bytes: &[u8]) -> Option<ParsedResponse> {
    let text = std::str::from_utf8(bytes).ok()?;
    let mut lines = text.strip_suffix("\r\n\r\n")?.split("\r\n");
    let status = lines.next()?.split_whitespace().nth(1)?.parse().ok()?;
    if !(100..=599).contains(&status) {
        return None;
    }
    let mut headers = Vec::new();
    let mut content_length = None;
    let mut chunked = false;
    for line in lines {
        let (name, value) = line.split_once(':')?;
        let name = name.trim().to_ascii_lowercase();
        let value = value.trim().to_owned();
        if name == "content-length" {
            let parsed = value.parse().ok()?;
            if content_length.replace(parsed).is_some() {
                return None;
            }
        } else if name == "transfer-encoding" {
            if value.eq_ignore_ascii_case("chunked") {
                chunked = true;
            } else {
                return None;
            }
        } else if !matches!(
            name.as_str(),
            "connection"
                | "keep-alive"
                | "proxy-authenticate"
                | "proxy-authorization"
                | "te"
                | "trailer"
                | "upgrade"
        ) {
            headers.push((name, value));
        }
    }
    if chunked && content_length.is_some() {
        return None;
    }
    Some(ParsedResponse {
        status,
        headers,
        content_length,
        chunked,
    })
}

enum BodyMode {
    ContentLength { remaining: usize },
    Chunked { remaining: usize },
    UntilClose,
    Done,
}

struct BodyState {
    stream: TcpStream,
    mode: BodyMode,
    delivered: usize,
    maximum: usize,
}

impl BodyState {
    fn next_chunk(&mut self) -> Result<Option<Vec<u8>>, RuntimeInvocationError> {
        let mut buffer = vec![0_u8; STREAM_CHUNK_BYTES];
        let read = match &mut self.mode {
            BodyMode::ContentLength { remaining } => {
                if *remaining == 0 {
                    self.mode = BodyMode::Done;
                    return Ok(None);
                }
                let wanted = buffer.len().min(*remaining);
                self.stream
                    .read_exact(&mut buffer[..wanted])
                    .map_err(|_| runtime_unavailable())?;
                *remaining -= wanted;
                wanted
            }
            BodyMode::UntilClose => self
                .stream
                .read(&mut buffer)
                .map_err(|_| runtime_unavailable())?,
            BodyMode::Chunked { remaining } => {
                if *remaining == 0 {
                    let line =
                        read_crlf_line(&mut self.stream).map_err(|_| runtime_unavailable())?;
                    let size = usize::from_str_radix(
                        line.split(';').next().unwrap_or_default().trim(),
                        16,
                    )
                    .map_err(|_| runtime_unavailable())?;
                    if size == 0 {
                        loop {
                            if read_crlf_line(&mut self.stream)
                                .map_err(|_| runtime_unavailable())?
                                .is_empty()
                            {
                                break;
                            }
                        }
                        self.mode = BodyMode::Done;
                        return Ok(None);
                    }
                    *remaining = size;
                }
                let wanted = buffer.len().min(*remaining);
                self.stream
                    .read_exact(&mut buffer[..wanted])
                    .map_err(|_| runtime_unavailable())?;
                *remaining -= wanted;
                if *remaining == 0 {
                    let mut terminator = [0_u8; 2];
                    self.stream
                        .read_exact(&mut terminator)
                        .map_err(|_| runtime_unavailable())?;
                    if terminator != *b"\r\n" {
                        return Err(runtime_unavailable());
                    }
                }
                wanted
            }
            BodyMode::Done => return Ok(None),
        };
        if read == 0 {
            self.mode = BodyMode::Done;
            return Ok(None);
        }
        self.delivered = self
            .delivered
            .checked_add(read)
            .ok_or_else(response_too_large)?;
        if self.delivered > self.maximum {
            return Err(response_too_large());
        }
        buffer.truncate(read);
        Ok(Some(buffer))
    }
}

fn read_crlf_line(stream: &mut TcpStream) -> io::Result<String> {
    let mut line = Vec::new();
    let mut byte = [0_u8; 1];
    while line.len() <= 4096 {
        stream.read_exact(&mut byte)?;
        line.push(byte[0]);
        if line.ends_with(b"\r\n") {
            line.truncate(line.len() - 2);
            return String::from_utf8(line)
                .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "chunk line"));
        }
    }
    Err(io::Error::new(
        io::ErrorKind::InvalidData,
        "chunk line too large",
    ))
}

const fn runtime_unavailable() -> RuntimeInvocationError {
    RuntimeInvocationError {
        code: RuntimeErrorCode::RuntimeUnavailable,
        retryable: true,
    }
}

const fn response_too_large() -> RuntimeInvocationError {
    RuntimeInvocationError {
        code: RuntimeErrorCode::ResponseTooLarge,
        retryable: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::{StreamExt, executor::block_on};
    use mako_api::{EnvironmentId, ProjectId, TenantScope};
    use mako_edge_gateway::{
        FunctionHttpMethod, FunctionInvocationActor, FunctionInvocationAuditContext,
    };
    use std::{net::TcpListener, thread};

    #[test]
    fn runtime_response_after_thirty_seconds_reaches_the_caller() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
        let endpoint = listener.local_addr().expect("address");
        let server = thread::spawn(move || {
            let (mut socket, _) = listener.accept().expect("accept");
            read_response_header(&mut socket).expect("request headers");
            // Exercise the real gateway socket past the previous hard-coded
            // deadline. The supervisor owns the tenant's execution timeout.
            thread::sleep(Duration::from_secs(32));
            socket
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok")
                .expect("response");
        });
        let tenant = TenantScope::new(
            ProjectId::parse("prj_d2b2dcc3f5acbcc2105200bafbde2134".to_owned()).expect("project"),
            EnvironmentId::parse("env_4cfece7ecdaeba1f83064208c1cd84d6".to_owned())
                .expect("environment"),
        );
        let context = FunctionInvocationAuditContext {
            tenant: tenant.clone(),
            organization_id: "org_test".to_owned(),
            function_name: "delayed-response".to_owned(),
            version: 1,
            region: "test".to_owned(),
            regional_failover: false,
            request_id: "req_test".to_owned(),
            trace_id: "trace_test".to_owned(),
            actor: FunctionInvocationActor::PublicWebhook,
        };
        let response = block_on(LoopbackRuntimeInvoker::new(endpoint).invoke(
            RuntimeFunctionInvocation {
                tenant,
                function_name: context.function_name.clone(),
                version: 1,
                region: context.region.clone(),
                method: FunctionHttpMethod::Get,
                path_and_query: "/".to_owned(),
                headers: Vec::new(),
                body: Vec::new(),
                response_limit_bytes: 1024,
                request_id: context.request_id.clone(),
                trace_id: context.trace_id.clone(),
                caller: None,
                caller_token: None,
                audit_context: context,
            },
        ))
        .expect("response survives transport wait");
        assert_eq!(response.status, 200);
        let chunks = block_on(response.body.collect::<Vec<_>>());
        assert_eq!(
            chunks
                .into_iter()
                .flat_map(Result::unwrap)
                .collect::<Vec<_>>(),
            b"ok"
        );
        server.join().expect("server");
    }
}
