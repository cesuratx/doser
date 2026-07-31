//! End-to-end test of the `monitor` subcommand's HTTP surface.
//!
//! The monitor is a real server bound to a real port, so this drives it as a
//! client would: spawn the binary, wait for the socket, then speak HTTP/1.1 over
//! a plain `TcpStream`. Using raw sockets (rather than an HTTP client crate)
//! keeps the dev-dependency set unchanged and lets each request set exactly the
//! headers under test — the CSRF header and `Host` gating are the point.
//!
//! Every spawn is wrapped in a guard whose `Drop` kills and reaps the child, so
//! a failing assertion unwinds without leaking a server holding a port.

mod common;

use common::{doser, write_valid_config};
use rstest::rstest;
use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::process::{Child, Stdio};
use std::time::{Duration, Instant};

/// Generous by design: these bounds only decide how long a *broken* server is
/// waited on, and the suite must not flake on a loaded machine.
const STARTUP_TIMEOUT: Duration = Duration::from_secs(30);
const IO_TIMEOUT: Duration = Duration::from_secs(15);

/// Kills and reaps the monitor on the way out, including during a panic unwind.
struct ChildGuard(Child);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Reserve a free port by binding it, reading the assigned number, and dropping
/// the listener. There is an unavoidable race between the drop and the child's
/// bind; the alternative (a fixed port) races against every other test run on
/// the machine, which is worse.
fn free_port() -> u16 {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("reserve a port");
    listener.local_addr().expect("local_addr").port()
}

/// Spawn `monitor` on loopback and block until it answers `GET /reading`.
fn spawn_monitor(extra_env: &[(&str, &str)]) -> (ChildGuard, u16) {
    let dir = tempfile::tempdir().expect("tempdir");
    let cfg = write_valid_config(&dir);
    let port = free_port();

    let mut cmd = doser(&cfg);
    cmd.args([
        "monitor",
        "--bind",
        "127.0.0.1",
        "--port",
        &port.to_string(),
        "--hz",
        "50",
    ])
    // The child outlives the assertions; null the pipes so it can never block
    // on a full stdout buffer while we are waiting on it.
    .stdout(Stdio::null())
    .stderr(Stdio::null());
    for (k, v) in extra_env {
        cmd.env(k, v);
    }
    let mut child = ChildGuard(cmd.spawn().expect("spawn monitor"));
    // The temp config must outlive startup; the binary reads it before binding.
    wait_until_serving(&mut child, port);
    drop(dir);
    (child, port)
}

fn wait_until_serving(child: &mut ChildGuard, port: u16) {
    let deadline = Instant::now() + STARTUP_TIMEOUT;
    let mut last = String::new();
    while Instant::now() < deadline {
        match try_request(port, "GET", "/reading", &[], "127.0.0.1") {
            Ok(resp) if resp.status == 200 => return,
            Ok(resp) => last = format!("status {}", resp.status),
            Err(e) => last = e,
        }
        // Fail fast (with the exit status) if the server died instead of
        // binding — e.g. the reserved port was taken between reserve and bind.
        if let Ok(Some(status)) = child.0.try_wait() {
            panic!("monitor exited during startup with {status}; last attempt: {last}");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("monitor did not start within {STARTUP_TIMEOUT:?}; last attempt: {last}");
}

struct HttpResponse {
    status: u16,
    headers: String,
    body: String,
}

impl HttpResponse {
    fn json(&self) -> serde_json::Value {
        serde_json::from_str(&self.body)
            .unwrap_or_else(|e| panic!("body is not JSON ({e}): {:?}", self.body))
    }
}

/// Total byte length of a complete response (headers + `Content-Length` body),
/// or `None` while the headers are still incomplete or carry no length.
fn framed_len(raw: &[u8]) -> Option<usize> {
    let head_end = raw.windows(4).position(|w| w == b"\r\n\r\n")? + 4;
    let head = String::from_utf8_lossy(&raw[..head_end]);
    let len: usize = head.lines().find_map(|l| {
        let (k, v) = l.split_once(':')?;
        if k.trim().eq_ignore_ascii_case("content-length") {
            v.trim().parse::<usize>().ok()
        } else {
            None
        }
    })?;
    Some(head_end + len)
}

/// One request/response over a fresh connection. `Connection: close` asks the
/// server to hang up after responding; the reader below does not rely on it.
fn try_request(
    port: u16,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    host: &str,
) -> Result<HttpResponse, String> {
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    let mut stream =
        TcpStream::connect_timeout(&addr, IO_TIMEOUT).map_err(|e| format!("connect: {e}"))?;
    stream.set_read_timeout(Some(IO_TIMEOUT)).ok();
    stream.set_write_timeout(Some(IO_TIMEOUT)).ok();

    let mut req = format!("{method} {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n");
    for (k, v) in headers {
        req.push_str(&format!("{k}: {v}\r\n"));
    }
    if method == "POST" {
        req.push_str("Content-Length: 0\r\n");
    }
    req.push_str("\r\n");

    stream
        .write_all(req.as_bytes())
        .map_err(|e| format!("write: {e}"))?;
    stream.flush().ok();

    // Read until the framed message is complete rather than until EOF: that way
    // the test does not depend on the server actually honouring `Connection:
    // close`, and a server that keeps the socket open costs a header parse
    // instead of a 15-second read timeout.
    let mut raw: Vec<u8> = Vec::new();
    let mut buf = [0u8; 4096];
    loop {
        if let Some(len) = framed_len(&raw)
            && raw.len() >= len
        {
            break;
        }
        match stream.read(&mut buf) {
            Ok(0) => break, // EOF
            Ok(n) => raw.extend_from_slice(&buf[..n]),
            Err(e) => return Err(format!("read: {e}")),
        }
    }
    let text = String::from_utf8_lossy(&raw).into_owned();
    let (head, body) = text
        .split_once("\r\n\r\n")
        .ok_or_else(|| format!("no header/body split in response: {text:?}"))?;
    let status = head
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|s| s.parse::<u16>().ok())
        .ok_or_else(|| format!("no status code in response: {head:?}"))?;
    Ok(HttpResponse {
        status,
        headers: head.to_string(),
        body: body.to_string(),
    })
}

fn request(port: u16, method: &str, path: &str, headers: &[(&str, &str)]) -> HttpResponse {
    try_request(port, method, path, headers, "127.0.0.1")
        .unwrap_or_else(|e| panic!("{method} {path} failed: {e}"))
}

/// Poll `/reading` until the sampler has published a real sample.
fn wait_for_reading(port: u16) -> serde_json::Value {
    let deadline = Instant::now() + STARTUP_TIMEOUT;
    while Instant::now() < deadline {
        let v = request(port, "GET", "/reading", &[]).json();
        if v["ok"] == serde_json::Value::Bool(true) {
            return v;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("monitor never published a reading within {STARTUP_TIMEOUT:?}");
}

/// Poll `/reading` until the sampler has reported a failed read.
fn wait_for_read_error(port: u16) -> serde_json::Value {
    let deadline = Instant::now() + STARTUP_TIMEOUT;
    let mut last = serde_json::Value::Null;
    while Instant::now() < deadline {
        last = request(port, "GET", "/reading", &[]).json();
        if last["err"].is_string() {
            return last;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("monitor never surfaced a read error within {STARTUP_TIMEOUT:?}; last: {last}");
}

const CSRF: (&str, &str) = ("X-Doser-Monitor", "1");

/// The whole read/write surface against one live server. Kept as a single test
/// so the suite pays for one process spawn rather than six, and so the tare
/// assertions run in a known order against known state.
#[rstest]
fn monitor_http_surface() {
    let (_child, port) = spawn_monitor(&[]);

    // GET / serves the embedded page.
    let index = request(port, "GET", "/", &[]);
    assert_eq!(index.status, 200);
    assert!(
        index.headers.to_ascii_lowercase().contains("text/html"),
        "headers were: {:?}",
        index.headers
    );
    assert!(index.body.contains("<!doctype html>"));
    assert!(index.body.contains("DOSER · LIVE WEIGHT"));
    assert!(
        index.body.contains("X-Doser-Monitor"),
        "the page must send the header its own server requires"
    );

    // GET /reading serves the documented JSON keys.
    let reading = request(port, "GET", "/reading", &[]);
    assert_eq!(reading.status, 200);
    assert!(
        reading
            .headers
            .to_ascii_lowercase()
            .contains("application/json"),
        "headers were: {:?}",
        reading.headers
    );
    let v = reading.json();
    for key in [
        "ok",
        "raw",
        "grams",
        "tare_set",
        "tared_raw",
        "tared_grams",
        "calibrated",
        "sps",
        "seq",
        "err",
    ] {
        assert!(v.get(key).is_some(), "/reading is missing `{key}`: {v}");
    }
    assert_eq!(
        v["calibrated"],
        serde_json::Value::Bool(false),
        "the test config carries no calibration"
    );
    assert_eq!(v["tare_set"], serde_json::Value::Bool(false));

    // A query string must not change routing.
    assert_eq!(
        request(port, "GET", "/reading?cachebust=1", &[]).status,
        200
    );

    // POST without the custom header is refused: that header is what forces a
    // browser to preflight a cross-origin POST, and the preflight has no CORS
    // response to satisfy it.
    let no_header = request(port, "POST", "/tare", &[]);
    assert_eq!(no_header.status, 403);
    let body = no_header.json();
    assert_eq!(body["ok"], serde_json::Value::Bool(false));
    assert_eq!(body["error"], "missing X-Doser-Monitor header");

    // With the header but a public `Host`, the DNS-rebinding guard refuses it.
    let rebound = try_request(port, "POST", "/tare", &[CSRF], "evil.example.com")
        .expect("request to a public Host still gets a response");
    assert_eq!(rebound.status, 403);
    assert_eq!(rebound.json()["error"], "Host is not a LAN address");

    // /tare/clear is gated identically.
    assert_eq!(request(port, "POST", "/tare/clear", &[]).status, 403);

    // Once a sample has landed, the header-bearing POST succeeds.
    let before = wait_for_reading(port);
    let seq_before = before["seq"].as_u64().expect("seq is a number");

    let tared = request(port, "POST", "/tare", &[CSRF]);
    assert_eq!(tared.status, 200, "body: {:?}", tared.body);
    assert_eq!(tared.json()["ok"], serde_json::Value::Bool(true));

    let after = request(port, "GET", "/reading", &[]).json();
    assert_eq!(after["tare_set"], serde_json::Value::Bool(true));
    assert!(after["tared_raw"].is_number(), "tared_raw: {after}");
    assert!(
        after["seq"].as_u64().expect("seq is a number") >= seq_before,
        "seq must not go backwards"
    );

    let cleared = request(port, "POST", "/tare/clear", &[CSRF]);
    assert_eq!(cleared.status, 200);
    assert_eq!(cleared.json()["ok"], serde_json::Value::Bool(true));
    let after_clear = request(port, "GET", "/reading", &[]).json();
    assert_eq!(after_clear["tare_set"], serde_json::Value::Bool(false));
    assert!(after_clear["tared_raw"].is_null());

    // Unknown routes 404; so does the wrong method on a known route.
    assert_eq!(request(port, "GET", "/nope", &[]).status, 404);
    assert_eq!(request(port, "GET", "/tare", &[]).status, 404);
    assert_eq!(request(port, "POST", "/reading", &[CSRF]).status, 404);
}

/// Tare is refused with 409 while the scale has never produced a sample —
/// zeroing against a nonexistent reading would capture a fictional zero, which
/// is exactly the situation an operator with a wiring fault is in.
/// `DOSER_TEST_SIM_TIMEOUT` makes every read fail, so `has_reading` stays unset.
#[rstest]
fn tare_before_any_reading_is_refused_with_409() {
    let (_child, port) = spawn_monitor(&[("DOSER_TEST_SIM_TIMEOUT", "1")]);

    // The server answers as soon as it binds, which can be before the reader
    // thread's first (failing) read has landed — so poll for the error rather
    // than racing it.
    let reading = wait_for_read_error(port);
    assert_eq!(reading["ok"], serde_json::Value::Bool(false));
    assert!(reading["raw"].is_null());

    let resp = request(port, "POST", "/tare", &[CSRF]);
    assert_eq!(resp.status, 409, "body: {:?}", resp.body);
    let body = resp.json();
    assert_eq!(body["ok"], serde_json::Value::Bool(false));
    assert!(
        body["error"]
            .as_str()
            .is_some_and(|e| e.contains("no reading yet")),
        "error was {:?}",
        body["error"]
    );

    // The refusal changed nothing.
    assert_eq!(
        request(port, "GET", "/reading", &[]).json()["tare_set"],
        serde_json::Value::Bool(false)
    );
}
