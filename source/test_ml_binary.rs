// Exercises the new binary-apply matcher

use std::hint::black_box;
use std::time::Instant;

const N: usize = 4_000_000;
const ITERS: usize = 30;

#[inline(never)]
fn apply_max(dst: &mut [f64], a: &[f64], b: &[f64]) {
    let n = dst.len();
    for i in 0..n { dst[i] = a[i].max(b[i]); }
}

#[inline(never)]
fn apply_min(dst: &mut [f64], a: &[f64], b: &[f64]) {
    let n = dst.len();
    for i in 0..n { dst[i] = a[i].min(b[i]); }
}

#[inline(never)]
fn apply_hypot(dst: &mut [f64], a: &[f64], b: &[f64]) {
    let n = dst.len();
    for i in 0..n { dst[i] = a[i].hypot(b[i]); }
}

#[inline(never)]
fn apply_atan2(dst: &mut [f64], a: &[f64], b: &[f64]) {
    let n = dst.len();
    for i in 0..n { dst[i] = a[i].atan2(b[i]); }
}

fn checksum(slice: &[f64]) -> f64 {
    let mut s = 0f64;
    for &x in slice { s += x; }
    s
}

fn main() {
    let mut a = vec![0f64; N];
    let mut b = vec![0f64; N];
    for i in 0..N {
        a[i] = ((i as f64) * 1e-7) % 4.0 - 2.0;
        b[i] = ((i as f64) * 2e-7) % 3.0 - 1.5;
    }
    let mut dst = vec![0f64; N];
    let t0 = Instant::now();
    for _ in 0..ITERS {
        apply_max(black_box(&mut dst), black_box(&a), black_box(&b));
        apply_min(black_box(&mut dst), black_box(&a), black_box(&b));
        apply_hypot(black_box(&mut dst), black_box(&a), black_box(&b));
        apply_atan2(black_box(&mut dst), black_box(&a), black_box(&b));
    }
    let elapsed_ms = t0.elapsed().as_secs_f64() * 1000.0;
    let cs = checksum(&dst);
    println!("BINARY N={} ITERS={} elapsed_ms={:.2} checksum={:.6e}",
        N, ITERS, elapsed_ms, cs);
}
