use std::hint::black_box;
use std::time::Instant;

const N: usize = 1_000_000;
const KS: usize = 32;
const ITERS: usize = 30;

#[parallelize_associative_float]
#[inline(never)]
fn conv1d(y: &mut [f64], x: &[f64], w: &[f64], n: usize, ks: usize) {
    for i in 0..(n - ks + 1) {
        let mut acc = 0.0;
        for k in 0..ks { acc += x[i + k] * w[k]; }
        y[i] = acc;
    }
}

fn checksum(s: &[f64]) -> f64 { let mut t=0.0; for &v in s {t+=v;} t }

fn main() {
    let mut x = vec![0f64; N];
    let mut w = vec![0f64; KS];
    let out_len = N - KS + 1;
    let mut y = vec![0f64; out_len];
    for i in 0..N { x[i] = ((i as f64) * 1e-7) % 4.0 - 2.0; }
    for k in 0..KS { w[k] = ((k as f64) * 0.1) - 1.5; }
    let t0 = Instant::now();
    for _ in 0..ITERS { conv1d(black_box(&mut y), black_box(&x), black_box(&w), N, KS); }
    let ms = t0.elapsed().as_secs_f64() * 1000.0;
    let cs = checksum(&y);
    println!("CONV1D N={} KS={} ITERS={} elapsed_ms={:.2} checksum={:.6e}", N, KS, ITERS, ms, cs);
}
