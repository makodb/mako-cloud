//! A minimal DNS client for one purpose: reading the TXT record that proves
//! control of a custom domain.
//!
//! One query over UDP to the configured resolver with recursion desired, a
//! bounded wait, one retry. The resolver does the recursion; this client
//! only encodes the question and decodes the answer section, following name
//! compression pointers and concatenating each TXT record's character
//! strings. Anything the client cannot trust -- a truncated answer, a server
//! failure, no answer in time -- is reported as unavailable rather than as
//! an absent record, so a resolver hiccup never reads as a withdrawn proof.

use std::{
    error::Error,
    fmt,
    io::ErrorKind,
    net::{SocketAddr, UdpSocket},
    time::{Duration, Instant},
};

use rand_core::{OsRng, RngCore};

/// How long one query waits for its answer.
pub const QUERY_TIMEOUT: Duration = Duration::from_secs(3);
/// One query and one retry.
pub const QUERY_ATTEMPTS: u32 = 2;
/// A fully qualified name is at most this long in presentation form.
pub const MAXIMUM_NAME_BYTES: usize = 253;
const MAXIMUM_LABEL_BYTES: usize = 63;
/// Larger than any answer a resolver returns without EDNS (512 bytes), with
/// room for resolvers that send more.
const MAXIMUM_MESSAGE_BYTES: usize = 4_096;
const HEADER_BYTES: usize = 12;
const TYPE_TXT: u16 = 16;
const CLASS_IN: u16 = 1;
const FLAG_RESPONSE: u16 = 0x8000;
const FLAG_TRUNCATED: u16 = 0x0200;
const FLAG_RECURSION_DESIRED: u16 = 0x0100;
const RCODE_MASK: u16 = 0x000F;
const RCODE_NO_ERROR: u16 = 0;
const RCODE_NAME_ERROR: u16 = 3;
/// A compression chain longer than this is a loop, not a name.
const MAXIMUM_POINTER_HOPS: usize = 64;

/// Answers "what TXT records does this name have right now".
pub trait TxtResolver: Send + Sync {
    /// Every TXT record of `name`, each record's character strings joined.
    /// An empty list means the name has no TXT record (or does not exist);
    /// an error means the question could not be answered.
    fn lookup_txt(&self, name: &str) -> Result<Vec<String>, DnsError>;
}

#[derive(Debug)]
pub enum DnsError {
    /// The name cannot be encoded as a DNS question.
    InvalidName,
    /// No answer arrived within the bounded wait.
    Timeout,
    /// The resolver's answer did not fit in the datagram; a partial answer
    /// is not evidence of anything.
    Truncated,
    /// The resolver answered with an error other than "no such name".
    ServerFailure(u16),
    /// The datagram was not a well-formed answer to the question.
    Malformed,
    /// The datagram answered another question; the client keeps waiting.
    UnexpectedId,
    Io(std::io::Error),
}

impl DnsError {
    /// Whether the outcome says nothing about the record: the lookup failed
    /// rather than the record being absent.
    #[must_use]
    pub const fn is_unavailable(&self) -> bool {
        !matches!(self, Self::InvalidName)
    }
}

impl fmt::Display for DnsError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidName => formatter.write_str("DNS name is invalid"),
            Self::Timeout => formatter.write_str("DNS resolver did not answer in time"),
            Self::Truncated => formatter.write_str("DNS answer was truncated"),
            Self::ServerFailure(code) => write!(formatter, "DNS resolver answered rcode {code}"),
            Self::Malformed => formatter.write_str("DNS answer is malformed"),
            Self::UnexpectedId => formatter.write_str("DNS answer is for another query"),
            Self::Io(_) => formatter.write_str("DNS resolver could not be reached"),
        }
    }
}

impl Error for DnsError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            _ => None,
        }
    }
}

impl From<std::io::Error> for DnsError {
    fn from(error: std::io::Error) -> Self {
        match error.kind() {
            ErrorKind::WouldBlock | ErrorKind::TimedOut => Self::Timeout,
            _ => Self::Io(error),
        }
    }
}

/// The UDP client: one resolver, a bounded wait, one retry.
#[derive(Clone, Debug)]
pub struct UdpTxtResolver {
    resolver: SocketAddr,
    timeout: Duration,
    attempts: u32,
}

impl UdpTxtResolver {
    #[must_use]
    pub const fn new(resolver: SocketAddr) -> Self {
        Self {
            resolver,
            timeout: QUERY_TIMEOUT,
            attempts: QUERY_ATTEMPTS,
        }
    }

    /// A shorter wait for tests that talk to a stub on loopback.
    #[must_use]
    pub const fn with_timeout(mut self, timeout: Duration, attempts: u32) -> Self {
        self.timeout = timeout;
        self.attempts = if attempts == 0 { 1 } else { attempts };
        self
    }

    #[must_use]
    pub const fn resolver(&self) -> SocketAddr {
        self.resolver
    }

    fn query_once(&self, name: &str) -> Result<Vec<String>, DnsError> {
        let id = random_id();
        let query = encode_txt_query(id, name)?;
        let bind: SocketAddr = if self.resolver.is_ipv4() {
            "0.0.0.0:0".parse().expect("IPv4 wildcard")
        } else {
            "[::]:0".parse().expect("IPv6 wildcard")
        };
        let socket = UdpSocket::bind(bind)?;
        // Connecting filters datagrams from anyone but the resolver.
        socket.connect(self.resolver)?;
        socket.send(&query)?;
        let deadline = Instant::now() + self.timeout;
        let mut buffer = [0_u8; MAXIMUM_MESSAGE_BYTES];
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(DnsError::Timeout);
            }
            socket.set_read_timeout(Some(remaining))?;
            let read = socket.recv(&mut buffer)?;
            match decode_txt_response(id, &buffer[..read]) {
                Err(DnsError::UnexpectedId) => {}
                outcome => return outcome,
            }
        }
    }
}

impl TxtResolver for UdpTxtResolver {
    fn lookup_txt(&self, name: &str) -> Result<Vec<String>, DnsError> {
        let mut last = DnsError::Timeout;
        for _ in 0..self.attempts {
            match self.query_once(name) {
                Ok(records) => return Ok(records),
                // Neither a bad name nor a truncated answer improves on retry.
                Err(error @ (DnsError::InvalidName | DnsError::Truncated)) => return Err(error),
                Err(error) => last = error,
            }
        }
        Err(last)
    }
}

fn random_id() -> u16 {
    let mut bytes = [0_u8; 2];
    OsRng.fill_bytes(&mut bytes);
    u16::from_be_bytes(bytes)
}

/// A standard query for `name`'s TXT records: recursion desired, one
/// question, class IN.
pub fn encode_txt_query(id: u16, name: &str) -> Result<Vec<u8>, DnsError> {
    let name = name.strip_suffix('.').unwrap_or(name);
    if name.is_empty() || name.len() > MAXIMUM_NAME_BYTES {
        return Err(DnsError::InvalidName);
    }
    let mut message = Vec::with_capacity(HEADER_BYTES + name.len() + 6);
    message.extend_from_slice(&id.to_be_bytes());
    message.extend_from_slice(&FLAG_RECURSION_DESIRED.to_be_bytes());
    message.extend_from_slice(&1_u16.to_be_bytes());
    message.extend_from_slice(&[0_u8; 6]);
    for label in name.split('.') {
        if label.is_empty()
            || label.len() > MAXIMUM_LABEL_BYTES
            || !label
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        {
            return Err(DnsError::InvalidName);
        }
        message.push(u8::try_from(label.len()).map_err(|_| DnsError::InvalidName)?);
        message.extend_from_slice(label.as_bytes());
    }
    message.push(0);
    message.extend_from_slice(&TYPE_TXT.to_be_bytes());
    message.extend_from_slice(&CLASS_IN.to_be_bytes());
    Ok(message)
}

/// The TXT records in the answer section of a response to query `id`.
///
/// "No such name" is an empty list. Any other error code, a truncated
/// answer, or a datagram that is not a response is an error. TXT records
/// in the answer section are collected whatever their owner name, so a
/// name that is a CNAME to the record still resolves.
pub fn decode_txt_response(id: u16, message: &[u8]) -> Result<Vec<String>, DnsError> {
    if message.len() < HEADER_BYTES {
        return Err(DnsError::Malformed);
    }
    if read_u16(message, 0)? != id {
        return Err(DnsError::UnexpectedId);
    }
    let flags = read_u16(message, 2)?;
    if flags & FLAG_RESPONSE == 0 {
        return Err(DnsError::Malformed);
    }
    if flags & FLAG_TRUNCATED != 0 {
        return Err(DnsError::Truncated);
    }
    match flags & RCODE_MASK {
        RCODE_NO_ERROR => {}
        RCODE_NAME_ERROR => return Ok(Vec::new()),
        code => return Err(DnsError::ServerFailure(code)),
    }
    let question_count = read_u16(message, 4)?;
    let answer_count = read_u16(message, 6)?;
    let mut offset = HEADER_BYTES;
    for _ in 0..question_count {
        let (_, next) = read_name(message, offset)?;
        offset = next
            .checked_add(4)
            .filter(|end| *end <= message.len())
            .ok_or(DnsError::Malformed)?;
    }
    let mut records = Vec::new();
    for _ in 0..answer_count {
        let (_, next) = read_name(message, offset)?;
        offset = next;
        let record_type = read_u16(message, offset)?;
        let record_class = read_u16(message, offset + 2)?;
        let data_length = usize::from(read_u16(message, offset + 8)?);
        offset = offset.checked_add(10).ok_or(DnsError::Malformed)?;
        let data = message
            .get(offset..offset.checked_add(data_length).ok_or(DnsError::Malformed)?)
            .ok_or(DnsError::Malformed)?;
        offset += data_length;
        if record_type == TYPE_TXT && record_class == CLASS_IN {
            records.push(decode_character_strings(data)?);
        }
    }
    Ok(records)
}

/// A TXT record's `<character-string>`s, concatenated in order.
fn decode_character_strings(data: &[u8]) -> Result<String, DnsError> {
    let mut text = Vec::with_capacity(data.len());
    let mut cursor = 0;
    while cursor < data.len() {
        let length = usize::from(data[cursor]);
        cursor += 1;
        let chunk = data
            .get(cursor..cursor + length)
            .ok_or(DnsError::Malformed)?;
        text.extend_from_slice(chunk);
        cursor += length;
    }
    Ok(String::from_utf8_lossy(&text).into_owned())
}

/// A possibly compressed name at `offset`: the name in presentation form and
/// the offset just past it in the message (past the first pointer when the
/// name was compressed).
fn read_name(message: &[u8], mut offset: usize) -> Result<(String, usize), DnsError> {
    let mut labels = Vec::new();
    let mut resume: Option<usize> = None;
    let mut hops = 0;
    loop {
        let length = *message.get(offset).ok_or(DnsError::Malformed)?;
        match length & 0xC0 {
            0x00 => {
                offset += 1;
                if length == 0 {
                    break;
                }
                let label = message
                    .get(offset..offset + usize::from(length))
                    .ok_or(DnsError::Malformed)?;
                labels.push(String::from_utf8_lossy(label).into_owned());
                offset += usize::from(length);
            }
            0xC0 => {
                let low = *message.get(offset + 1).ok_or(DnsError::Malformed)?;
                let pointer = (usize::from(length & 0x3F) << 8) | usize::from(low);
                // Pointers only ever point backwards; anything else is a loop.
                if pointer >= offset {
                    return Err(DnsError::Malformed);
                }
                if resume.is_none() {
                    resume = Some(offset + 2);
                }
                hops += 1;
                if hops > MAXIMUM_POINTER_HOPS {
                    return Err(DnsError::Malformed);
                }
                offset = pointer;
            }
            _ => return Err(DnsError::Malformed),
        }
    }
    Ok((labels.join("."), resume.unwrap_or(offset)))
}

fn read_u16(message: &[u8], offset: usize) -> Result<u16, DnsError> {
    let bytes = message
        .get(offset..offset.checked_add(2).ok_or(DnsError::Malformed)?)
        .ok_or(DnsError::Malformed)?;
    Ok(u16::from_be_bytes([bytes[0], bytes[1]]))
}

#[cfg(test)]
mod tests {
    use std::{net::UdpSocket, thread, time::Duration};

    use super::*;

    /// The header and question exactly as RFC 1035 lays them out.
    #[test]
    fn a_query_is_a_standard_recursive_txt_question() {
        let query = encode_txt_query(0xBEEF, "_mako-verify.api.example.com.").expect("query");
        let mut expected = vec![0xBE, 0xEF, 0x01, 0x00, 0x00, 0x01, 0, 0, 0, 0, 0, 0];
        for label in ["_mako-verify", "api", "example", "com"] {
            expected.push(u8::try_from(label.len()).expect("short label"));
            expected.extend_from_slice(label.as_bytes());
        }
        expected.extend_from_slice(&[0, 0x00, 0x10, 0x00, 0x01]);
        assert_eq!(query, expected);
        for invalid in [
            "",
            ".",
            "a..b",
            &"a".repeat(64),
            "bad label!",
            &"a.".repeat(128),
        ] {
            assert!(
                matches!(
                    encode_txt_query(1, invalid).expect_err("invalid name"),
                    DnsError::InvalidName
                ),
                "{invalid:?} must be refused"
            );
        }
    }

    /// A response with the question echoed, a CNAME, and two TXT records --
    /// one of them split across two character strings -- using compression
    /// pointers for every owner name.
    fn sample_response(id: u16, flags: u16) -> Vec<u8> {
        let mut message = Vec::new();
        message.extend_from_slice(&id.to_be_bytes());
        message.extend_from_slice(&flags.to_be_bytes());
        message.extend_from_slice(&[0, 1, 0, 3, 0, 0, 0, 0]);
        // Question at offset 12: _mako-verify.example.test TXT IN
        for label in ["_mako-verify", "example", "test"] {
            message.push(u8::try_from(label.len()).expect("short label"));
            message.extend_from_slice(label.as_bytes());
        }
        message.push(0);
        message.extend_from_slice(&[0, 16, 0, 1]);
        // Answer 1: pointer to the question name, CNAME to "proof.example.test"
        // where "example.test" is itself a pointer into the question name.
        message.extend_from_slice(&[0xC0, 12, 0, 5, 0, 1, 0, 0, 0, 60]);
        let cname_start = message.len() + 2;
        let mut cname = vec![5];
        cname.extend_from_slice(b"proof");
        cname.extend_from_slice(&[0xC0, 12 + 13]);
        message.extend_from_slice(&u16::try_from(cname.len()).expect("length").to_be_bytes());
        message.extend_from_slice(&cname);
        // Answer 2: owner = pointer to the CNAME target, TXT in two strings.
        message.extend_from_slice(&[
            0xC0,
            u8::try_from(cname_start).expect("offset"),
            0,
            16,
            0,
            1,
            0,
            0,
            0,
            60,
        ]);
        let mut text = vec![16];
        text.extend_from_slice(b"mako-domain-veri");
        text.push(7);
        text.extend_from_slice(b"fy=abcd");
        message.extend_from_slice(&u16::try_from(text.len()).expect("length").to_be_bytes());
        message.extend_from_slice(&text);
        // Answer 3: an unrelated TXT record on the same owner.
        message.extend_from_slice(&[
            0xC0,
            u8::try_from(cname_start).expect("offset"),
            0,
            16,
            0,
            1,
            0,
            0,
            0,
            60,
        ]);
        let mut other = vec![9];
        other.extend_from_slice(b"v=spf1 -a");
        message.extend_from_slice(&u16::try_from(other.len()).expect("length").to_be_bytes());
        message.extend_from_slice(&other);
        message
    }

    #[test]
    fn an_answer_is_decoded_through_compression_and_split_strings() {
        let records = decode_txt_response(7, &sample_response(7, 0x8180)).expect("decoded");
        assert_eq!(
            records,
            vec!["mako-domain-verify=abcd".to_owned(), "v=spf1 -a".to_owned()],
            "character strings are concatenated per record, records kept in order"
        );
        let (name, next) = read_name(&sample_response(7, 0x8180), 12).expect("question name");
        assert_eq!(name, "_mako-verify.example.test");
        assert_eq!(next, 12 + 27);
    }

    #[test]
    fn answers_the_client_cannot_trust_are_errors_not_absence() {
        assert!(matches!(
            decode_txt_response(8, &sample_response(7, 0x8180)).expect_err("other id"),
            DnsError::UnexpectedId
        ));
        assert!(matches!(
            decode_txt_response(7, &sample_response(7, 0x8380)).expect_err("truncated"),
            DnsError::Truncated
        ));
        assert!(matches!(
            decode_txt_response(7, &sample_response(7, 0x8182)).expect_err("servfail"),
            DnsError::ServerFailure(2)
        ));
        assert!(matches!(
            decode_txt_response(7, &sample_response(7, 0x0180)).expect_err("not a response"),
            DnsError::Malformed
        ));
        assert!(matches!(
            decode_txt_response(7, &sample_response(7, 0x8180)[..40]).expect_err("cut short"),
            DnsError::Malformed
        ));
        assert!(matches!(
            decode_txt_response(7, &[0, 7, 0x81]).expect_err("shorter than a header"),
            DnsError::Malformed
        ));
        assert_eq!(
            decode_txt_response(7, &sample_response(7, 0x8183)).expect("nxdomain"),
            Vec::<String>::new(),
            "no such name is an empty answer, not an error"
        );
        // A pointer that points forwards (or at itself) is a loop.
        let mut looped = sample_response(7, 0x8180);
        let question_end = 12 + 27 + 4;
        looped[question_end] = 0xC0;
        looped[question_end + 1] = u8::try_from(question_end).expect("offset");
        assert!(matches!(
            decode_txt_response(7, &looped).expect_err("pointer loop"),
            DnsError::Malformed
        ));
        // A name with no TXT record at all is also an empty answer.
        let mut no_answers = sample_response(7, 0x8180);
        no_answers[7] = 0;
        no_answers.truncate(12 + 27 + 4);
        assert_eq!(
            decode_txt_response(7, &no_answers).expect("no records"),
            Vec::<String>::new()
        );
    }

    /// A loopback stub that answers only the second query proves the retry;
    /// one that never answers proves the timeout; one that answers
    /// truncated proves truncation is not retried.
    #[test]
    fn the_udp_client_retries_once_and_reports_silence_as_unavailable() {
        let server = UdpSocket::bind("127.0.0.1:0").expect("stub socket");
        let address = server.local_addr().expect("stub address");
        server
            .set_read_timeout(Some(Duration::from_secs(5)))
            .expect("timeout");
        let stub = thread::spawn(move || {
            let mut buffer = [0_u8; 512];
            // Swallow the first query, answer the second.
            let _ = server.recv_from(&mut buffer).expect("first query");
            let (read, peer) = server.recv_from(&mut buffer).expect("second query");
            let id = u16::from_be_bytes([buffer[0], buffer[1]]);
            let mut response = buffer[..read].to_vec();
            response[2] = 0x81;
            response[3] = 0x80;
            response[7] = 1;
            response.extend_from_slice(&[0xC0, 12, 0, 16, 0, 1, 0, 0, 0, 1, 0, 6, 5]);
            response.extend_from_slice(b"hello");
            server.send_to(&response, peer).expect("answer");
            // A third query gets a truncated answer.
            let (read, peer) = server.recv_from(&mut buffer).expect("third query");
            let mut truncated = buffer[..read].to_vec();
            truncated[2] = 0x83;
            truncated[3] = 0x80;
            server.send_to(&truncated, peer).expect("truncated answer");
            id
        });
        let client =
            UdpTxtResolver::new(address).with_timeout(Duration::from_millis(300), QUERY_ATTEMPTS);
        assert_eq!(
            client
                .lookup_txt("_mako-verify.example.test")
                .expect("retried"),
            vec!["hello".to_owned()]
        );
        assert!(matches!(
            client
                .lookup_txt("_mako-verify.example.test")
                .expect_err("truncated"),
            DnsError::Truncated
        ));
        stub.join().expect("stub");
        let silent = UdpSocket::bind("127.0.0.1:0").expect("silent socket");
        let client = UdpTxtResolver::new(silent.local_addr().expect("address"))
            .with_timeout(Duration::from_millis(100), 2);
        let error = client
            .lookup_txt("_mako-verify.example.test")
            .expect_err("silence");
        assert!(error.is_unavailable(), "{error}");
        assert!(matches!(
            client.lookup_txt("").expect_err("bad name"),
            DnsError::InvalidName
        ));
    }
}
