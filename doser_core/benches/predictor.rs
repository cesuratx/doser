use criterion::{BatchSize, Criterion, black_box, criterion_group, criterion_main};

// Generate a synthetic trace: sine with additive white noise
fn synth_trace(n: usize, noise_amp: f32, seed: u32) -> Vec<f32> {
    // tiny PRNG
    let mut state = seed.max(1);
    let mut next_f32 = || {
        let mut x = state;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        state = x;
        (x as f32) / (u32::MAX as f32 + 1.0)
    };
    let mut v = Vec::with_capacity(n);
    for i in 0..n {
        let t = i as f32 / 200.0; // ~low frequency
        let s = (t).sin();
        let noise = (next_f32() * 2.0 - 1.0) * noise_amp; // [-amp, +amp]
        v.push(s + noise);
    }
    v
}

// Compute finite-difference slope
fn slope_from_trace(x: &[f32]) -> Vec<f32> {
    let mut out = Vec::with_capacity(x.len());
    if x.is_empty() {
        return out;
    }
    out.push(0.0);
    for i in 1..x.len() {
        out.push(x[i] - x[i - 1]);
    }
    out
}

// Simple EMA update over an iterator of samples
#[inline]
fn ema_update(samples: &[f32], alpha: f32) -> f32 {
    let mut y = 0.0f32;
    let a = alpha.clamp(0.0, 1.0);
    let one_m_a = 1.0 - a;
    for &x in samples {
        y = a * x + one_m_a * y;
    }
    y
}

pub fn bench_ema_slope(c: &mut Criterion) {
    let mut g = c.benchmark_group("ema_slope");
    // Allow quick tweaking without CLI flags (Criterion 0.5):
    //   BENCH_SAMPLE_SIZE=10 BENCH_MEAS_MS=50 cargo bench -p doser_core --bench predictor
    if let Ok(ss) = std::env::var("BENCH_SAMPLE_SIZE") {
        if let Ok(n) = ss.parse::<usize>() {
            g.sample_size(n.max(1));
        }
    } else {
        g.sample_size(50);
    }
    if let Ok(ms) = std::env::var("BENCH_MEAS_MS")
        && let Ok(ms_u64) = ms.parse::<u64>()
    {
        g.measurement_time(std::time::Duration::from_millis(ms_u64));
    }

    let n = 50_000usize;
    let trace = synth_trace(n, 0.02, 0xC0FFEE);
    let slope = slope_from_trace(&trace);

    for &alpha in &[0.05f32, 0.15, 0.30] {
        g.bench_function(format!("ema_alpha_{alpha}"), |b| {
            b.iter_batched(
                || slope.clone(),
                |s| {
                    let y = ema_update(black_box(&s), black_box(alpha));
                    black_box(y);
                },
                BatchSize::SmallInput,
            )
        });
    }
    g.finish();
}

criterion_group!(predictor, bench_ema_slope);
criterion_main!(predictor);
