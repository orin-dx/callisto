//! Real local npm, PyPI and Cargo registry servers for the real-registry suites.
#![cfg(unix)]
#![allow(dead_code, unused_imports)]

use std::fs;
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use crate::release_harness::*;

pub const NPM_ORIGIN: &str = "https://github.com/example/npm-registry-fixture.git";

/// Finds `program` on `PATH`, the way a shell would. Never panics, so a test
/// can skip cleanly when the real tool isn't installed.
pub fn find_on_path(program: &str) -> Option<PathBuf> {
    std::env::var_os("PATH").and_then(|path| {
        std::env::split_paths(&path)
            .map(|dir| dir.join(program))
            .find(|candidate| candidate.is_file())
    })
}

/// An ephemeral loopback port: bind to port 0, read back what the OS
/// assigned, release it immediately. The gap between release and the real
/// server binding it is a bounded TOCTOU risk, acceptable for test
/// infrastructure that owns the box.
pub fn free_loopback_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("loopback listener must bind");
    listener
        .local_addr()
        .expect("listener must have a local address")
        .port()
}

/// A spawned server process, killed on drop so a panicking assertion never
/// leaks it past the test. `log_path` is its combined stdout+stderr, written
/// to a file rather than piped so a chatty server can never fill an
/// undrained pipe and deadlock the test.
pub struct ServerGuard {
    child: Child,
    log_path: PathBuf,
}

impl ServerGuard {
    pub fn log(&self) -> String {
        fs::read_to_string(&self.log_path).unwrap_or_default()
    }
}

impl Drop for ServerGuard {
    fn drop(&mut self) {
        drop(self.child.kill());
        drop(self.child.wait());
    }
}

/// Redirects a spawned command's stdout and stderr to one combined log file
/// under `dir`/`name`, so a startup crash is diagnosable instead of silent.
pub fn log_to_file(command: &mut Command, dir: &Path, name: &str) -> PathBuf {
    let log_path = dir.join(name);
    let file = fs::File::create(&log_path).expect("server log file must be creatable");
    let file_for_stderr = file.try_clone().expect("log file handle must be cloneable");
    command.stdin(Stdio::null()).stdout(file).stderr(file_for_stderr);
    log_path
}

/// Blocks until `port` accepts a loopback TCP connection, or panics after
/// `timeout` -- including the server's captured output in the panic message
/// either way, and its exit status if it already died. Checking `try_wait`
/// first means a server that crashes at startup fails fast with its real
/// error instead of spinning out the whole timeout with no evidence.
pub fn wait_for_port(guard: &mut ServerGuard, port: u16, timeout: Duration) {
    let deadline = Instant::now() + timeout;
    loop {
        if TcpStream::connect(("127.0.0.1", port)).is_ok() {
            return;
        }
        if let Ok(Some(status)) = guard.child.try_wait() {
            panic!(
                "server exited with {status} before binding 127.0.0.1:{port}; output:\n{}",
                guard.log()
            );
        }
        if Instant::now() >= deadline {
            panic!(
                "server on 127.0.0.1:{port} never became reachable within {timeout:?}; output:\n{}",
                guard.log()
            );
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// GETs `url` with `curl` -- the same tool callisto's own PyPI observation
/// uses -- for an assertion independent of callisto's own HTTP client code.
pub fn curl_get(url: &str) -> String {
    let output = Command::new("curl")
        .args(["-sS", url])
        .output()
        .expect("curl must be runnable");
    assert!(
        output.status.success(),
        "curl {url} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).into_owned()
}

/// A self-signed certificate for `127.0.0.1`, so a local registry can be bound over `https://` exactly as production
/// requires. Package managers trust it through their own CA environment variables, see [`Tls::trust_env`].
pub struct Tls {
    pub cert: PathBuf,
    pub key: PathBuf,
}

impl Tls {
    pub fn generate(dir: &Path) -> Self {
        let tls = Self {
            cert: dir.join("registry-cert.pem"),
            key: dir.join("registry-key.pem"),
        };
        let output = Command::new("openssl")
            .args([
                "req",
                "-x509",
                "-newkey",
                "rsa:2048",
                "-nodes",
                "-days",
                "1",
                "-subj",
                "/CN=localhost",
                "-addext",
                "subjectAltName=IP:127.0.0.1,DNS:localhost",
                "-keyout",
            ])
            .arg(&tls.key)
            .arg("-out")
            .arg(&tls.cert)
            .output()
            .expect("openssl must be runnable");
        assert!(
            output.status.success(),
            "openssl failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        tls
    }

    /// The variables that make npm, curl, and twine's `requests` trust this certificate.
    pub fn trust_env(&self) -> Vec<(&'static str, String)> {
        let cert = self.cert.display().to_string();
        vec![
            ("NODE_EXTRA_CA_CERTS", cert.clone()),
            ("CURL_CA_BUNDLE", cert.clone()),
            ("REQUESTS_CA_BUNDLE", cert),
        ]
    }
}

/// Starts a real local Verdaccio on an ephemeral loopback port with
/// anonymous publish enabled (`access`/`publish`/`unpublish: $all`) and no
/// uplinks configured, so a lookup for an unknown package fails closed (404)
/// instead of reaching the real npm registry. With `tls` it serves `https://`. A fixed `port` lets a restarted
/// registry come back where `callisto.toml` points, and `name` names its directory: the storage survives a restart.
/// Panics if it never becomes reachable.
pub fn start_verdaccio(external: &Path, name: &str, port: u16, tls: Option<&Tls>) -> ServerGuard {
    start_verdaccio_with(external, name, port, tls, VERDACCIO_OPEN_ACCESS)
}

/// Anonymous read, publish and unpublish for every package.
pub const VERDACCIO_OPEN_ACCESS: &str =
    "packages:\n  '**':\n    access: $all\n    publish: $all\n    unpublish: $all\n";

/// [`start_verdaccio`] with a custom `packages:` stanza, for a registry that refuses chosen packages.
pub fn start_verdaccio_with(external: &Path, name: &str, port: u16, tls: Option<&Tls>, packages: &str) -> ServerGuard {
    let config_dir = external.join(format!("verdaccio-{name}"));
    fs::create_dir_all(config_dir.join("storage")).unwrap();
    let scheme = if tls.is_some() { "https" } else { "http" };
    let https = tls.map_or_else(String::new, |tls| {
        format!("https:\n  key: {}\n  cert: {}\n", tls.key.display(), tls.cert.display())
    });
    fs::write(
        config_dir.join("config.yaml"),
        format!(
            "storage: ./storage\n\
             self_path: ./storage\n\
             web:\n  enable: false\n\
             log: {{ type: stdout, format: pretty, level: warn }}\n\
             listen: {scheme}://127.0.0.1:{port}\n\
             {https}{packages}"
        ),
    )
    .unwrap();
    let mut command = Command::new("verdaccio");
    command.args(["-c", "config.yaml"]);
    command.current_dir(&config_dir);
    let log_path = log_to_file(&mut command, external, &format!("verdaccio-{name}.log"));
    let child = command.spawn().expect("verdaccio must be spawnable");
    let mut guard = ServerGuard { child, log_path };
    wait_for_port(&mut guard, port, Duration::from_secs(20));
    guard
}

/// Starts a real local `pypiserver` on an ephemeral loopback port with
/// authentication fully disabled and `--disable-fallback`, so a lookup for an
/// unknown project 404s instead of proxying to real PyPI. `packages_dir` is the
/// served package store; with `tls` gunicorn terminates TLS.
///
/// gunicorn rather than pypiserver's default backend: `wsgiref` resolves the
/// bind address via `socket.getfqdn()`, a reverse DNS lookup that measured
/// 12-35s on macOS, against under a second for gunicorn.
pub fn start_pypiserver(external: &Path, name: &str, port: u16, packages_dir: &Path, tls: Option<&Tls>) -> ServerGuard {
    fs::create_dir_all(packages_dir).unwrap();
    let log_dir = external.join(format!("pypiserver-{name}"));
    fs::create_dir_all(&log_dir).unwrap();
    let mut command;
    if let Some(tls) = tls {
        command = Command::new("gunicorn");
        command
            .args(["--certfile"])
            .arg(&tls.cert)
            .arg("--keyfile")
            .arg(&tls.key)
            .args(["-b", &format!("127.0.0.1:{port}")])
            .arg(format!(
                "pypiserver:app(roots=['{}'], authenticate=[], password_file='.', disable_fallback=True)",
                packages_dir.display()
            ));
    } else {
        command = Command::new("pypi-server");
        command.args([
            "run",
            "--server",
            "gunicorn",
            "--host",
            "127.0.0.1",
            "--port",
            &port.to_string(),
            "--authenticate",
            ".",
            "--passwords",
            ".",
            "--disable-fallback",
            packages_dir.to_str().unwrap(),
        ]);
    }
    let log_path = log_to_file(&mut command, &log_dir, "pypiserver.log");
    let child = command.spawn().expect("pypiserver must be spawnable");
    let mut guard = ServerGuard { child, log_path };
    wait_for_port(&mut guard, port, Duration::from_secs(20));
    guard
}

/// The PEP 691 JSON simple index over a directory of distributions: what callisto's PyPI observation reads and
/// `pypiserver` cannot serve. Files in the directory are the index's content, so copying one in makes it visible.
const JSON_SIMPLE_INDEX: &str = r#"
import hashlib, http.server, json, os, re, ssl, sys
root, port, cert, key = sys.argv[1], int(sys.argv[2]), sys.argv[3], sys.argv[4]
normalize = lambda name: re.sub(r"[-_.]+", "-", name).lower()
class Handler(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        match = re.fullmatch(r"/simple/([^/]+)/", self.path)
        project = normalize(match.group(1)) if match else None
        files = []
        for name in sorted(os.listdir(root)) if project else []:
            stem = re.match(r"(.+?)-\d", name)
            if stem and normalize(stem.group(1)) == project:
                digest = hashlib.sha256(open(os.path.join(root, name), "rb").read()).hexdigest()
                files.append({"filename": name, "url": "/files/" + name, "hashes": {"sha256": digest}, "yanked": False})
        if not files:
            self.send_error(404)
            return
        body = json.dumps({"meta": {"api-version": "1.0"}, "name": project, "files": files}).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/vnd.pypi.simple.v1+json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)
    def log_message(self, format, *args):
        sys.stderr.write(format % args + "\n")
        sys.stderr.flush()
context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
context.load_cert_chain(cert, key)
server = http.server.ThreadingHTTPServer(("127.0.0.1", port), Handler)
server.socket = context.wrap_socket(server.socket, server_side=True)
server.serve_forever()
"#;

/// Starts [`JSON_SIMPLE_INDEX`] over `packages_dir` on `port`, served over `tls`.
pub fn start_json_simple_index(external: &Path, name: &str, port: u16, packages_dir: &Path, tls: &Tls) -> ServerGuard {
    fs::create_dir_all(packages_dir).unwrap();
    let script = external.join(format!("json-simple-index-{name}.py"));
    fs::write(&script, JSON_SIMPLE_INDEX).unwrap();
    let mut command = Command::new("python3");
    command
        .arg(&script)
        .arg(packages_dir)
        .arg(port.to_string())
        .arg(&tls.cert)
        .arg(&tls.key);
    let log_path = log_to_file(&mut command, external, &format!("json-simple-index-{name}.log"));
    let child = command.spawn().expect("python3 must be spawnable");
    let mut guard = ServerGuard { child, log_path };
    wait_for_port(&mut guard, port, Duration::from_secs(20));
    guard
}

/// A Cargo sparse registry with the publish and download API; its directory survives a restart. After `refuse_after`
/// publishes every publish fails, and the index shows a publish only `index_delay` seconds later. Logs to stderr.
const SPARSE_CARGO_REGISTRY: &str = r#"
import hashlib, http.server, json, os, re, sys, threading, time
root, port, refuse_after, index_delay = sys.argv[1], int(sys.argv[2]), int(sys.argv[3]), float(sys.argv[4])
state_path = os.path.join(root, "state.json")
os.makedirs(os.path.join(root, "crates"), exist_ok=True)
lock = threading.Lock()
state = json.load(open(state_path)) if os.path.exists(state_path) else {"entries": []}
published_this_run = 0
def index_path(name):
    name = name.lower()
    if len(name) <= 2:
        return "%d/%s" % (len(name), name)
    if len(name) == 3:
        return "3/%s/%s" % (name[0], name)
    return "%s/%s/%s" % (name[:2], name[2:4], name)
class Handler(http.server.BaseHTTPRequestHandler):
    def reply(self, status, body=b"", content_type="application/json"):
        self.send_response(status)
        self.send_header("Content-Type", content_type)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)
    def error(self, status, detail):
        self.reply(status, json.dumps({"errors": [{"detail": detail}]}).encode())
    def do_GET(self):
        if self.path == "/index/config.json":
            base = "http://127.0.0.1:%d" % port
            self.reply(200, json.dumps({"dl": base + "/api/v1/crates/{crate}/{version}/download", "api": base}).encode())
            return
        match = re.fullmatch(r"/api/v1/crates/([^/]+)/([^/]+)/download", self.path)
        if match:
            path = os.path.join(root, "crates", "%s-%s.crate" % match.groups())
            self.reply(200, open(path, "rb").read(), "application/octet-stream") if os.path.exists(path) else self.error(404, "no such crate")
            return
        if self.path.startswith("/index/"):
            wanted = self.path[len("/index/"):]
            with lock:
                lines = [e["line"] for e in state["entries"] if index_path(e["name"]) == wanted and e["visible_at"] <= time.time()]
            if lines:
                self.reply(200, ("\n".join(lines) + "\n").encode(), "text/plain")
                return
        self.error(404, "not found")
    def do_PUT(self):
        global published_this_run
        if self.path != "/api/v1/crates/new":
            self.error(404, "not found")
            return
        body = self.rfile.read(int(self.headers["Content-Length"]))
        size = int.from_bytes(body[:4], "little")
        meta = json.loads(body[4:4 + size])
        crate_size = int.from_bytes(body[4 + size:8 + size], "little")
        crate = body[8 + size:8 + size + crate_size]
        with lock:
            if published_this_run >= refuse_after:
                self.error(403, "publish refused by the test registry")
                return
            if any(e["name"] == meta["name"] and e["vers"] == meta["vers"] for e in state["entries"]):
                self.error(400, "crate version `%s` is already uploaded" % meta["vers"])
                return
            open(os.path.join(root, "crates", "%s-%s.crate" % (meta["name"], meta["vers"])), "wb").write(crate)
            deps = [{"name": d["name"], "req": d["version_req"], "features": d["features"], "optional": d["optional"],
                     "default_features": d["default_features"], "target": d["target"], "kind": d["kind"],
                     "registry": d.get("registry"), "package": d.get("explicit_name_in_toml")} for d in meta["deps"]]
            line = json.dumps({"name": meta["name"], "vers": meta["vers"], "deps": deps, "features": meta["features"],
                               "cksum": hashlib.sha256(crate).hexdigest(), "yanked": False, "v": 1})
            state["entries"].append({"name": meta["name"], "vers": meta["vers"], "line": line, "visible_at": time.time() + index_delay})
            json.dump(state, open(state_path, "w"))
            published_this_run += 1
            sys.stderr.write("published %s %s\n" % (meta["name"], meta["vers"]))
            sys.stderr.flush()
        self.reply(200, json.dumps({"warnings": {"invalid_categories": [], "invalid_badges": [], "other": []}}).encode())
    def log_message(self, format, *args):
        sys.stderr.write(format % args + "\n")
        sys.stderr.flush()
http.server.ThreadingHTTPServer(("127.0.0.1", port), Handler).serve_forever()
"#;

/// Starts [`SPARSE_CARGO_REGISTRY`] on `port`; `registry_dir` holds its crates and state across restarts.
pub fn start_sparse_cargo_registry(
    external: &Path,
    name: &str,
    port: u16,
    registry_dir: &Path,
    refuse_after: u32,
    index_delay: Duration,
) -> ServerGuard {
    fs::create_dir_all(registry_dir).unwrap();
    let script = external.join(format!("sparse-cargo-registry-{name}.py"));
    fs::write(&script, SPARSE_CARGO_REGISTRY).unwrap();
    let mut command = Command::new("python3");
    command
        .arg(&script)
        .arg(registry_dir)
        .arg(port.to_string())
        .arg(refuse_after.to_string())
        .arg(index_delay.as_secs_f64().to_string());
    let log_path = log_to_file(&mut command, external, &format!("sparse-cargo-registry-{name}.log"));
    let child = command.spawn().expect("python3 must be spawnable");
    let mut guard = ServerGuard { child, log_path };
    wait_for_port(&mut guard, port, Duration::from_secs(20));
    guard
}

/// Starts a real local `cargo-http-registry` on an ephemeral loopback port,
/// serving `registry_root` as its git-protocol index and package store; no
/// authentication is enforced (any registry token is accepted).
pub fn start_cargo_http_registry(external: &Path, registry_root: &Path) -> (ServerGuard, u16) {
    fs::create_dir_all(registry_root).unwrap();
    let port = free_loopback_port();
    let mut command = Command::new("cargo-http-registry");
    command.args(["-a", &format!("127.0.0.1:{port}"), registry_root.to_str().unwrap()]);
    let log_path = log_to_file(&mut command, external, "cargo-http-registry.log");
    let child = command.spawn().expect("cargo-http-registry must be spawnable");
    let mut guard = ServerGuard { child, log_path };
    wait_for_port(&mut guard, port, Duration::from_secs(20));
    (guard, port)
}

/// A minimal npm-workspace release fixture: a private workspace root plus one
/// publishable member, `publish-to = ["npm"]` only -- no `github-release`, so
/// `release execute` never touches `gh` (confirmed via `release plan`'s
/// operation list: exactly a registry-publish and a tag operation, nothing
/// forge-shaped). `.npmrc` is gitignored from the very first commit, because
/// it's written after the release commit is sealed and `release execute`'s
/// clean-worktree gate (E051) would otherwise reject the untracked file.
pub fn npm_registry_release_fixture(package_name: &str) -> (tempfile::TempDir, String) {
    npm_release_fixture(&[(package_name, &[])], "")
}

/// The same fixture for several npm packages, each with the packages it depends on, and `extra_config` appended to
/// `callisto.toml`. Every package gets a minor bump.
pub fn npm_release_fixture(packages: &[(&str, &[&str])], extra_config: &str) -> (tempfile::TempDir, String) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    callisto_fixtures::git::init_repo(root);
    git(root, &["remote", "add", "origin", NPM_ORIGIN]);

    fs::write(root.join(".gitignore"), ".npmrc\n").unwrap();
    fs::write(
        root.join("package.json"),
        "{\n  \"name\": \"npm-registry-fixture-root\",\n  \"version\": \"0.0.0\",\n  \"private\": true,\n  \"workspaces\": [\"packages/*\"]\n}\n",
    )
    .unwrap();
    for (name, dependencies) in packages {
        let directory = root.join("packages").join(name.replace('/', "-"));
        fs::create_dir_all(&directory).unwrap();
        let dependencies = dependencies
            .iter()
            .map(|dependency| format!("\"{dependency}\": \"^0.1.0\""))
            .collect::<Vec<_>>()
            .join(", ");
        fs::write(
            directory.join("package.json"),
            format!(
                "{{\n  \"name\": \"{name}\",\n  \"version\": \"0.1.0\",\n  \"dependencies\": {{{dependencies}}}\n}}\n"
            ),
        )
        .unwrap();
    }
    git(root, &["add", "."]);
    git(root, &["commit", "-m", "initial workspace"]);

    let init = callisto(root, &["init", "--yes", "--versioning", "independent"]);
    assert!(init.status.success(), "init failed: {}", stderr_of(&init));
    let config_path = root.join("callisto.toml");
    let mut config = fs::read_to_string(&config_path).unwrap();
    for (name, _) in packages {
        config.push_str(&format!(
            "\n[[package]]\nmatch = \"npm/{name}\"\npublish-to = [\"npm\"]\n"
        ));
    }
    config.push_str(extra_config);
    fs::write(&config_path, config).unwrap();
    git(root, &["add", "."]);
    git(root, &["commit", "-m", "configure callisto"]);

    let mut add_args = vec!["add".to_owned()];
    for (name, _) in packages {
        add_args.push("--package".to_owned());
        add_args.push(format!("npm/{name}:minor"));
    }
    add_args.push("--summary".to_owned());
    add_args.push("Ship real-registry e2e coverage".to_owned());
    let add_args: Vec<&str> = add_args.iter().map(String::as_str).collect();
    let add = callisto(root, &add_args);
    assert!(add.status.success(), "add failed: {}", stderr_of(&add));
    git(root, &["add", "."]);
    git(root, &["commit", "-m", "add release changeset"]);

    let version = callisto(root, &["version", "--emit-decision", DECISION_PATH]);
    assert!(version.status.success(), "version failed: {}", stderr_of(&version));

    for entry in fs::read_dir(root.join(".changeset")).unwrap() {
        let path = entry.unwrap().path();
        if path.file_name().is_some_and(|name| name != "README.md")
            && path.extension().is_some_and(|extension| extension == "md")
        {
            fs::remove_file(path).unwrap();
        }
    }
    git(root, &["add", "-A"]);
    git(root, &["commit", "-m", "release npm packages 0.2.0"]);
    let release_commit = git(root, &["rev-parse", "HEAD"]);
    git(root, &["checkout", "--detach", &release_commit]);
    (dir, release_commit)
}
