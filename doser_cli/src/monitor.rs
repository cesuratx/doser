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
    fn new(calib: Option<Calibration>) -> Self {
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
        self.latest_raw.store(i64::from(raw), Ordering::Relaxed);
        self.has_reading.store(true, Ordering::Relaxed);
        self.seq.fetch_add(1, Ordering::Relaxed);
        if let Ok(mut g) = self.last_err.lock() {
            *g = None;
        }
    }

    fn record_err(&self, msg: String) {
        if let Ok(mut g) = self.last_err.lock() {
            *g = Some(msg);
        }
    }

    fn set_tare(&self) {
        self.tare_raw
            .store(self.latest_raw.load(Ordering::Relaxed), Ordering::Relaxed);
        self.tare_set.store(true, Ordering::Relaxed);
    }

    fn clear_tare(&self) {
        self.tare_set.store(false, Ordering::Relaxed);
    }

    /// Build the JSON body for `/reading`.
    fn reading_json(&self) -> String {
        let has = self.has_reading.load(Ordering::Relaxed);
        let raw = self.latest_raw.load(Ordering::Relaxed);
        let tare_set = self.tare_set.load(Ordering::Relaxed);
        let tare = self.tare_raw.load(Ordering::Relaxed);
        let seq = self.seq.load(Ordering::Relaxed);
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
    shutdown: Arc<AtomicBool>,
) -> eyre::Result<()>
where
    S: Scale + Send + 'static,
{
    let calibrated = calib.is_some();
    let shared = Arc::new(Shared::new(calib));

    // Background sampling thread. `Scale::read` blocks until data-ready, which
    // paces hardware naturally; we add a sleep so fast backends (sim) don't spin.
    let reader_shared = Arc::clone(&shared);
    let reader_shutdown = Arc::clone(&shutdown);
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
                            // EMA (alpha = 0.2) to smooth the SPS readout.
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

fn handle_request(req: Request, shared: &Shared) {
    let method = req.method().clone();
    // `url()` includes any query string; we only use fixed paths so trim it.
    let path = req.url().split('?').next().unwrap_or("/").to_string();

    match (&method, path.as_str()) {
        (Method::Get, "/") => respond(req, INDEX_HTML.to_string(), "text/html; charset=utf-8"),
        (Method::Get, "/reading") => respond(req, shared.reading_json(), "application/json"),
        (Method::Post, "/tare") => {
            shared.set_tare();
            respond(req, "{\"ok\":true}".to_string(), "application/json");
        }
        (Method::Post, "/tare/clear") => {
            shared.clear_tare();
            respond(req, "{\"ok\":true}".to_string(), "application/json");
        }
        _ => {
            let resp = Response::from_string("not found").with_status_code(404);
            let _ = req.respond(resp);
        }
    }
}

fn respond(req: Request, body: String, content_type: &str) {
    let mut resp = Response::from_string(body);
    if let Ok(h) = Header::from_bytes(&b"Content-Type"[..], content_type.as_bytes()) {
        resp.add_header(h);
    }
    let _ = req.respond(resp);
}

/// Self-contained monitor page: live number, raw/Δ/SPS stats, tare controls and
/// a scrolling chart. Polls `/reading` and draws with plain canvas (no CDN).
const INDEX_HTML: &str = r####"<!doctype html>
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
const history = [];
let lastSeq = -1, staleTicks = 0;

const $ = (id) => document.getElementById(id);

function fmtRaw(v) { return v == null ? "--" : Math.round(v).toLocaleString(); }

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

  $("conn").textContent = "live"; $("conn").className = "";
  $("vlabel").textContent = label;
  $("unit").textContent = unit;
  $("raw").textContent = fmtRaw(d.raw);
  $("delta").textContent = d.tare_set ? fmtRaw(d.tared_raw) : "—";
  $("sps").textContent = (d.sps != null) ? d.sps.toFixed(1) : "--";
  $("err").textContent = d.err ? ("read error: " + d.err) : "";
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

async function poll() {
  try {
    const r = await fetch("/reading", { cache: "no-store" });
    const d = await r.json();
    if (d.seq === lastSeq) {
      if (++staleTicks > 20) { $("conn").textContent = "no new samples"; $("conn").className = "bad"; }
    } else { lastSeq = d.seq; staleTicks = 0; }
    render(d);
  } catch (e) {
    $("conn").textContent = "disconnected"; $("conn").className = "bad";
  }
}

$("tare").addEventListener("click", () => fetch("/tare", { method: "POST" }));
$("cleartare").addEventListener("click", () => fetch("/tare/clear", { method: "POST" }));
window.addEventListener("resize", draw);
setInterval(poll, 100);
poll();
</script>
</body>
</html>
"####;
