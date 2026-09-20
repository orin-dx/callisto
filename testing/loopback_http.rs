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
    use super::LoopbackResponse;
    use serde_json::Value;

    pub const CRATES_INDEX_200: &str = include_str!("fixtures/providers/crates-io/sparse-index-200.http");
    pub const CRATES_INDEX_404: &str = include_str!("fixtures/providers/crates-io/sparse-index-404.http");
    pub const CRATES_YANKED_LINE: &str = include_str!("fixtures/providers/crates-io/sparse-index-yanked-line.json");
    pub const PYPI_200: &str = include_str!("fixtures/providers/pypi/version-200.http");
    pub const PYPI_404: &str = include_str!("fixtures/providers/pypi/version-404.http");
    pub const GITHUB_RELEASE_PUBLISHED: &str = include_str!("fixtures/providers/github/release-published.http");
    pub const GITHUB_RELEASE_DRAFT: &str = include_str!("fixtures/providers/github/release-draft.http");
    pub const GITHUB_RELEASE_PRERELEASE: &str = include_str!("fixtures/providers/github/release-prerelease.http");
    pub const GITHUB_RELEASE_LIST: &str = include_str!("fixtures/providers/github/release-list-page.http");
    pub const GITHUB_RELEASE_404: &str = include_str!("fixtures/providers/github/release-404.http");
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

    /// The captured sparse-index line, renamed and re-versioned.
    pub fn crates_index_line(name: &str, version: &str, cksum: &str, yanked: bool) -> String {
        let line = body_of(CRATES_INDEX_200)
            .lines()
            .next()
            .expect("captured index has lines");
        let mut entry: Value = serde_json::from_str(line).expect("captured index line is JSON");
        entry["name"] = name.into();
        entry["vers"] = version.into();
        entry["cksum"] = cksum.into();
        entry["yanked"] = yanked.into();
        entry.to_string()
    }

    /// A 200 sparse-index answer carrying the given `(version, cksum, yanked)` lines.
    pub fn crates_index_response(name: &str, versions: &[(&str, &str, bool)]) -> LoopbackResponse {
        let mut response = LoopbackResponse::from_raw_http(CRATES_INDEX_200);
        response.body = versions
            .iter()
            .map(|(version, cksum, yanked)| format!("{}\n", crates_index_line(name, version, cksum, *yanked)))
            .collect();
        response
    }

    pub fn crates_index_not_found() -> LoopbackResponse {
        LoopbackResponse::from_raw_http(CRATES_INDEX_404)
    }

    /// The captured PyPI version document with the first file's digest and yank flag replaced.
    pub fn pypi_version_response(name: &str, version: &str, sha256: &str, yanked: bool) -> LoopbackResponse {
        let mut response = LoopbackResponse::from_raw_http(PYPI_200);
        let mut document: Value = serde_json::from_str(&response.body).expect("captured PyPI body is JSON");
        document["info"]["name"] = name.into();
        document["info"]["version"] = version.into();
        document["urls"][0]["digests"]["sha256"] = sha256.into();
        document["urls"][0]["yanked"] = yanked.into();
        response.body = document.to_string();
        response
    }

    pub fn pypi_not_found() -> LoopbackResponse {
        LoopbackResponse::from_raw_http(PYPI_404)
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
