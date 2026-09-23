// Phase 6, zip-div-write, abs-diff, FMA-write matchers

use std::hint::black_box;
use std::time::Instant;

const N: usize = 4_000_000;
const ITERS: usize = 30;

#[inline(never)]
fn do_div(dst: &mut [f64], a: &[f64], b: &[f64]) {
    let n = dst.len();
    for i in 0..n { dst[i] = a[i] / b[i]; }
}

#[inline(never)]
fn do_abs_diff(dst: &mut [f64], a: &[f64], b: &[f64]) {
    let n = dst.len();
    for i in 0..n { dst[i] = (a[i] - b[i]).abs(); }
}

#[inline(never)]
fn do_fma(dst: &mut [f64], a: &[f64], b: &[f64], c: &[f64]) {
    let n = dst.len();
    for i in 0..n { dst[i] = a[i] * b[i] + c[i]; }
}

fn checksum(slice: &[f64]) -> f64 {
    let mut s = 0f64;
    for &x in slice { s += x; }
    s
}

fn main() {
    let mut a = vec![0f64; N];
    let mut b = vec![0f64; N];
    let mut c = vec![0f64; N];
    for i in 0..N {
        a[i] = ((i as f64) * 1e-7) % 4.0 - 2.0;
        b[i] = ((i as f64) * 2e-7) % 3.0 + 0.5;  // > 0 to avoid div-by-zero
        c[i] = ((i as f64) * 3e-7) % 2.0 - 1.0;
    }
    let mut dst = vec![0f64; N];
    let t0 = Instant::now();
    for _ in 0..ITERS {
        do_div(black_box(&mut dst), black_box(&a), black_box(&b));
        do_abs_diff(black_box(&mut dst), black_box(&a), black_box(&b));
        do_fma(black_box(&mut dst), black_box(&a), black_box(&b), black_box(&c));
    }
    let elapsed_ms = t0.elapsed().as_secs_f64() * 1000.0;
    let cs = checksum(&dst);
    println!("PHASE6 N={} ITERS={} elapsed_ms={:.2} checksum={:.6e}",
        N, ITERS, elapsed_ms, cs);
}
