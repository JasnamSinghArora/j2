// linreg v4; inline predict_dot for matvec matcher

use std::hint::black_box;
use std::time::Instant;

const N: usize = 200_000;
const D: usize = 32;
const ITERS: usize = 50;
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

fn build_data(n: usize, d: usize, seed: u64) -> (Vec<f64>, Vec<f64>, Vec<f64>) {
    let mut r = Rng::new(seed);
    let mut x = vec![0f64; n * d];
    let mut y = vec![0f64; n];
    let mut w_true = vec![0f64; d];
    for v in w_true.iter_mut() { *v = r.next_f64(); }
    for i in 0..n {
        let mut acc = 0f64;
        for j in 0..d {
            let xv = r.next_f64();
            x[i * d + j] = xv;
            acc += xv * w_true[j];
        }
        y[i] = acc + r.next_f64() * 0.01;
    }
    (x, y, w_true)
}

#[parallelize_associative_float]
#[inline(never)]
fn mse_loss(yhat: &[f64], y: &[f64]) -> f64 {
    let s: f64 = yhat.iter().zip(y.iter()).map(|(a, b)| (a - b) * (a - b)).sum();
    s / (yhat.len() as f64)
}

#[parallelize_associative_float]
#[inline(never)]
fn residual_dot_col(resid: &[f64], col: &[f64]) -> f64 {
    resid.iter().zip(col.iter()).map(|(a, b)| a * b).sum()
}

#[parallelize_associative_float]
fn step(x: &[f64], y: &[f64], w: &mut [f64], n: usize, d: usize, lr: f64) {
    let mut yhat = vec![0f64; n];
    {
        // MATVEC matcher fires on this nested loop
        let yhat_s: &mut [f64] = &mut yhat;
        for i in 0..n {
            let mut acc = 0.0;
            for j in 0..d { acc += x[i * d + j] * w[j]; }
            yhat_s[i] = acc;
        }
    }
    let mut resid = vec![0f64; n];
    {
        let resid_s: &mut [f64] = &mut resid;
        for i in 0..n { resid_s[i] = yhat[i] - y[i]; }
    }
    let mut grad = vec![0f64; d];
    let mut col = vec![0f64; n];
    {
        let col_s: &mut [f64] = &mut col;
        for j in 0..d {
            for i in 0..n { col_s[i] = x[i * d + j]; }
            grad[j] = residual_dot_col(&resid, col_s);
        }
    }
    for j in 0..d { w[j] -= lr * grad[j]; }
}

fn main() {
    let (x, y, w_true) = build_data(N, D, 0xCAFEBABE);
    let mut w = vec![0f64; D];

    let t0 = Instant::now();
    for _ in 0..ITERS {
        step(black_box(&x), black_box(&y), black_box(&mut w), N, D, LR);
    }
    let elapsed_ms = t0.elapsed().as_secs_f64() * 1000.0;

    let mut yhat = vec![0f64; N];
    for i in 0..N {
        let mut acc = 0.0;
        for j in 0..D { acc += x[i * D + j] * w[j]; }
        yhat[i] = acc;
    }
    let final_loss = mse_loss(&yhat, &y);

    let mut w_dist = 0f64;
    for j in 0..D { w_dist += (w[j] - w_true[j]).powi(2); }

    println!("LINREG_V4 N={} D={} ITERS={} elapsed_ms={:.2} final_loss={:.6e} w_dist2={:.6e}",
        N, D, ITERS, elapsed_ms, final_loss, w_dist);
}
