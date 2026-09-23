// Exercises the new unary-apply matcher

use std::hint::black_box;
use std::time::Instant;

const N: usize = 4_000_000;
const ITERS: usize = 30;

#[inline(never)]
fn apply_tanh(dst: &mut [f64], src: &[f64]) {
    let n = dst.len();
    for i in 0..n { dst[i] = src[i].tanh(); }
}

#[inline(never)]
fn apply_exp(dst: &mut [f64], src: &[f64]) {
    let n = dst.len();
    for i in 0..n { dst[i] = src[i].exp(); }
}

#[inline(never)]
fn apply_sqrt(dst: &mut [f64], src: &[f64]) {
    let n = dst.len();
    for i in 0..n { dst[i] = src[i].sqrt(); }
}

#[inline(never)]
fn apply_abs(dst: &mut [f64], src: &[f64]) {
    let n = dst.len();
    for i in 0..n { dst[i] = src[i].abs(); }
}

fn checksum(slice: &[f64]) -> f64 {
    let mut s = 0f64;
    for &x in slice { s += x; }
    s
}

fn main() {
    let mut a = vec![0f64; N];
    for (i, x) in a.iter_mut().enumerate() {
        *x = ((i as f64) * 1e-7) % 4.0 - 2.0;
    }
    let mut dst = vec![0f64; N];

    let t0 = Instant::now();
    for _ in 0..ITERS {
        apply_tanh(black_box(&mut dst), black_box(&a));
        apply_exp(black_box(&mut dst), black_box(&a));
        apply_sqrt(black_box(&mut dst), black_box(&a));
        apply_abs(black_box(&mut dst), black_box(&a));
    }
    let elapsed_ms = t0.elapsed().as_secs_f64() * 1000.0;
    let cs = checksum(&dst);
    println!("UNARY N={} ITERS={} elapsed_ms={:.2} checksum={:.6e}",
        N, ITERS, elapsed_ms, cs);
}
