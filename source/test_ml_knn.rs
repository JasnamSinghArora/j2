// knn v2; slice refs so zip-diff-sq-add fires

use std::hint::black_box;
use std::time::Instant;

const N_TRAIN: usize = 200_000;
const N_QUERY: usize = 8;
const D: usize = 32;
const K: usize = 5;

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

fn build_data(n_train: usize, d: usize, n_query: usize, seed: u64)
    -> (Vec<Vec<f64>>, Vec<f64>, Vec<Vec<f64>>)
{
    let mut r = Rng::new(seed);
    let mut x_cols: Vec<Vec<f64>> = (0..d).map(|_| vec![0f64; n_train]).collect();
    let mut y = vec![0f64; n_train];
    for i in 0..n_train {
        for j in 0..d {
            x_cols[j][i] = r.next_f64();
        }
        y[i] = r.next_f64();
    }
    let queries: Vec<Vec<f64>> = (0..n_query)
        .map(|_| (0..d).map(|_| r.next_f64()).collect())
        .collect();
    (x_cols, y, queries)
}

#[inline(never)]
fn knn_predict(
    x_cols: &[Vec<f64>],
    y: &[f64],
    q: &[f64],
    n_train: usize,
    d: usize,
    k: usize,
) -> f64 {
    let mut dists = vec![0f64; n_train];
    let mut q_repeat = vec![0f64; n_train];
    {
        let dists_s: &mut [f64] = &mut dists;
        let q_repeat_s: &mut [f64] = &mut q_repeat;
        for j in 0..d {
            // Fill q_repeat with q[j]
            for i in 0..n_train { q_repeat_s[i] = q[j]; }
            let cs: &[f64] = &x_cols[j];
            let qs: &[f64] = q_repeat_s;
            // zip-diff-sq-add matcher target, replaces v1 smoke test
            for i in 0..n_train {
                let v = cs[i] - qs[i];
                dists_s[i] += v * v;
            }
        }
    }
    // Pick the k smallest distances.
    let mut top_idx: Vec<usize> = vec![0; k];
    let mut top_dst: Vec<f64> = vec![f64::INFINITY; k];
    for i in 0..n_train {
        let d2 = dists[i];
        if d2 < top_dst[k - 1] {
            let mut p = k - 1;
            top_dst[p] = d2; top_idx[p] = i;
            while p > 0 && top_dst[p] < top_dst[p - 1] {
                top_dst.swap(p, p - 1); top_idx.swap(p, p - 1);
                p -= 1;
            }
        }
    }
    let mut s = 0f64;
    for v in &top_idx { s += y[*v]; }
    s / (k as f64)
}

fn main() {
    let (x_cols, y, queries) = build_data(N_TRAIN, D, N_QUERY, 0xCAFE_F00D);
    let mut preds = vec![0f64; N_QUERY];
    let t0 = Instant::now();
    for (qi, q) in queries.iter().enumerate() {
        preds[qi] = knn_predict(black_box(&x_cols), black_box(&y), black_box(q), N_TRAIN, D, K);
    }
    let elapsed_ms = t0.elapsed().as_secs_f64() * 1000.0;

    let mut sum = 0f64;
    for v in &preds { sum += v; }
    println!("KNN_V2 N_TRAIN={} D={} N_QUERY={} K={} elapsed_ms={:.2} checksum={:.6e}",
        N_TRAIN, D, N_QUERY, K, elapsed_ms, sum);
}
