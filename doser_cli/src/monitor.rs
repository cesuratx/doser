//! Live weight monitor: a small blocking HTTP server that streams the current
//! scale reading to a self-contained web page, for development and testing.
//!
//! Architecture (no async runtime):
//! - A background reader thread samples the `Scale` at the configured rate and
//!   publishes the latest raw value into shared atomics.
//! - The main thread runs a `tiny_http` server. The page polls `/reading` ~10×/s
//!   for JSON and can `POST /tare` / `/tare/clear`.
//!
//! The page is embedded (no CDN) so it works on an offline Pi. When a
//! calibration is present the UI shows grams; otherwise it shows raw counts,
//! which is the useful view before the scale is calibrated.
//!
//! The server is unauthenticated and binds the LAN by default, so the two
//! state-changing endpoints are gated on a custom request header (see
//! `CSRF_HEADER`) plus a LAN check on `Host`; reads stay open.

// Display-oriented numeric casts (counts/intervals → f64/u64 for the UI). These
// are intentional and lossy-by-design; keep them out of the crate's pedantic gate.
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss,
    clippy::cast_possible_wrap
)]

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use doser_config::Calibration;
use doser_traits::Scale;
use tiny_http::{Header, Method, Request, Response, Server};

/// State shared between the sampling thread and the HTTP handlers.
struct Shared {
    /// Last successful raw reading (ADC counts), valid only once `has_reading`.
    latest_raw: AtomicI64,
    has_reading: AtomicBool,
    /// Tare offset in raw counts and whether the user has set one.
    tare_raw: AtomicI64,
    tare_set: AtomicBool,
    /// Monotonic sample counter, so the page can detect a stalled stream.
    seq: AtomicU64,
    /// EMA of the inter-sample interval in microseconds (0 until known).
    interval_us: AtomicU64,
    /// Last read error message, if the most recent read failed.
    last_err: std::sync::Mutex<Option<String>>,
    /// Optional calibration; when present the UI can show grams.
    calib: Option<Calibration>,
}

impl Shared {
    const fn new(calib: Option<Calibration>) -> Self {
        Self {
            latest_raw: AtomicI64::new(0),
            has_reading: AtomicBool::new(false),
            tare_raw: AtomicI64::new(0),
            tare_set: AtomicBool::new(false),
            seq: AtomicU64::new(0),
            interval_us: AtomicU64::new(0),
            last_err: std::sync::Mutex::new(None),
            calib,
        }
    }

    fn record_ok(&self, raw: i32) {
        // Publication pattern: store the payload (`latest_raw`) first, then the
        // flags that advertise it (`has_reading`, `seq`) with Release. The
        // handler threads load those flags with Acquire, which is what makes the
        // payload store visible to them; Relaxed alone gives no inter-variable
        // ordering and the Pi's ARM cores are weakly ordered, so a handler could
        // otherwise see `has_reading = true` while still reading `raw = 0`.
        self.latest_raw.store(i64::from(raw), Ordering::Relaxed);
        self.has_reading.store(true, Ordering::Release);
        self.seq.fetch_add(1, Ordering::Release);
        if let Ok(mut g) = self.last_err.lock() {
            *g = None;
        }
    }

    fn record_err(&self, msg: String) {
        if let Ok(mut g) = self.last_err.lock() {
            *g = Some(msg);
        }
    }

    /// Capture the current raw value as the zero. Returns `false` (and changes
    /// nothing) when no sample has landed yet: `latest_raw` is still 0 then, so
    /// the captured zero would be fiction — exactly the case where an operator
    /// with a wiring fault keeps hitting Tare.
    fn set_tare(&self) -> bool {
        // Acquire pairs with the Release store in `record_ok`: seeing the flag
        // guarantees the `latest_raw` we read below is the published sample.
        if !self.has_reading.load(Ordering::Acquire) {
            return false;
        }
        self.tare_raw
            .store(self.latest_raw.load(Ordering::Relaxed), Ordering::Relaxed);
        // Same publication pattern: payload first, then the flag with Release.
        self.tare_set.store(true, Ordering::Release);
        true
    }

    fn clear_tare(&self) {
        self.tare_set.store(false, Ordering::Release);
    }

    /// Build the JSON body for `/reading`.
    //
    // `suboptimal_flops` allowed: `mul_add` is a fused op and would round differently
    // from `doser_core`'s calibration path, so the number shown in the monitor UI would
    // stop matching the number the control loop is working from.
    #[allow(clippy::suboptimal_flops)]
    fn reading_json(&self) -> String {
        // Acquire loads pair with the Release stores in `record_ok`/`set_tare`;
        // each flag is loaded *before* the payload it advertises, so we never
        // publish a fresh `seq` (the page's staleness detector) or `ok: true`
        // alongside a stale `raw`.
        let has = self.has_reading.load(Ordering::Acquire);
        let seq = self.seq.load(Ordering::Acquire);
        let raw = self.latest_raw.load(Ordering::Relaxed);
        let tare_set = self.tare_set.load(Ordering::Acquire);
        let tare = self.tare_raw.load(Ordering::Relaxed);
        // Not a publication pair: a single independent value, so Relaxed is fine.
        let interval_us = self.interval_us.load(Ordering::Relaxed);
        let sps = if interval_us > 0 {
            (1_000_000.0 / interval_us as f64 * 10.0).round() / 10.0
        } else {
            0.0
        };
        let err = self.last_err.lock().ok().and_then(|g| g.clone());

        // grams (gross) and tared grams (net), only meaningful with calibration.
        // grams = scale_factor * (raw - offset) + offset_g; the tared figure is
        // relative to the captured zero so offset/offset_g cancel out.
        //
        // `mul_add` is deliberately not used: it is a fused op and would round
        // differently from `doser_core`'s calibration path, so the number shown in
        // the monitor UI would not match the number the control loop is using.
        let grams = self.calib.as_ref().map(|c| {
            f64::from(c.scale_factor) * (raw - i64::from(c.offset)) as f64 + f64::from(c.offset_g)
        });
        let tared_raw = if tare_set { Some(raw - tare) } else { None };
        let tared_grams = if tare_set {
            self.calib
                .as_ref()
                .map(|c| f64::from(c.scale_factor) * (raw - tare) as f64)
        } else {
            None
        };

        let obj = serde_json::json!({
            "ok": has,
            "raw": if has { serde_json::json!(raw) } else { serde_json::Value::Null },
            "grams": if has { serde_json::json!(grams) } else { serde_json::Value::Null },
            "tare_set": tare_set,
            "tared_raw": if has { serde_json::json!(tared_raw) } else { serde_json::Value::Null },
            "tared_grams": if has { serde_json::json!(tared_grams) } else { serde_json::Value::Null },
            "calibrated": self.calib.is_some(),
            "sps": sps,
            "seq": seq,
            "err": err,
        });
        obj.to_string()
    }
}

/// Run the live monitor server until `shutdown` is set (e.g. via Ctrl-C).
///
/// Generic over the concrete `Scale` so it works with both the real hardware
/// backend and the simulation backend.
pub fn run<S>(
    mut scale: S,
    calib: Option<Calibration>,
    sample_hz: u32,
    read_timeout: Duration,
    bind: &str,
    port: u16,
    shutdown: &Arc<AtomicBool>,
) -> eyre::Result<()>
where
    S: Scale + Send + 'static,
{
    let calibrated = calib.is_some();
    let shared = Arc::new(Shared::new(calib));

    // Background sampling thread. `Scale::read` blocks until data-ready, which
    // paces hardware naturally; we add a sleep so fast backends (sim) don't spin.
    let reader_shared = Arc::clone(&shared);
    let reader_shutdown = Arc::clone(shutdown);
    let target_interval = Duration::from_secs_f64(1.0 / f64::from(sample_hz.max(1)));
    let reader = std::thread::Builder::new()
        .name("weight-reader".into())
        .spawn(move || {
            let mut prev: Option<Instant> = None;
            while !reader_shutdown.load(Ordering::Relaxed) {
                let started = Instant::now();
                match scale.read(read_timeout) {
                    Ok(raw) => {
                        let now = Instant::now();
                        if let Some(p) = prev {
                            let inst = now.duration_since(p).as_micros() as u64;
                            let old = reader_shared.interval_us.load(Ordering::Relaxed);
                            // EMA (alpha = 0.2) to smooth the SPS readout. The
                            // integer divides truncate at most ~5 µs low per
                            // step (0.002% at 10 SPS) and the error cannot
                            // accumulate — measured; leave the arithmetic alone.
                            let ema = if old == 0 {
                                inst
                            } else {
                                inst / 5 + old * 4 / 5
                            };
                            reader_shared.interval_us.store(ema, Ordering::Relaxed);
                        }
                        prev = Some(now);
                        reader_shared.record_ok(raw);
                    }
                    Err(e) => reader_shared.record_err(e.to_string()),
                }
                if let Some(rem) = target_interval.checked_sub(started.elapsed()) {
                    std::thread::sleep(rem);
                }
            }
        })
        .map_err(|e| eyre::eyre!("spawn reader thread: {e}"))?;

    let addr = format!("{bind}:{port}");
    let server =
        Server::http(addr.as_str()).map_err(|e| eyre::eyre!("bind HTTP server on {addr}: {e}"))?;

    println!("weight monitor: serving on http://{addr}");
    if bind == "0.0.0.0" {
        println!("  open http://<this-pi-ip>:{port} from another machine on the LAN");
    }
    if !calibrated {
        println!(
            "  (uncalibrated: showing raw counts — run the scale-calibration skill for grams)"
        );
    }
    println!("  press Ctrl-C to stop");

    // Request loop. `recv_timeout` lets us notice the shutdown flag promptly.
    while !shutdown.load(Ordering::Relaxed) {
        match server.recv_timeout(Duration::from_millis(200)) {
            Ok(Some(req)) => handle_request(req, &shared),
            Ok(None) => {} // timed out; re-check shutdown
            Err(e) => {
                tracing::warn!(error = %e, "http recv error");
                break;
            }
        }
    }

    shutdown.store(true, Ordering::Relaxed);
    let _ = reader.join();
    Ok(())
}

/// Header the page attaches to every state-changing request.
///
/// The server has no auth and answers on the LAN, so without this any page the
/// operator's browser happens to load could `fetch('http://<pi>:8080/tare',
/// {method:'POST', mode:'no-cors'})` and zero the scale mid-dose: that is a CORS
/// "simple request", so it is sent without a preflight and the state change
/// lands even though the attacker cannot read the reply. A custom header makes
/// the request non-simple, so the browser must preflight it — and our responses
/// carry no CORS headers, so the preflight fails and the POST never happens.
/// Same-origin requests from the monitor page itself are unaffected.
const CSRF_HEADER: &str = "X-Doser-Monitor";

fn handle_request(req: Request, shared: &Shared) {
    let method = req.method().clone();
    // `url()` includes any query string; we only use fixed paths so trim it.
    let path = req.url().split('?').next().unwrap_or("/").to_string();

    match (&method, path.as_str()) {
        (Method::Get, "/") => respond(req, INDEX_HTML.to_string(), "text/html; charset=utf-8"),
        (Method::Get, "/reading") => respond(req, shared.reading_json(), "application/json"),
        (Method::Post, "/tare") => {
            if let Some((status, body)) = reject_state_change(&req) {
                return respond_status(req, status, body);
            }
            if shared.set_tare() {
                respond(req, r#"{"ok":true}"#.to_string(), "application/json");
            } else {
                // 409: the request was fine, the scale just has nothing to zero.
                respond_status(req, 409, err_json("no reading yet — check the wiring"));
            }
        }
        (Method::Post, "/tare/clear") => {
            if let Some((status, body)) = reject_state_change(&req) {
                return respond_status(req, status, body);
            }
            shared.clear_tare();
            respond(req, r#"{"ok":true}"#.to_string(), "application/json");
        }
        _ => {
            let resp = Response::from_string("not found").with_status_code(404);
            let _ = req.respond(resp);
        }
    }
}

/// Gate for the state-changing endpoints; `Some((status, json body))` refuses.
fn reject_state_change(req: &Request) -> Option<(u16, String)> {
    let headers = req.headers();
    if !has_csrf_header(headers) {
        return Some((403, err_json("missing X-Doser-Monitor header")));
    }
    if !host_header_is_lan(headers) {
        return Some((403, err_json("Host is not a LAN address")));
    }
    None
}

fn has_csrf_header(headers: &[Header]) -> bool {
    // Presence is what matters — a caller who can set the header can set any
    // value, so checking the value would buy nothing and only break curl users.
    headers.iter().any(|h| h.field.equiv(CSRF_HEADER))
}

/// Second line of defence: DNS rebinding would make an attacker page same-origin
/// with the Pi (so the custom header rides along freely), but the `Host` it
/// sends is still the attacker's registrable name. A missing `Host` is allowed —
/// browsers always send one, so only non-browser clients land here.
fn host_header_is_lan(headers: &[Header]) -> bool {
    headers
        .iter()
        .find(|h| h.field.equiv("Host"))
        .is_none_or(|h| host_is_lan(h.value.as_str()))
}

/// Is this `Host` value a way of naming a machine on the local network?
///
/// Accepts private/loopback/link-local/CGNAT IP literals plus single-label names
/// (`doser`, `raspberrypi`, `localhost`) and the LAN-only DNS suffixes. Those
/// cover access by IP, by mDNS and by router-assigned name; a rebinding attacker
/// needs a registrable public name, which always has a dot and none of these.
// `case_sensitive_file_extension_comparisons` is a false positive here: these are DNS
// suffixes, not filenames, and `name` was lowercased above so the match is already
// case-insensitive.
#[allow(clippy::case_sensitive_file_extension_comparisons)]
fn host_is_lan(host: &str) -> bool {
    let host = host.trim();
    // Strip the port. IPv6 literals must be bracketed when a port is present;
    // tolerate a bare unbracketed one (several colons, no port) as well.
    let name = if let Some(rest) = host.strip_prefix('[') {
        match rest.split_once(']') {
            Some((h, _)) => h,
            None => return false,
        }
    } else if host.matches(':').count() > 1 {
        host
    } else {
        host.rsplit_once(':').map_or(host, |(h, _)| h)
    };

    if let Ok(ip) = name.parse::<std::net::IpAddr>() {
        return ip_is_lan(&ip);
    }
    // Drop the root label of a fully qualified name ("doser.local." == "doser.local").
    let name = name.trim_end_matches('.').to_ascii_lowercase();
    !name.contains('.')
        || name.ends_with(".local")
        || name.ends_with(".lan")
        || name.ends_with(".internal")
        || name.ends_with(".home.arpa")
}

fn ip_is_lan(ip: &std::net::IpAddr) -> bool {
    match ip {
        std::net::IpAddr::V4(v4) => {
            let o = v4.octets();
            v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                // 100.64.0.0/10 (CGNAT) — Tailscale and similar overlays.
                || (o[0] == 100 && (64..128).contains(&o[1]))
        }
        std::net::IpAddr::V6(v6) => {
            let head = v6.segments()[0];
            v6.is_loopback()
                || (head & 0xfe00) == 0xfc00 // unique local, fc00::/7
                || (head & 0xffc0) == 0xfe80 // link local, fe80::/10
                || v6
                    .to_ipv4_mapped()
                    .is_some_and(|m| ip_is_lan(&std::net::IpAddr::V4(m)))
        }
    }
}

fn err_json(msg: &str) -> String {
    serde_json::json!({ "ok": false, "error": msg }).to_string()
}

fn respond(req: Request, body: String, content_type: &str) {
    let mut resp = Response::from_string(body);
    if let Ok(h) = Header::from_bytes(&b"Content-Type"[..], content_type.as_bytes()) {
        resp.add_header(h);
    }
    let _ = req.respond(resp);
}

fn respond_status(req: Request, status: u16, body: String) {
    let mut resp = Response::from_string(body).with_status_code(status);
    if let Ok(h) = Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..]) {
        resp.add_header(h);
    }
    let _ = req.respond(resp);
}

/// Self-contained monitor page: live number, raw/Δ/SPS stats, tare controls and
/// a scrolling chart. Polls `/reading` and draws with plain canvas (no CDN).
const INDEX_HTML: &str = r##"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>doser — live weight</title>
<style>
  :root { color-scheme: dark; }
  * { box-sizing: border-box; }
  body {
    margin: 0; font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
    background: #0b0f14; color: #e6edf3; display: flex; flex-direction: column;
    min-height: 100vh; padding: 24px; gap: 20px;
  }
  header { display: flex; justify-content: space-between; align-items: baseline; }
  h1 { font-size: 15px; font-weight: 600; color: #7d8590; margin: 0; letter-spacing: .04em; }
  #conn { font-size: 13px; color: #3fb950; }
  #conn.bad { color: #f85149; }
  .value-wrap { text-align: center; margin: 8px 0; }
  #value { font-size: clamp(56px, 16vw, 140px); font-weight: 700; line-height: 1;
           font-variant-numeric: tabular-nums; }
  #unit { font-size: 28px; color: #7d8590; margin-left: 6px; }
  #vlabel { font-size: 14px; color: #7d8590; text-transform: uppercase; letter-spacing: .1em; }
  .stats { display: flex; gap: 28px; justify-content: center; flex-wrap: wrap;
           font-size: 15px; color: #adbac7; }
  .stats b { color: #e6edf3; font-weight: 600; font-variant-numeric: tabular-nums; }
  .controls { display: flex; gap: 12px; justify-content: center; }
  button { font: inherit; font-size: 15px; padding: 10px 22px; border-radius: 8px;
           border: 1px solid #30363d; background: #21262d; color: #e6edf3; cursor: pointer; }
  button:hover { background: #30363d; }
  button.primary { background: #1f6feb; border-color: #1f6feb; }
  button.primary:hover { background: #388bfd; }
  #chart { width: 100%; flex: 1; min-height: 220px; background: #0d1117;
           border: 1px solid #21262d; border-radius: 10px; }
  #note { text-align: center; font-size: 13px; color: #d29922; min-height: 18px; }
  #err { text-align: center; font-size: 13px; color: #f85149; min-height: 18px; }
</style>
</head>
<body>
  <header>
    <h1>DOSER · LIVE WEIGHT</h1>
    <span id="conn">connecting…</span>
  </header>

  <div class="value-wrap">
    <div><span id="value">--</span><span id="unit"></span></div>
    <div id="vlabel">reading</div>
  </div>

  <div class="stats">
    <div>raw <b id="raw">--</b></div>
    <div>Δ tare <b id="delta">--</b></div>
    <div>rate <b id="sps">--</b> SPS</div>
  </div>

  <div class="controls">
    <button class="primary" id="tare">Tare / Zero</button>
    <button id="cleartare">Clear tare</button>
  </div>

  <div id="note"></div>
  <div id="err"></div>
  <canvas id="chart"></canvas>

<script>
const MAX_POINTS = 400;
const POLL_MS = 100;
const STALE_TICKS = 20;
const NOTICE_MS = 5000;
const history = [];
let lastSeq = -1, staleTicks = 0, inFlight = false;
// A refused/failed command message, held for a few seconds so the next poll's
// render() does not wipe it before the operator has read it.
let notice = "", noticeUntil = 0;

const $ = (id) => document.getElementById(id);

function fmtRaw(v) { return v == null ? "--" : Math.round(v).toLocaleString(); }

function setNotice(msg) {
  notice = msg; noticeUntil = Date.now() + NOTICE_MS;
  $("err").textContent = msg;
}

function render(d) {
  // Choose the primary value: grams when calibrated, else raw counts; use the
  // tared (net) figure when the user has zeroed.
  let val, unit, label;
  if (d.calibrated) {
    unit = "g";
    if (d.tare_set) { val = d.tared_grams; label = "net weight"; }
    else { val = d.grams; label = "gross weight"; }
  } else {
    unit = "";
    if (d.tare_set) { val = d.tared_raw; label = "Δ raw (tared)"; }
    else { val = d.raw; label = "raw counts"; }
  }

  $("vlabel").textContent = label;
  $("unit").textContent = unit;
  $("raw").textContent = fmtRaw(d.raw);
  $("delta").textContent = d.tare_set ? fmtRaw(d.tared_raw) : "—";
  $("sps").textContent = (d.sps != null) ? d.sps.toFixed(1) : "--";
  if (notice && Date.now() < noticeUntil) {
    $("err").textContent = notice;
  } else {
    notice = "";
    $("err").textContent = d.err ? ("read error: " + d.err) : "";
  }
  $("note").textContent = d.calibrated ? "" :
    "Uncalibrated — showing raw ADC counts. Run the scale-calibration skill to display grams.";

  if (d.ok && val != null) {
    $("value").textContent = d.calibrated ? val.toFixed(2) : fmtRaw(val);
    history.push(val);
    if (history.length > MAX_POINTS) history.shift();
    draw();
  } else {
    $("value").textContent = "--";
  }
}

function draw() {
  const c = $("chart");
  const dpr = window.devicePixelRatio || 1;
  const W = c.width = Math.floor(c.clientWidth * dpr);
  const H = c.height = Math.floor(c.clientHeight * dpr);
  const ctx = c.getContext("2d");
  ctx.clearRect(0, 0, W, H);
  if (history.length < 2) return;

  let min = Math.min(...history), max = Math.max(...history);
  if (min === max) { min -= 1; max += 1; }
  const pad = (max - min) * 0.12; min -= pad; max += pad;
  const n = history.length;
  const x = (i) => (i / (n - 1)) * W;
  const y = (v) => H - ((v - min) / (max - min)) * H;

  // baseline at the tare/zero level if it is within view
  if (min <= 0 && max >= 0) {
    ctx.strokeStyle = "#30363d"; ctx.lineWidth = dpr;
    ctx.beginPath(); ctx.moveTo(0, y(0)); ctx.lineTo(W, y(0)); ctx.stroke();
  }

  ctx.strokeStyle = "#4ade80"; ctx.lineWidth = 2 * dpr; ctx.lineJoin = "round";
  ctx.beginPath();
  for (let i = 0; i < n; i++) {
    const px = x(i), py = y(history[i]);
    if (i === 0) ctx.moveTo(px, py); else ctx.lineTo(px, py);
  }
  ctx.stroke();
}

function setConn(text, bad) {
  $("conn").textContent = text; $("conn").className = bad ? "bad" : "";
}

async function poll() {
  // In-flight guard: a stalled Pi (or the single-threaded server) must not let
  // fetches pile up — the browser caps ~6 per host and each one is a connection
  // and a thread server-side, and on recovery they all resolve at once.
  if (inFlight) return;
  inFlight = true;
  try {
    const r = await fetch("/reading", { cache: "no-store" });
    const d = await r.json();
    const seq = (typeof d.seq === "number") ? d.seq : -1;
    // Out-of-order arrival: a response older than what we already drew would
    // walk the value and the staleness detector backwards. Drop it.
    if (seq < lastSeq) return;
    if (seq === lastSeq) { staleTicks++; } else { lastSeq = seq; staleTicks = 0; }
    setConn(staleTicks > STALE_TICKS ? "no new samples" : "live", staleTicks > STALE_TICKS);
    render(d);
  } catch (e) {
    setConn("disconnected", true);
  } finally {
    inFlight = false;
  }
}

// Self-scheduling loop: the next poll starts only after this one settles.
function pollLoop() { poll().catch(() => {}).finally(() => setTimeout(pollLoop, POLL_MS)); }

async function command(path, what) {
  try {
    // The custom header is what stops any other site the browser has open from
    // POSTing here; the server refuses the request without it.
    const r = await fetch(path, { method: "POST", headers: { "X-Doser-Monitor": "1" } });
    const d = await r.json().catch(() => ({}));
    if (!r.ok) {
      setNotice(what + " refused: " + (d.error || ("HTTP " + r.status)));
      return;
    }
    notice = ""; $("err").textContent = "";
    poll();
  } catch (e) {
    setNotice(what + " failed: monitor unreachable");
  }
}

$("tare").addEventListener("click", () => command("/tare", "tare"));
$("cleartare").addEventListener("click", () => command("/tare/clear", "clear tare"));
window.addEventListener("resize", draw);
pollLoop();
</script>
</body>
</html>
"##;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    fn calib() -> Calibration {
        // grams = 0.001 * (raw - 1000) + 0.5
        Calibration {
            offset: 1000,
            scale_factor: 0.001,
            offset_g: 0.5,
        }
    }

    fn parse(s: &Shared) -> Value {
        serde_json::from_str(&s.reading_json()).expect("reading_json emits valid JSON")
    }

    fn hdr(field: &str, value: &str) -> Header {
        Header::from_bytes(field.as_bytes(), value.as_bytes()).expect("valid header")
    }

    #[test]
    fn reading_json_before_first_sample_is_all_null() {
        let s = Shared::new(None);
        let v = parse(&s);
        assert_eq!(v["ok"], Value::Bool(false));
        assert!(v["raw"].is_null());
        assert!(v["grams"].is_null());
        assert!(v["tared_raw"].is_null());
        assert!(v["tared_grams"].is_null());
        assert_eq!(v["calibrated"], Value::Bool(false));
        assert_eq!(v["tare_set"], Value::Bool(false));
        assert_eq!(v["seq"], 0);
        assert_eq!(v["sps"], 0.0);
        assert!(v["err"].is_null());
    }

    #[test]
    fn reading_json_uncalibrated_reports_raw_only() {
        let s = Shared::new(None);
        s.record_ok(12_345);
        let v = parse(&s);
        assert_eq!(v["ok"], Value::Bool(true));
        assert_eq!(v["raw"], 12_345);
        assert!(v["grams"].is_null(), "no calibration means no grams");
        assert_eq!(v["calibrated"], Value::Bool(false));
        assert_eq!(v["seq"], 1);
    }

    #[test]
    fn reading_json_applies_calibration_to_gross_grams() {
        let s = Shared::new(Some(calib()));
        s.record_ok(3_000);
        let v = parse(&s);
        assert_eq!(v["calibrated"], Value::Bool(true));
        // 0.001 * (3000 - 1000) + 0.5; tolerance covers the f32→f64 widening of
        // the calibration constants.
        let grams = v["grams"].as_f64().expect("grams present");
        assert!((grams - 2.5).abs() < 1e-6, "grams was {grams}");
        assert!(v["tared_grams"].is_null(), "no tare set");
    }

    #[test]
    fn record_err_surfaces_and_clears() {
        let s = Shared::new(None);
        s.record_err("timeout waiting for DOUT".to_string());
        assert_eq!(parse(&s)["err"], "timeout waiting for DOUT");
        s.record_ok(7);
        assert!(parse(&s)["err"].is_null(), "a good sample clears the error");
    }

    #[test]
    fn tare_is_refused_before_the_first_reading() {
        let s = Shared::new(Some(calib()));
        assert!(!s.set_tare(), "nothing to zero yet");
        let v = parse(&s);
        assert_eq!(v["tare_set"], Value::Bool(false));
        assert!(v["tared_raw"].is_null());
    }

    #[test]
    fn tare_nets_out_the_captured_zero() {
        let s = Shared::new(Some(calib()));
        s.record_ok(2_000);
        assert!(s.set_tare());
        s.record_ok(2_500);
        let v = parse(&s);
        assert_eq!(v["tare_set"], Value::Bool(true));
        assert_eq!(v["tared_raw"], 500);
        // net grams are relative to the captured zero: offset/offset_g cancel.
        let net = v["tared_grams"].as_f64().expect("tared_grams present");
        assert!((net - 0.5).abs() < 1e-6, "tared_grams was {net}");
        // gross still carries the calibration offsets: 0.001 * (2500 - 1000) + 0.5
        let gross = v["grams"].as_f64().expect("grams present");
        assert!((gross - 2.0).abs() < 1e-6, "grams was {gross}");
    }

    #[test]
    fn clear_tare_drops_the_net_figures() {
        let s = Shared::new(Some(calib()));
        s.record_ok(2_000);
        assert!(s.set_tare());
        s.clear_tare();
        let v = parse(&s);
        assert_eq!(v["tare_set"], Value::Bool(false));
        assert!(v["tared_raw"].is_null());
        assert!(v["tared_grams"].is_null());
    }

    #[test]
    fn tare_without_calibration_reports_raw_delta_only() {
        let s = Shared::new(None);
        s.record_ok(100);
        assert!(s.set_tare());
        s.record_ok(140);
        let v = parse(&s);
        assert_eq!(v["tared_raw"], 40);
        assert!(v["tared_grams"].is_null());
    }

    #[test]
    fn seq_advances_once_per_successful_sample() {
        let s = Shared::new(None);
        for _ in 0..3 {
            s.record_ok(1);
        }
        assert_eq!(parse(&s)["seq"], 3);
        s.record_err("bad".to_string());
        assert_eq!(parse(&s)["seq"], 3, "failed reads do not advance seq");
    }

    #[test]
    fn csrf_header_is_matched_case_insensitively() {
        assert!(has_csrf_header(&[hdr("x-doser-monitor", "1")]));
        assert!(has_csrf_header(&[
            hdr("Host", "doser.local"),
            hdr(CSRF_HEADER, "anything")
        ]));
        assert!(!has_csrf_header(&[hdr("Host", "doser.local")]));
        assert!(!has_csrf_header(&[]));
    }

    #[test]
    fn lan_hosts_are_accepted() {
        for h in [
            "192.168.1.42:8080",
            "10.0.0.7:8080",
            "172.16.3.1",
            "127.0.0.1:8080",
            "169.254.4.5:8080",
            "100.101.102.103:8080", // tailscale CGNAT
            "[::1]:8080",
            "[fe80::1]:8080",
            "[fd00::abcd]:8080",
            "localhost:8080",
            "doser",
            "raspberrypi:8080",
            "doser.local:8080",
            "doser.local.:8080", // fully qualified form
            "pi.lan",
            "doser.home.arpa:8080",
        ] {
            assert!(host_is_lan(h), "{h} should be treated as LAN");
        }
    }

    #[test]
    fn public_hosts_are_rejected() {
        for h in [
            "evil.example.com:8080",
            "rebind.attacker.net",
            "8.8.8.8:8080",
            "172.32.0.1:8080", // just outside 172.16/12
            "[2001:4860:4860::8888]:8080",
            "[bad",
        ] {
            assert!(!host_is_lan(h), "{h} should not be treated as LAN");
        }
    }

    #[test]
    fn err_json_is_escaped_json() {
        let v: Value = serde_json::from_str(&err_json("bad \"quote\"")).expect("valid JSON");
        assert_eq!(v["ok"], Value::Bool(false));
        assert_eq!(v["error"], "bad \"quote\"");
    }
}
