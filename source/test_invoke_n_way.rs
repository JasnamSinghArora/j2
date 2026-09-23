// N-way invoke bench, N=2..10, chain capped 8

use std::hint::black_box;
use std::time::Instant;

#[inline(never)]
fn pass_a(text: &[u8]) -> u64 {
    let mut h: u64 = 0xa1;
    let n = text.len();
    let mut i = 0usize;
    while i + 64 < n {
        if text[i] == b'A' {
            let mut j = i;
            while j < i + 64 {
                let b = text[j] as u64;
                h = h.wrapping_mul(0x100000001b3).wrapping_add(b);
                h ^= h.rotate_right(33);
                h = h.wrapping_mul(0xff51afd7ed558ccd);
                j = j.wrapping_add(1);
            }
        }
        i = i.wrapping_add(1);
    }
    h
}
#[inline(never)] fn pass_b(text: &[u8]) -> u64 { pass_with_marker(text, b'B', 0xb2) }
#[inline(never)] fn pass_c(text: &[u8]) -> u64 { pass_with_marker(text, b'C', 0xc3) }
#[inline(never)] fn pass_d(text: &[u8]) -> u64 { pass_with_marker(text, b'D', 0xd4) }
#[inline(never)] fn pass_e(text: &[u8]) -> u64 { pass_with_marker(text, b'E', 0xe5) }
#[inline(never)] fn pass_f(text: &[u8]) -> u64 { pass_with_marker(text, b'F', 0xf6) }
#[inline(never)] fn pass_g(text: &[u8]) -> u64 { pass_with_marker(text, b'G', 0x07) }
#[inline(never)] fn pass_h(text: &[u8]) -> u64 { pass_with_marker(text, b'H', 0x18) }
#[inline(never)] fn pass_i(text: &[u8]) -> u64 { pass_with_marker(text, b'I', 0x29) }
#[inline(never)] fn pass_j(text: &[u8]) -> u64 { pass_with_marker(text, b'J', 0x3a) }
#[inline(never)] fn pass_k(text: &[u8]) -> u64 { pass_with_marker(text, b'K', 0x4b) }
#[inline(never)] fn pass_l(text: &[u8]) -> u64 { pass_with_marker(text, b'L', 0x5c) }
#[inline(never)] fn pass_m(text: &[u8]) -> u64 { pass_with_marker(text, b'M', 0x6d) }
#[inline(never)] fn pass_n(text: &[u8]) -> u64 { pass_with_marker(text, b'N', 0x7e) }
#[inline(never)] fn pass_o(text: &[u8]) -> u64 { pass_with_marker(text, b'O', 0x8f) }
#[inline(never)] fn pass_p(text: &[u8]) -> u64 { pass_with_marker(text, b'P', 0x9a) }

#[inline(always)]
fn pass_with_marker(text: &[u8], marker: u8, seed: u64) -> u64 {
    let mut h: u64 = seed;
    let n = text.len();
    let mut i = 0usize;
    while i + 64 < n {
        if text[i] == marker {
            let mut j = i;
            while j < i + 64 {
                let b = text[j] as u64;
                h = h.wrapping_mul(0x100000001b3).wrapping_add(b);
                h ^= h.rotate_right(33);
                h = h.wrapping_mul(0xff51afd7ed558ccd);
                j = j.wrapping_add(1);
            }
        }
        i = i.wrapping_add(1);
    }
    h
}

#[inline(never)]
fn run_2(text: &[u8]) -> u64 {
    let a = pass_a(text);
    let b = pass_b(text);
    a ^ b
}
#[inline(never)]
fn run_3(text: &[u8]) -> u64 {
    let a = pass_a(text);
    let b = pass_b(text);
    let c = pass_c(text);
    a ^ b ^ c
}
#[inline(never)]
fn run_4(text: &[u8]) -> u64 {
    let a = pass_a(text);
    let b = pass_b(text);
    let c = pass_c(text);
    let d = pass_d(text);
    a ^ b ^ c ^ d
}
#[inline(never)]
fn run_5(text: &[u8]) -> u64 {
    let a = pass_a(text);
    let b = pass_b(text);
    let c = pass_c(text);
    let d = pass_d(text);
    let e = pass_e(text);
    a ^ b ^ c ^ d ^ e
}
#[inline(never)]
fn run_6(text: &[u8]) -> u64 {
    let a = pass_a(text);
    let b = pass_b(text);
    let c = pass_c(text);
    let d = pass_d(text);
    let e = pass_e(text);
    let f = pass_f(text);
    a ^ b ^ c ^ d ^ e ^ f
}
#[inline(never)]
fn run_7(text: &[u8]) -> u64 {
    let a = pass_a(text);
    let b = pass_b(text);
    let c = pass_c(text);
    let d = pass_d(text);
    let e = pass_e(text);
    let f = pass_f(text);
    let g = pass_g(text);
    a ^ b ^ c ^ d ^ e ^ f ^ g
}
#[inline(never)]
fn run_8(text: &[u8]) -> u64 {
    let a = pass_a(text);
    let b = pass_b(text);
    let c = pass_c(text);
    let d = pass_d(text);
    let e = pass_e(text);
    let f = pass_f(text);
    let g = pass_g(text);
    let h = pass_h(text);
    a ^ b ^ c ^ d ^ e ^ f ^ g ^ h
}
#[inline(never)]
fn run_9(text: &[u8]) -> u64 {
    let a = pass_a(text);
    let b = pass_b(text);
    let c = pass_c(text);
    let d = pass_d(text);
    let e = pass_e(text);
    let f = pass_f(text);
    let g = pass_g(text);
    let h = pass_h(text);
    let i = pass_i(text);
    a ^ b ^ c ^ d ^ e ^ f ^ g ^ h ^ i
}
#[inline(never)]
fn run_10(text: &[u8]) -> u64 {
    let a = pass_a(text);
    let b = pass_b(text);
    let c = pass_c(text);
    let d = pass_d(text);
    let e = pass_e(text);
    let f = pass_f(text);
    let g = pass_g(text);
    let h = pass_h(text);
    let i = pass_i(text);
    let j = pass_j(text);
    a ^ b ^ c ^ d ^ e ^ f ^ g ^ h ^ i ^ j
}
#[inline(never)]
fn run_12(text: &[u8]) -> u64 {
    let a = pass_a(text);
    let b = pass_b(text);
    let c = pass_c(text);
    let d = pass_d(text);
    let e = pass_e(text);
    let f = pass_f(text);
    let g = pass_g(text);
    let h = pass_h(text);
    let i = pass_i(text);
    let j = pass_j(text);
    let k = pass_k(text);
    let l = pass_l(text);
    a ^ b ^ c ^ d ^ e ^ f ^ g ^ h ^ i ^ j ^ k ^ l
}
#[inline(never)]
fn run_14(text: &[u8]) -> u64 {
    let a = pass_a(text);
    let b = pass_b(text);
    let c = pass_c(text);
    let d = pass_d(text);
    let e = pass_e(text);
    let f = pass_f(text);
    let g = pass_g(text);
    let h = pass_h(text);
    let i = pass_i(text);
    let j = pass_j(text);
    let k = pass_k(text);
    let l = pass_l(text);
    let m = pass_m(text);
    let n = pass_n(text);
    a ^ b ^ c ^ d ^ e ^ f ^ g ^ h ^ i ^ j ^ k ^ l ^ m ^ n
}
#[inline(never)]
fn run_16(text: &[u8]) -> u64 {
    let a = pass_a(text);
    let b = pass_b(text);
    let c = pass_c(text);
    let d = pass_d(text);
    let e = pass_e(text);
    let f = pass_f(text);
    let g = pass_g(text);
    let h = pass_h(text);
    let i = pass_i(text);
    let j = pass_j(text);
    let k = pass_k(text);
    let l = pass_l(text);
    let m = pass_m(text);
    let n = pass_n(text);
    let o = pass_o(text);
    let p = pass_p(text);
    a ^ b ^ c ^ d ^ e ^ f ^ g ^ h ^ i ^ j ^ k ^ l ^ m ^ n ^ o ^ p
}

fn build_text(n_bytes: usize) -> Vec<u8> {
    let mut v = Vec::with_capacity(n_bytes);
    for k in 0..n_bytes {
        // Rotate A..P markers so passes match evenly
        let m = (k % 100) as u8;
        v.push(if m < 16 { b'A' + m } else { b'.' });
    }
    v
}

fn time_min<F: FnMut() -> u64>(reps: usize, mut f: F) -> (f64, u64) {
    let mut best = f64::INFINITY;
    let mut last = 0u64;
    for _ in 0..reps {
        let t = Instant::now();
        last = black_box(f());
        let ms = t.elapsed().as_secs_f64() * 1000.0;
        if ms < best { best = ms; }
    }
    (best, last)
}

fn main() {
    let n_bytes: usize = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(8_000_000);
    let text = build_text(n_bytes);
    let t = text.as_slice();

    // Warm pool with a 4-way invoke
    let _ = black_box(run_4(black_box(t)));

    let (m2, _) = time_min(5, || run_2(black_box(t)));
    let (m3, _) = time_min(5, || run_3(black_box(t)));
    let (m4, _) = time_min(5, || run_4(black_box(t)));
    let (m5, _) = time_min(5, || run_5(black_box(t)));
    let (m6, _) = time_min(5, || run_6(black_box(t)));
    let (m7, _) = time_min(5, || run_7(black_box(t)));
    let (m8, _) = time_min(5, || run_8(black_box(t)));
    let (m9, _) = time_min(5, || run_9(black_box(t)));
    let (m10, _) = time_min(5, || run_10(black_box(t)));
    let (m12, _) = time_min(5, || run_12(black_box(t)));
    let (m14, _) = time_min(5, || run_14(black_box(t)));
    let (m16, _) = time_min(5, || run_16(black_box(t)));

    println!("input_bytes={n_bytes}");
    println!("N=2  best_ms={m2:8.3}");
    println!("N=3  best_ms={m3:8.3}");
    println!("N=4  best_ms={m4:8.3}");
    println!("N=5  best_ms={m5:8.3}");
    println!("N=6  best_ms={m6:8.3}");
    println!("N=7  best_ms={m7:8.3}");
    println!("N=8  best_ms={m8:8.3}");
    println!("N=9  best_ms={m9:8.3}");
    println!("N=10 best_ms={m10:8.3}");
    println!("N=12 best_ms={m12:8.3}");
    println!("N=14 best_ms={m14:8.3}");
    println!("N=16 best_ms={m16:8.3}");

    // Compute approximate per-call cost
    println!("PASS");
}
