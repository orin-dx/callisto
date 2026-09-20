//! A dependency-free loopback HTTP server for tests.
//!
//! Included verbatim by the provider protocol tests in `callisto-graph` and by
//! the CLI end-to-end release harness, so both exercise one server rather than
//! two fakes that can drift apart. It binds `127.0.0.1:0`, so nothing it serves
//! can reach or be reached from a real registry.
#![allow(dead_code)]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::{Arc, Mutex};

/// One request the server received, as the handler and the assertions see it.
#[derive(Clone, Debug)]
pub struct LoopbackRequest {
    pub method: String,
    pub path: String,
    pub headers: Vec<(String, String)>,
}

impl LoopbackRequest {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }
}

#[derive(Clone, Debug)]
pub struct LoopbackResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: String,
}

impl LoopbackResponse {
    pub fn new(status: u16, body: impl Into<String>) -> Self {
        Self {
            status,
            headers: Vec::new(),
            body: body.into(),
        }
    }

    pub fn with_header(mut self, name: &str, value: &str) -> Self {
        self.headers.push((name.to_owned(), value.to_owned()));
        self
    }

    pub fn not_found() -> Self {
        Self::new(404, "{}")
    }
}

/// A running server. Dropping it leaves the accept thread alive for the rest
/// of the process, which is what a test binary wants: no shutdown race, and no
/// port reuse between tests because each server binds its own ephemeral port.
pub struct LoopbackServer {
    address: SocketAddr,
    requests: Arc<Mutex<Vec<LoopbackRequest>>>,
}

impl LoopbackServer {
    pub fn start<H>(handler: H) -> Self
    where
        H: Fn(&LoopbackRequest) -> LoopbackResponse + Send + Sync + 'static,
    {
        let listener = TcpListener::bind("127.0.0.1:0").expect("loopback listener must bind");
        let address = listener.local_addr().expect("bound listener has an address");
        let requests = Arc::new(Mutex::new(Vec::new()));
        let recorded = Arc::clone(&requests);
        let handler = Arc::new(handler);
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { continue };
                let recorded = Arc::clone(&recorded);
                let handler = Arc::clone(&handler);
                std::thread::spawn(move || serve_one(stream, &*handler, &recorded));
            }
        });
        Self { address, requests }
    }

    /// The base URL, with a trailing slash.
    pub fn base_url(&self) -> String {
        format!("http://{}/", self.address)
    }

    pub fn requests(&self) -> Vec<LoopbackRequest> {
        self.requests.lock().expect("request log is not poisoned").clone()
    }

    pub fn paths(&self) -> Vec<String> {
        self.requests().into_iter().map(|request| request.path).collect()
    }
}

fn serve_one<H>(mut stream: TcpStream, handler: &H, recorded: &Mutex<Vec<LoopbackRequest>>)
where
    H: Fn(&LoopbackRequest) -> LoopbackResponse + ?Sized,
{
    let Some(request) = read_request(&mut stream) else {
        return;
    };
    recorded
        .lock()
        .expect("request log is not poisoned")
        .push(request.clone());
    let response = handler(&request);
    let mut raw = format!(
        "HTTP/1.1 {} {}\r\ncontent-length: {}\r\nconnection: close\r\n",
        response.status,
        reason_phrase(response.status),
        response.body.len()
    );
    for (name, value) in &response.headers {
        raw.push_str(&format!("{name}: {value}\r\n"));
    }
    raw.push_str("\r\n");
    raw.push_str(&response.body);
    drop(stream.write_all(raw.as_bytes()));
    drop(stream.flush());
}

fn read_request(stream: &mut TcpStream) -> Option<LoopbackRequest> {
    let mut reader = BufReader::new(stream.try_clone().ok()?);
    let mut request_line = String::new();
    if reader.read_line(&mut request_line).ok()? == 0 {
        return None;
    }
    let mut parts = request_line.split_whitespace();
    let method = parts.next()?.to_owned();
    let path = parts.next()?.to_owned();
    let mut headers = Vec::new();
    let mut content_length = 0_usize;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).ok()? == 0 {
            break;
        }
        let line = line.trim_end_matches(['\r', '\n']);
        if line.is_empty() {
            break;
        }
        if let Some((name, value)) = line.split_once(':') {
            if name.eq_ignore_ascii_case("content-length") {
                content_length = value.trim().parse().unwrap_or(0);
            }
            headers.push((name.trim().to_owned(), value.trim().to_owned()));
        }
    }
    if content_length > 0 {
        let mut body = vec![0_u8; content_length];
        reader.read_exact(&mut body).ok()?;
    }
    Some(LoopbackRequest { method, path, headers })
}

fn reason_phrase(status: u16) -> &'static str {
    match status {
        200 => "OK",
        204 => "No Content",
        403 => "Forbidden",
        404 => "Not Found",
        429 => "Too Many Requests",
        500 => "Internal Server Error",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        _ => "Status",
    }
}
