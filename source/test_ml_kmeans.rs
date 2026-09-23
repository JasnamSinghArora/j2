// kmeans v2; slice refs so zip-diff-sq-add fires

use std::hint::black_box;
use std::time::Instant;

const N: usize = 100_000;
const D: usize = 16;
const K: usize = 8;
const ITERS: usize = 30;

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

fn build_points(n: usize, d: usize, seed: u64) -> Vec<Vec<f64>> {
    let mut r = Rng::new(seed);
    let mut cols: Vec<Vec<f64>> = (0..d).map(|_| vec![0f64; n]).collect();
    for i in 0..n {
        for j in 0..d {
            cols[j][i] = r.next_f64();
        }
    }
    cols
}

#[parallelize_associative_float]
#[inline(never)]
fn sse_col(col: &[f64], cval: &[f64]) -> f64 {
    col.iter().zip(cval.iter()).map(|(a, b)| (a - b) * (a - b)).sum()
}

fn assign_step(
    cols: &[Vec<f64>],
    centroids: &[Vec<f64>],
    n: usize, d: usize, k: usize,
) -> Vec<usize> {
    let mut labels = vec![0usize; n];
    let mut dists: Vec<Vec<f64>> = (0..k).map(|_| vec![0f64; n]).collect();
    let mut buf = vec![0f64; n];
    {
        let buf_s: &mut [f64] = &mut buf;
        for c in 0..k {
            let dists_c: &mut [f64] = &mut dists[c];
            for j in 0..d {
                let cv = centroids[c][j];
                for i in 0..n { buf_s[i] = cv; }
                let cs: &[f64] = &cols[j];
                let bs: &[f64] = buf_s;
                // ^ ZIP-DIFF-SQ-ADD MATCHER target ^
                for i in 0..n {
                    let v = cs[i] - bs[i];
                    dists_c[i] += v * v;
                }
            }
        }
    }
    for i in 0..n {
        let mut best_c = 0;
        let mut best_d = dists[0][i];
        for c in 1..k {
            if dists[c][i] < best_d { best_d = dists[c][i]; best_c = c; }
        }
        labels[i] = best_c;
    }
    labels
}

fn update_step(
    cols: &[Vec<f64>],
    labels: &[usize],
    n: usize, d: usize, k: usize,
) -> Vec<Vec<f64>> {
    let mut sums = vec![vec![0f64; d]; k];
    let mut counts = vec![0usize; k];
    for i in 0..n {
        let c = labels[i];
        counts[c] += 1;
        for j in 0..d { sums[c][j] += cols[j][i]; }
    }
    let mut centroids = vec![vec![0f64; d]; k];
    for c in 0..k {
        let cnt = counts[c].max(1) as f64;
        for j in 0..d { centroids[c][j] = sums[c][j] / cnt; }
    }
    centroids
}

fn main() {
    let cols = build_points(N, D, 0xC0FFEE);
    let mut centroids: Vec<Vec<f64>> =
        (0..K).map(|c| (0..D).map(|j| cols[j][c]).collect()).collect();

    let t0 = Instant::now();
    let mut last_inertia = f64::INFINITY;
    for _ in 0..ITERS {
        let labels = assign_step(black_box(&cols), black_box(&centroids), N, D, K);
        centroids = update_step(black_box(&cols), &labels, N, D, K);
        let mut inertia = 0f64;
        let mut buf = vec![0f64; N];
        {
            let buf_s: &mut [f64] = &mut buf;
            for c in 0..K {
                for j in 0..D {
                    let cv = centroids[c][j];
                    for i in 0..N { buf_s[i] = cv; }
                    inertia += sse_col(&cols[j], buf_s);
                }
            }
        }
        last_inertia = inertia;
    }
    let elapsed_ms = t0.elapsed().as_secs_f64() * 1000.0;

    println!("KMEANS_V2 N={} D={} K={} ITERS={} elapsed_ms={:.2} final_inertia={:.6e}",
        N, D, K, ITERS, elapsed_ms, last_inertia);
}
