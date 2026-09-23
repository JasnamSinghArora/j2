// parallel_invoke allocator stress, N=2..16, checksums must match

#![feature(parallel_runtime_internal)]

use std::collections::HashMap;
use std::hint::black_box;
use std::parallel_runtime as pr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

const PER_CALL_VEC_PUSHES: usize = 4_000;
const PER_CALL_HASH_INSERTS: usize = 1_000;
const PER_CALL_STRING_CONCATS: usize = 200;

#[derive(Debug)]
struct WorkOut {
    vec_sum: u64,
    hash_sum: u64,
    string_len: usize,
    string_hash: u64,
}

impl WorkOut {
    fn checksum(&self) -> u64 {
        self.vec_sum
            .wrapping_mul(0x100000001b3)
            .wrapping_add(self.hash_sum)
            .wrapping_mul(0x100000001b3)
            .wrapping_add(self.string_len as u64)
            .wrapping_mul(0x100000001b3)
            .wrapping_add(self.string_hash)
    }
}

#[inline(never)]
fn allocator_work(seed: u64) -> WorkOut {
    // Vec churn: grow + occasional shrink.
    let mut v: Vec<u64> = Vec::new();
    let mut k: u64 = seed.wrapping_add(1);
    for i in 0..PER_CALL_VEC_PUSHES {
        v.push(k);
        k = k.wrapping_mul(0x9E37_79B9_7F4A_7C15).wrapping_add(i as u64);
        // Periodic shrink to force realloc thrash
        if i % 257 == 0 && v.len() > 100 {
            v.truncate(v.len() - 50);
        }
    }
    let vec_sum = v.iter().copied().fold(0u64, |a, b| a.wrapping_add(b));

    // HashMap churn
    let mut m: HashMap<u64, u64> = HashMap::new();
    let mut h: u64 = seed.wrapping_add(0xdead_beef);
    for i in 0..PER_CALL_HASH_INSERTS {
        h = h.wrapping_mul(0xC2B2_AE35_3A11_05F1).wrapping_add(i as u64);
        m.insert(h, h.wrapping_mul(7));
        // Periodic remove to force bucket thrash
        if i % 33 == 0 && i > 0 {
            let key_to_remove = h.wrapping_sub(7);
            m.remove(&key_to_remove);
        }
    }
    let hash_sum = m.values().copied().fold(0u64, |a, b| a.wrapping_add(b));

    // String concat: many small allocations + drops
    let mut s = String::with_capacity(64);
    for i in 0..PER_CALL_STRING_CONCATS {
        let piece = format!("{seed:08x}-{i:04}-");
        s.push_str(&piece);
        // Periodic shrink to force String realloc
        if i % 47 == 0 && s.len() > 200 {
            s.truncate(s.len() - 100);
        }
    }
    let string_len = s.len();
    let string_hash = s.bytes().fold(0u64, |a, b| {
        a.wrapping_mul(0x100000001b3).wrapping_add(b as u64)
    });

    WorkOut { vec_sum, hash_sum, string_len, string_hash }
}

static TOTAL_CALLS: AtomicU64 = AtomicU64::new(0);

fn record(out: &WorkOut) -> u64 {
    TOTAL_CALLS.fetch_add(1, Ordering::Relaxed);
    out.checksum()
}

// Serial baseline for N, returns combined checksum
fn serial_n(n: u32, base_seed: u64) -> u64 {
    let mut acc = 0u64;
    for i in 0..n {
        let out = allocator_work(base_seed.wrapping_add(i as u64));
        acc = acc.wrapping_mul(0x100000001b3).wrapping_add(record(&out));
    }
    acc
}

// Combine N WorkOuts in serial_n's order
macro_rules! combine {
    ($($w:expr),+ $(,)?) => {{
        let mut acc: u64 = 0;
        $(acc = acc.wrapping_mul(0x100000001b3).wrapping_add(record(&$w));)+
        acc
    }};
}

// Per-N destructure helpers; tuples aren't variadic
macro_rules! pn {
    (5,  $base:expr) => {{
        let b = $base;
        let (a,b1,c,d,e) = pr::parallel_invoke_5(
            move || allocator_work(b.wrapping_add(0)),
            move || allocator_work(b.wrapping_add(1)),
            move || allocator_work(b.wrapping_add(2)),
            move || allocator_work(b.wrapping_add(3)),
            move || allocator_work(b.wrapping_add(4)),
        );
        combine!(a,b1,c,d,e)
    }};
    (6,  $base:expr) => {{
        let b = $base;
        let (a,b1,c,d,e,f) = pr::parallel_invoke_6(
            move || allocator_work(b.wrapping_add(0)),
            move || allocator_work(b.wrapping_add(1)),
            move || allocator_work(b.wrapping_add(2)),
            move || allocator_work(b.wrapping_add(3)),
            move || allocator_work(b.wrapping_add(4)),
            move || allocator_work(b.wrapping_add(5)),
        );
        combine!(a,b1,c,d,e,f)
    }};
    (7,  $base:expr) => {{
        let b = $base;
        let (a,b1,c,d,e,f,g) = pr::parallel_invoke_7(
            move || allocator_work(b.wrapping_add(0)),
            move || allocator_work(b.wrapping_add(1)),
            move || allocator_work(b.wrapping_add(2)),
            move || allocator_work(b.wrapping_add(3)),
            move || allocator_work(b.wrapping_add(4)),
            move || allocator_work(b.wrapping_add(5)),
            move || allocator_work(b.wrapping_add(6)),
        );
        combine!(a,b1,c,d,e,f,g)
    }};
    (8,  $base:expr) => {{
        let b = $base;
        let (a,b1,c,d,e,f,g,h) = pr::parallel_invoke_8(
            move || allocator_work(b.wrapping_add(0)),
            move || allocator_work(b.wrapping_add(1)),
            move || allocator_work(b.wrapping_add(2)),
            move || allocator_work(b.wrapping_add(3)),
            move || allocator_work(b.wrapping_add(4)),
            move || allocator_work(b.wrapping_add(5)),
            move || allocator_work(b.wrapping_add(6)),
            move || allocator_work(b.wrapping_add(7)),
        );
        combine!(a,b1,c,d,e,f,g,h)
    }};
    (9,  $base:expr) => {{
        let b = $base;
        let (a,b1,c,d,e,f,g,h,i) = pr::parallel_invoke_9(
            move || allocator_work(b.wrapping_add(0)),
            move || allocator_work(b.wrapping_add(1)),
            move || allocator_work(b.wrapping_add(2)),
            move || allocator_work(b.wrapping_add(3)),
            move || allocator_work(b.wrapping_add(4)),
            move || allocator_work(b.wrapping_add(5)),
            move || allocator_work(b.wrapping_add(6)),
            move || allocator_work(b.wrapping_add(7)),
            move || allocator_work(b.wrapping_add(8)),
        );
        combine!(a,b1,c,d,e,f,g,h,i)
    }};
    (10, $base:expr) => {{
        let b = $base;
        let (a,b1,c,d,e,f,g,h,i,j) = pr::parallel_invoke_10(
            move || allocator_work(b.wrapping_add(0)),
            move || allocator_work(b.wrapping_add(1)),
            move || allocator_work(b.wrapping_add(2)),
            move || allocator_work(b.wrapping_add(3)),
            move || allocator_work(b.wrapping_add(4)),
            move || allocator_work(b.wrapping_add(5)),
            move || allocator_work(b.wrapping_add(6)),
            move || allocator_work(b.wrapping_add(7)),
            move || allocator_work(b.wrapping_add(8)),
            move || allocator_work(b.wrapping_add(9)),
        );
        combine!(a,b1,c,d,e,f,g,h,i,j)
    }};
    (12, $base:expr) => {{
        let b = $base;
        let (a,b1,c,d,e,f,g,h,i,j,k,l) = pr::parallel_invoke_12(
            move || allocator_work(b.wrapping_add(0)),
            move || allocator_work(b.wrapping_add(1)),
            move || allocator_work(b.wrapping_add(2)),
            move || allocator_work(b.wrapping_add(3)),
            move || allocator_work(b.wrapping_add(4)),
            move || allocator_work(b.wrapping_add(5)),
            move || allocator_work(b.wrapping_add(6)),
            move || allocator_work(b.wrapping_add(7)),
            move || allocator_work(b.wrapping_add(8)),
            move || allocator_work(b.wrapping_add(9)),
            move || allocator_work(b.wrapping_add(10)),
            move || allocator_work(b.wrapping_add(11)),
        );
        combine!(a,b1,c,d,e,f,g,h,i,j,k,l)
    }};
    (16, $base:expr) => {{
        let b = $base;
        let (a,b1,c,d,e,f,g,h,i,j,k,l,m,n,o,p) = pr::parallel_invoke_16(
            move || allocator_work(b.wrapping_add(0)),
            move || allocator_work(b.wrapping_add(1)),
            move || allocator_work(b.wrapping_add(2)),
            move || allocator_work(b.wrapping_add(3)),
            move || allocator_work(b.wrapping_add(4)),
            move || allocator_work(b.wrapping_add(5)),
            move || allocator_work(b.wrapping_add(6)),
            move || allocator_work(b.wrapping_add(7)),
            move || allocator_work(b.wrapping_add(8)),
            move || allocator_work(b.wrapping_add(9)),
            move || allocator_work(b.wrapping_add(10)),
            move || allocator_work(b.wrapping_add(11)),
            move || allocator_work(b.wrapping_add(12)),
            move || allocator_work(b.wrapping_add(13)),
            move || allocator_work(b.wrapping_add(14)),
            move || allocator_work(b.wrapping_add(15)),
        );
        combine!(a,b1,c,d,e,f,g,h,i,j,k,l,m,n,o,p)
    }};
}

fn run_pair(label: &str, n: u32, base_seed: u64, par_checksum: u64) {
    let serial = serial_n(n, base_seed);
    if serial == par_checksum {
        println!("OK   N={n} ({label}): par=ser=0x{:016x}", par_checksum);
    } else {
        println!("FAIL N={n} ({label}): serial=0x{:016x} parallel=0x{:016x}", serial, par_checksum);
        std::process::exit(1);
    }
}

fn main() {
    const REPS: u64 = 8;
    let t0 = Instant::now();
    println!("alloc-stress: {} reps, ~{} alloc ops/call",
        REPS,
        PER_CALL_VEC_PUSHES + PER_CALL_HASH_INSERTS + PER_CALL_STRING_CONCATS,
    );

    for rep in 0..REPS {
        let base = 0x1000_0000u64.wrapping_mul(rep + 1);
        let p5  = pn!(5,  base);
        run_pair("rep", 5, base, p5);
        let p6  = pn!(6,  base);
        run_pair("rep", 6, base, p6);
        let p7  = pn!(7,  base);
        run_pair("rep", 7, base, p7);
        let p8  = pn!(8,  base);
        run_pair("rep", 8, base, p8);
        let p9  = pn!(9,  base);
        run_pair("rep", 9, base, p9);
        let p10 = pn!(10, base);
        run_pair("rep", 10, base, p10);
        let p12 = pn!(12, base);
        run_pair("rep", 12, base, p12);
        let p16 = pn!(16, base);
        run_pair("rep", 16, base, p16);

        let _ = black_box((p5, p6, p7, p8, p9, p10, p12, p16));
    }

    let total = TOTAL_CALLS.load(Ordering::Relaxed);
    let dt = t0.elapsed().as_secs_f64();
    println!("---");
    println!("TOTAL_CALLS={total} elapsed_s={dt:.2}");
    println!("OVERALL=PASS");
}
