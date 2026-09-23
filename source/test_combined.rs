// Workload exercising both graph and reduction passes

#![feature(parallel_runtime_internal)]

use std::time::Instant;

const N: u64 = 200_000_000;

/// 16 mutually-independent wrapping-mul lanes, tree-XOR fold
#[inline(never)]
pub fn body(i: u64) -> u64 {
    // Mix index into seed to defeat CSE
    let s = i.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ 0xDEAD_BEEF_CAFE_BABE;

    // 16 independent lanes for cohort coalescer
    let r00 = s.wrapping_mul(0x01).wrapping_mul(0x21);
    let r01 = s.wrapping_mul(0x03).wrapping_mul(0x23);
    let r02 = s.wrapping_mul(0x05).wrapping_mul(0x25);
    let r03 = s.wrapping_mul(0x07).wrapping_mul(0x27);
    let r04 = s.wrapping_mul(0x09).wrapping_mul(0x29);
    let r05 = s.wrapping_mul(0x0B).wrapping_mul(0x2B);
    let r06 = s.wrapping_mul(0x0D).wrapping_mul(0x2D);
    let r07 = s.wrapping_mul(0x0F).wrapping_mul(0x2F);
    let r08 = s.wrapping_mul(0x11).wrapping_mul(0x31);
    let r09 = s.wrapping_mul(0x13).wrapping_mul(0x33);
    let r0a = s.wrapping_mul(0x15).wrapping_mul(0x35);
    let r0b = s.wrapping_mul(0x17).wrapping_mul(0x37);
    let r0c = s.wrapping_mul(0x19).wrapping_mul(0x39);
    let r0d = s.wrapping_mul(0x1B).wrapping_mul(0x3B);
    let r0e = s.wrapping_mul(0x1D).wrapping_mul(0x3D);
    let r0f = s.wrapping_mul(0x1F).wrapping_mul(0x3F);

    // Tree-XOR fold; each level a parallel layer
    let p0 = r00 ^ r01;
    let p1 = r02 ^ r03;
    let p2 = r04 ^ r05;
    let p3 = r06 ^ r07;
    let p4 = r08 ^ r09;
    let p5 = r0a ^ r0b;
    let p6 = r0c ^ r0d;
    let p7 = r0e ^ r0f;

    let q0 = p0 ^ p1;
    let q1 = p2 ^ p3;
    let q2 = p4 ^ p5;
    let q3 = p6 ^ p7;

    let m0 = q0 ^ q1;
    let m1 = q2 ^ q3;

    m0 ^ m1
}

/// Canonical reducible loop; auto-reduction rewrites to parallel_reduce_add_u64
fn run(n: u64) -> u64 {
    let mut acc: u64 = 0;
    let mut i: u64 = 0;
    while i < n {
        acc = acc.wrapping_add(body(i));
        i = i.wrapping_add(1);
    }
    acc
}

fn main() {
    let t0 = Instant::now();
    let acc = run(N);
    let dt_ms = t0.elapsed().as_secs_f64() * 1000.0;

    let mut sink = [0u64; 1];
    unsafe { std::ptr::write_volatile(&mut sink[0], acc); }

    println!("RESULT={:#018x}", acc);
    println!("COMBINED_MS={:.3}", dt_ms);
}
