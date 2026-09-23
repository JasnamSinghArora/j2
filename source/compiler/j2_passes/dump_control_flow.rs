//! CHANGED BY JAYITHI GAVVA: dumps CFG/PDT/CDG DOTs

use rustc_middle::mir::Body;
use rustc_middle::ty::TyCtxt;
use rustc_mir_dataflow::control_flow_analysis::{ControlFlowAnalysis, debug};
use std::fs;
use std::io::Write;
use std::path::PathBuf;

// CHANGED BY JAYITHI GAVVA: dump pass struct
pub(super) struct DumpControlFlow;

impl<'tcx> crate::MirPass<'tcx> for DumpControlFlow {
    // CHANGED BY JAYITHI GAVVA: debug-only, not required
    fn is_required(&self) -> bool {
        false
    }

    // CHANGED BY JAYITHI GAVVA: enablement check
    fn is_enabled(&self, sess: &rustc_session::Session) -> bool {
        // enabled by -Z dump-control-flow or DUMP_CONTROL_FLOW
        sess.opts.unstable_opts.dump_mir.is_some()
            || std::env::var("DUMP_CONTROL_FLOW").is_ok()
    }

    // CHANGED BY JAYITHI GAVVA: analyze and dump
    fn run_pass(&self, tcx: TyCtxt<'tcx>, body: &mut Body<'tcx>) {
        // CHANGED BY JAYITHI GAVVA: output fn name
        let def_id = body.source.def_id();
        let fn_name = tcx.def_path_str(def_id);
        let safe_fn_name: String = fn_name
            .chars()
            .map(|c| if c.is_alphanumeric() || c == '_' { c } else { '_' })
            .collect();

        // CHANGED BY JAYITHI GAVVA: Create output directory
        let output_dir = PathBuf::from(
            std::env::var("DUMP_CONTROL_FLOW_DIR")
                .unwrap_or_else(|_| "./control_flow_graphs".to_string())
        );
        let fn_output_dir = output_dir.join(&safe_fn_name);

        if let Err(e) = fs::create_dir_all(&fn_output_dir) {
            tracing::warn!("Failed to create output directory: {}", e);
            return;
        }

        // CHANGED BY JAYITHI GAVVA: run analysis
        let analysis = ControlFlowAnalysis::analyze(body);

        // CHANGED BY JAYITHI GAVVA: write CFG
        let cfg_path = fn_output_dir.join("cfg.dot");
        if let Ok(mut file) = fs::File::create(&cfg_path) {
            let dot = debug::cfg_to_dot(&analysis.cfg);
            if let Err(e) = file.write_all(dot.as_bytes()) {
                tracing::warn!("Failed to write CFG: {}", e);
            } else {
                tracing::info!("Wrote CFG to: {}", cfg_path.display());
            }
        }

        // CHANGED BY JAYITHI GAVVA: write PDT
        let pdt_path = fn_output_dir.join("pdt.dot");
        if let Ok(mut file) = fs::File::create(&pdt_path) {
            let dot = debug::pdt_to_dot(&analysis.post_dominator_tree, &analysis.cfg);
            if let Err(e) = file.write_all(dot.as_bytes()) {
                tracing::warn!("Failed to write PDT: {}", e);
            } else {
                tracing::info!("Wrote Post-Dominator Tree to: {}", pdt_path.display());
            }
        }

        // CHANGED BY JAYITHI GAVVA: write CDG
        let cdg_path = fn_output_dir.join("cdg.dot");
        if let Ok(mut file) = fs::File::create(&cdg_path) {
            let dot = debug::cdg_to_dot(&analysis.cdg, &analysis.cfg);
            if let Err(e) = file.write_all(dot.as_bytes()) {
                tracing::warn!("Failed to write CDG: {}", e);
            } else {
                tracing::info!("Wrote CDG to: {}", cdg_path.display());
            }
        }

        // CHANGED BY JAYITHI GAVVA: write combined CFG+CDG
        let combined_path = fn_output_dir.join("combined_cfg_cdg.dot");
        if let Ok(mut file) = fs::File::create(&combined_path) {
            let dot = debug::combined_cfg_cdg_to_dot(&analysis.cfg, &analysis.cdg);
            if let Err(e) = file.write_all(dot.as_bytes()) {
                tracing::warn!("Failed to write combined graph: {}", e);
            } else {
                tracing::info!("Wrote combined CFG+CDG to: {}", combined_path.display());
            }
        }

        // CHANGED BY JAYITHI GAVVA: Write human-readable summary
        let summary_path = fn_output_dir.join("summary.txt");
        if let Ok(mut file) = fs::File::create(&summary_path) {
            let mut summary = String::new();
            summary.push_str(&format!("Control Flow Analysis for: {}\n", fn_name));
            summary.push_str(&format!("========================================\n\n"));

            summary.push_str(&format!("Basic Blocks: {}\n", analysis.cfg.nodes().len() - 1)); // -1 for EXIT
            summary.push_str(&format!("CFG Edges: {}\n", analysis.cfg.edges().len()));
            summary.push_str(&format!("CDG Edges: {}\n\n", analysis.cdg.edges().len()));

            summary.push_str("CFG Edges:\n");
            for edge in analysis.cfg.edges() {
                summary.push_str(&format!("  {} -> {} [{:?}]\n", edge.from, edge.to, edge.kind));
            }

            summary.push_str("\nImmediate Post-Dominators:\n");
            for (node, ipdom) in analysis.post_dominator_tree.iter_ipostdom() {
                let ipdom_str = ipdom.map(|n| n.to_string()).unwrap_or_else(|| "None".to_string());
                summary.push_str(&format!("  ipdom({}) = {}\n", node, ipdom_str));
            }

            summary.push_str("\nControl Dependence Edges:\n");
            for edge in analysis.cdg.edges() {
                summary.push_str(&format!("  {} is control-dependent on {} [{:?}]\n",
                    edge.dependent, edge.controller, edge.label));
            }

            if let Err(e) = file.write_all(summary.as_bytes()) {
                tracing::warn!("Failed to write summary: {}", e);
            } else {
                tracing::info!("Wrote summary to: {}", summary_path.display());
            }
        }

        // CHANGED BY JAYITHI GAVVA: verbose console print
        if std::env::var("DUMP_CONTROL_FLOW_VERBOSE").is_ok() {
            println!("\n=== Control Flow Analysis for {} ===\n", fn_name);
            debug::print_analysis(&analysis);
        }

        println!("[JAYITHI GAVVA] Generated control flow graphs for '{}' in: {}",
            fn_name, fn_output_dir.display());
    }
}
