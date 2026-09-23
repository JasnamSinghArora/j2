// Demonstrates the AXPY MIR matcher

use std::hint::black_box;
use std::time::Instant;

const N: usize = 200_000;
const D: usize = 64;
const ITERS: usize = 100;
const LR: f64 = 5e-7;

struct Rng(u64);
impl Rng {
    fn new(seed: u64) -> Self { Self(seed.max(1)) }
    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13; x ^= x >> 7; x ^= x << 17;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn next_f64(&mut self) -> f64 {
        let v = self.next_u64() >> 11;
        ((v as f64) / ((1u64 << 53) as f64)) * 2.0 - 1.0
    }
}

fn build(n: usize, d: usize, seed: u64) -> (Vec<Vec<f64>>, Vec<f64>) {
    let mut r = Rng::new(seed);
    let mut x_cols: Vec<Vec<f64>> = (0..d).map(|_| vec![0f64; n]).collect();
    let mut y = vec![0f64; n];
    let mut w_true = vec![0f64; d];
    for v in w_true.iter_mut() { *v = r.next_f64(); }
    for i in 0..n {
        let mut acc = 0f64;
        for j in 0..d {
            let v = r.next_f64();
            x_cols[j][i] = v;
            acc += v * w_true[j];
        }
        y[i] = acc + 0.01 * r.next_f64();
    }
    (x_cols, y)
}

// AXPY inner loop, exactly the matcher's pattern
#[parallelize_associative_float]
#[inline(never)]
fn forward_axpy(yhat: &mut [f64], x_cols: &[Vec<f64>], w: &[f64]) {
    let n = yhat.len();
    let d = x_cols.len();
    for i in 0..n { yhat[i] = 0.0; }
    for j in 0..d {
        let wj = w[j];
        let xs: &[f64] = &x_cols[j];
        // AXPY matcher fires on this loop
        for i in 0..n { yhat[i] += wj * xs[i]; }
    }
}

#[parallelize_associative_float]
#[inline(never)]
fn step(x_cols: &[Vec<f64>], y: &[f64], w: &mut [f64], lr: f64) -> f64 {
    let n = y.len();
    let d = w.len();
    let mut yhat = vec![0f64; n];
    forward_axpy(&mut yhat, x_cols, w);

    // residual r[i] = yhat[i] - y[i]
    let mut resid = vec![0f64; n];
    for i in 0..n { resid[i] = yhat[i] - y[i]; }
    // Gradient via serial dot-products, then weight update
    let mut grad = vec![0f64; d];
    for j in 0..d {
        let xs = &x_cols[j];
        let mut g = 0f64;
        for i in 0..n { g += resid[i] * xs[i]; }
        grad[j] = g;
    }
    for j in 0..d { w[j] -= lr * grad[j]; }
    // mse loss
    let mut mse = 0f64;
    for i in 0..n { mse += resid[i] * resid[i]; }
    mse / (n as f64)
}

fn main() {
    let (x_cols, y) = build(N, D, 0xCAFE);
    let mut w = vec![0f64; D];
    let t0 = Instant::now();
    let mut last = f64::NAN;
    for _ in 0..ITERS {
        last = step(black_box(&x_cols), black_box(&y), black_box(&mut w), LR);
    }
    let ms = t0.elapsed().as_secs_f64() * 1000.0;
    println!("AXPY_AUTO N={} D={} ITERS={} ms={:.2} loss={:.6e}", N, D, ITERS, ms, last);
}
