// Exercises broadcast, axis-reduce, transpose matchers

use std::hint::black_box;
use std::time::Instant;

const M: usize = 2048;
const N: usize = 1024;
const ITERS: usize = 30;

#[parallelize_associative_float]
#[inline(never)]
fn broadcast_add(a: &mut [f64], b: &[f64], m: usize, n: usize) {
    for i in 0..m {
        for j in 0..n { a[i * n + j] += b[j]; }
    }
}

#[parallelize_associative_float]
#[inline(never)]
fn axis_sum(out: &mut [f64], a: &[f64], m: usize, n: usize) {
    for i in 0..m {
        let mut s = 0.0;
        for j in 0..n { s += a[i * n + j]; }
        out[i] = s;
    }
}

#[parallelize_associative_float]
#[inline(never)]
fn transpose(b: &mut [f64], a: &[f64], m: usize, n: usize) {
    for i in 0..m {
        for j in 0..n { b[j * m + i] = a[i * n + j]; }
    }
}

fn checksum(slice: &[f64]) -> f64 {
    let mut s = 0f64;
    for &x in slice { s += x; }
    s
}

fn main() {
    let mut a = vec![0f64; M * N];
    let b = vec![0.5f64; N];
    let mut at = vec![0f64; N * M];
    let mut row_sums = vec![0f64; M];
    for i in 0..M*N { a[i] = ((i as f64) * 1e-7) % 4.0 - 2.0; }
    let a_orig = a.clone();

    let t0 = Instant::now();
    for _ in 0..ITERS {
        a.copy_from_slice(&a_orig);
        broadcast_add(black_box(&mut a), black_box(&b), M, N);
        axis_sum(black_box(&mut row_sums), black_box(&a), M, N);
        transpose(black_box(&mut at), black_box(&a), M, N);
    }
    let elapsed_ms = t0.elapsed().as_secs_f64() * 1000.0;
    let cs = checksum(&row_sums) + checksum(&at);
    println!("MULTIDIM M={} N={} ITERS={} elapsed_ms={:.2} checksum={:.6e}",
        M, N, ITERS, elapsed_ms, cs);
}
