// Slim matcher-only entry point

use crate::pass_manager::MirPass;
use rustc_middle::mir;
use rustc_middle::ty::TyCtxt;
use rustc_middle::ty::print::with_no_trimmed_paths;

pub(super) struct ParallelLowering;

impl<'tcx> MirPass<'tcx> for ParallelLowering {
    fn name(&self) -> &'static str {
        "ParallelLowering"
    }

    fn is_required(&self) -> bool {
        false
    }

    fn run_pass(&self, tcx: TyCtxt<'tcx>, body: &mut mir::Body<'tcx>) {
        // Log-only detection; no_trimmed_paths avoids diagnostic invariant
        with_no_trimmed_paths!({
            crate::parallel_idioms::detect_idioms(tcx, body);
            crate::parallel_idioms::try_transform_reductions(tcx, body);
            crate::parallel_idioms::try_transform_iterator_methods(tcx, body);
            // Combinators after iterator_methods; simpler matcher claims first
            crate::parallel_idioms::try_transform_iterator_combinators(tcx, body);
            // Phase A.1/D.1, for_each and iter_mut().for_each
            crate::parallel_idioms::try_transform_iterator_for_each(tcx, body);
            // Phase A.2, collect-to-Vec
            crate::parallel_idioms::try_transform_iterator_collect(tcx, body);
            // Phase C, HashMap iteration
            crate::parallel_idioms::try_transform_hashmap_for_each(tcx, body);
            // Generic-T fallback for any-type for_each / filter+count
            crate::parallel_idioms::try_transform_generic_iter_patterns(tcx, body);
            // N-way before 2-way, greedy longer chains
            crate::parallel_idioms::try_transform_parallel_invoke_n_way(tcx, body);
            crate::parallel_idioms::try_transform_parallel_invoke(tcx, body);
            // Multi-dim matchers, most specific first, matvec last
            crate::parallel_idioms::try_transform_slice_transpose_f64(tcx, body);
            crate::parallel_idioms::try_transform_slice_broadcast_add_f64(tcx, body);
            crate::parallel_idioms::try_transform_slice_axis_reduce_sum_f64(tcx, body);
            // conv1d, inner index i+k, runs before matvec
            crate::parallel_idioms::try_transform_slice_conv1d_f64(tcx, body);
            // cross-fn matvec; runs before in-place matvec
            crate::parallel_idioms::try_transform_outer_dot_call_matvec_f64(tcx, body);
            // matvec; must precede AXPY, consumes both loops
            crate::parallel_idioms::try_transform_slice_matvec_f64(tcx, body);
            // slice AXPY; runs late as residual matcher
            crate::parallel_idioms::try_transform_slice_axpy_f64(tcx, body);
            // zip-sub-write, dst[i] = a[i] - b[i]
            crate::parallel_idioms::try_transform_slice_zip_sub_write_f64(tcx, body);
            // zip-diff-sq-add, squared-distance accumulator (k-NN, k-means)
            crate::parallel_idioms::try_transform_slice_zip_diff_sq_add_f64(tcx, body);
            // elemwise write; after AXPY claims += shapes
            crate::parallel_idioms::try_transform_slice_elemwise_write_f64(tcx, body);
            // scale, y[i] *= alpha
            crate::parallel_idioms::try_transform_slice_scale_f64(tcx, body);
            // unary apply, dst[i] = src[i].METHOD()
            crate::parallel_idioms::try_transform_slice_unary_apply_f64(tcx, body);
            // binary apply, dst[i] = a[i].METHOD(b[i])
            crate::parallel_idioms::try_transform_slice_binary_apply_f64(tcx, body);
            // abs-diff; before elemwise, which would claim loop
            crate::parallel_idioms::try_transform_slice_abs_diff_f64(tcx, body);
            // clamp, dst[i] = a[i].clamp(lo, hi)
            crate::parallel_idioms::try_transform_slice_clamp_f64(tcx, body);
            // multi-AXPY; before FMA-write, which would misclaim it
            crate::parallel_idioms::try_transform_slice_multi_axpy_f64(tcx, body);
            // FMA-write, dst[i] = a[i]*b[i] + c[i]
            crate::parallel_idioms::try_transform_slice_fma_write_f64(tcx, body);
        });
    }
}
