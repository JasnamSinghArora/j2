// Stress version of traffic_system

use std::time::Instant;

#[inline(always)]
fn force_fn<F: Fn(&&String) -> bool + Sync>(f: F) -> F { f }

#[inline(never)]
fn check_ambulance(plates: &[String], db: &[&'static str]) -> usize {
    let f = force_fn(|p: &&String| db.contains(&p.as_str()));
    plates.iter().filter(f).count()
}

fn main() {
    let n: usize = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(1_000_000);

    // Mimic an ANPR camera dump
    let db: Vec<&'static str> = vec![
        "PB10AB1234", "DL09CD5678", "MH04XY7890", "RJ14EF3456",
        "KA03GH5678", "TN07IJ6789", "UP16KL7890", "WB20MN8901",
        "CH01OP9012", "GJ05QR0123", "KA19P8488",
    ];
    let plates: Vec<String> = (0..n)
        .map(|i| {
            // Every 10000th plate is a real ambulance
            if i % 10_000 == 0 {
                db[i % db.len()].to_string()
            } else {
                format!("XX{:02}AB{:06}", i % 100, i)
            }
        })
        .collect();

    // Warm up the parallel pool
    let _ = check_ambulance(&plates[..1024.min(plates.len())], &db);

    // Hot timing: 5 runs, take the min.
    let mut best_ms = f64::INFINITY;
    let mut count_observed = 0usize;
    for _ in 0..5 {
        let t = Instant::now();
        let c = std::hint::black_box(check_ambulance(std::hint::black_box(&plates), &db));
        let ms = t.elapsed().as_secs_f64() * 1000.0;
        if ms < best_ms { best_ms = ms; count_observed = c; }
    }

    let ref_count = plates.iter().filter(|p| db.contains(&p.as_str())).count();
    assert_eq!(count_observed, ref_count, "correctness");

    println!("traffic_stress  N={n}  best_ms={best_ms:.3}  count={count_observed}");
    println!("PASS");
}
