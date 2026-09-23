// 2-layer MLP; slice-ref row so AXPY fires

use std::hint::black_box;
use std::time::Instant;

const N: usize = 100_000;
const D: usize = 32;
const H: usize = 64;
const EPOCHS: usize = 20;
const LR: f64 = 1e-4;

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

fn build_data(n: usize, d: usize, seed: u64) -> (Vec<Vec<f64>>, Vec<f64>) {
    let mut r = Rng::new(seed);
    let mut x: Vec<Vec<f64>> = (0..d).map(|_| vec![0f64; n]).collect();
    let mut y = vec![0f64; n];
    let mut w_true = vec![0f64; d];
    for v in w_true.iter_mut() { *v = r.next_f64(); }
    for i in 0..n {
        let mut s = 0f64;
        for j in 0..d {
            let xv = r.next_f64();
            x[j][i] = xv;
            s += xv * w_true[j];
        }
        y[i] = s.tanh() + 0.01 * r.next_f64();
    }
    (x, y)
}

#[inline(always)] fn relu(v: f64) -> f64 { if v > 0.0 { v } else { 0.0 } }

#[parallelize_associative_float]
#[inline(never)]
fn dot(a: &[f64], b: &[f64]) -> f64 {
    a.iter().zip(b.iter()).map(|(x, y)| x * y).sum()
}

#[parallelize_associative_float]
#[inline(never)]
fn sse(a: &[f64], b: &[f64]) -> f64 {
    a.iter().zip(b.iter()).map(|(x, y)| (x - y) * (x - y)).sum()
}

#[parallelize_associative_float]
#[inline(never)]
fn forward(
    x_cols: &[Vec<f64>],
    w1: &[f64], b1: &[f64], w2: &[f64], b2: f64,
    n: usize, d: usize, h: usize,
) -> (Vec<Vec<f64>>, Vec<f64>) {
    let mut a: Vec<Vec<f64>> = (0..h).map(|_| vec![0f64; n]).collect();
    for hi in 0..h {
        // borrow row as &mut [f64] once
        let row: &mut [f64] = &mut a[hi];
        for i in 0..n { row[i] = b1[hi]; }
        for j in 0..d {
            let w = w1[hi * d + j];
            let xs: &[f64] = &x_cols[j];
            // AXPY-MATCHER target, `row[i] += w * xs[i]`
            for i in 0..n { row[i] += w * xs[i]; }
        }
        for i in 0..n { row[i] = relu(row[i]); }
    }
    let mut yhat = vec![b2; n];
    let yh: &mut [f64] = &mut yhat;
    for hi in 0..h {
        let w = w2[hi];
        let as_: &[f64] = &a[hi];
        // ^ AXPY-MATCHER target ^
        for i in 0..n { yh[i] += w * as_[i]; }
    }
    (a, yhat)
}

#[parallelize_associative_float]
#[inline(never)]
fn backward_step(
    x_cols: &[Vec<f64>],
    a: &[Vec<f64>],
    yhat: &[f64],
    y: &[f64],
    w1: &mut [f64], b1: &mut [f64], w2: &mut [f64], b2: &mut f64,
    n: usize, d: usize, h: usize, lr: f64,
) {
    let mut grad_y = vec![0f64; n];
    let scale = 2.0 / (n as f64);
    for i in 0..n { grad_y[i] = (yhat[i] - y[i]) * scale; }

    let mut dw2 = vec![0f64; h];
    let gs: &[f64] = &grad_y;
    for hi in 0..h {
        let as_: &[f64] = &a[hi];
        dw2[hi] = dot(gs, as_);
    }
    let db2: f64 = grad_y.iter().sum::<f64>();

    let mut dw1 = vec![0f64; h * d];
    let mut db1 = vec![0f64; h];
    let mut grad_a_h = vec![0f64; n];
    for hi in 0..h {
        let w = w2[hi];
        let as_: &[f64] = &a[hi];
        for i in 0..n {
            grad_a_h[i] = if as_[i] > 0.0 { grad_y[i] * w } else { 0.0 };
        }
        let gas: &[f64] = &grad_a_h;
        for j in 0..d {
            let xs: &[f64] = &x_cols[j];
            dw1[hi * d + j] = dot(gas, xs);
        }
        db1[hi] = gas.iter().sum::<f64>();
    }

    for k in 0..(h * d) { w1[k] -= lr * dw1[k]; }
    for hi in 0..h { b1[hi] -= lr * db1[hi]; }
    for hi in 0..h { w2[hi] -= lr * dw2[hi]; }
    *b2 -= lr * db2;
}

fn main() {
    let (x_cols, y) = build_data(N, D, 0xBADCAFE);
    let mut r = Rng::new(0x1234_5678);
    let mut w1 = vec![0f64; H * D];
    let mut b1 = vec![0f64; H];
    let mut w2 = vec![0f64; H];
    let mut b2 = 0f64;
    let scale = (1.0 / (D as f64).sqrt()) * 0.5;
    for v in w1.iter_mut() { *v = r.next_f64() * scale; }
    let scale_h = (1.0 / (H as f64).sqrt()) * 0.5;
    for v in w2.iter_mut() { *v = r.next_f64() * scale_h; }

    let t0 = Instant::now();
    let mut last_loss = f64::NAN;
    for _ep in 0..EPOCHS {
        let (a, yhat) = forward(black_box(&x_cols), &w1, &b1, &w2, b2, N, D, H);
        last_loss = sse(&yhat, &y) / (N as f64);
        backward_step(&x_cols, &a, &yhat, &y, &mut w1, &mut b1, &mut w2, &mut b2,
                      N, D, H, LR);
    }
    let elapsed_ms = t0.elapsed().as_secs_f64() * 1000.0;

    println!("NN_V3 N={} D={} H={} EPOCHS={} elapsed_ms={:.2} final_loss={:.6e}",
        N, D, H, EPOCHS, elapsed_ms, last_loss);
}
