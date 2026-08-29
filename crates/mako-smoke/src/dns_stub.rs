//! A loopback DNS server that answers TXT questions from a table, so a smoke
//! test can publish and withdraw a custom domain's verification record
//! without touching real DNS.
//!
//! Point the control plane at it with `MAKO_DNS_RESOLVER=127.0.0.1:<port>`.
//! Every question for a name in the table is answered with that name's TXT
//! records; every other question is answered NXDOMAIN. The stub speaks
//! only as much DNS as the control plane's client sends: one question,
//! class IN, no EDNS.
use std::{
    collections::BTreeMap,
    net::{SocketAddr, UdpSocket},
    sync::{Arc, Mutex},
    thread,
    time::Duration,
};

const TYPE_TXT: u16 = 16;
const CLASS_IN: u16 = 1;
const RCODE_NAME_ERROR: u16 = 3;

pub struct DnsStub {
    /// The address to configure as `MAKO_DNS_RESOLVER`.
    pub address: SocketAddr,
    records: Arc<Mutex<BTreeMap<String, Vec<String>>>>,
    queries: Arc<Mutex<Vec<String>>>,
}

impl DnsStub {
    pub fn start() -> Self {
        let socket = UdpSocket::bind("127.0.0.1:0").expect("dns stub socket");
        let address = socket.local_addr().expect("dns stub address");
        socket
            .set_read_timeout(Some(Duration::from_millis(250)))
            .expect("dns stub timeout");
        let records = Arc::new(Mutex::new(BTreeMap::new()));
        let queries = Arc::new(Mutex::new(Vec::new()));
        let (table, seen) = (Arc::clone(&records), Arc::clone(&queries));
        thread::spawn(move || {
            let mut buffer = [0_u8; 1024];
            loop {
                let (read, peer) = match socket.recv_from(&mut buffer) {
                    Ok(received) => received,
                    Err(error)
                        if matches!(
                            error.kind(),
                            std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                        ) =>
                    {
                        // Stop once the stub has been dropped.
                        if Arc::strong_count(&table) == 1 {
                            return;
                        }
                        continue;
                    }
                    Err(_) => return,
                };
                let Some(question) = parse_question(&buffer[..read]) else {
                    continue;
                };
                seen.lock().expect("queries").push(question.name.clone());
                let answers = table
                    .lock()
                    .expect("records")
                    .get(&question.name.to_ascii_lowercase())
                    .cloned();
                let response = build_response(&buffer[..read], &question, answers.as_deref());
                let _ = socket.send_to(&response, peer);
            }
        });
        Self {
            address,
            records,
            queries,
        }
    }

    /// Publishes `values` as the TXT records of `name` (case-insensitive,
    /// with or without a trailing dot).
    pub fn set_txt(&self, name: &str, values: &[&str]) {
        self.records.lock().expect("records").insert(
            normalize(name),
            values.iter().map(|value| (*value).to_owned()).collect(),
        );
    }

    /// Removes every record of `name`; the next question is NXDOMAIN.
    pub fn clear(&self, name: &str) {
        self.records
            .lock()
            .expect("records")
            .remove(&normalize(name));
    }

    /// Every question name the stub has answered, in order.
    pub fn queries(&self) -> Vec<String> {
        self.queries.lock().expect("queries").clone()
    }
}

fn normalize(name: &str) -> String {
    name.trim_end_matches('.').to_ascii_lowercase()
}

struct Question {
    name: String,
    record_type: u16,
    record_class: u16,
    /// Where the question section ends in the query.
    end: usize,
}

/// The single question of a query: its name in presentation form and where
/// it ends, so the response can echo it byte for byte.
fn parse_question(message: &[u8]) -> Option<Question> {
    if message.len() < 12 || u16::from_be_bytes([message[4], message[5]]) != 1 {
        return None;
    }
    let mut offset = 12;
    let mut labels = Vec::new();
    loop {
        let length = usize::from(*message.get(offset)?);
        offset += 1;
        if length == 0 {
            break;
        }
        if length & 0xC0 != 0 {
            // A compressed question name is not something the client sends.
            return None;
        }
        labels.push(String::from_utf8_lossy(message.get(offset..offset + length)?).into_owned());
        offset += length;
    }
    let record_type = u16::from_be_bytes([*message.get(offset)?, *message.get(offset + 1)?]);
    let record_class = u16::from_be_bytes([*message.get(offset + 2)?, *message.get(offset + 3)?]);
    Some(Question {
        name: labels.join("."),
        record_type,
        record_class,
        end: offset + 4,
    })
}

/// The response: the query's header with QR, RD, and RA set, the question
/// echoed, and one TXT answer per value (owner name as a pointer to the
/// question, split into 255-byte character strings), or NXDOMAIN.
fn build_response(query: &[u8], question: &Question, answers: Option<&[String]>) -> Vec<u8> {
    let mut response = query[..question.end].to_vec();
    let mut flags = 0x8180_u16;
    let answers = match answers {
        Some(values) if question.record_type == TYPE_TXT && question.record_class == CLASS_IN => {
            values
        }
        Some(_) => &[],
        None => {
            flags |= RCODE_NAME_ERROR;
            &[]
        }
    };
    response[2..4].copy_from_slice(&flags.to_be_bytes());
    response[6..8].copy_from_slice(&u16::try_from(answers.len()).unwrap_or(0).to_be_bytes());
    response[8..12].copy_from_slice(&[0, 0, 0, 0]);
    for value in answers {
        response.extend_from_slice(&[0xC0, 12]);
        response.extend_from_slice(&TYPE_TXT.to_be_bytes());
        response.extend_from_slice(&CLASS_IN.to_be_bytes());
        response.extend_from_slice(&30_u32.to_be_bytes());
        let mut data = Vec::with_capacity(value.len() + 2);
        for chunk in value.as_bytes().chunks(255) {
            data.push(u8::try_from(chunk.len()).expect("chunked to 255"));
            data.extend_from_slice(chunk);
        }
        response.extend_from_slice(&u16::try_from(data.len()).unwrap_or(u16::MAX).to_be_bytes());
        response.extend_from_slice(&data);
    }
    response
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use mako_control_plane::{DnsError, TxtResolver, UdpTxtResolver};

    use super::*;

    /// The control plane's own client reads what the stub publishes, sees
    /// the record disappear when it is cleared, and gets a clean "no such
    /// name" for anything else.
    #[test]
    fn the_control_plane_client_reads_the_stub() {
        let stub = DnsStub::start();
        let client = UdpTxtResolver::new(stub.address).with_timeout(Duration::from_secs(2), 2);
        assert_eq!(
            client
                .lookup_txt("_mako-verify.api.example.test")
                .expect("nxdomain"),
            Vec::<String>::new(),
            "an unknown name is an empty answer, not unavailability"
        );
        let long = "x".repeat(300);
        stub.set_txt(
            "_mako-verify.API.example.test.",
            &["mako-domain-verify=abc123", "v=spf1 -all", &long],
        );
        assert_eq!(
            client
                .lookup_txt("_mako-verify.api.example.test")
                .expect("published"),
            vec![
                "mako-domain-verify=abc123".to_owned(),
                "v=spf1 -all".to_owned(),
                long,
            ],
            "records are answered in order, long values reassembled from their character strings"
        );
        stub.clear("_mako-verify.api.example.test");
        assert_eq!(
            client
                .lookup_txt("_mako-verify.api.example.test")
                .expect("withdrawn"),
            Vec::<String>::new()
        );
        assert_eq!(
            stub.queries(),
            vec!["_mako-verify.api.example.test".to_owned(); 3]
        );
        assert!(matches!(
            client.lookup_txt("").expect_err("empty name"),
            DnsError::InvalidName
        ));
    }
}
