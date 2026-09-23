// 4-model grid search; clones break matcher chain

#![feature(parallel_runtime_internal)]

use std::hint::black_box;
use std::time::Instant;

const N: usize = 80_000;
const D: usize = 32;

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

fn build_data(n: usize, d: usize, seed: u64) -> (Vec<f64>, Vec<f64>) {
    let mut r = Rng::new(seed);
    let mut x = vec![0f64; n * d];
    let mut y = vec![0f64; n];
    let mut w_true = vec![0f64; d];
    for v in w_true.iter_mut() { *v = r.next_f64(); }
    for i in 0..n {
        let mut s = 0f64;
        for j in 0..d {
            let xv = r.next_f64();
            x[i * d + j] = xv;
            s += xv * w_true[j];
        }
        y[i] = s + 0.01 * r.next_f64();
    }
    (x, y)
}

#[inline(never)]
fn dot(x_row: &[f64], w: &[f64]) -> f64 {
    let mut s = 0f64;
    for k in 0..x_row.len() { s += x_row[k] * w[k]; }
    s
}

// Train one model with given hyperparams
#[derive(Clone, Copy)]
struct HP { lr: f64, iters: usize, model_id: u32 }

#[derive(Clone, Copy, Debug)]
struct Out { final_loss: f64, w_norm: f64, model_id: u32 }

// Bundle params into one struct for parallel_invoke_4_fn1
struct TrainCfg { x: Vec<f64>, y: Vec<f64>, n: usize, d: usize, hp: HP }

#[inline(never)]
fn train_one(cfg: TrainCfg) -> Out {
    let TrainCfg { x, y, n, d, hp } = cfg;
    let mut w = vec![0f64; d];
    let mut yhat = vec![0f64; n];
    let mut resid = vec![0f64; n];
    let mut grad = vec![0f64; d];
    let mut col = vec![0f64; n];
    for _ in 0..hp.iters {
        // forward
        for i in 0..n {
            yhat[i] = dot(&x[i * d..(i + 1) * d], &w);
            resid[i] = yhat[i] - y[i];
        }
        // Gradient, grad[j] = sum_i resid[i]*X[i*D+j]
        for j in 0..d {
            for i in 0..n { col[i] = x[i * d + j]; }
            let mut s = 0f64;
            for i in 0..n { s += resid[i] * col[i]; }
            grad[j] = s;
        }
        for j in 0..d { w[j] -= hp.lr * grad[j]; }
    }
    // Final loss
    let mut s = 0f64;
    for i in 0..n {
        let yh = dot(&x[i * d..(i + 1) * d], &w);
        let r = yh - y[i];
        s += r * r;
    }
    let loss = s / (n as f64);
    let mut w_norm = 0f64;
    for v in &w { w_norm += v * v; }
    Out { final_loss: loss, w_norm, model_id: hp.model_id }
}

#[inline(never)]
fn grid_search(x: &[f64], y: &[f64], n: usize, d: usize) -> [Out; 4] {
    // Four independent `train_one` calls
    let hp_a = HP { lr: 1e-7, iters: 12, model_id: 0 };
    let hp_b = HP { lr: 5e-7, iters: 12, model_id: 1 };
    let hp_c = HP { lr: 1e-6, iters: 12, model_id: 2 };
    let hp_d = HP { lr: 5e-6, iters: 12, model_id: 3 };

    let cfg_a = TrainCfg { x: x.to_vec(), y: y.to_vec(), n, d, hp: hp_a };
    let cfg_b = TrainCfg { x: x.to_vec(), y: y.to_vec(), n, d, hp: hp_b };
    let cfg_c = TrainCfg { x: x.to_vec(), y: y.to_vec(), n, d, hp: hp_c };
    let cfg_d = TrainCfg { x: x.to_vec(), y: y.to_vec(), n, d, hp: hp_d };
    let (oa, ob, oc, od) = std::parallel_runtime::parallel_invoke_4(
        move || train_one(cfg_a),
        move || train_one(cfg_b),
        move || train_one(cfg_c),
        move || train_one(cfg_d),
    );
    [oa, ob, oc, od]
}

#[inline(never)]
fn grid_search_serial(x: &[f64], y: &[f64], n: usize, d: usize) -> [Out; 4] {
    let hp_a = HP { lr: 1e-7, iters: 12, model_id: 0 };
    let hp_b = HP { lr: 5e-7, iters: 12, model_id: 1 };
    let hp_c = HP { lr: 1e-6, iters: 12, model_id: 2 };
    let hp_d = HP { lr: 5e-6, iters: 12, model_id: 3 };
    let cfg_a = TrainCfg { x: x.to_vec(), y: y.to_vec(), n, d, hp: hp_a };
    let cfg_b = TrainCfg { x: x.to_vec(), y: y.to_vec(), n, d, hp: hp_b };
    let cfg_c = TrainCfg { x: x.to_vec(), y: y.to_vec(), n, d, hp: hp_c };
    let cfg_d = TrainCfg { x: x.to_vec(), y: y.to_vec(), n, d, hp: hp_d };
    let oa = train_one(cfg_a);
    let ob = train_one(cfg_b);
    let oc = train_one(cfg_c);
    let od = train_one(cfg_d);
    [oa, ob, oc, od]
}

fn main() {
    let (x, y) = build_data(N, D, 0xBADD00D);

    // Serial, 4 calls back to back
    let t_ser = Instant::now();
    let outs_ser = grid_search_serial(black_box(&x), black_box(&y), N, D);
    let ms_ser = t_ser.elapsed().as_secs_f64() * 1000.0;

    // Parallel, parallel_invoke_4 dispatches each to a worker
    let t_par = Instant::now();
    let outs_par = grid_search(black_box(&x), black_box(&y), N, D);
    let ms_par = t_par.elapsed().as_secs_f64() * 1000.0;

    // Verify identical outputs.
    let mut best_ser = &outs_ser[0];
    for o in &outs_ser[1..] { if o.final_loss < best_ser.final_loss { best_ser = o; } }
    let mut best_par = &outs_par[0];
    for o in &outs_par[1..] { if o.final_loss < best_par.final_loss { best_par = o; } }

    let speedup = ms_ser / ms_par;
    println!("GRID N={} D={} models=4 ser_ms={:.2} par_ms={:.2} speedup={:.2}x best_loss={:.6e}",
        N, D, ms_ser, ms_par, speedup, best_par.final_loss);
}
