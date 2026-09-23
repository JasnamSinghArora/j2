// parallel runtime dispatch latency microbench

#![feature(parallel_runtime_internal)]

use std::parallel_runtime as pr;
use std::time::Instant;

fn build_group(shape_id: usize, task_count: usize, ops_per_task: usize) -> usize {
    let max_par = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(8);
    let handle = pr::group_new_cached(shape_id, task_count, max_par);
    for t in 0..task_count {
        // two input slots, then ops_per_task outputs
        let slot_count = 2 + ops_per_task;
        pr::task_init_outputs4(handle, t, slot_count, 1, /*out0=*/ 1 + ops_per_task, 0, 0, 0);
        // chain of add then xor ops
        if ops_per_task >= 1 {
            pr::task_push_binary::<u64>(handle, t, 2, 0, 1, /*ADD*/ 0);
        }
        for i in 1..ops_per_task {
            pr::task_push_binary::<u64>(handle, t, 2 + i, 2 + i - 1, 1, /*XOR*/ 5);
        }
    }
    handle
}

fn run_one(label: &str, task_count: usize, ops_per_task: usize, iters: usize) {
    // Use a unique shape_id per
    let shape_id = 0xDEAD_0000 ^ ((task_count as usize) << 8) ^ (ops_per_task as usize);
    let handle = build_group(shape_id, task_count, ops_per_task);

    // Warmup
    for k in 0..256 {
        for t in 0..task_count {
            pr::group_set_input_raw(handle, t, 0, (k as usize).wrapping_add(t));
            pr::group_set_input_raw(handle, t, 1, (k as usize).wrapping_mul(0x9E37));
        }
        pr::group_execute(handle);
        for t in 0..task_count {
            let _v: u64 = pr::group_get_output_ready(handle, t, 0);
            std::hint::black_box(_v);
        }
    }

    // Measure
    let mut sink: u64 = 0;
    let start = Instant::now();
    for k in 0..iters {
        for t in 0..task_count {
            pr::group_set_input_raw(handle, t, 0, k.wrapping_add(t));
            pr::group_set_input_raw(handle, t, 1, k.wrapping_mul(0x9E37));
        }
        pr::group_execute(handle);
        for t in 0..task_count {
            let v: u64 = pr::group_get_output_ready(handle, t, 0);
            sink ^= v;
        }
    }
    let elapsed = start.elapsed();
    let ns_total = elapsed.as_nanos() as u64;
    let ns_per_dispatch = ns_total as f64 / iters as f64;
    let ns_per_op = ns_per_dispatch / (task_count * ops_per_task).max(1) as f64;
    println!(
        "{:<20} tasks={:>2} ops/task={:>3} iters={:>8}  ns/dispatch={:>9.1}  ns/op={:>6.2}  sink={:#x}",
        label, task_count, ops_per_task, iters, ns_per_dispatch, ns_per_op, sink
    );
    pr::group_free(handle);
}

fn main() {
    println!("=== parallel_runtime dispatch latency ===");
    println!("hardware parallelism: {}",
             std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1));
    println!();

    // sweep small (dispatch-bound) to large (compute-bound) tasks
    println!("-- small ops, varying task count (measures dispatch floor) --");
    for tc in [1usize, 2, 3, 4, 6, 8, 12, 16] {
        run_one("small", tc, 4, 1_000_000);
    }
    println!();
    println!("-- medium ops, varying task count --");
    for tc in [2usize, 4, 8, 16] {
        run_one("medium", tc, 16, 200_000);
    }
    println!();
    println!("-- large ops, varying task count (measures parallel speedup) --");
    // MAX_TASK_SLOTS = 32 in the runtime
    for tc in [1usize, 2, 4, 8, 16] {
        run_one("large", tc, 24, 100_000);
    }

    pr::parallel_runtime_stats_dump_final();
}
