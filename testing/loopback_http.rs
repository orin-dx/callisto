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

    /// Serves a captured `--include` response: its status, its real headers
    /// (minus framing the server writes itself) and its body.
    pub fn from_raw_http(raw: &str) -> Self {
        let (head, body) = fixtures::split_raw_http(raw);
        let mut lines = head.lines();
        let status = lines
            .next()
            .and_then(|line| line.split_whitespace().nth(1))
            .and_then(|code| code.parse().ok())
            .expect("captured response starts with an HTTP status line");
        let headers = lines
            .filter_map(|line| line.trim_end_matches('\r').split_once(':'))
            .filter(|(name, _)| {
                !["content-length", "connection", "transfer-encoding", "content-encoding"]
                    .iter()
                    .any(|framing| name.eq_ignore_ascii_case(framing))
            })
            .map(|(name, value)| (name.trim().to_owned(), value.trim().to_owned()))
            .collect();
        Self {
            status,
            headers,
            body: body.to_owned(),
        }
    }
}

/// Captured provider responses (see `testing/fixtures/providers/*/PROVENANCE.md`)
/// and the templates that substitute names, versions, tags and assets into
/// their real shapes, so fakes serve provider bytes rather than a guess at them.
pub mod fixtures {
    use serde_json::Value;

    pub const CARGO_INFO_FOUND: &str = include_str!("fixtures/providers/cargo-info/found.cmd");
    pub const CARGO_INFO_ABSENT_VERSION: &str = include_str!("fixtures/providers/cargo-info/absent-version.cmd");
    pub const CARGO_INFO_UNKNOWN_CRATE: &str = include_str!("fixtures/providers/cargo-info/unknown-crate.cmd");
    pub const CARGO_INFO_YANKED: &str = include_str!("fixtures/providers/cargo-info/yanked.cmd");
    pub const CARGO_INFO_NETWORK_FAILURE: &str = include_str!("fixtures/providers/cargo-info/network-failure.cmd");
    pub const NPM_VIEW_FOUND: &str = include_str!("fixtures/providers/npm-view/found.cmd");
    pub const NPM_VIEW_MISSING: &str = include_str!("fixtures/providers/npm-view/missing.cmd");
    pub const GITHUB_RELEASE_PUBLISHED: &str = include_str!("fixtures/providers/github/release-published.http");
    pub const GITHUB_RELEASE_DRAFT: &str = include_str!("fixtures/providers/github/release-draft.http");
    pub const GITHUB_RELEASE_PRERELEASE: &str = include_str!("fixtures/providers/github/release-prerelease.http");
    pub const GITHUB_RELEASE_LIST: &str = include_str!("fixtures/providers/github/release-list-page.http");
    pub const GITHUB_RELEASE_404: &str = include_str!("fixtures/providers/github/release-404.http");
    pub const PYPI_SIMPLE_FOUND: &str = include_str!("fixtures/providers/pypi-simple/found.http");
    pub const PYPI_SIMPLE_YANKED: &str = include_str!("fixtures/providers/pypi-simple/yanked.http");
    pub const PYPI_SIMPLE_ABSENT: &str = include_str!("fixtures/providers/pypi-simple/absent.http");
    pub const LS_REMOTE_ANNOTATED: &str = include_str!("fixtures/providers/git/ls-remote-annotated.txt");
    pub const LS_REMOTE_LIGHTWEIGHT: &str = include_str!("fixtures/providers/git/ls-remote-lightweight.txt");
    pub const LS_REMOTE_ABSENT: &str = include_str!("fixtures/providers/git/ls-remote-absent.txt");

    /// The header block (through the blank line) and the body of a raw response.
    pub fn split_raw_http(raw: &str) -> (&str, &str) {
        let split = raw
            .find("\r\n\r\n")
            .map(|at| at + 4)
            .or_else(|| raw.find("\n\n").map(|at| at + 2))
            .expect("captured response has a blank line after its headers");
        raw.split_at(split)
    }

    pub fn body_of(raw: &str) -> &str {
        split_raw_http(raw).1
    }

    /// One captured command run: its exit code, stdout and stderr.
    pub struct CapturedCommand {
        pub exit_code: i32,
        pub stdout: String,
        pub stderr: String,
    }

    /// Reads a `.cmd` fixture (`exit: N`, `--- stdout`, `--- stderr`).
    pub fn captured_command(raw: &str) -> CapturedCommand {
        let mut exit_code = 0;
        let mut stdout = String::new();
        let mut stderr = String::new();
        let mut stream = 0_u8;
        for line in raw.lines() {
            match line {
                "--- stdout" => stream = 1,
                "--- stderr" => stream = 2,
                _ if stream == 0 => {
                    if let Some(code) = line.strip_prefix("exit: ") {
                        exit_code = code.trim().parse().expect("captured exit code is a number");
                    }
                }
                _ => {
                    let target = if stream == 1 { &mut stdout } else { &mut stderr };
                    target.push_str(line);
                    target.push('\n');
                }
            }
        }
        CapturedCommand {
            exit_code,
            stdout,
            stderr,
        }
    }

    /// The captured GitHub release object with the fields the code reads replaced.
    /// `assets` are `(name, size, digest)`; each is stamped onto the captured asset shape.
    pub fn github_release(
        tag: &str,
        draft: bool,
        prerelease: bool,
        target_commitish: &str,
        assets: &[(&str, u64, &str)],
    ) -> Value {
        let mut release: Value =
            serde_json::from_str(body_of(GITHUB_RELEASE_PUBLISHED)).expect("captured release is JSON");
        let asset_shape = release["assets"][0].clone();
        release["tag_name"] = tag.into();
        release["draft"] = draft.into();
        release["prerelease"] = prerelease.into();
        release["target_commitish"] = target_commitish.into();
        release["assets"] = assets
            .iter()
            .map(|(name, size, digest)| {
                let mut asset = asset_shape.clone();
                asset["name"] = (*name).into();
                asset["size"] = (*size).into();
                asset["digest"] = format!("sha256:{digest}").into();
                asset
            })
            .collect();
        release
    }

    /// What `gh api --include` prints for a 200 carrying `body`: the captured status line and headers, then the body.
    pub fn gh_api_stdout(body: &str) -> String {
        format!("{}{body}", split_raw_http(GITHUB_RELEASE_PUBLISHED).0)
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
