// Phase 9, matvec matcher on idiomatic source

use std::hint::black_box;
use std::time::Instant;

const M: usize = 4096;
const N: usize = 1024;
const ITERS: usize = 30;

#[parallelize_associative_float]
#[inline(never)]
fn matvec(y: &mut [f64], a: &[f64], x: &[f64], m: usize, n: usize) {
    for i in 0..m {
        let mut acc = 0.0;
        for j in 0..n {
            acc += a[i * n + j] * x[j];
        }
        y[i] = acc;
    }
}

fn checksum(slice: &[f64]) -> f64 {
    let mut s = 0f64;
    for &x in slice { s += x; }
    s
}

fn main() {
    let mut a = vec![0f64; M * N];
    let mut x = vec![0f64; N];
    let mut y = vec![0f64; M];
    for i in 0..M*N { a[i] = ((i as f64) * 1e-7) % 4.0 - 2.0; }
    for i in 0..N { x[i] = ((i as f64) * 2e-7) % 3.0 - 1.5; }

    let t0 = Instant::now();
    for _ in 0..ITERS {
        matvec(black_box(&mut y), black_box(&a), black_box(&x), M, N);
    }
    let elapsed_ms = t0.elapsed().as_secs_f64() * 1000.0;
    let cs = checksum(&y);
    println!("MATVEC_AUTO M={} N={} ITERS={} elapsed_ms={:.2} checksum={:.6e}",
        M, N, ITERS, elapsed_ms, cs);
}
