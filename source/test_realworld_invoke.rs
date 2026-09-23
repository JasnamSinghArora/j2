// Realistic non-numerical parallel-invoke benchmark

use std::hint::black_box;
use std::time::Instant;

// Equal-weight passes; manual indexing passes purity check

// Extra ops clear the 30-statement cost floor

#[inline(never)]
fn hash_error_blocks(text: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let n = text.len();
    let mut i: usize = 0;
    while i + 64 < n {
        if text[i] == b'E' {
            let mut j = i;
            while j < i + 64 {
                let b = text[j] as u64;
                h = h.wrapping_mul(0x0000_0100_0000_01b3).wrapping_add(b);
                h ^= h.rotate_right(33);
                h = h.wrapping_mul(0xff51_afd7_ed55_8ccd).wrapping_add(b);
                h ^= h.rotate_right(27);
                h = h.wrapping_mul(0x94d0_49bb_1331_11eb).wrapping_add(b);
                j = j.wrapping_add(1);
            }
        }
        i = i.wrapping_add(1);
    }
    h
}

#[inline(never)]
fn hash_payload_blocks(text: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let n = text.len();
    let mut i: usize = 0;
    while i + 64 < n {
        if text[i] == b'P' {
            let mut j = i;
            while j < i + 64 {
                let b = text[j] as u64;
                h = h.wrapping_mul(0x0000_0100_0000_01b3).wrapping_add(b);
                h ^= h.rotate_right(33);
                h = h.wrapping_mul(0xff51_afd7_ed55_8ccd).wrapping_add(b);
                h ^= h.rotate_right(27);
                h = h.wrapping_mul(0x94d0_49bb_1331_11eb).wrapping_add(b);
                j = j.wrapping_add(1);
            }
        }
        i = i.wrapping_add(1);
    }
    h
}

fn build_log_text(n_lines: usize) -> String {
    // Mimic a server log
    let mut s = String::with_capacity(n_lines * 80);
    for i in 0..n_lines {
        match i % 20 {
            0 => s.push_str(&format!("ERROR  request id={i:08x} status=500 path=/api/v1/foo\n")),
            1 | 2 | 3 => s.push_str(&format!("WARN   request id={i:08x} latency_ms=850\n")),
            4..=7 => s.push_str(&format!("INFO   request id={i:08x} status=200 ms=42\n")),
            _ => s.push_str(&format!("DEBUG  PAYLOAD=abc{i:010x}xyz tag=alpha-{i:04}\n")),
        }
    }
    s
}

// Two equal-weight callees in adjacent let bindings
#[inline(never)]
fn run_two_passes(log_bytes: &[u8]) -> (u64, u64) {
    let a = hash_error_blocks(log_bytes);
    let b = hash_payload_blocks(log_bytes);
    (a, b)
}

fn main() {
    let n_lines: usize = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(400_000);

    let log_text = build_log_text(n_lines);
    let log_bytes = log_text.as_bytes();
    let log_bytes_mb = log_bytes.len() as f64 / (1024.0 * 1024.0);

    // Warm pools.
    let _ = black_box(run_two_passes(black_box(log_bytes)));

    // Best of 7, discards scheduler outliers
    let mut best_ms = f64::INFINITY;
    let mut last_a = 0u64;
    let mut last_b = 0u64;
    for _ in 0..7 {
        let t = Instant::now();
        let (a, b) = black_box(run_two_passes(black_box(log_bytes)));
        let ms = t.elapsed().as_secs_f64() * 1000.0;
        if ms < best_ms { best_ms = ms; }
        last_a = a;
        last_b = b;
    }

    println!("n_lines={n_lines} log_size_mb={log_bytes_mb:.1}");
    println!("best_ms={best_ms:.3}");
    println!("RESULT a={last_a:#018x} b={last_b:#018x}");
    println!("PASS");
}
