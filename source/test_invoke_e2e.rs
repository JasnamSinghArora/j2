// E2E test, compiler parallelises two adjacent calls

use std::hint::black_box;
use std::time::Instant;

#[inline(never)]
pub fn heavy_a(seed: u64) -> u64 {
    let mut x = seed;
    let k = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    let mut i = 0u64;
    while i < 4096 {
        x = x.wrapping_mul(0xC2B2_AE35_3A11_05F1).wrapping_add(k);
        x ^= x.rotate_right(33);
        x = x.wrapping_mul(0xFF51_AFD7_ED55_8CCD);
        x ^= x.rotate_right(27);
        i = i.wrapping_add(1);
    }
    x
}

#[inline(never)]
pub fn heavy_b(seed: u64) -> u64 {
    let mut x = seed.wrapping_add(0x12345);
    let k = seed.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    let mut i = 0u64;
    while i < 4096 {
        x = x.wrapping_mul(0x94D0_49BB_1331_11EB).wrapping_add(k);
        x ^= x.rotate_left(17);
        x = x.wrapping_mul(0xCBF2_9CE4_8422_2325);
        x ^= x.rotate_right(31);
        i = i.wrapping_add(1);
    }
    x
}

/// The target pattern for the matcher.
#[inline(never)]
pub fn run(seed: u64) -> u64 {
    let a = heavy_a(seed);
    let b = heavy_b(seed);
    a ^ b
}

fn main() {
    let iters: u64 = std::env::args().nth(1).and_then(|s| s.parse().ok()).unwrap_or(2_000);

    // Warm any pools.
    let _ = black_box(run(black_box(1)));

    let t = Instant::now();
    let mut sink = 0u64;
    for k in 0..iters {
        sink ^= run(black_box(k));
    }
    let ms = t.elapsed().as_secs_f64() * 1000.0;
    let per_iter_ns = ms * 1_000_000.0 / iters as f64;

    println!("RESULT={:#x}", black_box(sink));
    println!("ITERS={}", iters);
    println!("TOTAL_MS={:.3}", ms);
    println!("NS_PER_ITER={:.1}", per_iter_ns);
}
