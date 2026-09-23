// Phase 7, clamp and multi-AXPY matchers

use std::hint::black_box;
use std::time::Instant;

const N: usize = 4_000_000;
const ITERS: usize = 30;

#[inline(never)]
fn do_clamp(dst: &mut [f64], a: &[f64], lo: f64, hi: f64) {
    let n = dst.len();
    for i in 0..n { dst[i] = a[i].clamp(lo, hi); }
}

#[inline(never)]
fn do_multi_axpy(dst: &mut [f64], x: &[f64], alpha: f64, z: &[f64], beta: f64) {
    let n = dst.len();
    for i in 0..n { dst[i] = alpha * x[i] + beta * z[i]; }
}

fn checksum(slice: &[f64]) -> f64 {
    let mut s = 0f64;
    for &x in slice { s += x; }
    s
}

fn main() {
    let mut a = vec![0f64; N];
    let mut z = vec![0f64; N];
    for i in 0..N {
        a[i] = ((i as f64) * 1e-7) % 4.0 - 2.0;
        z[i] = ((i as f64) * 2e-7) % 3.0 - 1.5;
    }
    let mut dst = vec![0f64; N];
    let t0 = Instant::now();
    for _ in 0..ITERS {
        do_clamp(black_box(&mut dst), black_box(&a), -0.5, 0.5);
        do_multi_axpy(black_box(&mut dst), black_box(&a), 0.3, black_box(&z), 0.7);
    }
    let elapsed_ms = t0.elapsed().as_secs_f64() * 1000.0;
    let cs = checksum(&dst);
    println!("PHASE7 N={} ITERS={} elapsed_ms={:.2} checksum={:.6e}",
        N, ITERS, elapsed_ms, cs);
}
