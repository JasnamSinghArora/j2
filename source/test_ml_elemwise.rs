// Element-wise and scale matchers on idiomatic slices

use std::hint::black_box;
use std::time::Instant;

const N: usize = 2_000_000;
const ITERS: usize = 30;

#[inline(never)]
fn do_fill(dst: &mut [f64], c: f64) {
    let n = dst.len();
    for i in 0..n { dst[i] = c; }
}

#[inline(never)]
fn do_copy(dst: &mut [f64], src: &[f64]) {
    let n = dst.len();
    for i in 0..n { dst[i] = src[i]; }
}

#[inline(never)]
fn do_zip_add(dst: &mut [f64], a: &[f64], b: &[f64]) {
    let n = dst.len();
    for i in 0..n { dst[i] = a[i] + b[i]; }
}

#[inline(never)]
fn do_zip_mul(dst: &mut [f64], a: &[f64], b: &[f64]) {
    let n = dst.len();
    for i in 0..n { dst[i] = a[i] * b[i]; }
}

#[inline(never)]
fn do_scale(dst: &mut [f64], alpha: f64) {
    let n = dst.len();
    for i in 0..n { dst[i] *= alpha; }
}

fn checksum(slice: &[f64]) -> f64 {
    let mut s = 0f64;
    for &x in slice { s += x; }
    s
}

fn main() {
    let mut a = vec![1.0f64; N];
    let mut b = vec![2.0f64; N];
    let mut dst = vec![0f64; N];

    let t0 = Instant::now();
    for _ in 0..ITERS {
        do_fill(black_box(&mut a), 0.5);
        do_copy(black_box(&mut dst), black_box(&a));
        do_zip_add(black_box(&mut dst), black_box(&a), black_box(&b));
        do_zip_mul(black_box(&mut dst), black_box(&a), black_box(&b));
        do_scale(black_box(&mut dst), 0.5);
    }
    let elapsed_ms = t0.elapsed().as_secs_f64() * 1000.0;

    let cs = checksum(&dst);
    println!("ELEMWISE N={} ITERS={} elapsed_ms={:.2} checksum={:.6e}",
        N, ITERS, elapsed_ms, cs);
}
