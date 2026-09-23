// FILE CREATED BY JASNAM

// This is an experimental matcher module
#![allow(dead_code, unused_variables, unused_assignments)]

use rustc_middle::mir;
use rustc_middle::ty::TyCtxt;
use std::collections::BTreeSet;
use std::sync::atomic::{AtomicU8, Ordering};

/// Developer diagnostic gate, on with `-Z par-dump`
#[inline]
fn par_dump_enabled<'tcx>(tcx: TyCtxt<'tcx>) -> bool {
    static CACHE: AtomicU8 = AtomicU8::new(0);
    match CACHE.load(Ordering::Relaxed) {
        1 => return false,
        2 => return true,
        _ => {}
    }
    let on = tcx.sess.opts.unstable_opts.par_dump;
    CACHE.store(if on { 2 } else { 1 }, Ordering::Relaxed);
    on
}

/// Gated eprintln; near-zero cost when off
macro_rules! par_dump {
    ($tcx:expr, $($arg:tt)*) => {
        if par_dump_enabled($tcx) {
            eprintln!($($arg)*);
        }
    };
}

/// Per-function findings sink so caller can batch
#[derive(Debug, Default)]
pub(crate) struct IdiomFindings {
    pub(crate) reductions: Vec<ReductionFinding>,
    pub(crate) privatizable: Vec<PrivatizableFinding>,
    pub(crate) inductions: Vec<InductionFinding>,
}

#[derive(Debug, Clone)]
pub(crate) struct ReductionFinding {
    pub(crate) loop_header: mir::BasicBlock,
    pub(crate) accumulator: mir::Local,
    pub(crate) op: mir::BinOp,
    /// Block holding the accumulator update statement
    pub(crate) update_block: mir::BasicBlock,
    pub(crate) update_index: usize,
}

#[derive(Debug, Clone)]
pub(crate) struct PrivatizableFinding {
    pub(crate) loop_header: mir::BasicBlock,
    pub(crate) local: mir::Local,
}

#[derive(Debug, Clone)]
pub(crate) struct InductionFinding {
    pub(crate) loop_header: mir::BasicBlock,
    pub(crate) iv_local: mir::Local,
    pub(crate) op: mir::BinOp,
}

/// cached env probe, 1 off 2 on
fn idiom_detect_enabled() -> bool {
    static CACHE: AtomicU8 = AtomicU8::new(0);
    match CACHE.load(Ordering::Relaxed) {
        1 => false,
        2 => true,
        _ => {
            let on = std::env::var("PARALLEL_IDIOMS_DETECT").ok().as_deref()
                == Some("1");
            CACHE.store(if on { 2 } else { 1 }, Ordering::Relaxed);
            on
        }
    }
}

/// Runs all analyses, prints and returns findings
pub(crate) fn detect_idioms<'tcx>(
    tcx: TyCtxt<'tcx>,
    body: &mir::Body<'tcx>,
) -> IdiomFindings {
    let mut findings = IdiomFindings::default();
    if !idiom_detect_enabled() {
        return findings;
    }

    let loops = find_simple_loops(body);
    if loops.is_empty() {
        return findings;
    }

    for lp in &loops {
        // reductions kept; map to `parallel_reduce_*`, measured 7x
        detect_reductions_in_loop(body, lp, &mut findings);
        detect_reductions_via_call(tcx, body, lp, &mut findings);

        // privatization/induction off; analysis-only, costs compile time
        let _ = detect_privatizable_in_loop;
        let _ = detect_induction_in_loop;
        let _ = detect_induction_via_call;
    }

    // diagnostic output; fn name from def_id
    let fn_name = tcx.def_path_str(body.source.def_id());
    for r in &findings.reductions {
        par_dump!(tcx, 
            "[PAR-IDIOM-REDUCE] fn={} loop_header=bb{} acc=_{} op={:?} update=bb{}:{}",
            fn_name,
            r.loop_header.index(),
            r.accumulator.index(),
            r.op,
            r.update_block.index(),
            r.update_index
        );
    }
    for p in &findings.privatizable {
        par_dump!(tcx, 
            "[PAR-IDIOM-PRIV] fn={} loop_header=bb{} local=_{}",
            fn_name,
            p.loop_header.index(),
            p.local.index()
        );
    }
    for ind in &findings.inductions {
        par_dump!(tcx, 
            "[PAR-IDIOM-INDV] fn={} loop_header=bb{} iv=_{} op={:?}",
            fn_name,
            ind.loop_header.index(),
            ind.iv_local.index(),
            ind.op
        );
    }

    findings
}

// ── Loop detection ───────────────────────────────────────────────────

/// Header plus confidently attributed body blocks
#[derive(Debug)]
pub(crate) struct SimpleLoop {
    pub(crate) header: mir::BasicBlock,
    pub(crate) body: BTreeSet<mir::BasicBlock>,
}

/// Find loops by scanning for back-edges
fn find_simple_loops<'tcx>(body: &mir::Body<'tcx>) -> Vec<SimpleLoop> {
    let mut loops: Vec<SimpleLoop> = Vec::new();
    let mut seen_headers: BTreeSet<mir::BasicBlock> = BTreeSet::new();

    for (bb_idx, bb) in body.basic_blocks.iter_enumerated() {
        let term = bb.terminator();
        for succ in term.successors() {
            if succ.index() >= bb_idx.index() {
                continue;
            }
            if !seen_headers.insert(succ) {
                continue;
            }
            // indices succ to latch; skips dominator tree
            let mut body_set: BTreeSet<mir::BasicBlock> = BTreeSet::new();
            for (b, _) in body.basic_blocks.iter_enumerated() {
                if b.index() >= succ.index() && b.index() <= bb_idx.index() {
                    body_set.insert(b);
                }
            }
            loops.push(SimpleLoop { header: succ, body: body_set });
        }
    }
    loops
}

// ── Reduction detection ──────────────────────────────────────────────

/// Associative+commutative ops the reduce primitives can split
fn is_associative_commutative(op: mir::BinOp) -> bool {
    matches!(
        op,
        mir::BinOp::Add
            | mir::BinOp::AddUnchecked
            | mir::BinOp::AddWithOverflow
            | mir::BinOp::Mul
            | mir::BinOp::MulUnchecked
            | mir::BinOp::MulWithOverflow
            | mir::BinOp::BitXor
            | mir::BinOp::BitAnd
            | mir::BinOp::BitOr
    )
}

/// Does operand read `local`? Constants never do
fn operand_reads_local<'tcx>(operand: &mir::Operand<'tcx>, local: mir::Local) -> bool {
    match operand {
        mir::Operand::Copy(place) | mir::Operand::Move(place) => {
            place.local == local && place.projection.is_empty()
        }
        // Constant and RuntimeChecks operands read no Locals
        _ => false,
    }
}

/// Map `wrapping_*` method callee to its BinOp
fn wrapping_method_to_binop<'tcx>(tcx: TyCtxt<'tcx>, def_id: rustc_hir::def_id::DefId) -> Option<mir::BinOp> {
    let path = tcx.def_path_str(def_id);
    // match on last `::Foo` segment only
    let last = path.rsplit("::").next()?;
    Some(match last {
        "wrapping_add" | "saturating_add" | "checked_add" | "overflowing_add" => mir::BinOp::Add,
        "wrapping_sub" | "saturating_sub" | "checked_sub" | "overflowing_sub" => mir::BinOp::Sub,
        "wrapping_mul" | "saturating_mul" | "checked_mul" | "overflowing_mul" => mir::BinOp::Mul,
        _ => return None,
    })
}

fn detect_reductions_in_loop<'tcx>(
    body: &mir::Body<'tcx>,
    lp: &SimpleLoop,
    findings: &mut IdiomFindings,
) {
    detect_reductions_via_binop(body, lp, findings);
    // Call-terminator variant needs tcx; see `detect_idioms`
}

fn detect_reductions_via_binop<'tcx>(
    body: &mir::Body<'tcx>,
    lp: &SimpleLoop,
    findings: &mut IdiomFindings,
) {
    for &bb in &lp.body {
        let block = &body.basic_blocks[bb];
        for (idx, stmt) in block.statements.iter().enumerate() {
            let mir::StatementKind::Assign(box (place, rvalue)) = &stmt.kind else {
                continue;
            };
            if !place.projection.is_empty() {
                continue;
            }
            let acc_local = place.local;

            let (op, lhs, rhs) = match rvalue {
                mir::Rvalue::BinaryOp(op, box (lhs, rhs)) => (*op, lhs, rhs),
                _ => continue,
            };
            if !is_associative_commutative(op) {
                continue;
            }
            let acc_on_lhs = operand_reads_local(lhs, acc_local);
            let acc_on_rhs = operand_reads_local(rhs, acc_local);
            if !(acc_on_lhs ^ acc_on_rhs) {
                continue;
            }
            let other = if acc_on_lhs { rhs } else { lhs };
            if operand_reads_local(other, acc_local) {
                continue;
            }
            findings.reductions.push(ReductionFinding {
                loop_header: lp.header,
                accumulator: acc_local,
                op,
                update_block: bb,
                update_index: idx,
            });
        }
    }
}

/// Detect reductions updated via `wrapping_*` calls
fn detect_reductions_via_call<'tcx>(
    tcx: TyCtxt<'tcx>,
    body: &mir::Body<'tcx>,
    lp: &SimpleLoop,
    findings: &mut IdiomFindings,
) {
    for &bb in &lp.body {
        let block = &body.basic_blocks[bb];
        let term = block.terminator();
        let mir::TerminatorKind::Call { func, args, destination, target, .. } =
            &term.kind
        else {
            continue;
        };
        if !destination.projection.is_empty() {
            continue;
        }
        let result_local = destination.local;

        // Resolve callee.
        let callee_const = match func {
            mir::Operand::Constant(c) => c,
            _ => continue,
        };
        let rustc_middle::ty::FnDef(def_id, _) = callee_const.const_.ty().kind() else {
            continue;
        };
        let Some(op) = wrapping_method_to_binop(tcx, *def_id) else {
            continue;
        };
        if !is_associative_commutative(op) {
            continue;
        }
        if args.len() != 2 {
            continue;
        }

        // step 2, successor writes `_acc` from result
        let Some(bb_post) = target else { continue };
        if !lp.body.contains(bb_post) {
            continue;
        }
        let post_block = &body.basic_blocks[*bb_post];
        let mut acc_local: Option<mir::Local> = None;
        for stmt in &post_block.statements {
            let mir::StatementKind::Assign(box (place, rvalue)) = &stmt.kind else {
                continue;
            };
            if !place.projection.is_empty() {
                continue;
            }
            let candidate = place.local;
            // `move _result` or `copy _result`.
            let reads_result = match rvalue {
                mir::Rvalue::Use(operand) => operand_reads_local(operand, result_local),
                _ => false,
            };
            if reads_result {
                acc_local = Some(candidate);
                break;
            }
            // another write to result_local first aborts
            if candidate == result_local {
                break;
            }
        }
        let Some(acc_local) = acc_local else { continue };
        if acc_local == result_local {
            continue;
        }

        // step 3, an arg temp loads `_acc`
        let mut arg_locals: Vec<mir::Local> = Vec::with_capacity(2);
        for a in args {
            match &a.node {
                mir::Operand::Move(p) | mir::Operand::Copy(p) if p.projection.is_empty() => {
                    arg_locals.push(p.local);
                }
                _ => {}
            }
        }
        let mut found = false;
        for stmt in block.statements.iter().rev() {
            let mir::StatementKind::Assign(box (place, rvalue)) = &stmt.kind else {
                continue;
            };
            if !place.projection.is_empty() {
                continue;
            }
            let lhs_local = place.local;
            if !arg_locals.contains(&lhs_local) {
                continue;
            }
            // RHS must read `_acc`.
            let reads_acc = match rvalue {
                mir::Rvalue::Use(operand) => operand_reads_local(operand, acc_local),
                _ => false,
            };
            if reads_acc {
                found = true;
                break;
            }
        }
        if !found {
            continue;
        }

        findings.reductions.push(ReductionFinding {
            loop_header: lp.header,
            accumulator: acc_local,
            op,
            update_block: bb,
            update_index: block.statements.len(), // marks "terminator"
        });
    }
}

// ── Privatizable detection ───────────────────────────────────────────

/// Privatizable Local, written before read each iteration
fn detect_privatizable_in_loop<'tcx>(
    body: &mir::Body<'tcx>,
    lp: &SimpleLoop,
    findings: &mut IdiomFindings,
) {
    use std::collections::BTreeMap;
    #[derive(Copy, Clone, PartialEq, Eq)]
    enum FirstUse {
        Write,
        Read,
    }
    let mut first_use: BTreeMap<mir::Local, FirstUse> = BTreeMap::new();

    let block_iter: Vec<_> = lp.body.iter().copied().collect();
    // collect events, fold after, avoids closure double-borrow
    let mut events: Vec<(mir::Local, FirstUse)> = Vec::new();
    for &bb in &block_iter {
        let block = &body.basic_blocks[bb];
        for stmt in &block.statements {
            if let mir::StatementKind::Assign(box (place, rvalue)) = &stmt.kind {
                // RHS reads precede LHS bind
                visit_rvalue_locals(rvalue, &mut |local| {
                    events.push((local, FirstUse::Read));
                });
                for elem in place.projection.iter() {
                    if let mir::ProjectionElem::Index(i) = elem {
                        events.push((i, FirstUse::Read));
                    }
                }
                // LHS write last.
                if place.projection.is_empty() {
                    events.push((place.local, FirstUse::Write));
                }
            }
        }
    }
    for (local, kind) in events {
        first_use.entry(local).or_insert(kind);
    }

    for (local, first) in first_use {
        if first == FirstUse::Write && !is_user_var_skip(body, local) {
            findings.privatizable.push(PrivatizableFinding {
                loop_header: lp.header,
                local,
            });
        }
    }
}

/// Skip iteration vars and compiler bookkeeping locals
fn is_user_var_skip<'tcx>(body: &mir::Body<'tcx>, local: mir::Local) -> bool {
    // return and arg locals are never loop-local
    local == mir::RETURN_PLACE || local.as_usize() <= body.arg_count
}

fn visit_rvalue_locals<'tcx, F: FnMut(mir::Local)>(
    rvalue: &mir::Rvalue<'tcx>,
    f: &mut F,
) {
    fn visit_op<'tcx, F: FnMut(mir::Local)>(operand: &mir::Operand<'tcx>, f: &mut F) {
        if let mir::Operand::Copy(p) | mir::Operand::Move(p) = operand {
            f(p.local);
        }
    }
    match rvalue {
        mir::Rvalue::Use(o) | mir::Rvalue::Repeat(o, _) | mir::Rvalue::UnaryOp(_, o) => {
            visit_op(o, f);
        }
        mir::Rvalue::BinaryOp(_, box (a, b)) => {
            visit_op(a, f);
            visit_op(b, f);
        }
        mir::Rvalue::Cast(_, o, _) => visit_op(o, f),
        mir::Rvalue::Ref(_, _, place) | mir::Rvalue::CopyForDeref(place) => f(place.local),
        mir::Rvalue::Aggregate(_, ops) => {
            for o in ops {
                visit_op(o, f);
            }
        }
        _ => {}
    }
}

// ── Induction detection ──────────────────────────────────────────────

/// Induction var updated only by constant Add
fn detect_induction_in_loop<'tcx>(
    body: &mir::Body<'tcx>,
    lp: &SimpleLoop,
    findings: &mut IdiomFindings,
) {
    use std::collections::BTreeMap;
    #[derive(Copy, Clone)]
    enum WriteKind {
        AffineAdd(mir::BinOp), // qualifies
        Other,                 // disqualifies
    }
    let mut writes: BTreeMap<mir::Local, WriteKind> = BTreeMap::new();

    for &bb in &lp.body {
        let block = &body.basic_blocks[bb];
        for stmt in &block.statements {
            let mir::StatementKind::Assign(box (place, rvalue)) = &stmt.kind else {
                continue;
            };
            if !place.projection.is_empty() {
                continue;
            }
            let local = place.local;

            let qualifies = match rvalue {
                mir::Rvalue::BinaryOp(op, box (lhs, rhs)) => {
                    if !matches!(op, mir::BinOp::Add | mir::BinOp::AddUnchecked) {
                        false
                    } else {
                        // either side is `local`, other a constant
                        let lhs_is_self = operand_reads_local(lhs, local);
                        let rhs_is_self = operand_reads_local(rhs, local);
                        let lhs_is_const = matches!(lhs, mir::Operand::Constant(_));
                        let rhs_is_const = matches!(rhs, mir::Operand::Constant(_));
                        (lhs_is_self && rhs_is_const) || (rhs_is_self && lhs_is_const)
                    }
                }
                _ => false,
            };

            // once disqualified, stays disqualified
            let entry: &mut WriteKind = writes.entry(local).or_insert(WriteKind::Other);
            if qualifies {
                if matches!(entry, WriteKind::Other) {
                    if let mir::Rvalue::BinaryOp(op, _) = rvalue {
                        *entry = WriteKind::AffineAdd(*op);
                    }
                }
                // already AffineAdd: stays AffineAdd
            } else {
                *entry = WriteKind::Other;
            }
        }
    }

    for (local, kind) in writes {
        if let WriteKind::AffineAdd(op) = kind {
            if !is_user_var_skip(body, local) {
                findings.inductions.push(InductionFinding {
                    loop_header: lp.header,
                    iv_local: local,
                    op,
                });
            }
        }
    }
}

/// Induction detection for the `wrapping_add` call shape
fn detect_induction_via_call<'tcx>(
    tcx: TyCtxt<'tcx>,
    body: &mir::Body<'tcx>,
    lp: &SimpleLoop,
    findings: &mut IdiomFindings,
) {
    use std::collections::BTreeMap;
    #[derive(Copy, Clone)]
    enum Status {
        Affine,
        Other,
    }
    let mut status: BTreeMap<mir::Local, Status> = BTreeMap::new();

    // pass 1, non-pattern write disqualifies the Local
    for &bb in &lp.body {
        let block = &body.basic_blocks[bb];
        for stmt in &block.statements {
            if let mir::StatementKind::Assign(box (place, _)) = &stmt.kind {
                if place.projection.is_empty() {
                    // default Other; pass 2 upgrades to Affine
                    status.entry(place.local).or_insert(Status::Other);
                }
            }
        }
    }

    // pass 2, wrapping_add writeback pattern marks Affine
    for &bb in &lp.body {
        let block = &body.basic_blocks[bb];
        let term = block.terminator();
        let mir::TerminatorKind::Call { func, args, destination, target, .. } =
            &term.kind
        else {
            continue;
        };
        if !destination.projection.is_empty() {
            continue;
        }
        let result_local = destination.local;

        let callee_const = match func {
            mir::Operand::Constant(c) => c,
            _ => continue,
        };
        let rustc_middle::ty::FnDef(def_id, _) = callee_const.const_.ty().kind() else {
            continue;
        };
        let op = match wrapping_method_to_binop(tcx, *def_id) {
            Some(mir::BinOp::Add) => mir::BinOp::Add,
            _ => continue,
        };
        if args.len() != 2 {
            continue;
        }

        let Some(bb_post) = target else { continue };
        if !lp.body.contains(bb_post) {
            continue;
        }
        let post_block = &body.basic_blocks[*bb_post];
        let mut iv_local: Option<mir::Local> = None;
        for stmt in &post_block.statements {
            let mir::StatementKind::Assign(box (place, rvalue)) = &stmt.kind else {
                continue;
            };
            if !place.projection.is_empty() {
                continue;
            }
            let candidate = place.local;
            let reads_result = match rvalue {
                mir::Rvalue::Use(operand) => operand_reads_local(operand, result_local),
                _ => false,
            };
            if reads_result {
                iv_local = Some(candidate);
                break;
            }
            if candidate == result_local {
                break;
            }
        }
        let Some(iv_local) = iv_local else { continue };

        // one arg from `_iv`, other a constant
        let mut arg_kinds: Vec<(mir::Local, bool /*is_const*/)> = Vec::new();
        for a in args {
            match &a.node {
                mir::Operand::Move(p) | mir::Operand::Copy(p) if p.projection.is_empty() => {
                    arg_kinds.push((p.local, false));
                }
                mir::Operand::Constant(_) => {
                    arg_kinds.push((mir::RETURN_PLACE, true));
                }
                _ => return,
            }
        }
        let one_const = arg_kinds.iter().filter(|(_, c)| *c).count() == 1;
        if !one_const {
            continue;
        }
        let temp_local = arg_kinds
            .iter()
            .find(|(_, c)| !*c)
            .map(|(l, _)| *l);
        let Some(temp_local) = temp_local else { continue };

        let mut found_iv_load = false;
        for stmt in block.statements.iter().rev() {
            let mir::StatementKind::Assign(box (place, rvalue)) = &stmt.kind else {
                continue;
            };
            if !place.projection.is_empty() {
                continue;
            }
            if place.local != temp_local {
                continue;
            }
            let reads_iv = match rvalue {
                mir::Rvalue::Use(operand) => operand_reads_local(operand, iv_local),
                _ => false,
            };
            if reads_iv {
                found_iv_load = true;
            }
            break;  // last write to temp_local
        }
        if !found_iv_load {
            continue;
        }

        // post-block move must be the only write
        let count_other_writes = body
            .basic_blocks
            .iter_enumerated()
            .filter(|(b, _)| lp.body.contains(b))
            .flat_map(|(_, blk)| blk.statements.iter())
            .filter(|stmt| {
                if let mir::StatementKind::Assign(box (p, _)) = &stmt.kind {
                    p.projection.is_empty() && p.local == iv_local
                } else {
                    false
                }
            })
            .count();
        // Exactly one write expected (the post-call move).
        if count_other_writes != 1 {
            continue;
        }
        status.insert(iv_local, Status::Affine);
        let _ = op;
    }

    for (local, kind) in status {
        if matches!(kind, Status::Affine) && !is_user_var_skip(body, local) {
            findings.inductions.push(InductionFinding {
                loop_header: lp.header,
                iv_local: local,
                op: mir::BinOp::Add,
            });
        }
    }
}

// REDUCTION TRANSFORMATION ───────────────────────────────────────── Goal

use rustc_middle::ty::{self};
use rustc_span::Symbol;

fn idiom_transform_enabled<'tcx>(tcx: TyCtxt<'tcx>) -> bool {
    // Never transform proc-macro crates
    use rustc_session::config::CrateType;
    if tcx.crate_types().contains(&CrateType::ProcMacro) {
        return false;
    }
    use rustc_session::config::ParallelizeMode;
    match tcx.sess.opts.cg.parallelize {
        // The matcher fires by default
        ParallelizeMode::On | ParallelizeMode::Auto => true,
        // kill-switch, stock codegen, no matchers; aids bisecting
        ParallelizeMode::Off => false,
    }
}

/// Sysroot gate by crate name, def_path unreliable
fn is_in_sysroot_crate<'tcx>(tcx: TyCtxt<'tcx>, did: rustc_hir::def_id::DefId) -> bool {
    let name = tcx.crate_name(did.krate);
    matches!(
        name.as_str(),
        "core"
            | "std"
            | "alloc"
            | "proc_macro"
            | "panic_unwind"
            | "panic_abort"
            | "compiler_builtins"
            | "test"
            | "unwind"
            | "addr2line"
            | "object"
            | "miniz_oxide"
            | "rustc_demangle"
            | "rustc_std_workspace_core"
            | "rustc_std_workspace_alloc"
            | "rustc_std_workspace_std"
            | "std_detect"
            | "backtrace"
            | "hashbrown"
            | "libc"
            | "cfg_if"
    )
}

/// Loop bound, u64 constant or runtime Local
#[derive(Debug, Clone)]
enum LoopBound {
    Const(u64),
    Local(mir::Local),
}

/// One supported accumulator type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AccTyTag {
    U32,
    U64,
    I32,
    I64,
    Usize,
    Isize,
    F64,
}

/// Supported reduction op, associative and commutative
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReduceOp {
    Add,
    Mul,
    BitAnd,
    BitOr,
    BitXor,
    Min,
    Max,
}

impl ReduceOp {
    /// Identity element bits; init must match it
    fn identity_bits(self, tag: AccTyTag) -> u128 {
        let width: u32 = match tag {
            AccTyTag::U32 | AccTyTag::I32 => 32,
            AccTyTag::U64
            | AccTyTag::I64
            | AccTyTag::Usize
            | AccTyTag::Isize
            | AccTyTag::F64 => 64,
        };
        let mask: u128 = if width == 128 { u128::MAX } else { (1u128 << width) - 1 };
        let signed = matches!(tag, AccTyTag::I32 | AccTyTag::I64 | AccTyTag::Isize);
        match self {
            ReduceOp::Add | ReduceOp::BitOr | ReduceOp::BitXor => 0,
            ReduceOp::Mul => 1 & mask,
            ReduceOp::BitAnd => mask, // all-ones bit pattern of the type
            // Min identity is T::MAX
            ReduceOp::Min => {
                if signed {
                    (1u128 << (width - 1)) - 1
                } else {
                    mask
                }
            }
            // Max identity is T::MIN
            ReduceOp::Max => {
                if signed { 1u128 << (width - 1) } else { 0 }
            }
        }
    }

    /// Diagnostic item of matching runtime reduce helper
    fn helper_diag_item(self, tag: AccTyTag) -> &'static str {
        match (self, tag) {
            (ReduceOp::Add, AccTyTag::U32) => "parallel_runtime_parallel_reduce_add_u32",
            (ReduceOp::Add, AccTyTag::U64) => "parallel_runtime_parallel_reduce_add_u64",
            (ReduceOp::Add, AccTyTag::I32) => "parallel_runtime_parallel_reduce_add_i32",
            (ReduceOp::Add, AccTyTag::I64) => "parallel_runtime_parallel_reduce_add_i64",
            (ReduceOp::Add, AccTyTag::Usize) => "parallel_runtime_parallel_reduce_add_usize",
            (ReduceOp::Add, AccTyTag::Isize) => "parallel_runtime_parallel_reduce_add_isize",
            (ReduceOp::Mul, AccTyTag::U32) => "parallel_runtime_parallel_reduce_mul_u32",
            (ReduceOp::Mul, AccTyTag::U64) => "parallel_runtime_parallel_reduce_mul_u64",
            (ReduceOp::Mul, AccTyTag::I32) => "parallel_runtime_parallel_reduce_mul_i32",
            (ReduceOp::Mul, AccTyTag::I64) => "parallel_runtime_parallel_reduce_mul_i64",
            (ReduceOp::Mul, AccTyTag::Usize) => "parallel_runtime_parallel_reduce_mul_usize",
            (ReduceOp::Mul, AccTyTag::Isize) => "parallel_runtime_parallel_reduce_mul_isize",
            (ReduceOp::BitAnd, AccTyTag::U32) => "parallel_runtime_parallel_reduce_and_u32",
            (ReduceOp::BitAnd, AccTyTag::U64) => "parallel_runtime_parallel_reduce_and_u64",
            (ReduceOp::BitAnd, AccTyTag::I32) => "parallel_runtime_parallel_reduce_and_i32",
            (ReduceOp::BitAnd, AccTyTag::I64) => "parallel_runtime_parallel_reduce_and_i64",
            (ReduceOp::BitAnd, AccTyTag::Usize) => "parallel_runtime_parallel_reduce_and_usize",
            (ReduceOp::BitAnd, AccTyTag::Isize) => "parallel_runtime_parallel_reduce_and_isize",
            (ReduceOp::BitOr, AccTyTag::U32) => "parallel_runtime_parallel_reduce_or_u32",
            (ReduceOp::BitOr, AccTyTag::U64) => "parallel_runtime_parallel_reduce_or_u64",
            (ReduceOp::BitOr, AccTyTag::I32) => "parallel_runtime_parallel_reduce_or_i32",
            (ReduceOp::BitOr, AccTyTag::I64) => "parallel_runtime_parallel_reduce_or_i64",
            (ReduceOp::BitOr, AccTyTag::Usize) => "parallel_runtime_parallel_reduce_or_usize",
            (ReduceOp::BitOr, AccTyTag::Isize) => "parallel_runtime_parallel_reduce_or_isize",
            (ReduceOp::BitXor, AccTyTag::U32) => "parallel_runtime_parallel_reduce_xor_u32",
            (ReduceOp::BitXor, AccTyTag::U64) => "parallel_runtime_parallel_reduce_xor_u64",
            (ReduceOp::BitXor, AccTyTag::I32) => "parallel_runtime_parallel_reduce_xor_i32",
            (ReduceOp::BitXor, AccTyTag::I64) => "parallel_runtime_parallel_reduce_xor_i64",
            (ReduceOp::BitXor, AccTyTag::Usize) => "parallel_runtime_parallel_reduce_xor_usize",
            (ReduceOp::BitXor, AccTyTag::Isize) => "parallel_runtime_parallel_reduce_xor_isize",
            (ReduceOp::Min, AccTyTag::U32) => "parallel_runtime_parallel_reduce_min_u32",
            (ReduceOp::Min, AccTyTag::U64) => "parallel_runtime_parallel_reduce_min_u64",
            (ReduceOp::Min, AccTyTag::I32) => "parallel_runtime_parallel_reduce_min_i32",
            (ReduceOp::Min, AccTyTag::I64) => "parallel_runtime_parallel_reduce_min_i64",
            (ReduceOp::Min, AccTyTag::Usize) => "parallel_runtime_parallel_reduce_min_usize",
            (ReduceOp::Min, AccTyTag::Isize) => "parallel_runtime_parallel_reduce_min_isize",
            (ReduceOp::Max, AccTyTag::U32) => "parallel_runtime_parallel_reduce_max_u32",
            (ReduceOp::Max, AccTyTag::U64) => "parallel_runtime_parallel_reduce_max_u64",
            (ReduceOp::Max, AccTyTag::I32) => "parallel_runtime_parallel_reduce_max_i32",
            (ReduceOp::Max, AccTyTag::I64) => "parallel_runtime_parallel_reduce_max_i64",
            (ReduceOp::Max, AccTyTag::Usize) => "parallel_runtime_parallel_reduce_max_usize",
            (ReduceOp::Max, AccTyTag::Isize) => "parallel_runtime_parallel_reduce_max_isize",
            // floats lack index-form helper; sentinel means no-op
            (_, AccTyTag::F64) => "",
        }
    }
}

impl AccTyTag {
    /// Native width in bits for this type
    fn width_bits(self) -> u32 {
        match self {
            AccTyTag::U32 | AccTyTag::I32 => 32,
            AccTyTag::U64
            | AccTyTag::I64
            | AccTyTag::Usize
            | AccTyTag::Isize
            | AccTyTag::F64 => 64,
        }
    }

    #[allow(rustc::usage_of_qualified_ty)]
    fn from_ty<'tcx>(t: ty::Ty<'tcx>) -> Option<Self> {
        match t.kind() {
            ty::Uint(ty::UintTy::U32) => Some(AccTyTag::U32),
            ty::Uint(ty::UintTy::U64) => Some(AccTyTag::U64),
            ty::Uint(ty::UintTy::Usize) => Some(AccTyTag::Usize),
            ty::Int(ty::IntTy::I32) => Some(AccTyTag::I32),
            ty::Int(ty::IntTy::I64) => Some(AccTyTag::I64),
            ty::Int(ty::IntTy::Isize) => Some(AccTyTag::Isize),
            ty::Float(ty::FloatTy::F64) => Some(AccTyTag::F64),
            _ => None,
        }
    }

    fn is_float(self) -> bool {
        matches!(self, AccTyTag::F64)
    }
}

#[allow(dead_code)]
#[derive(Debug, Clone)]
struct ReducePlan<'tcx> {
    entry_bb: mir::BasicBlock,
    exit_bb: mir::BasicBlock,
    acc_local: mir::Local,
    body_def_id: rustc_hir::def_id::DefId,
    body_args: ty::GenericArgsRef<'tcx>,
    end_bound: LoopBound,
    /// Accumulator MIR type; picks the runtime helper
    #[allow(rustc::usage_of_qualified_ty)]
    acc_ty: ty::Ty<'tcx>,
    acc_ty_tag: AccTyTag,
    /// Reduction operator that combines `acc` and `body(i)`
    reduce_op: ReduceOp,
    /// Iter Local's init value
    iter_start_bits: u128,
}

pub(crate) fn try_transform_reductions<'tcx>(
    tcx: TyCtxt<'tcx>,
    body: &mut mir::Body<'tcx>,
) -> usize {
    if !idiom_transform_enabled(tcx) {
        return 0;
    }
    // Never transform sysroot crate internals
    if is_in_sysroot_crate(tcx, body.source.def_id()) {
        return 0;
    }
    // pre-filter, tiny bodies cannot hold loop+reduction
    if body.basic_blocks.len() < 4 {
        return 0;
    }
    let fn_name = tcx.def_path_str(body.source.def_id());
    if fn_name.contains("parallel_reduce_") || fn_name.contains("parallel_runtime") {
        return 0;
    }
    // wrapper hides literal from `symbol-intern-string-literal` lint
    fn lookup(tcx: TyCtxt<'_>, name: &str) -> Option<rustc_hir::def_id::DefId> {
        tcx.get_diagnostic_item(Symbol::intern(name))
    }
    // u64 reduce primitive is cheapest existence check
    if lookup(tcx, "parallel_runtime_parallel_reduce_add_u64").is_none() {
        return 0;
    }
    let body_ref: &mir::Body<'tcx> = body;
    let plans: Vec<ReducePlan<'tcx>> = find_simple_loops(body_ref)
        .iter()
        .filter_map(|lp| analyze_loop_for_transform(tcx, body_ref, lp))
        .collect();
    for plan in &plans {
        let bound_desc = match plan.end_bound {
            LoopBound::Const(n) => format!("const {}", n),
            LoopBound::Local(l) => format!("_{}", l.index()),
        };
        par_dump!(tcx, 
            "[PAR-IDIOM-TRANSFORM-CANDIDATE] fn={} entry=bb{} exit=bb{} acc=_{} acc_ty={:?} op={:?} end={} body={}",
            fn_name,
            plan.entry_bb.index(),
            plan.exit_bb.index(),
            plan.acc_local.index(),
            plan.acc_ty_tag,
            plan.reduce_op,
            bound_desc,
            tcx.def_path_str(plan.body_def_id),
        );
    }

    let mut count = 0;
    for plan in &plans {
        // purity gate, impure MIR reorders effects
        let purity = check_body_purity(tcx, plan.body_def_id, body.source.def_id());
        if let Err(reason) = &purity {
            par_dump!(tcx, 
                "[PAR-IDIOM-TRANSFORM-IMPURE] fn={} body={} reason={}",
                fn_name,
                tcx.def_path_str(plan.body_def_id),
                reason
            );
            continue;
        }

        // overflow-checks gate, mirrors iter-method matcher
        if tcx.sess.overflow_checks()
            && matches!(plan.reduce_op, ReduceOp::Add | ReduceOp::Mul)
            && matches!(
                plan.acc_ty_tag,
                AccTyTag::U32 | AccTyTag::U64 | AccTyTag::I32 | AccTyTag::I64
                | AccTyTag::Usize | AccTyTag::Isize
            )
        {
            par_dump!(tcx,
                "[PAR-IDIOM-TRANSFORM-OVERFLOW-CHECKS-SKIP] fn={} op={:?} acc_ty={:?} (overflow-checks on)",
                fn_name, plan.reduce_op, plan.acc_ty_tag,
            );
            continue;
        }

        // pick typed helper for (op, acc type)
        let helper_name = plan.reduce_op.helper_diag_item(plan.acc_ty_tag);
        let Some(reduce_def_id) = lookup(tcx, helper_name) else {
            par_dump!(tcx, 
                "[PAR-IDIOM-TRANSFORM-NO-HELPER] fn={} op={:?} acc_ty={:?} helper={}",
                fn_name, plan.reduce_op, plan.acc_ty_tag, helper_name,
            );
            continue;
        };

        if apply_reduce_transform(tcx, body, plan, reduce_def_id) {
            par_dump!(tcx, 
                "[PAR-IDIOM-TRANSFORM-APPLIED] fn={} entry=bb{} op={:?} acc_ty={:?} body={}",
                fn_name,
                plan.entry_bb.index(),
                plan.reduce_op,
                plan.acc_ty_tag,
                tcx.def_path_str(plan.body_def_id),
            );
            count += 1;
        } else {
            par_dump!(tcx, 
                "[PAR-IDIOM-TRANSFORM-FAILED] fn={} entry=bb{}",
                fn_name,
                plan.entry_bb.index(),
            );
        }
    }

    // slice-loop matcher needs slice-aware primitive
    let slice_sum_def_id = lookup(tcx, "parallel_runtime_parallel_reduce_sum_slice_u64");
    if let Some(slice_sum_def_id) = slice_sum_def_id {
        // re-borrow immutably; mutable borrow ended
        let slice_plans: Vec<SliceReducePlan> = {
            let body_ref: &mir::Body<'tcx> = body;
            find_simple_loops(body_ref)
                .iter()
                .filter_map(|lp| analyze_slice_loop_for_transform(tcx, body_ref, lp))
                .collect()
        };
        for plan in &slice_plans {
            par_dump!(tcx, 
                "[PAR-IDIOM-TRANSFORM-SLICE-CANDIDATE] fn={} entry=bb{} exit=bb{} acc=_{} slice=_{}",
                fn_name,
                plan.entry_bb.index(),
                plan.exit_bb.index(),
                plan.acc_local.index(),
                plan.slice_local.index(),
            );
        }
        for plan in &slice_plans {
            if apply_slice_reduce_transform(tcx, body, plan, slice_sum_def_id) {
                par_dump!(tcx, 
                    "[PAR-IDIOM-TRANSFORM-SLICE-APPLIED] fn={} entry=bb{}",
                    fn_name, plan.entry_bb.index()
                );
                count += 1;
            } else {
                par_dump!(tcx, 
                    "[PAR-IDIOM-TRANSFORM-SLICE-FAILED] fn={} entry=bb{}",
                    fn_name, plan.entry_bb.index()
                );
            }
        }
    }

    count
}

// Phase 4, rewrite `.sum()`/`.product()` to slice helpers
pub(crate) fn try_transform_iterator_methods<'tcx>(
    tcx: TyCtxt<'tcx>,
    body: &mut mir::Body<'tcx>,
) -> usize {
    if !idiom_transform_enabled(tcx) {
        return 0;
    }
    // Never transform sysroot crate internals
    if is_in_sysroot_crate(tcx, body.source.def_id()) {
        return 0;
    }
    // pre-filter, needs a Call terminator
    if body.basic_blocks.len() < 2
        || !body.basic_blocks.iter().any(|bb| {
            matches!(bb.terminator().kind, mir::TerminatorKind::Call { .. })
        })
    {
        return 0;
    }
    let fn_name = tcx.def_path_str(body.source.def_id());
    if fn_name.contains("parallel_reduce_") || fn_name.contains("parallel_runtime") {
        return 0;
    }

    fn lookup(tcx: TyCtxt<'_>, name: &str) -> Option<rustc_hir::def_id::DefId> {
        tcx.get_diagnostic_item(Symbol::intern(name))
    }
    // u64 sum helper must exist in std
    if lookup(tcx, "parallel_runtime_parallel_reduce_sum_slice_u64").is_none() {
        return 0;
    }

    /// Helper diag-item for method plus element type
    fn slice_helper_for(method: &str, tag: AccTyTag) -> Option<&'static str> {
        let op = match method {
            "sum" => ReduceOp::Add,
            "product" => ReduceOp::Mul,
            _ => return None,
        };
        let base = op.helper_diag_item(tag);
        // Translate index-helper diag item into slice-helper one
        match (op, tag) {
            (ReduceOp::Add, AccTyTag::U32) => Some("parallel_runtime_parallel_reduce_sum_slice_u32"),
            (ReduceOp::Add, AccTyTag::U64) => Some("parallel_runtime_parallel_reduce_sum_slice_u64"),
            (ReduceOp::Add, AccTyTag::I32) => Some("parallel_runtime_parallel_reduce_sum_slice_i32"),
            (ReduceOp::Add, AccTyTag::I64) => Some("parallel_runtime_parallel_reduce_sum_slice_i64"),
            (ReduceOp::Add, AccTyTag::Usize) => Some("parallel_runtime_parallel_reduce_sum_slice_usize"),
            (ReduceOp::Add, AccTyTag::Isize) => Some("parallel_runtime_parallel_reduce_sum_slice_isize"),
            (ReduceOp::Mul, AccTyTag::U32) => Some("parallel_runtime_parallel_reduce_mul_slice_u32"),
            (ReduceOp::Mul, AccTyTag::U64) => Some("parallel_runtime_parallel_reduce_mul_slice_u64"),
            (ReduceOp::Mul, AccTyTag::I32) => Some("parallel_runtime_parallel_reduce_mul_slice_i32"),
            (ReduceOp::Mul, AccTyTag::I64) => Some("parallel_runtime_parallel_reduce_mul_slice_i64"),
            (ReduceOp::Mul, AccTyTag::Usize) => Some("parallel_runtime_parallel_reduce_mul_slice_usize"),
            (ReduceOp::Mul, AccTyTag::Isize) => Some("parallel_runtime_parallel_reduce_mul_slice_isize"),
            // floats opt-in only; gated at call site
            (ReduceOp::Add, AccTyTag::F64) => Some("parallel_runtime_parallel_reduce_sum_slice_f64"),
            (ReduceOp::Mul, AccTyTag::F64) => Some("parallel_runtime_parallel_reduce_mul_slice_f64"),
            _ => {
                let _ = base;
                None
            }
        }
    }

    struct IterMethodPlan<'tcx> {
        bb_iter: mir::BasicBlock,
        slice_arg: mir::Operand<'tcx>,
        method_dest: mir::Place<'tcx>,
        after_method_bb: mir::BasicBlock,
        method: &'static str,
        elem_tag: AccTyTag,
    }

    let mut plans: Vec<IterMethodPlan<'tcx>> = Vec::new();

    let bbs: Vec<(mir::BasicBlock, mir::Terminator<'tcx>)> = body
        .basic_blocks
        .iter_enumerated()
        .map(|(b, d)| (b, d.terminator().clone()))
        .collect();

    for (bb_iter, term) in &bbs {
        let mir::TerminatorKind::Call {
            func: iter_func,
            args: iter_args,
            destination: iter_dest,
            target: Some(iter_target_bb),
            ..
        } = &term.kind
        else {
            continue;
        };
        if !iter_dest.projection.is_empty() {
            continue;
        }
        let mir::Operand::Constant(iter_c) = iter_func else { continue };
        let ty::FnDef(iter_def_id, _) = iter_c.const_.ty().kind() else { continue };
        let iter_path = tcx.def_path_str(*iter_def_id);
        let last_seg = iter_path.rsplit("::").next().unwrap_or("");
        if last_seg != "iter" || !iter_path.contains("slice") {
            continue;
        }
        if iter_args.len() != 1 {
            continue;
        }
        let slice_arg = iter_args[0].node.clone();
        let slice_arg_ty = match &slice_arg {
            mir::Operand::Copy(p) | mir::Operand::Move(p) => p.ty(&body.local_decls, tcx).ty,
            mir::Operand::Constant(c) => c.const_.ty(),
            _ => continue,
        };
        // slice element type, supported integers only
        let elem_tag = match slice_arg_ty.kind() {
            ty::Ref(_, inner, _) => match inner.kind() {
                ty::Slice(elem) => match AccTyTag::from_ty(*elem) {
                    Some(t) => t,
                    None => continue,
                },
                _ => continue,
            },
            _ => continue,
        };

        // successor must call an Iterator method
        let target_idx = iter_target_bb.as_usize();
        if target_idx >= bbs.len() {
            continue;
        }
        let target_term = &bbs[target_idx].1;
        let mir::TerminatorKind::Call {
            func: m_func,
            args: m_args,
            destination: m_dest,
            target: Some(after_m_bb),
            ..
        } = &target_term.kind
        else {
            continue;
        };
        let mir::Operand::Constant(m_c) = m_func else { continue };
        let ty::FnDef(m_def_id, _) = m_c.const_.ty().kind() else { continue };
        let m_path = tcx.def_path_str(*m_def_id);
        let method = match m_path.rsplit("::").next().unwrap_or("") {
            "sum" => "sum",
            "product" => "product",
            // .count() is a special case
            "count" => "count",
            _ => continue,
        };
        if m_args.len() != 1 {
            continue;
        }
        let m_arg_ok = match &m_args[0].node {
            mir::Operand::Copy(p) | mir::Operand::Move(p) => {
                p.projection.is_empty() && p.local == iter_dest.local
            }
            _ => false,
        };
        if !m_arg_ok {
            continue;
        }

        plans.push(IterMethodPlan {
            bb_iter: *bb_iter,
            slice_arg,
            method_dest: *m_dest,
            after_method_bb: *after_m_bb,
            method,
            elem_tag,
        });
    }

    let mut count = 0usize;
    for plan in plans {
        // `.count()` becomes `Len` rvalue, no helper needed
        if plan.method == "count" {
            // The slice arg is `&[T]`
            let len_rvalue = mir::Rvalue::UnaryOp(
                mir::UnOp::PtrMetadata,
                plan.slice_arg.clone(),
            );
            let len_stmt = mir::Statement::new(
                mir::SourceInfo::outermost(rustc_span::DUMMY_SP),
                mir::StatementKind::Assign(Box::new((plan.method_dest, len_rvalue))),
            );
            // append Len stmt; terminator becomes goto bb_after
            let bb_data = &mut body.basic_blocks_mut()[plan.bb_iter];
            bb_data.statements.push(len_stmt);
            bb_data.terminator_mut().kind = mir::TerminatorKind::Goto {
                target: plan.after_method_bb,
            };
            par_dump!(tcx, 
                "[PAR-IDIOM-ITER-COUNT-APPLIED] fn={} bb_iter=bb{} elem_ty={:?}",
                fn_name,
                plan.bb_iter.index(),
                plan.elem_tag,
            );
            count += 1;
            continue;
        }

        let Some(helper_name) = slice_helper_for(plan.method, plan.elem_tag) else {
            continue;
        };
        // skip sum/product under overflow checks; helper wraps
        if tcx.sess.overflow_checks()
            && (plan.method == "sum" || plan.method == "product")
            && matches!(
                plan.elem_tag,
                AccTyTag::U32 | AccTyTag::U64 | AccTyTag::I32 | AccTyTag::I64
                | AccTyTag::Usize | AccTyTag::Isize
            )
        {
            par_dump!(tcx,
                "[PAR-IDIOM-ITER-OVERFLOW-CHECKS-SKIP] fn={} method={} elem_ty={:?} (overflow-checks on; would replace panic-on-overflow with wrapping)",
                fn_name, plan.method, plan.elem_tag,
            );
            continue;
        }
        // Float reductions are non-associative
        if plan.elem_tag.is_float() {
            let mut attrs = tcx.get_attrs(body.source.def_id(), rustc_span::sym::parallelize_associative_float);
            if attrs.next().is_none() {
                par_dump!(tcx,
                    "[PAR-IDIOM-ITER-FLOAT-NO-OPTIN] fn={} method={} elem_ty={:?} (add #[parallelize_associative_float] to opt in)",
                    fn_name, plan.method, plan.elem_tag,
                );
                continue;
            }
            // skip plain float sum/product, helper defeats auto-vectorisation
            if matches!(plan.method, "sum" | "product") {
                par_dump!(tcx,
                    "[PAR-IDIOM-ITER-FLOAT-PLAIN-SKIP] fn={} method={} elem_ty={:?} (plain float sum/product is memory-bound; helper de-optimises auto-vectorisation)",
                    fn_name, plan.method, plan.elem_tag,
                );
                continue;
            }
        }
        let Some(helper_def_id) = lookup(tcx, helper_name) else {
            par_dump!(tcx, 
                "[PAR-IDIOM-ITER-NO-HELPER] fn={} method={} elem_ty={:?} helper={}",
                fn_name, plan.method, plan.elem_tag, helper_name,
            );
            continue;
        };

        par_dump!(tcx, 
            "[PAR-IDIOM-ITER-CANDIDATE] fn={} bb_iter=bb{} method=.{}() elem_ty={:?} after=bb{}",
            fn_name,
            plan.bb_iter.index(),
            plan.method,
            plan.elem_tag,
            plan.after_method_bb.index(),
        );

        let helper_callee = mir::Operand::function_handle(
            tcx,
            helper_def_id,
            std::iter::empty(),
            rustc_span::DUMMY_SP,
        );
        let new_args: Box<[rustc_span::source_map::Spanned<mir::Operand<'tcx>>]> =
            Box::new([rustc_span::source_map::Spanned {
                node: plan.slice_arg.clone(),
                span: rustc_span::DUMMY_SP,
            }]);
        let new_term_kind = mir::TerminatorKind::Call {
            func: helper_callee,
            args: new_args,
            destination: plan.method_dest,
            target: Some(plan.after_method_bb),
            unwind: mir::UnwindAction::Unreachable,
            call_source: mir::CallSource::Misc,
            fn_span: rustc_span::DUMMY_SP,
        };
        body.basic_blocks_mut()[plan.bb_iter].terminator_mut().kind = new_term_kind;
        par_dump!(tcx, 
            "[PAR-IDIOM-ITER-APPLIED] fn={} bb_iter=bb{} method=.{}() elem_ty={:?}",
            fn_name,
            plan.bb_iter.index(),
            plan.method,
            plan.elem_tag,
        );
        count += 1;
    }

    count += try_transform_range_iterator_methods(tcx, body, &fn_name);
    count += try_transform_map_iterator_methods(tcx, body, &fn_name);

    count
}

/// Recognises the 3-block chain bb_iter
fn try_transform_map_iterator_methods<'tcx>(
    tcx: TyCtxt<'tcx>,
    body: &mut mir::Body<'tcx>,
    fn_name: &str,
) -> usize {
    fn lookup(tcx: TyCtxt<'_>, name: &str) -> Option<rustc_hir::def_id::DefId> {
        tcx.get_diagnostic_item(Symbol::intern(name))
    }
    fn map_helper_for(method: &str, tag: AccTyTag) -> Option<&'static str> {
        Some(match (method, tag) {
            ("sum", AccTyTag::U32) => "parallel_runtime_parallel_reduce_sum_slice_map_u32",
            ("sum", AccTyTag::U64) => "parallel_runtime_parallel_reduce_sum_slice_map_u64",
            ("sum", AccTyTag::I32) => "parallel_runtime_parallel_reduce_sum_slice_map_i32",
            ("sum", AccTyTag::I64) => "parallel_runtime_parallel_reduce_sum_slice_map_i64",
            ("sum", AccTyTag::Usize) => "parallel_runtime_parallel_reduce_sum_slice_map_usize",
            ("sum", AccTyTag::Isize) => "parallel_runtime_parallel_reduce_sum_slice_map_isize",
            ("product", AccTyTag::U32) => "parallel_runtime_parallel_reduce_mul_slice_map_u32",
            ("product", AccTyTag::U64) => "parallel_runtime_parallel_reduce_mul_slice_map_u64",
            ("product", AccTyTag::I32) => "parallel_runtime_parallel_reduce_mul_slice_map_i32",
            ("product", AccTyTag::I64) => "parallel_runtime_parallel_reduce_mul_slice_map_i64",
            _ => return None,
        })
    }

    struct MapPlan<'tcx> {
        bb_iter: mir::BasicBlock,
        slice_arg: mir::Operand<'tcx>,
        body_operand: mir::Operand<'tcx>,
        red_dest: mir::Place<'tcx>,
        after_red_bb: mir::BasicBlock,
        method: &'static str,
        elem_tag: AccTyTag,
        /// True if `body_operand` is a Closure
        body_is_closure: bool,
        body_is_closure_with_captures: bool,
        /// bb_map pre-terminator stmts, relocated to bb_iter tail
        map_bb_stmts: Vec<mir::Statement<'tcx>>,
    }

    let mut plans: Vec<MapPlan<'tcx>> = Vec::new();

    let bbs: Vec<(mir::BasicBlock, mir::Terminator<'tcx>)> = body
        .basic_blocks
        .iter_enumerated()
        .map(|(b, d)| (b, d.terminator().clone()))
        .collect();

    for (bb_iter, term) in &bbs {
        // 1. bb_iter: Call to slice::iter, taking &[T].
        let mir::TerminatorKind::Call {
            func: iter_func,
            args: iter_args,
            destination: iter_dest,
            target: Some(map_bb),
            ..
        } = &term.kind
        else {
            continue;
        };
        if !iter_dest.projection.is_empty() {
            continue;
        }
        let mir::Operand::Constant(iter_c) = iter_func else { continue };
        let ty::FnDef(iter_def_id, _) = iter_c.const_.ty().kind() else { continue };
        let iter_path = tcx.def_path_str(*iter_def_id);
        if iter_path.rsplit("::").next() != Some("iter") || !iter_path.contains("slice") {
            continue;
        }
        if iter_args.len() != 1 {
            continue;
        }
        let slice_arg = iter_args[0].node.clone();
        let slice_arg_ty = match &slice_arg {
            mir::Operand::Copy(p) | mir::Operand::Move(p) => p.ty(&body.local_decls, tcx).ty,
            mir::Operand::Constant(c) => c.const_.ty(),
            _ => continue,
        };
        let elem_tag = match slice_arg_ty.kind() {
            ty::Ref(_, inner, _) => match inner.kind() {
                ty::Slice(elem) => match AccTyTag::from_ty(*elem) {
                    Some(t) => t,
                    None => continue,
                },
                _ => continue,
            },
            _ => continue,
        };

        // bb_map calls Iterator::map(iter_dest, body_fn)
        let map_idx = map_bb.as_usize();
        if map_idx >= bbs.len() {
            continue;
        }
        let map_term = &bbs[map_idx].1;
        let mir::TerminatorKind::Call {
            func: map_func,
            args: map_args,
            destination: map_dest,
            target: Some(red_bb),
            ..
        } = &map_term.kind
        else {
            continue;
        };
        if !map_dest.projection.is_empty() {
            continue;
        }
        let mir::Operand::Constant(map_c) = map_func else { continue };
        let ty::FnDef(map_def_id, _) = map_c.const_.ty().kind() else { continue };
        let map_path = tcx.def_path_str(*map_def_id);
        if map_path.rsplit("::").next() != Some("map") {
            continue;
        }
        if map_args.len() != 2 {
            continue;
        }
        // arg0 is iter, arg1 body fn
        let map_iter_arg_ok = match &map_args[0].node {
            mir::Operand::Copy(p) | mir::Operand::Move(p) => {
                p.projection.is_empty() && p.local == iter_dest.local
            }
            _ => false,
        };
        if !map_iter_arg_ok {
            continue;
        }
        let body_operand = map_args[1].node.clone();
        // Resolve the body type
        let body_ty = match &body_operand {
            mir::Operand::Constant(c) => c.const_.ty(),
            mir::Operand::Copy(p) | mir::Operand::Move(p) => {
                if !p.projection.is_empty() {
                    continue;
                }
                body.local_decls[p.local].ty
            }
            _ => continue,
        };
        // Resolve the body's effective signature
        let (body_is_closure, body_is_closure_with_captures, sig_inputs_owned, sig_output) =
            match body_ty.kind() {
                ty::FnDef(_, _) => {
                    let s = body_ty.fn_sig(tcx);
                    let s = tcx.instantiate_bound_regions_with_erased(s);
                    (false, false, s.inputs().to_vec(), s.output())
                }
                ty::Closure(_, closure_args) => {
                    let cargs = closure_args.as_closure();
                    let has_captures = !cargs.upvar_tys().is_empty();
                    // reject FnMut captures, dyn Fn coercion fails
                    if has_captures {
                        let kind_ty = cargs.kind_ty();
                        let is_fn_kind = matches!(kind_ty.kind(), ty::Int(rustc_middle::ty::IntTy::I8));
                        if !is_fn_kind {
                            par_dump!(tcx, 
                                "[PAR-IDIOM-MAP-CLOSURE-NOT-FN] fn={} closure_kind_ty={:?} reason=closure-with-captures must impl Fn (typically true when the closure is bound to a F: Fn parameter; .iter().map(|...|) infers FnMut and is rejected as unsound to share across threads)",
                                fn_name, kind_ty,
                            );
                            continue;
                        }
                    }
                    let s = cargs.sig();
                    let s = tcx.instantiate_bound_regions_with_erased(s);
                    let raw_inputs = s.inputs();
                    if raw_inputs.len() != 1 {
                        continue;
                    }
                    let unpacked: Vec<_> = match raw_inputs[0].kind() {
                        ty::Tuple(elems) => elems.iter().collect(),
                        _ => continue,
                    };
                    (true, has_captures, unpacked, s.output())
                }
                _ => continue,
            };
        if sig_inputs_owned.len() != 1 {
            continue;
        }
        // input must be &T matching elem_tag
        let in_ty = sig_inputs_owned[0];
        let in_inner_ty = match in_ty.kind() {
            ty::Ref(_, inner, _) => *inner,
            _ => continue,
        };
        let in_tag = AccTyTag::from_ty(in_inner_ty);
        if in_tag != Some(elem_tag) {
            continue;
        }
        // Output must be T (same as elem_tag).
        let out_tag = AccTyTag::from_ty(sig_output);
        if out_tag != Some(elem_tag) {
            continue;
        }

        // 3. red_bb: Call to Iterator::<sum|product>(map_dest).
        let red_idx = red_bb.as_usize();
        if red_idx >= bbs.len() {
            continue;
        }
        let red_term = &bbs[red_idx].1;
        let mir::TerminatorKind::Call {
            func: r_func,
            args: r_args,
            destination: r_dest,
            target: Some(after_red_bb),
            ..
        } = &red_term.kind
        else {
            continue;
        };
        let mir::Operand::Constant(r_c) = r_func else { continue };
        let ty::FnDef(r_def_id, _) = r_c.const_.ty().kind() else { continue };
        let r_path = tcx.def_path_str(*r_def_id);
        let method = match r_path.rsplit("::").next().unwrap_or("") {
            "sum" => "sum",
            "product" => "product",
            _ => continue,
        };
        if r_args.len() != 1 {
            continue;
        }
        let r_arg_ok = match &r_args[0].node {
            mir::Operand::Copy(p) | mir::Operand::Move(p) => {
                p.projection.is_empty() && p.local == map_dest.local
            }
            _ => false,
        };
        if !r_arg_ok {
            continue;
        }
        // capture bb_map stmts, closure aggregate built here
        let map_bb_stmts: Vec<mir::Statement<'tcx>> =
            body.basic_blocks[*map_bb].statements.clone();
        plans.push(MapPlan {
            bb_iter: *bb_iter,
            slice_arg,
            body_operand,
            red_dest: *r_dest,
            after_red_bb: *after_red_bb,
            method,
            elem_tag,
            body_is_closure,
            body_is_closure_with_captures,
            map_bb_stmts,
        });
    }

    let mut count = 0usize;
    for plan in plans {
        // capturing closures use dyn-Fn helper, sum/u64 only
        if plan.body_is_closure_with_captures {
            // (method, T) → dyn-Fn closure helper
            let closure_helper_name = match (plan.method, plan.elem_tag) {
                ("sum", AccTyTag::U64) =>
                    "parallel_runtime_parallel_reduce_sum_slice_map_closure_dyn_u64",
                ("sum", AccTyTag::U32) =>
                    "parallel_runtime_parallel_reduce_sum_slice_map_closure_dyn_u32",
                ("sum", AccTyTag::I64) =>
                    "parallel_runtime_parallel_reduce_sum_slice_map_closure_dyn_i64",
                ("sum", AccTyTag::I32) =>
                    "parallel_runtime_parallel_reduce_sum_slice_map_closure_dyn_i32",
                ("product", AccTyTag::U64) =>
                    "parallel_runtime_parallel_reduce_mul_slice_map_closure_dyn_u64",
                ("product", AccTyTag::U32) =>
                    "parallel_runtime_parallel_reduce_mul_slice_map_closure_dyn_u32",
                ("product", AccTyTag::I64) =>
                    "parallel_runtime_parallel_reduce_mul_slice_map_closure_dyn_i64",
                ("product", AccTyTag::I32) =>
                    "parallel_runtime_parallel_reduce_mul_slice_map_closure_dyn_i32",
                _ => {
                    par_dump!(tcx, 
                        "[PAR-IDIOM-MAP-CLOSURE-UNSUPPORTED] fn={} method=.{}() elem_ty={:?}",
                        fn_name, plan.method, plan.elem_tag,
                    );
                    continue;
                }
            };
            let Some(helper_def_id) = lookup(tcx, closure_helper_name) else {
                par_dump!(tcx, 
                    "[PAR-IDIOM-MAP-NO-HELPER] fn={} dyn closure helper missing",
                    fn_name,
                );
                continue;
            };
            // We need the closure as a Place
            let closure_place = match &plan.body_operand {
                mir::Operand::Move(p) | mir::Operand::Copy(p) if p.projection.is_empty() => *p,
                _ => continue,
            };
            // pull dyn ref type from helper sig
            let helper_sig = tcx.fn_sig(helper_def_id).instantiate_identity();
            let helper_sig = tcx.instantiate_bound_regions_with_erased(helper_sig);
            if helper_sig.inputs().len() != 2 {
                continue;
            }
            // erase regions, BrNamed confuses CoerceUnsized validation
            let dyn_ref_ty = tcx.erase_and_anonymize_regions(helper_sig.inputs()[1]);
            // two locals: &Closure ref, unsized &dyn Fn
            #[allow(rustc::usage_of_qualified_ty)]
            let closure_ref_ty = ty::Ty::new_imm_ref(
                tcx,
                tcx.lifetimes.re_erased,
                body.local_decls[closure_place.local].ty,
            );
            let ref_local = body.local_decls.push(mir::LocalDecl::new(
                closure_ref_ty,
                rustc_span::DUMMY_SP,
            ));
            let dyn_local = body.local_decls.push(mir::LocalDecl::new(
                dyn_ref_ty,
                rustc_span::DUMMY_SP,
            ));
            // ref_local = &(*&_closure_local) - actually just &_closure_local
            let ref_rvalue = mir::Rvalue::Ref(
                tcx.lifetimes.re_erased,
                mir::BorrowKind::Shared,
                closure_place,
            );
            let ref_stmt = mir::Statement::new(
                mir::SourceInfo::outermost(rustc_span::DUMMY_SP),
                mir::StatementKind::Assign(Box::new((
                    mir::Place::from(ref_local),
                    ref_rvalue,
                ))),
            );
            // unsize cast; Copy operand mirrors user-written shape
            let unsize_rvalue = mir::Rvalue::Cast(
                mir::CastKind::PointerCoercion(
                    ty::adjustment::PointerCoercion::Unsize,
                    mir::CoercionSource::Implicit,
                ),
                mir::Operand::Copy(mir::Place::from(ref_local)),
                dyn_ref_ty,
            );
            let unsize_stmt = mir::Statement::new(
                mir::SourceInfo::outermost(rustc_span::DUMMY_SP),
                mir::StatementKind::Assign(Box::new((
                    mir::Place::from(dyn_local),
                    unsize_rvalue,
                ))),
            );
            let helper_callee = mir::Operand::function_handle(
                tcx,
                helper_def_id,
                std::iter::empty(),
                rustc_span::DUMMY_SP,
            );
            let new_args: Box<[rustc_span::source_map::Spanned<mir::Operand<'tcx>>]> = Box::new([
                rustc_span::source_map::Spanned {
                    node: plan.slice_arg.clone(),
                    span: rustc_span::DUMMY_SP,
                },
                rustc_span::source_map::Spanned {
                    node: mir::Operand::Move(mir::Place::from(dyn_local)),
                    span: rustc_span::DUMMY_SP,
                },
            ]);
            let new_term_kind = mir::TerminatorKind::Call {
                func: helper_callee,
                args: new_args,
                destination: plan.red_dest,
                target: Some(plan.after_red_bb),
                unwind: mir::UnwindAction::Unreachable,
                call_source: mir::CallSource::Misc,
                fn_span: rustc_span::DUMMY_SP,
            };
            par_dump!(tcx, 
                "[PAR-IDIOM-MAP-CANDIDATE] fn={} bb_iter=bb{} method=.{}() elem_ty={:?} CLOSURE_WITH_CAPTURES_DYN after=bb{}",
                fn_name,
                plan.bb_iter.index(),
                plan.method,
                plan.elem_tag,
                plan.after_red_bb.index(),
            );
            // Suppress unused warnings for the typed locals
            let _ = closure_ref_ty;
            // Inject bb_map's pre-terminator statements
            let bb_data = &mut body.basic_blocks_mut()[plan.bb_iter];
            bb_data.statements.extend(plan.map_bb_stmts.iter().cloned());
            bb_data.statements.push(ref_stmt);
            bb_data.statements.push(unsize_stmt);
            bb_data.terminator_mut().kind = new_term_kind;
            par_dump!(tcx, 
                "[PAR-IDIOM-MAP-APPLIED] fn={} bb_iter=bb{} method=.{}() elem_ty={:?} CLOSURE_WITH_CAPTURES_DYN",
                fn_name,
                plan.bb_iter.index(),
                plan.method,
                plan.elem_tag,
            );
            count += 1;
            continue;
        }
        let Some(helper_name) = map_helper_for(plan.method, plan.elem_tag) else {
            continue;
        };
        let Some(helper_def_id) = lookup(tcx, helper_name) else {
            par_dump!(tcx, 
                "[PAR-IDIOM-MAP-NO-HELPER] fn={} method=.{}() elem_ty={:?} helper={}",
                fn_name, plan.method, plan.elem_tag, helper_name,
            );
            continue;
        };

        // coerce body FnDef ZST to fn pointer
        let elem_ty = match plan.elem_tag {
            AccTyTag::U32 => tcx.types.u32,
            AccTyTag::U64 => tcx.types.u64,
            AccTyTag::I32 => tcx.types.i32,
            AccTyTag::I64 => tcx.types.i64,
            AccTyTag::Usize => tcx.types.usize,
            AccTyTag::Isize => tcx.types.isize,
            AccTyTag::F64 => continue,
        };
        #[allow(rustc::usage_of_qualified_ty)]
        let ref_elem_ty = ty::Ty::new_imm_ref(tcx, tcx.lifetimes.re_erased, elem_ty);
        #[allow(rustc::usage_of_qualified_ty)]
        let fn_ptr_ty = ty::Ty::new_fn_ptr(
            tcx,
            ty::Binder::dummy(tcx.mk_fn_sig(
                [ref_elem_ty],
                elem_ty,
                false,
                rustc_hir::Safety::Safe,
                rustc_abi::ExternAbi::Rust,
            )),
        );
        let fn_ptr_local = body.local_decls.push(mir::LocalDecl::new(
            fn_ptr_ty,
            rustc_span::DUMMY_SP,
        ));
        // ClosureFnPointer for closures, ReifyFnPointer for fn items
        let coercion = if plan.body_is_closure {
            ty::adjustment::PointerCoercion::ClosureFnPointer(rustc_hir::Safety::Safe)
        } else {
            ty::adjustment::PointerCoercion::ReifyFnPointer(rustc_hir::Safety::Safe)
        };
        let cast_rvalue = mir::Rvalue::Cast(
            mir::CastKind::PointerCoercion(coercion, mir::CoercionSource::Implicit),
            plan.body_operand.clone(),
            fn_ptr_ty,
        );
        let cast_stmt = mir::Statement::new(
            mir::SourceInfo::outermost(rustc_span::DUMMY_SP),
            mir::StatementKind::Assign(Box::new((
                mir::Place::from(fn_ptr_local),
                cast_rvalue,
            ))),
        );

        let helper_callee = mir::Operand::function_handle(
            tcx,
            helper_def_id,
            std::iter::empty(),
            rustc_span::DUMMY_SP,
        );
        let new_args: Box<[rustc_span::source_map::Spanned<mir::Operand<'tcx>>]> = Box::new([
            rustc_span::source_map::Spanned {
                node: plan.slice_arg.clone(),
                span: rustc_span::DUMMY_SP,
            },
            rustc_span::source_map::Spanned {
                node: mir::Operand::Move(mir::Place::from(fn_ptr_local)),
                span: rustc_span::DUMMY_SP,
            },
        ]);
        let new_term_kind = mir::TerminatorKind::Call {
            func: helper_callee,
            args: new_args,
            destination: plan.red_dest,
            target: Some(plan.after_red_bb),
            unwind: mir::UnwindAction::Unreachable,
            call_source: mir::CallSource::Misc,
            fn_span: rustc_span::DUMMY_SP,
        };

        par_dump!(tcx, 
            "[PAR-IDIOM-MAP-CANDIDATE] fn={} bb_iter=bb{} method=.{}() elem_ty={:?} after=bb{}",
            fn_name,
            plan.bb_iter.index(),
            plan.method,
            plan.elem_tag,
            plan.after_red_bb.index(),
        );

        let bb_data = &mut body.basic_blocks_mut()[plan.bb_iter];
        bb_data.statements.push(cast_stmt);
        bb_data.terminator_mut().kind = new_term_kind;

        par_dump!(tcx, 
            "[PAR-IDIOM-MAP-APPLIED] fn={} bb_iter=bb{} method=.{}() elem_ty={:?}",
            fn_name,
            plan.bb_iter.index(),
            plan.method,
            plan.elem_tag,
        );
        count += 1;
    }
    count
}

// iterator-combinator matcher for filter and zip+map chains

pub(crate) fn try_transform_iterator_combinators<'tcx>(
    tcx: TyCtxt<'tcx>,
    body: &mut mir::Body<'tcx>,
) -> usize {
    if !idiom_transform_enabled(tcx) {
        return 0;
    }
    // Never transform sysroot crate internals
    if is_in_sysroot_crate(tcx, body.source.def_id()) {
        return 0;
    }
    // cheap pre-filter, needs a Call terminator
    if !body.basic_blocks.iter().any(|bb| {
        matches!(bb.terminator().kind, mir::TerminatorKind::Call { .. })
    }) {
        return 0;
    }
    let fn_name = tcx.def_path_str(body.source.def_id());
    if fn_name.contains("parallel_runtime") || fn_name.contains("parallel_reduce_") {
        return 0;
    }

    fn lookup(tcx: TyCtxt<'_>, name: &str) -> Option<rustc_hir::def_id::DefId> {
        tcx.get_diagnostic_item(Symbol::intern(name))
    }
    // bail unless u64 filter helper exists
    if lookup(tcx, "parallel_runtime_parallel_reduce_sum_slice_filter_u64").is_none() {
        return 0;
    }

    /// resolve operand through `_local = &(*_param)` re-borrow
    fn resolve_through_reborrow<'tcx>(
        body: &mir::Body<'tcx>,
        bb: mir::BasicBlock,
        op: &mir::Operand<'tcx>,
    ) -> mir::Operand<'tcx> {
        let local = match op {
            mir::Operand::Move(p) | mir::Operand::Copy(p)
                if p.projection.is_empty() =>
            {
                p.local
            }
            _ => return op.clone(),
        };
        for stmt in body.basic_blocks[bb].statements.iter() {
            if let mir::StatementKind::Assign(box (place, rvalue)) = &stmt.kind {
                if place.local == local && place.projection.is_empty() {
                    if let mir::Rvalue::Ref(_, _, src_place) = rvalue {
                        if src_place.projection.len() == 1
                            && matches!(
                                src_place.projection[0],
                                mir::ProjectionElem::Deref
                            )
                        {
                            return mir::Operand::Copy(mir::Place::from(
                                src_place.local,
                            ));
                        }
                    }
                }
            }
        }
        op.clone()
    }

    /// fn-ptr coercion kind, None if not coercible
    #[allow(rustc::usage_of_qualified_ty)]
    fn callee_coercion_kind<'tcx>(
        ty: ty::Ty<'tcx>,
    ) -> Option<ty::adjustment::PointerCoercion> {
        match ty.kind() {
            ty::FnDef(..) => Some(ty::adjustment::PointerCoercion::ReifyFnPointer(
                rustc_hir::Safety::Safe,
            )),
            ty::Closure(_, args) => {
                if args.as_closure().upvar_tys().is_empty() {
                    Some(ty::adjustment::PointerCoercion::ClosureFnPointer(
                        rustc_hir::Safety::Safe,
                    ))
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    /// filter helper diagnostic name for (op, T)
    fn filter_helper_for(op: &str, tag: AccTyTag) -> Option<&'static str> {
        match (op, tag) {
            ("sum", AccTyTag::U64) =>
                Some("parallel_runtime_parallel_reduce_sum_slice_filter_u64"),
            ("sum", AccTyTag::U32) =>
                Some("parallel_runtime_parallel_reduce_sum_slice_filter_u32"),
            ("sum", AccTyTag::I64) =>
                Some("parallel_runtime_parallel_reduce_sum_slice_filter_i64"),
            ("sum", AccTyTag::I32) =>
                Some("parallel_runtime_parallel_reduce_sum_slice_filter_i32"),
            ("product", AccTyTag::U64) =>
                Some("parallel_runtime_parallel_reduce_mul_slice_filter_u64"),
            ("product", AccTyTag::I64) =>
                Some("parallel_runtime_parallel_reduce_mul_slice_filter_i64"),
            _ => None,
        }
    }

    /// Map T → zip-mul helper diag-item.
    fn zip_mul_helper_for(tag: AccTyTag) -> Option<&'static str> {
        match tag {
            AccTyTag::U64 => Some("parallel_runtime_parallel_reduce_sum_slice_zip_mul_u64"),
            AccTyTag::U32 => Some("parallel_runtime_parallel_reduce_sum_slice_zip_mul_u32"),
            AccTyTag::I64 => Some("parallel_runtime_parallel_reduce_sum_slice_zip_mul_i64"),
            AccTyTag::I32 => Some("parallel_runtime_parallel_reduce_sum_slice_zip_mul_i32"),
            // float helpers gated by `parallelize_associative_float` attr
            AccTyTag::F64 => Some("parallel_runtime_parallel_reduce_sum_slice_zip_mul_f64"),
            _ => None,
        }
    }

    /// Map T → general zip-map helper diag-item.
    fn zip_map_helper_for(tag: AccTyTag) -> Option<&'static str> {
        match tag {
            AccTyTag::U64 => Some("parallel_runtime_parallel_reduce_sum_slice_zip_map_u64"),
            AccTyTag::U32 => Some("parallel_runtime_parallel_reduce_sum_slice_zip_map_u32"),
            AccTyTag::I64 => Some("parallel_runtime_parallel_reduce_sum_slice_zip_map_i64"),
            AccTyTag::I32 => Some("parallel_runtime_parallel_reduce_sum_slice_zip_map_i32"),
            AccTyTag::F64 => Some("parallel_runtime_parallel_reduce_sum_slice_zip_map_f64"),
            _ => None,
        }
    }

    /// Inspect a closure's MIR
    fn closure_is_simple_mul<'tcx>(
        tcx: TyCtxt<'tcx>,
        closure_def_id: rustc_hir::def_id::DefId,
        // body_did guards optimized_mir query cycle on self
        body_did: rustc_hir::def_id::DefId,
    ) -> bool {
        if closure_def_id == body_did {
            return false;
        }
        if !tcx.is_mir_available(closure_def_id) {
            return false;
        }
        let body = tcx.optimized_mir(closure_def_id);
        // Body must be a single basic block
        let mut mul_count = 0usize;
        let mut other_binop_count = 0usize;
        for bb in body.basic_blocks.iter() {
            for stmt in &bb.statements {
                if let mir::StatementKind::Assign(box (_, rvalue)) = &stmt.kind {
                    if let mir::Rvalue::BinaryOp(op, _) = rvalue {
                        match op {
                            mir::BinOp::Mul
                            | mir::BinOp::MulUnchecked
                            | mir::BinOp::MulWithOverflow => mul_count += 1,
                            _ => other_binop_count += 1,
                        }
                    }
                }
            }
        }
        // Exactly one Mul, no other arithmetic
        mul_count == 1 && other_binop_count == 0
    }

    let bbs: Vec<(mir::BasicBlock, mir::Terminator<'tcx>)> = body
        .basic_blocks
        .iter_enumerated()
        .map(|(b, d)| (b, d.terminator().clone()))
        .collect();

    #[allow(rustc::usage_of_qualified_ty)]
    enum PlanKind<'tcx> {
        Filter {
            op: &'static str,
            elem_tag: AccTyTag,
            slice_op: mir::Operand<'tcx>,
            predicate_op: mir::Operand<'tcx>,
            predicate_ty: ty::Ty<'tcx>,
            /// Destination Place written by the method's Call
            method_dest: mir::Place<'tcx>,
        },
        ZipMul {
            elem_tag: AccTyTag,
            a_slice_op: mir::Operand<'tcx>,
            b_slice_op: mir::Operand<'tcx>,
            method_dest: mir::Place<'tcx>,
        },
        ZipMap {
            elem_tag: AccTyTag,
            a_slice_op: mir::Operand<'tcx>,
            b_slice_op: mir::Operand<'tcx>,
            combiner_op: mir::Operand<'tcx>,
            combiner_ty: ty::Ty<'tcx>,
            method_dest: mir::Place<'tcx>,
        },
    }

    struct Plan<'tcx> {
        kind: PlanKind<'tcx>,
        /// block whose terminator becomes the helper Call
        bb_to_replace: mir::BasicBlock,
        after_bb: mir::BasicBlock,
    }

    let mut plans: Vec<Plan<'tcx>> = Vec::new();

    // filter detection, walk bb_iter, bb_filter, bb_method chain
    for (bb_iter, term) in &bbs {
        // bb_iter: Call core::slice::<impl [T]>::iter(slice) → bb_filter
        let mir::TerminatorKind::Call {
            func: iter_func,
            args: iter_args,
            destination: iter_dest,
            target: Some(bb_filter),
            ..
        } = &term.kind
        else { continue };
        if !iter_dest.projection.is_empty() { continue }
        let mir::Operand::Constant(ic) = iter_func else { continue };
        let ty::FnDef(iter_def_id, _) = ic.const_.ty().kind() else { continue };
        let iter_path = tcx.def_path_str(*iter_def_id);
        if iter_path.rsplit("::").next() != Some("iter")
            || !iter_path.contains("slice")
        {
            continue;
        }
        if iter_args.len() != 1 { continue }
        // resolve re-borrow, temp's block becomes unreachable
        let slice_arg = resolve_through_reborrow(body, *bb_iter, &iter_args[0].node);
        let slice_arg_ty = match &slice_arg {
            mir::Operand::Copy(p) | mir::Operand::Move(p) => {
                p.ty(&body.local_decls, tcx).ty
            }
            mir::Operand::Constant(c) => c.const_.ty(),
            _ => continue,
        };
        let elem_tag = match slice_arg_ty.kind() {
            ty::Ref(_, inner, _) => match inner.kind() {
                ty::Slice(elem) => match AccTyTag::from_ty(*elem) {
                    Some(t) => t,
                    None => continue,
                },
                _ => continue,
            },
            _ => continue,
        };

        // bb_filter: Call Iterator::filter(iter, pred) → bb_method
        let filter_idx = bb_filter.as_usize();
        if filter_idx >= bbs.len() { continue }
        let filter_term = &bbs[filter_idx].1;
        let mir::TerminatorKind::Call {
            func: filter_fn,
            args: filter_args,
            destination: filter_dest,
            target: Some(bb_method),
            ..
        } = &filter_term.kind
        else { continue };
        let mir::Operand::Constant(fc) = filter_fn else { continue };
        let ty::FnDef(filter_def_id, _) = fc.const_.ty().kind() else { continue };
        let filter_path = tcx.def_path_str(*filter_def_id);
        if filter_path.rsplit("::").next() != Some("filter") { continue }
        if !filter_path.contains("Iterator") { continue }
        if filter_args.len() != 2 { continue }
        // arg[0] must be the iter from bb_iter
        let iter_arg_ok = match &filter_args[0].node {
            mir::Operand::Copy(p) | mir::Operand::Move(p) => {
                p.projection.is_empty() && p.local == iter_dest.local
            }
            _ => false,
        };
        if !iter_arg_ok { continue }
        // arg[1] is predicate closure or fn ZST
        let pred_op = filter_args[1].node.clone();
        let pred_ty = match &pred_op {
            mir::Operand::Constant(c) => c.const_.ty(),
            mir::Operand::Copy(p) | mir::Operand::Move(p) => {
                p.ty(&body.local_decls, tcx).ty
            }
            _ => continue,
        };
        if callee_coercion_kind(pred_ty).is_none() { continue }

        // bb_method: Call Iterator::sum/product/count(filter) → bb_after
        let method_idx = bb_method.as_usize();
        if method_idx >= bbs.len() { continue }
        let method_term = &bbs[method_idx].1;
        let mir::TerminatorKind::Call {
            func: method_fn,
            args: method_args,
            destination: method_dest,
            target: Some(bb_after),
            ..
        } = &method_term.kind
        else { continue };
        let mir::Operand::Constant(mc) = method_fn else { continue };
        let ty::FnDef(method_def_id, _) = mc.const_.ty().kind() else { continue };
        let method_path = tcx.def_path_str(*method_def_id);
        if !method_path.contains("Iterator") { continue }
        let op_str = match method_path.rsplit("::").next().unwrap_or("") {
            "sum" => "sum",
            "product" => "product",
            "count" => "count",
            _ => continue,
        };
        if method_args.len() != 1 { continue }
        let method_arg_ok = match &method_args[0].node {
            mir::Operand::Copy(p) | mir::Operand::Move(p) => {
                p.projection.is_empty() && p.local == filter_dest.local
            }
            _ => false,
        };
        if !method_arg_ok { continue }

        // slice_arg from iter Call feeds helper directly
        plans.push(Plan {
            kind: PlanKind::Filter {
                op: op_str,
                elem_tag,
                slice_op: slice_arg,
                predicate_op: pred_op,
                predicate_ty: pred_ty,
                method_dest: *method_dest,
            },
            bb_to_replace: *bb_iter,
            after_bb: *bb_after,
        });
    }

    // zip detection, 5-call chain, skip Goto-only blocks
    fn walk_goto<'tcx>(body: &mir::Body<'tcx>, mut bb: mir::BasicBlock, _tcx: TyCtxt<'tcx>) -> mir::BasicBlock {
        for _ in 0..6 {
            let data = &body.basic_blocks[bb];
            let pure_passthrough = data.statements.iter().all(|s| matches!(
                s.kind,
                mir::StatementKind::StorageLive(_)
                | mir::StatementKind::StorageDead(_)
                | mir::StatementKind::Nop
                | mir::StatementKind::FakeRead(..)
                | mir::StatementKind::PlaceMention(..)
                // shared re-borrows are transparent too
                | mir::StatementKind::Assign(box (_, mir::Rvalue::Ref(_, mir::BorrowKind::Shared, _))),
            ));
            if !pure_passthrough { return bb; }
            match &data.terminator().kind {
                mir::TerminatorKind::Goto { target } => bb = *target,
                // Walk through a *transparent* `Vec<T>::deref`
                _ => return bb,
            }
        }
        bb
    }

    for (bb_iter_a, term) in &bbs {
        let mir::TerminatorKind::Call {
            func: ifa, args: ifa_args, destination: a_iter_dest, target: Some(bb_iter_b), ..
        } = &term.kind else { continue };
        if !a_iter_dest.projection.is_empty() { continue }
        let mir::Operand::Constant(c) = ifa else { continue };
        let ty::FnDef(d, _) = c.const_.ty().kind() else { continue };
        let p = tcx.def_path_str(*d);
        if p.rsplit("::").next() != Some("iter") || !p.contains("slice") { continue }
        if ifa_args.len() != 1 { continue }
        // trace re-borrow; temp stays in kept bb_iter_a
        let a_slice_op = resolve_through_reborrow(body, *bb_iter_a, &ifa_args[0].node);
        let a_slice_ty = match &a_slice_op {
            mir::Operand::Copy(p) | mir::Operand::Move(p) => p.ty(&body.local_decls, tcx).ty,
            mir::Operand::Constant(c) => c.const_.ty(),
            _ => continue,
        };
        let a_tag = match a_slice_ty.kind() {
            ty::Ref(_, inner, _) => match inner.kind() {
                ty::Slice(e) => match AccTyTag::from_ty(*e) { Some(t) => t, None => continue },
                _ => continue,
            },
            _ => continue,
        };

        let bb_iter_b_walked = walk_goto(body, *bb_iter_b, tcx);
        let idx_b = bb_iter_b_walked.as_usize();
        if idx_b >= bbs.len() { continue }
        let bt = &bbs[idx_b].1;
        let mir::TerminatorKind::Call {
            func: ifb, args: ifb_args, destination: b_iter_dest, target: Some(bb_zip), ..
        } = &bt.kind else { continue };
        if !b_iter_dest.projection.is_empty() { continue }
        let mir::Operand::Constant(c) = ifb else { continue };
        let ty::FnDef(d, _) = c.const_.ty().kind() else { continue };
        let pb = tcx.def_path_str(*d);
        if pb.rsplit("::").next() != Some("iter") || !pb.contains("slice") { continue }
        if ifb_args.len() != 1 { continue }
        // bb_iter_b discarded, trace b's re-borrow to param
        let b_slice_op = resolve_through_reborrow(body, *bb_iter_b, &ifb_args[0].node);
        let b_slice_ty = match &b_slice_op {
            mir::Operand::Copy(p) | mir::Operand::Move(p) => p.ty(&body.local_decls, tcx).ty,
            mir::Operand::Constant(c) => c.const_.ty(),
            _ => continue,
        };
        let b_tag = match b_slice_ty.kind() {
            ty::Ref(_, inner, _) => match inner.kind() {
                ty::Slice(e) => match AccTyTag::from_ty(*e) { Some(t) => t, None => continue },
                _ => continue,
            },
            _ => continue,
        };
        if a_tag != b_tag { continue }

        let bb_zip_walked = walk_goto(body, *bb_zip, tcx);
        let idx_z = bb_zip_walked.as_usize();
        if idx_z >= bbs.len() { continue }
        let zt = &bbs[idx_z].1;
        let mir::TerminatorKind::Call {
            func: zf, args: z_args, destination: zip_dest, target: Some(bb_map), ..
        } = &zt.kind else { continue };
        let mir::Operand::Constant(c) = zf else { continue };
        let ty::FnDef(d, _) = c.const_.ty().kind() else { continue };
        let pz = tcx.def_path_str(*d);
        if pz.rsplit("::").next() != Some("zip") { continue }
        if !pz.contains("Iterator") { continue }
        if z_args.len() != 2 { continue }
        let arg0_ok = matches!(&z_args[0].node, mir::Operand::Move(p) | mir::Operand::Copy(p) if p.projection.is_empty() && p.local == a_iter_dest.local);
        let arg1_ok = matches!(&z_args[1].node, mir::Operand::Move(p) | mir::Operand::Copy(p) if p.projection.is_empty() && p.local == b_iter_dest.local);
        if !arg0_ok || !arg1_ok { continue }

        let bb_map_walked = walk_goto(body, *bb_map, tcx);
        let idx_m = bb_map_walked.as_usize();
        if idx_m >= bbs.len() { continue }
        let mt = &bbs[idx_m].1;
        let mir::TerminatorKind::Call {
            func: mf, args: m_args, destination: map_dest, target: Some(bb_method), ..
        } = &mt.kind else { continue };
        let mir::Operand::Constant(c) = mf else { continue };
        let ty::FnDef(d, _) = c.const_.ty().kind() else { continue };
        let pm = tcx.def_path_str(*d);
        if pm.rsplit("::").next() != Some("map") { continue }
        if !pm.contains("Iterator") { continue }
        if m_args.len() != 2 { continue }
        let arg0_ok = matches!(&m_args[0].node, mir::Operand::Move(p) | mir::Operand::Copy(p) if p.projection.is_empty() && p.local == zip_dest.local);
        if !arg0_ok { continue }
        let combiner_op = m_args[1].node.clone();
        let combiner_ty = match &combiner_op {
            mir::Operand::Constant(c) => c.const_.ty(),
            mir::Operand::Copy(p) | mir::Operand::Move(p) => p.ty(&body.local_decls, tcx).ty,
            _ => continue,
        };
        if callee_coercion_kind(combiner_ty).is_none() {
            continue
        }

        let bb_method_walked = walk_goto(body, *bb_method, tcx);
        let idx_meth = bb_method_walked.as_usize();
        if idx_meth >= bbs.len() { continue }
        let meth_t = &bbs[idx_meth].1;
        let mir::TerminatorKind::Call {
            func: methf, args: meth_args, destination: meth_dest, target: Some(bb_after), ..
        } = &meth_t.kind else { continue };
        let mir::Operand::Constant(c) = methf else { continue };
        let ty::FnDef(d, _) = c.const_.ty().kind() else { continue };
        let pmeth = tcx.def_path_str(*d);
        if !pmeth.contains("Iterator") { continue }
        if pmeth.rsplit("::").next() != Some("sum") { continue }  // only sum for now
        if meth_args.len() != 1 { continue }
        let arg0_ok = matches!(&meth_args[0].node, mir::Operand::Move(p) | mir::Operand::Copy(p) if p.projection.is_empty() && p.local == map_dest.local);
        if !arg0_ok { continue }

        // dot-product combiner uses zip_mul, else zip_map
        let is_mul = match combiner_ty.kind() {
            ty::Closure(cdid, _) => closure_is_simple_mul(tcx, *cdid, body.source.def_id()),
            ty::FnDef(fdid, _) if fdid.is_local() => closure_is_simple_mul(tcx, *fdid, body.source.def_id()),
            _ => false,
        };
        if is_mul {
            plans.push(Plan {
                kind: PlanKind::ZipMul {
                    elem_tag: a_tag,
                    a_slice_op,
                    b_slice_op,
                    method_dest: *meth_dest,
                },
                bb_to_replace: *bb_iter_a,
                after_bb: *bb_after,
            });
        } else {
            plans.push(Plan {
                kind: PlanKind::ZipMap {
                    elem_tag: a_tag,
                    a_slice_op,
                    b_slice_op,
                    combiner_op,
                    combiner_ty,
                    method_dest: *meth_dest,
                },
                bb_to_replace: *bb_iter_a,
                after_bb: *bb_after,
            });
        }
    }

    let mut count = 0usize;
    for plan in plans {
        match plan.kind {
            PlanKind::Filter {
                op,
                elem_tag,
                slice_op,
                predicate_op,
                predicate_ty,
                method_dest,
            } => {
                // For "count", route to the count helper
                let helper_name = if op == "count" {
                    if elem_tag != AccTyTag::U64 {
                        par_dump!(tcx, 
                            "[PAR-IDIOM-FILTER-UNSUPPORTED] fn={} reason=count_only_u64 elem_ty={:?}",
                            fn_name, elem_tag,
                        );
                        continue;
                    }
                    "parallel_runtime_parallel_reduce_count_slice_filter_u64"
                } else {
                    match filter_helper_for(op, elem_tag) {
                        Some(n) => n,
                        None => {
                            par_dump!(tcx, 
                                "[PAR-IDIOM-FILTER-UNSUPPORTED] fn={} op={} elem_ty={:?}",
                                fn_name, op, elem_tag,
                            );
                            continue;
                        }
                    }
                };
                let Some(helper_def_id) = lookup(tcx, helper_name) else {
                    par_dump!(tcx, 
                        "[PAR-IDIOM-FILTER-NO-HELPER] fn={} helper={}",
                        fn_name, helper_name,
                    );
                    continue;
                };

                let elem_ty = match elem_tag {
                    AccTyTag::U64 => tcx.types.u64,
                    AccTyTag::U32 => tcx.types.u32,
                    AccTyTag::I64 => tcx.types.i64,
                    AccTyTag::I32 => tcx.types.i32,
                    _ => continue,
                };
                #[allow(rustc::usage_of_qualified_ty)]
                let ref_elem_ty = ty::Ty::new_imm_ref(tcx, tcx.lifetimes.re_erased, elem_ty);
                // Predicate is sourced from `Iterator::filter`'s F
                #[allow(rustc::usage_of_qualified_ty)]
                let ref_ref_elem_ty = ty::Ty::new_imm_ref(tcx, tcx.lifetimes.re_erased, ref_elem_ty);
                #[allow(rustc::usage_of_qualified_ty)]
                let pred_ptr_ty = ty::Ty::new_fn_ptr(
                    tcx,
                    ty::Binder::dummy(tcx.mk_fn_sig(
                        [ref_ref_elem_ty],
                        tcx.types.bool,
                        false,
                        rustc_hir::Safety::Safe,
                        rustc_abi::ExternAbi::Rust,
                    )),
                );
                let pred_local = body.local_decls.push(mir::LocalDecl::new(
                    pred_ptr_ty,
                    rustc_span::DUMMY_SP,
                ));
                let coercion = match callee_coercion_kind(predicate_ty) {
                    Some(c) => c,
                    None => continue,
                };
                let cast_rvalue = mir::Rvalue::Cast(
                    mir::CastKind::PointerCoercion(coercion, mir::CoercionSource::Implicit),
                    predicate_op,
                    pred_ptr_ty,
                );
                let cast_stmt = mir::Statement::new(
                    mir::SourceInfo::outermost(rustc_span::DUMMY_SP),
                    mir::StatementKind::Assign(Box::new((
                        mir::Place::from(pred_local),
                        cast_rvalue,
                    ))),
                );

                let helper_callee = mir::Operand::function_handle(
                    tcx,
                    helper_def_id,
                    std::iter::empty(),
                    rustc_span::DUMMY_SP,
                );
                let new_args: Box<[rustc_span::source_map::Spanned<mir::Operand<'tcx>>]> = Box::new([
                    rustc_span::source_map::Spanned {
                        node: slice_op,
                        span: rustc_span::DUMMY_SP,
                    },
                    rustc_span::source_map::Spanned {
                        node: mir::Operand::Move(mir::Place::from(pred_local)),
                        span: rustc_span::DUMMY_SP,
                    },
                ]);
                let new_term = mir::TerminatorKind::Call {
                    func: helper_callee,
                    args: new_args,
                    destination: method_dest,
                    target: Some(plan.after_bb),
                    unwind: mir::UnwindAction::Unreachable,
                    call_source: mir::CallSource::Misc,
                    fn_span: rustc_span::DUMMY_SP,
                };
                // drop original stmts, keep only our cast
                let bb_data = &mut body.basic_blocks_mut()[plan.bb_to_replace];
                bb_data.statements.push(cast_stmt);
                bb_data.terminator_mut().kind = new_term;
                par_dump!(tcx, 
                    "[PAR-IDIOM-FILTER-APPLIED] fn={} bb={} op={} elem_ty={:?}",
                    fn_name,
                    plan.bb_to_replace.index(),
                    op,
                    elem_tag,
                );
                count += 1;
            }
            PlanKind::ZipMul {
                elem_tag,
                a_slice_op,
                b_slice_op,
                method_dest,
            } => {
                // float zip-mul opt-in, parallel sum not bit-exact
                if elem_tag.is_float() {
                    let mut attrs = tcx.get_attrs(
                        body.source.def_id(),
                        rustc_span::sym::parallelize_associative_float,
                    );
                    if attrs.next().is_none() {
                        par_dump!(tcx,
                            "[PAR-IDIOM-ZIP-MUL-FLOAT-NO-OPTIN] fn={} elem_ty={:?} (add #[parallelize_associative_float] to opt in)",
                            fn_name, elem_tag,
                        );
                        continue;
                    }
                }
                let Some(helper_name) = zip_mul_helper_for(elem_tag) else { continue };
                let Some(helper_def_id) = lookup(tcx, helper_name) else {
                    par_dump!(tcx, "[PAR-IDIOM-ZIP-NO-HELPER] fn={} helper={}", fn_name, helper_name);
                    continue;
                };
                let helper_callee = mir::Operand::function_handle(
                    tcx,
                    helper_def_id,
                    std::iter::empty(),
                    rustc_span::DUMMY_SP,
                );
                let new_args: Box<[rustc_span::source_map::Spanned<mir::Operand<'tcx>>]> = Box::new([
                    rustc_span::source_map::Spanned {
                        node: a_slice_op,
                        span: rustc_span::DUMMY_SP,
                    },
                    rustc_span::source_map::Spanned {
                        node: b_slice_op,
                        span: rustc_span::DUMMY_SP,
                    },
                ]);
                let new_term = mir::TerminatorKind::Call {
                    func: helper_callee,
                    args: new_args,
                    destination: method_dest,
                    target: Some(plan.after_bb),
                    unwind: mir::UnwindAction::Unreachable,
                    call_source: mir::CallSource::Misc,
                    fn_span: rustc_span::DUMMY_SP,
                };
                let bb_data = &mut body.basic_blocks_mut()[plan.bb_to_replace];
                bb_data.terminator_mut().kind = new_term;
                par_dump!(tcx, 
                    "[PAR-IDIOM-ZIP-MUL-APPLIED] fn={} bb={} elem_ty={:?}",
                    fn_name,
                    plan.bb_to_replace.index(),
                    elem_tag,
                );
                count += 1;
            }
            PlanKind::ZipMap {
                elem_tag,
                a_slice_op,
                b_slice_op,
                combiner_op,
                combiner_ty,
                method_dest,
            } => {
                // float zip-map opt-in, see ZipMul above
                if elem_tag.is_float() {
                    let mut attrs = tcx.get_attrs(
                        body.source.def_id(),
                        rustc_span::sym::parallelize_associative_float,
                    );
                    if attrs.next().is_none() {
                        par_dump!(tcx,
                            "[PAR-IDIOM-ZIP-MAP-FLOAT-NO-OPTIN] fn={} elem_ty={:?} (add #[parallelize_associative_float] to opt in)",
                            fn_name, elem_tag,
                        );
                        continue;
                    }
                }
                let Some(helper_name) = zip_map_helper_for(elem_tag) else { continue };
                let Some(helper_def_id) = lookup(tcx, helper_name) else {
                    par_dump!(tcx, "[PAR-IDIOM-ZIP-NO-HELPER] fn={} helper={}", fn_name, helper_name);
                    continue;
                };
                let elem_ty = match elem_tag {
                    AccTyTag::U64 => tcx.types.u64,
                    AccTyTag::U32 => tcx.types.u32,
                    AccTyTag::I64 => tcx.types.i64,
                    AccTyTag::I32 => tcx.types.i32,
                    AccTyTag::F64 => tcx.types.f64,
                    _ => continue,
                };
                #[allow(rustc::usage_of_qualified_ty)]
                let ref_elem_ty = ty::Ty::new_imm_ref(tcx, tcx.lifetimes.re_erased, elem_ty);
                // combiner takes tuple arg matching Zip's Item
                #[allow(rustc::usage_of_qualified_ty)]
                let tuple_arg_ty = ty::Ty::new_tup(tcx, &[ref_elem_ty, ref_elem_ty]);
                #[allow(rustc::usage_of_qualified_ty)]
                let comb_ptr_ty = ty::Ty::new_fn_ptr(
                    tcx,
                    ty::Binder::dummy(tcx.mk_fn_sig(
                        [tuple_arg_ty],
                        elem_ty,
                        false,
                        rustc_hir::Safety::Safe,
                        rustc_abi::ExternAbi::Rust,
                    )),
                );
                let comb_local = body.local_decls.push(mir::LocalDecl::new(
                    comb_ptr_ty,
                    rustc_span::DUMMY_SP,
                ));
                let coercion = match callee_coercion_kind(combiner_ty) {
                    Some(c) => c,
                    None => continue,
                };
                let cast_rvalue = mir::Rvalue::Cast(
                    mir::CastKind::PointerCoercion(coercion, mir::CoercionSource::Implicit),
                    combiner_op,
                    comb_ptr_ty,
                );
                let cast_stmt = mir::Statement::new(
                    mir::SourceInfo::outermost(rustc_span::DUMMY_SP),
                    mir::StatementKind::Assign(Box::new((
                        mir::Place::from(comb_local),
                        cast_rvalue,
                    ))),
                );
                let helper_callee = mir::Operand::function_handle(
                    tcx,
                    helper_def_id,
                    std::iter::empty(),
                    rustc_span::DUMMY_SP,
                );
                let new_args: Box<[rustc_span::source_map::Spanned<mir::Operand<'tcx>>]> = Box::new([
                    rustc_span::source_map::Spanned {
                        node: a_slice_op,
                        span: rustc_span::DUMMY_SP,
                    },
                    rustc_span::source_map::Spanned {
                        node: b_slice_op,
                        span: rustc_span::DUMMY_SP,
                    },
                    rustc_span::source_map::Spanned {
                        node: mir::Operand::Move(mir::Place::from(comb_local)),
                        span: rustc_span::DUMMY_SP,
                    },
                ]);
                let new_term = mir::TerminatorKind::Call {
                    func: helper_callee,
                    args: new_args,
                    destination: method_dest,
                    target: Some(plan.after_bb),
                    unwind: mir::UnwindAction::Unreachable,
                    call_source: mir::CallSource::Misc,
                    fn_span: rustc_span::DUMMY_SP,
                };
                let bb_data = &mut body.basic_blocks_mut()[plan.bb_to_replace];
                bb_data.statements.push(cast_stmt);
                bb_data.terminator_mut().kind = new_term;
                par_dump!(tcx, 
                    "[PAR-IDIOM-ZIP-MAP-APPLIED] fn={} bb={} elem_ty={:?}",
                    fn_name,
                    plan.bb_to_replace.index(),
                    elem_tag,
                );
                count += 1;
            }
        }
    }
    count
}

// Phase A.1/D.1, slice iter/iter_mut for_each matcher

// generic-T fallback for_each/filter-count, runs after typed matchers

pub(crate) fn try_transform_generic_iter_patterns<'tcx>(
    tcx: TyCtxt<'tcx>,
    body: &mut mir::Body<'tcx>,
) -> usize {
    if !idiom_transform_enabled(tcx) {
        return 0;
    }
    // Never transform sysroot crate internals
    if is_in_sysroot_crate(tcx, body.source.def_id()) {
        return 0;
    }
    if body.basic_blocks.len() < 2 {
        return 0;
    }
    if !body.basic_blocks.iter().any(|bb| {
        matches!(bb.terminator().kind, mir::TerminatorKind::Call { .. })
    }) {
        return 0;
    }
    let fn_name = tcx.def_path_str(body.source.def_id());
    if fn_name.contains("parallel_runtime") || fn_name.contains("parallel_reduce_") {
        return 0;
    }

    fn lookup(tcx: TyCtxt<'_>, name: &str) -> Option<rustc_hir::def_id::DefId> {
        tcx.get_diagnostic_item(Symbol::intern(name))
    }
    let Some(for_each_def_id) = lookup(tcx, "parallel_runtime_parallel_for_each_slice_dyn_generic") else {
        return 0;
    };
    let Some(filter_count_def_id) = lookup(tcx, "parallel_runtime_parallel_filter_count_slice_dyn_generic") else {
        return 0;
    };

    /// Re-borrow tracer (same pattern as elsewhere).
    fn resolve_through_reborrow<'tcx>(
        body: &mir::Body<'tcx>,
        bb: mir::BasicBlock,
        op: &mir::Operand<'tcx>,
    ) -> mir::Operand<'tcx> {
        let local = match op {
            mir::Operand::Move(p) | mir::Operand::Copy(p) if p.projection.is_empty() => p.local,
            _ => return op.clone(),
        };
        for stmt in body.basic_blocks[bb].statements.iter() {
            if let mir::StatementKind::Assign(box (place, rvalue)) = &stmt.kind {
                if place.local == local && place.projection.is_empty() {
                    if let mir::Rvalue::Ref(_, _, src_place) = rvalue {
                        if src_place.projection.len() == 1
                            && matches!(src_place.projection[0], mir::ProjectionElem::Deref)
                        {
                            return mir::Operand::Copy(mir::Place::from(src_place.local));
                        }
                    }
                }
            }
        }
        op.clone()
    }

    /// True iff `ty` is a Fn-kind closure
    #[allow(rustc::usage_of_qualified_ty)]
    fn is_fn_kind_callable<'tcx>(_tcx: TyCtxt<'tcx>, ty: ty::Ty<'tcx>) -> bool {
        match ty.kind() {
            ty::Closure(_, args) => matches!(
                args.as_closure().kind_ty().kind(),
                ty::Int(rustc_middle::ty::IntTy::I8)
            ),
            ty::FnDef(..) => true,
            _ => false,
        }
    }

    enum GenericKind {
        ForEach,
        FilterCount,
    }

    #[allow(rustc::usage_of_qualified_ty)]
    struct Plan<'tcx> {
        kind: GenericKind,
        bb_to_replace: mir::BasicBlock,
        bb_after: mir::BasicBlock,
        result_dest: mir::Place<'tcx>,
        slice_op: mir::Operand<'tcx>,
        elem_ty: ty::Ty<'tcx>,
        body_op: mir::Operand<'tcx>,
        method_bb_stmts: Vec<mir::Statement<'tcx>>,
        /// filter+count also keeps bb_method (count) stmts
        count_bb_stmts: Vec<mir::Statement<'tcx>>,
    }

    let bbs: Vec<(mir::BasicBlock, mir::Terminator<'tcx>)> = body
        .basic_blocks
        .iter_enumerated()
        .map(|(b, d)| (b, d.terminator().clone()))
        .collect();

    let mut plans: Vec<Plan<'tcx>> = Vec::new();

    // detect iter/for_each and iter/filter/count shapes
    for (bb_iter, term) in &bbs {
        let mir::TerminatorKind::Call {
            func: i_func,
            args: i_args,
            destination: iter_dest,
            target: Some(bb_next),
            ..
        } = &term.kind else { continue };
        if !iter_dest.projection.is_empty() { continue }
        let mir::Operand::Constant(ic) = i_func else { continue };
        let ty::FnDef(i_def_id, _) = ic.const_.ty().kind() else { continue };
        let i_path = tcx.def_path_str(*i_def_id);
        if i_path.rsplit("::").next() != Some("iter") || !i_path.contains("slice") { continue }
        if i_args.len() != 1 { continue }
        let slice_op = resolve_through_reborrow(body, *bb_iter, &i_args[0].node);
        let slice_ty = match &slice_op {
            mir::Operand::Copy(p) | mir::Operand::Move(p) => p.ty(&body.local_decls, tcx).ty,
            mir::Operand::Constant(c) => c.const_.ty(),
            _ => continue,
        };
        let elem_ty = match slice_ty.kind() {
            ty::Ref(_, inner, _) => match inner.kind() {
                ty::Slice(elem) => *elem,
                _ => continue,
            },
            _ => continue,
        };
        // Skip if it's a primitive type
        if AccTyTag::from_ty(elem_ty).is_some() { continue }

        // next block decides the shape
        let next_idx = bb_next.as_usize();
        if next_idx >= bbs.len() { continue }
        let next_term = &bbs[next_idx].1;
        let mir::TerminatorKind::Call {
            func: nf,
            args: nargs,
            destination: ndest,
            target: Some(bb_after_next),
            ..
        } = &next_term.kind else { continue };
        let mir::Operand::Constant(nc) = nf else { continue };
        let ty::FnDef(n_def_id, _) = nc.const_.ty().kind() else { continue };
        let n_path = tcx.def_path_str(*n_def_id);
        let last = n_path.rsplit("::").next().unwrap_or("");

        // capture bb stmts, strip Storage* (lifetime mismatch)
        let capture_method_block = |bb: mir::BasicBlock| -> Vec<mir::Statement<'tcx>> {
            body.basic_blocks[bb].statements.iter()
                .filter(|s| !matches!(
                    s.kind,
                    mir::StatementKind::StorageLive(_) | mir::StatementKind::StorageDead(_)
                ))
                .cloned()
                .collect()
        };

        if last == "for_each" && n_path.contains("Iterator") {
            // Shape 1: iter() → for_each(body)
            if nargs.len() != 2 { continue }
            let iter_arg_ok = matches!(&nargs[0].node, mir::Operand::Move(p) | mir::Operand::Copy(p) if p.projection.is_empty() && p.local == iter_dest.local);
            if !iter_arg_ok { continue }
            let body_op = nargs[1].node.clone();
            let body_ty = match &body_op {
                mir::Operand::Constant(c) => c.const_.ty(),
                mir::Operand::Move(p) | mir::Operand::Copy(p) => p.ty(&body.local_decls, tcx).ty,
                _ => continue,
            };
            if !is_fn_kind_callable(tcx, body_ty) { continue }
            // closures only, fn item lacks a Place
            if matches!(body_ty.kind(), ty::Closure(..)) {
                // ok
            } else {
                continue;
            }
            plans.push(Plan {
                kind: GenericKind::ForEach,
                bb_to_replace: *bb_iter,
                bb_after: *bb_after_next,
                result_dest: *ndest,
                slice_op,
                elem_ty,
                body_op,
                method_bb_stmts: capture_method_block(*bb_next),
                count_bb_stmts: vec![],
            });
            continue;
        }

        if last == "filter" && n_path.contains("Iterator") {
            // Shape 2: iter() → filter(p) → count()
            if nargs.len() != 2 { continue }
            let iter_arg_ok = matches!(&nargs[0].node, mir::Operand::Move(p) | mir::Operand::Copy(p) if p.projection.is_empty() && p.local == iter_dest.local);
            if !iter_arg_ok { continue }
            let pred_op = nargs[1].node.clone();
            let pred_ty = match &pred_op {
                mir::Operand::Constant(c) => c.const_.ty(),
                mir::Operand::Move(p) | mir::Operand::Copy(p) => p.ty(&body.local_decls, tcx).ty,
                _ => continue,
            };
            if !is_fn_kind_callable(tcx, pred_ty) { continue }
            if !matches!(pred_ty.kind(), ty::Closure(..)) { continue }

            // Look at bb_after_next: should be Iterator::count(filter)
            let count_idx = bb_after_next.as_usize();
            if count_idx >= bbs.len() { continue }
            let count_term = &bbs[count_idx].1;
            let mir::TerminatorKind::Call {
                func: cf,
                args: cargs,
                destination: cdest,
                target: Some(bb_after_count),
                ..
            } = &count_term.kind else { continue };
            let mir::Operand::Constant(cc) = cf else { continue };
            let ty::FnDef(c_def_id, _) = cc.const_.ty().kind() else { continue };
            let c_path = tcx.def_path_str(*c_def_id);
            if c_path.rsplit("::").next() != Some("count") || !c_path.contains("Iterator") { continue }
            if cargs.len() != 1 { continue }
            let count_arg_ok = matches!(&cargs[0].node, mir::Operand::Move(p) | mir::Operand::Copy(p) if p.projection.is_empty() && p.local == ndest.local);
            if !count_arg_ok { continue }

            plans.push(Plan {
                kind: GenericKind::FilterCount,
                bb_to_replace: *bb_iter,
                bb_after: *bb_after_count,
                result_dest: *cdest,
                slice_op,
                elem_ty,
                body_op: pred_op,
                method_bb_stmts: capture_method_block(*bb_next),
                count_bb_stmts: capture_method_block(*bb_after_next),
            });
            continue;
        }
    }

    // post-inlined shape scan, walk back from count

    /// find latest unprojected assignment to a local
    fn find_assign<'tcx, 'a>(
        body: &'a mir::Body<'tcx>,
        local: mir::Local,
    ) -> Option<&'a mir::Rvalue<'tcx>> {
        for bb_data in body.basic_blocks.iter() {
            for stmt in bb_data.statements.iter() {
                if let mir::StatementKind::Assign(box (place, rvalue)) = &stmt.kind {
                    if place.local == local && place.projection.is_empty() {
                        return Some(rvalue);
                    }
                }
            }
        }
        None
    }

    /// trace Iter.ptr back to slice ref Local
    fn trace_slice_ref_from_iter_ptr<'tcx>(
        body: &mir::Body<'tcx>,
        ptr_op: &mir::Operand<'tcx>,
    ) -> Option<mir::Local> {
        let l_nonnull = match ptr_op {
            mir::Operand::Copy(p) | mir::Operand::Move(p) if p.projection.is_empty() => p.local,
            _ => return None,
        };
        let rv_nonnull = find_assign(body, l_nonnull)?;
        let mir::Rvalue::Aggregate(_, fields) = rv_nonnull else { return None };
        if fields.len() != 1 { return None }
        let l_cast = match fields.raw.get(0)? {
            mir::Operand::Copy(p) | mir::Operand::Move(p) if p.projection.is_empty() => p.local,
            _ => return None,
        };
        let rv_cast = find_assign(body, l_cast)?;
        let l_raw = match rv_cast {
            mir::Rvalue::Cast(mir::CastKind::PtrToPtr, op, _) => match op {
                mir::Operand::Copy(p) | mir::Operand::Move(p) if p.projection.is_empty() => p.local,
                _ => return None,
            },
            _ => return None,
        };
        let rv_raw = find_assign(body, l_raw)?;
        let mir::Rvalue::RawPtr(_, place) = rv_raw else { return None };
        if place.projection.len() != 1 { return None }
        if !matches!(place.projection[0], mir::ProjectionElem::Deref) { return None }
        Some(place.local)
    }

    for (bb_count, term) in &bbs {
        let mir::TerminatorKind::Call {
            func: c_func,
            args: c_args,
            destination: c_dest,
            target: Some(bb_after),
            ..
        } = &term.kind else { continue };
        if c_args.len() != 1 { continue }
        let mir::Operand::Constant(cc) = c_func else { continue };
        let ty::FnDef(c_def_id, _) = cc.const_.ty().kind() else { continue };
        let c_path = tcx.def_path_str(*c_def_id);
        if c_path.rsplit("::").next() != Some("count") || !c_path.contains("Iterator") { continue }

        // count(move _filter_local)
        let l_filter = match &c_args[0].node {
            mir::Operand::Copy(p) | mir::Operand::Move(p) if p.projection.is_empty() => p.local,
            _ => continue,
        };
        // filter_local = Filter { iter
        let Some(rv_filter) = find_assign(body, l_filter) else { continue };
        let mir::Rvalue::Aggregate(box agg_kind, fields) = rv_filter else { continue };
        let mir::AggregateKind::Adt(adt_def_id, _, generic_args, _, _) = agg_kind else { continue };
        let adt_path = tcx.def_path_str(*adt_def_id);
        if !adt_path.ends_with("Filter") || !adt_path.contains("filter") { continue }
        if fields.len() != 2 { continue }

        let iter_op = &fields.raw[0];
        let pred_op = &fields.raw[1];

        // iter_op: Copy/Move of slice::Iter aggregate local
        let l_iter = match iter_op {
            mir::Operand::Copy(p) | mir::Operand::Move(p) if p.projection.is_empty() => p.local,
            _ => continue,
        };
        let Some(rv_iter) = find_assign(body, l_iter) else { continue };
        let mir::Rvalue::Aggregate(box agg_iter_kind, iter_fields) = rv_iter else { continue };
        let mir::AggregateKind::Adt(iter_adt_def_id, _, iter_generic_args, _, _) = agg_iter_kind else { continue };
        let iter_adt_path = tcx.def_path_str(*iter_adt_def_id);
        if !iter_adt_path.ends_with("Iter") || !iter_adt_path.contains("slice") { continue }
        // slice::Iter::<T> - first generic arg is T
        let elem_ty = match iter_generic_args.iter().next().and_then(|g| g.as_type()) {
            Some(t) => t,
            None => continue,
        };
        // Skip primitives.
        if AccTyTag::from_ty(elem_ty).is_some() { continue }

        // Trace iter.ptr back to slice ref local.
        if iter_fields.len() < 1 { continue }
        let Some(slice_local) = trace_slice_ref_from_iter_ptr(body, &iter_fields.raw[0]) else { continue };
        // Validate slice_local has type &[T].
        let slice_ty = body.local_decls[slice_local].ty;
        let elem_ty_match = match slice_ty.kind() {
            ty::Ref(_, inner, _) => match inner.kind() {
                ty::Slice(t) => *t == elem_ty,
                _ => false,
            },
            _ => false,
        };
        if !elem_ty_match { continue }
        let slice_op = mir::Operand::Copy(mir::Place::from(slice_local));

        // pred_op: closure aggregate. Validate Fn-kind.
        let pred_ty = match pred_op {
            mir::Operand::Constant(c) => c.const_.ty(),
            mir::Operand::Move(p) | mir::Operand::Copy(p) => p.ty(&body.local_decls, tcx).ty,
            _ => continue,
        };
        if !is_fn_kind_callable(tcx, pred_ty) { continue }
        if !matches!(pred_ty.kind(), ty::Closure(..)) { continue }

        // skip bbs already planned by first scan
        if plans.iter().any(|p| p.bb_to_replace == *bb_count) { continue }

        // Suppress generic_args unused warning in stable rustc.
        let _ = generic_args;

        plans.push(Plan {
            kind: GenericKind::FilterCount,
            bb_to_replace: *bb_count,
            bb_after: *bb_after,
            result_dest: *c_dest,
            slice_op,
            elem_ty,
            body_op: pred_op.clone(),
            // no hoisting, closure aggregate already in bb_count
            method_bb_stmts: vec![],
            count_bb_stmts: vec![],
        });
    }

    let mut count = 0usize;
    for plan in plans {
        let helper_def_id = match plan.kind {
            GenericKind::ForEach => for_each_def_id,
            GenericKind::FilterCount => filter_count_def_id,
        };

        // dyn ref type from substituted helper sig
        let helper_substs = tcx.mk_args_from_iter([
            rustc_middle::ty::GenericArg::from(plan.elem_ty),
        ].into_iter());
        let helper_sig = tcx.fn_sig(helper_def_id).instantiate(tcx, helper_substs);
        let helper_sig = tcx.instantiate_bound_regions_with_erased(helper_sig);
        if helper_sig.inputs().len() != 2 { continue }
        let dyn_ref_ty = tcx.erase_and_anonymize_regions(helper_sig.inputs()[1]);

        // need closure as Place to borrow
        let closure_place = match &plan.body_op {
            mir::Operand::Move(p) | mir::Operand::Copy(p) if p.projection.is_empty() => *p,
            _ => continue,
        };
        #[allow(rustc::usage_of_qualified_ty)]
        let closure_ref_ty = ty::Ty::new_imm_ref(
            tcx,
            tcx.lifetimes.re_erased,
            body.local_decls[closure_place.local].ty,
        );
        let ref_local = body.local_decls.push(mir::LocalDecl::new(
            closure_ref_ty,
            rustc_span::DUMMY_SP,
        ));
        let dyn_local = body.local_decls.push(mir::LocalDecl::new(
            dyn_ref_ty,
            rustc_span::DUMMY_SP,
        ));
        let ref_rvalue = mir::Rvalue::Ref(
            tcx.lifetimes.re_erased,
            mir::BorrowKind::Shared,
            closure_place,
        );
        let unsize_rvalue = mir::Rvalue::Cast(
            mir::CastKind::PointerCoercion(
                ty::adjustment::PointerCoercion::Unsize,
                mir::CoercionSource::Implicit,
            ),
            mir::Operand::Copy(mir::Place::from(ref_local)),
            dyn_ref_ty,
        );
        let cast_stmts = vec![
            mir::Statement::new(
                mir::SourceInfo::outermost(rustc_span::DUMMY_SP),
                mir::StatementKind::Assign(Box::new((
                    mir::Place::from(ref_local),
                    ref_rvalue,
                ))),
            ),
            mir::Statement::new(
                mir::SourceInfo::outermost(rustc_span::DUMMY_SP),
                mir::StatementKind::Assign(Box::new((
                    mir::Place::from(dyn_local),
                    unsize_rvalue,
                ))),
            ),
        ];

        let helper_callee = mir::Operand::function_handle(
            tcx,
            helper_def_id,
            [rustc_middle::ty::GenericArg::from(plan.elem_ty)].into_iter(),
            rustc_span::DUMMY_SP,
        );
        let new_args: Box<[rustc_span::source_map::Spanned<mir::Operand<'tcx>>]> = Box::new([
            rustc_span::source_map::Spanned {
                node: plan.slice_op,
                span: rustc_span::DUMMY_SP,
            },
            rustc_span::source_map::Spanned {
                node: mir::Operand::Move(mir::Place::from(dyn_local)),
                span: rustc_span::DUMMY_SP,
            },
        ]);
        let new_term = mir::TerminatorKind::Call {
            func: helper_callee,
            args: new_args,
            destination: plan.result_dest,
            target: Some(plan.bb_after),
            unwind: mir::UnwindAction::Unreachable,
            call_source: mir::CallSource::Misc,
            fn_span: rustc_span::DUMMY_SP,
        };
        let bb_data = &mut body.basic_blocks_mut()[plan.bb_to_replace];
        // strip closure Storage*, StorageDead invalidates injected ref
        bb_data.statements.retain(|s| match &s.kind {
            mir::StatementKind::StorageDead(l) | mir::StatementKind::StorageLive(l)
                if *l == closure_place.local =>
            {
                false
            }
            _ => true,
        });
        // Hoist statements from filter/count or for_each block
        for s in plan.method_bb_stmts {
            bb_data.statements.push(s);
        }
        for s in plan.count_bb_stmts {
            bb_data.statements.push(s);
        }
        for s in cast_stmts {
            bb_data.statements.push(s);
        }
        bb_data.terminator_mut().kind = new_term;
        let kind_str = match plan.kind {
            GenericKind::ForEach => "for_each",
            GenericKind::FilterCount => "filter+count",
        };
        par_dump!(
            tcx,
            "[PAR-IDIOM-GENERIC-APPLIED] fn={} bb={} kind={} elem_ty={:?}",
            fn_name,
            plan.bb_to_replace.index(),
            kind_str,
            plan.elem_ty,
        );
        count += 1;
    }
    count
}

pub(crate) fn try_transform_iterator_for_each<'tcx>(
    tcx: TyCtxt<'tcx>,
    body: &mut mir::Body<'tcx>,
) -> usize {
    if !idiom_transform_enabled(tcx) {
        return 0;
    }
    // Never transform sysroot crate internals
    if is_in_sysroot_crate(tcx, body.source.def_id()) {
        return 0;
    }
    if body.basic_blocks.len() < 2 {
        return 0;
    }
    if !body.basic_blocks.iter().any(|bb| {
        matches!(bb.terminator().kind, mir::TerminatorKind::Call { .. })
    }) {
        return 0;
    }
    let fn_name = tcx.def_path_str(body.source.def_id());
    if fn_name.contains("parallel_runtime") || fn_name.contains("parallel_reduce_") {
        return 0;
    }

    fn lookup(tcx: TyCtxt<'_>, name: &str) -> Option<rustc_hir::def_id::DefId> {
        tcx.get_diagnostic_item(Symbol::intern(name))
    }

    fn for_each_helper_for(tag: AccTyTag, is_mut: bool) -> Option<&'static str> {
        match (tag, is_mut) {
            (AccTyTag::U64, false) => Some("parallel_runtime_parallel_for_each_slice_u64"),
            (AccTyTag::U32, false) => Some("parallel_runtime_parallel_for_each_slice_u32"),
            (AccTyTag::I64, false) => Some("parallel_runtime_parallel_for_each_slice_i64"),
            (AccTyTag::I32, false) => Some("parallel_runtime_parallel_for_each_slice_i32"),
            (AccTyTag::Usize, false) => Some("parallel_runtime_parallel_for_each_slice_usize"),
            (AccTyTag::Isize, false) => Some("parallel_runtime_parallel_for_each_slice_isize"),
            (AccTyTag::U64, true) => Some("parallel_runtime_parallel_for_each_mut_slice_u64"),
            (AccTyTag::U32, true) => Some("parallel_runtime_parallel_for_each_mut_slice_u32"),
            (AccTyTag::I64, true) => Some("parallel_runtime_parallel_for_each_mut_slice_i64"),
            (AccTyTag::I32, true) => Some("parallel_runtime_parallel_for_each_mut_slice_i32"),
            _ => None,
        }
    }
    /// Capturing-closure (Fn-kind) dyn-Fn helper, read-only only.
    fn for_each_dyn_helper_for(tag: AccTyTag) -> Option<&'static str> {
        match tag {
            AccTyTag::U64 => Some("parallel_runtime_parallel_for_each_slice_closure_dyn_u64"),
            AccTyTag::U32 => Some("parallel_runtime_parallel_for_each_slice_closure_dyn_u32"),
            AccTyTag::I64 => Some("parallel_runtime_parallel_for_each_slice_closure_dyn_i64"),
            AccTyTag::I32 => Some("parallel_runtime_parallel_for_each_slice_closure_dyn_i32"),
            _ => None,
        }
    }

    if lookup(tcx, "parallel_runtime_parallel_for_each_slice_u64").is_none() {
        return 0;
    }

    /// re-borrow tracer, same as combinator matcher
    fn resolve_through_reborrow<'tcx>(
        body: &mir::Body<'tcx>,
        bb: mir::BasicBlock,
        op: &mir::Operand<'tcx>,
    ) -> mir::Operand<'tcx> {
        let local = match op {
            mir::Operand::Move(p) | mir::Operand::Copy(p)
                if p.projection.is_empty() => p.local,
            _ => return op.clone(),
        };
        for stmt in body.basic_blocks[bb].statements.iter() {
            if let mir::StatementKind::Assign(box (place, rvalue)) = &stmt.kind {
                if place.local == local && place.projection.is_empty() {
                    if let mir::Rvalue::Ref(_, _, src_place) = rvalue {
                        if src_place.projection.len() == 1
                            && matches!(
                                src_place.projection[0],
                                mir::ProjectionElem::Deref
                            )
                        {
                            return mir::Operand::Copy(mir::Place::from(src_place.local));
                        }
                    }
                }
            }
        }
        op.clone()
    }

    #[allow(rustc::usage_of_qualified_ty)]
    fn callee_coercion_kind<'tcx>(
        ty: ty::Ty<'tcx>,
    ) -> Option<ty::adjustment::PointerCoercion> {
        match ty.kind() {
            ty::FnDef(..) => Some(ty::adjustment::PointerCoercion::ReifyFnPointer(
                rustc_hir::Safety::Safe,
            )),
            ty::Closure(_, args) => {
                if args.as_closure().upvar_tys().is_empty() {
                    Some(ty::adjustment::PointerCoercion::ClosureFnPointer(
                        rustc_hir::Safety::Safe,
                    ))
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    /// true for capturing Fn closure, FnMut unshareable
    #[allow(rustc::usage_of_qualified_ty)]
    fn is_fn_closure_with_captures<'tcx>(ty: ty::Ty<'tcx>) -> bool {
        if let ty::Closure(_, args) = ty.kind() {
            let cargs = args.as_closure();
            if cargs.upvar_tys().is_empty() {
                return false;
            }
            return matches!(
                cargs.kind_ty().kind(),
                ty::Int(rustc_middle::ty::IntTy::I8)
            );
        }
        false
    }

    #[allow(rustc::usage_of_qualified_ty)]
    struct Plan<'tcx> {
        bb_iter: mir::BasicBlock,
        bb_after: mir::BasicBlock,
        method_dest: mir::Place<'tcx>,
        slice_op: mir::Operand<'tcx>,
        body_op: mir::Operand<'tcx>,
        body_ty: ty::Ty<'tcx>,
        elem_tag: AccTyTag,
        is_mut: bool,
        /// capturing Fn closure: dyn-Fn helper, else fn-ptr
        use_dyn: bool,
        /// Pre-terminator statements of `bb_method`
        method_bb_stmts: Vec<mir::Statement<'tcx>>,
    }

    let bbs: Vec<(mir::BasicBlock, mir::Terminator<'tcx>)> = body
        .basic_blocks
        .iter_enumerated()
        .map(|(b, d)| (b, d.terminator().clone()))
        .collect();

    let mut plans: Vec<Plan<'tcx>> = Vec::new();

    for (bb_iter, term) in &bbs {
        // bb_iter: Call slice::iter or slice::iter_mut → bb_method
        let mir::TerminatorKind::Call {
            func: iter_func,
            args: iter_args,
            destination: iter_dest,
            target: Some(bb_method),
            ..
        } = &term.kind
        else { continue };
        if !iter_dest.projection.is_empty() { continue }
        let mir::Operand::Constant(ic) = iter_func else { continue };
        let ty::FnDef(iter_def_id, _) = ic.const_.ty().kind() else { continue };
        let iter_path = tcx.def_path_str(*iter_def_id);
        let last = iter_path.rsplit("::").next().unwrap_or("");
        if !iter_path.contains("slice") { continue }
        let is_mut = match last {
            "iter" => false,
            "iter_mut" => true,
            _ => continue,
        };
        if iter_args.len() != 1 { continue }
        let slice_op = resolve_through_reborrow(body, *bb_iter, &iter_args[0].node);
        let slice_ty = match &slice_op {
            mir::Operand::Copy(p) | mir::Operand::Move(p) => p.ty(&body.local_decls, tcx).ty,
            mir::Operand::Constant(c) => c.const_.ty(),
            _ => continue,
        };
        let elem_tag = match slice_ty.kind() {
            ty::Ref(_, inner, mutability) => {
                if is_mut && !matches!(mutability, rustc_ast::Mutability::Mut) {
                    continue;
                }
                match inner.kind() {
                    ty::Slice(elem) => match AccTyTag::from_ty(*elem) {
                        Some(t) => t,
                        None => continue,
                    },
                    _ => continue,
                }
            }
            _ => continue,
        };

        // bb_method: Call Iterator::for_each(iter, body) → bb_after
        let m_idx = bb_method.as_usize();
        if m_idx >= bbs.len() { continue }
        let m_term = &bbs[m_idx].1;
        let mir::TerminatorKind::Call {
            func: m_func,
            args: m_args,
            destination: m_dest,
            target: Some(bb_after),
            ..
        } = &m_term.kind
        else { continue };
        let mir::Operand::Constant(mc) = m_func else { continue };
        let ty::FnDef(m_def_id, _) = mc.const_.ty().kind() else { continue };
        let m_path = tcx.def_path_str(*m_def_id);
        if m_path.rsplit("::").next() != Some("for_each") { continue }
        if !m_path.contains("Iterator") { continue }
        if m_args.len() != 2 { continue }
        let iter_arg_ok = match &m_args[0].node {
            mir::Operand::Move(p) | mir::Operand::Copy(p) => {
                p.projection.is_empty() && p.local == iter_dest.local
            }
            _ => false,
        };
        if !iter_arg_ok { continue }

        let body_op = m_args[1].node.clone();
        let body_ty = match &body_op {
            mir::Operand::Constant(c) => c.const_.ty(),
            mir::Operand::Move(p) | mir::Operand::Copy(p) => {
                p.ty(&body.local_decls, tcx).ty
            }
            _ => continue,
        };
        // fn-ptr coercible, capturing Fn (read-only), or reject
        let use_dyn = if callee_coercion_kind(body_ty).is_some() {
            false
        } else if !is_mut && is_fn_closure_with_captures(body_ty) {
            true
        } else {
            continue;
        };

        // Capture bb_method's pre-terminator statements
        let method_bb_stmts: Vec<mir::Statement<'tcx>> =
            body.basic_blocks[*bb_method].statements.clone();
        plans.push(Plan {
            bb_iter: *bb_iter,
            bb_after: *bb_after,
            method_dest: *m_dest,
            slice_op,
            body_op,
            body_ty,
            elem_tag,
            is_mut,
            use_dyn,
            method_bb_stmts,
        });
    }

    let mut count = 0usize;
    for plan in plans {
        // fn-ptr or dyn-Fn helper per plan.use_dyn
        let helper_name = if plan.use_dyn {
            match for_each_dyn_helper_for(plan.elem_tag) {
                Some(h) => h,
                None => continue,
            }
        } else {
            match for_each_helper_for(plan.elem_tag, plan.is_mut) {
                Some(h) => h,
                None => continue,
            }
        };
        let Some(helper_def_id) = lookup(tcx, helper_name) else {
            par_dump!(tcx, "[PAR-IDIOM-FOR-EACH-NO-HELPER] fn={} helper={}", fn_name, helper_name);
            continue;
        };
        let elem_ty = match plan.elem_tag {
            AccTyTag::U64 => tcx.types.u64,
            AccTyTag::U32 => tcx.types.u32,
            AccTyTag::I64 => tcx.types.i64,
            AccTyTag::I32 => tcx.types.i32,
            AccTyTag::Usize => tcx.types.usize,
            AccTyTag::Isize => tcx.types.isize,
            _ => continue,
        };
        // dyn: &closure then Unsize; fn-ptr: reify cast
        let cast_stmts: Vec<mir::Statement<'tcx>>;
        let body_arg_op: mir::Operand<'tcx>;

        if plan.use_dyn {
            // dyn Fn arg type from helper sig
            let helper_sig = tcx.fn_sig(helper_def_id).instantiate_identity();
            let helper_sig = tcx.instantiate_bound_regions_with_erased(helper_sig);
            if helper_sig.inputs().len() != 2 {
                continue;
            }
            let dyn_ref_ty = tcx.erase_and_anonymize_regions(helper_sig.inputs()[1]);

            // closure must be a Place to ref
            let closure_place = match &plan.body_op {
                mir::Operand::Move(p) | mir::Operand::Copy(p) if p.projection.is_empty() => *p,
                _ => continue,
            };
            #[allow(rustc::usage_of_qualified_ty)]
            let closure_ref_ty = ty::Ty::new_imm_ref(
                tcx,
                tcx.lifetimes.re_erased,
                body.local_decls[closure_place.local].ty,
            );
            let ref_local = body.local_decls.push(mir::LocalDecl::new(
                closure_ref_ty,
                rustc_span::DUMMY_SP,
            ));
            let dyn_local = body.local_decls.push(mir::LocalDecl::new(
                dyn_ref_ty,
                rustc_span::DUMMY_SP,
            ));
            let ref_rvalue = mir::Rvalue::Ref(
                tcx.lifetimes.re_erased,
                mir::BorrowKind::Shared,
                closure_place,
            );
            let unsize_rvalue = mir::Rvalue::Cast(
                mir::CastKind::PointerCoercion(
                    ty::adjustment::PointerCoercion::Unsize,
                    mir::CoercionSource::Implicit,
                ),
                mir::Operand::Copy(mir::Place::from(ref_local)),
                dyn_ref_ty,
            );
            cast_stmts = vec![
                mir::Statement::new(
                    mir::SourceInfo::outermost(rustc_span::DUMMY_SP),
                    mir::StatementKind::Assign(Box::new((
                        mir::Place::from(ref_local),
                        ref_rvalue,
                    ))),
                ),
                mir::Statement::new(
                    mir::SourceInfo::outermost(rustc_span::DUMMY_SP),
                    mir::StatementKind::Assign(Box::new((
                        mir::Place::from(dyn_local),
                        unsize_rvalue,
                    ))),
                ),
            ];
            body_arg_op = mir::Operand::Move(mir::Place::from(dyn_local));
        } else {
            #[allow(rustc::usage_of_qualified_ty)]
            let arg_elem_ref_ty = if plan.is_mut {
                ty::Ty::new_mut_ref(tcx, tcx.lifetimes.re_erased, elem_ty)
            } else {
                ty::Ty::new_imm_ref(tcx, tcx.lifetimes.re_erased, elem_ty)
            };
            #[allow(rustc::usage_of_qualified_ty)]
            let body_ptr_ty = ty::Ty::new_fn_ptr(
                tcx,
                ty::Binder::dummy(tcx.mk_fn_sig(
                    [arg_elem_ref_ty],
                    tcx.types.unit,
                    false,
                    rustc_hir::Safety::Safe,
                    rustc_abi::ExternAbi::Rust,
                )),
            );
            let body_local = body.local_decls.push(mir::LocalDecl::new(
                body_ptr_ty,
                rustc_span::DUMMY_SP,
            ));
            let coercion = match callee_coercion_kind(plan.body_ty) {
                Some(c) => c,
                None => continue,
            };
            let cast_rvalue = mir::Rvalue::Cast(
                mir::CastKind::PointerCoercion(coercion, mir::CoercionSource::Implicit),
                plan.body_op,
                body_ptr_ty,
            );
            cast_stmts = vec![mir::Statement::new(
                mir::SourceInfo::outermost(rustc_span::DUMMY_SP),
                mir::StatementKind::Assign(Box::new((
                    mir::Place::from(body_local),
                    cast_rvalue,
                ))),
            )];
            body_arg_op = mir::Operand::Move(mir::Place::from(body_local));
        }

        let helper_callee = mir::Operand::function_handle(
            tcx,
            helper_def_id,
            std::iter::empty(),
            rustc_span::DUMMY_SP,
        );
        let new_args: Box<[rustc_span::source_map::Spanned<mir::Operand<'tcx>>]> = Box::new([
            rustc_span::source_map::Spanned {
                node: plan.slice_op,
                span: rustc_span::DUMMY_SP,
            },
            rustc_span::source_map::Spanned {
                node: body_arg_op,
                span: rustc_span::DUMMY_SP,
            },
        ]);
        let new_term = mir::TerminatorKind::Call {
            func: helper_callee,
            args: new_args,
            destination: plan.method_dest,
            target: Some(plan.bb_after),
            unwind: mir::UnwindAction::Unreachable,
            call_source: mir::CallSource::Misc,
            fn_span: rustc_span::DUMMY_SP,
        };
        let bb_data = &mut body.basic_blocks_mut()[plan.bb_iter];
        // hoist bb_method stmts first so closure exists
        for s in plan.method_bb_stmts {
            bb_data.statements.push(s);
        }
        for s in cast_stmts {
            bb_data.statements.push(s);
        }
        bb_data.terminator_mut().kind = new_term;
        par_dump!(
            tcx,
            "[PAR-IDIOM-FOR-EACH-APPLIED] fn={} bb={} elem_ty={:?} mut={} dyn={}",
            fn_name,
            plan.bb_iter.index(),
            plan.elem_tag,
            plan.is_mut,
            plan.use_dyn,
        );
        count += 1;
    }
    count
}

// Phase A.2, range/slice map-collect into Vec<T>

// Phase C, HashMap iter().for_each() matcher (BTreeMap later)
pub(crate) fn try_transform_hashmap_for_each<'tcx>(
    tcx: TyCtxt<'tcx>,
    body: &mut mir::Body<'tcx>,
) -> usize {
    if !idiom_transform_enabled(tcx) {
        return 0;
    }
    // Never transform sysroot crate internals
    if is_in_sysroot_crate(tcx, body.source.def_id()) {
        return 0;
    }
    if body.basic_blocks.len() < 2 {
        return 0;
    }
    if !body.basic_blocks.iter().any(|bb| {
        matches!(bb.terminator().kind, mir::TerminatorKind::Call { .. })
    }) {
        return 0;
    }
    let fn_name = tcx.def_path_str(body.source.def_id());
    if fn_name.contains("parallel_runtime") || fn_name.contains("parallel_reduce_") {
        return 0;
    }

    fn lookup(tcx: TyCtxt<'_>, name: &str) -> Option<rustc_hir::def_id::DefId> {
        tcx.get_diagnostic_item(Symbol::intern(name))
    }
    let Some(helper_def_id) = lookup(tcx, "parallel_runtime_parallel_for_each_hashmap_iter_dyn") else {
        return 0;
    };

    /// trace map re-borrow to original param
    fn resolve_through_reborrow<'tcx>(
        body: &mir::Body<'tcx>,
        bb: mir::BasicBlock,
        op: &mir::Operand<'tcx>,
    ) -> mir::Operand<'tcx> {
        let local = match op {
            mir::Operand::Move(p) | mir::Operand::Copy(p)
                if p.projection.is_empty() => p.local,
            _ => return op.clone(),
        };
        for stmt in body.basic_blocks[bb].statements.iter() {
            if let mir::StatementKind::Assign(box (place, rvalue)) = &stmt.kind {
                if place.local == local && place.projection.is_empty() {
                    if let mir::Rvalue::Ref(_, _, src_place) = rvalue {
                        if src_place.projection.len() == 1
                            && matches!(src_place.projection[0], mir::ProjectionElem::Deref)
                        {
                            return mir::Operand::Copy(mir::Place::from(src_place.local));
                        }
                    }
                }
            }
        }
        op.clone()
    }

    /// True iff `ty` is a Fn-kind closure
    #[allow(rustc::usage_of_qualified_ty)]
    fn is_fn_closure<'tcx>(ty: ty::Ty<'tcx>) -> bool {
        match ty.kind() {
            ty::Closure(_, args) => matches!(
                args.as_closure().kind_ty().kind(),
                ty::Int(rustc_middle::ty::IntTy::I8)
            ),
            // Top-level fn item also acceptable
            ty::FnDef(..) => true,
            _ => false,
        }
    }

    #[allow(rustc::usage_of_qualified_ty, dead_code)]
    struct Plan<'tcx> {
        bb_iter: mir::BasicBlock,
        bb_after: mir::BasicBlock,
        method_dest: mir::Place<'tcx>,
        map_op: mir::Operand<'tcx>,
        body_op: mir::Operand<'tcx>,
        body_ty: ty::Ty<'tcx>,
        k_ty: ty::Ty<'tcx>,
        v_ty: ty::Ty<'tcx>,
        method_bb_stmts: Vec<mir::Statement<'tcx>>,
    }

    let bbs: Vec<(mir::BasicBlock, mir::Terminator<'tcx>)> = body
        .basic_blocks
        .iter_enumerated()
        .map(|(b, d)| (b, d.terminator().clone()))
        .collect();

    let mut plans: Vec<Plan<'tcx>> = Vec::new();

    for (bb_iter, term) in &bbs {
        let mir::TerminatorKind::Call {
            func: i_func,
            args: i_args,
            destination: iter_dest,
            target: Some(bb_method),
            ..
        } = &term.kind else { continue };
        if !iter_dest.projection.is_empty() { continue }
        let mir::Operand::Constant(ic) = i_func else { continue };
        let ty::FnDef(i_def_id, _) = ic.const_.ty().kind() else { continue };
        let i_path = tcx.def_path_str(*i_def_id);
        // only HashMap::iter, helper takes &HashMap<K, V>
        if i_path.rsplit("::").next() != Some("iter") { continue }
        if !i_path.contains("HashMap") { continue }
        if i_args.len() != 1 { continue }
        let map_op = resolve_through_reborrow(body, *bb_iter, &i_args[0].node);
        let map_ty = match &map_op {
            mir::Operand::Copy(p) | mir::Operand::Move(p) => p.ty(&body.local_decls, tcx).ty,
            mir::Operand::Constant(c) => c.const_.ty(),
            _ => continue,
        };
        // Strip outer reference if present
        let inner_map_ty = match map_ty.kind() {
            ty::Ref(_, inner, _) => *inner,
            _ => map_ty,
        };
        let (k_ty, v_ty) = match inner_map_ty.kind() {
            ty::Adt(adt, args) => {
                let path = tcx.def_path_str(adt.did());
                if !path.contains("HashMap") { continue }
                let mut tys = args.types();
                let Some(k) = tys.next() else { continue };
                let Some(v) = tys.next() else { continue };
                (k, v)
            }
            _ => continue,
        };

        // bb_method: Call <hash_map::Iter as Iterator>::for_each → bb_after
        let m_idx = bb_method.as_usize();
        if m_idx >= bbs.len() { continue }
        let m_term = &bbs[m_idx].1;
        let mir::TerminatorKind::Call {
            func: m_func,
            args: m_args,
            destination: m_dest,
            target: Some(bb_after),
            ..
        } = &m_term.kind else { continue };
        let mir::Operand::Constant(mc) = m_func else { continue };
        let ty::FnDef(m_def_id, _) = mc.const_.ty().kind() else { continue };
        let m_path = tcx.def_path_str(*m_def_id);
        if m_path.rsplit("::").next() != Some("for_each") { continue }
        if !m_path.contains("Iterator") { continue }
        if m_args.len() != 2 { continue }
        let iter_arg_ok = match &m_args[0].node {
            mir::Operand::Move(p) | mir::Operand::Copy(p) => {
                p.projection.is_empty() && p.local == iter_dest.local
            }
            _ => false,
        };
        if !iter_arg_ok { continue }
        let body_op = m_args[1].node.clone();
        let body_ty = match &body_op {
            mir::Operand::Constant(c) => c.const_.ty(),
            mir::Operand::Move(p) | mir::Operand::Copy(p) => p.ty(&body.local_decls, tcx).ty,
            _ => continue,
        };
        if !is_fn_closure(body_ty) { continue }

        let method_bb_stmts: Vec<mir::Statement<'tcx>> =
            body.basic_blocks[*bb_method].statements.clone();
        plans.push(Plan {
            bb_iter: *bb_iter,
            bb_after: *bb_after,
            method_dest: *m_dest,
            map_op,
            body_op,
            body_ty,
            k_ty,
            v_ty,
            method_bb_stmts,
        });
    }

    let mut count = 0usize;
    for plan in plans {
        // dyn param type from substituted helper sig
        let helper_substs = tcx.mk_args_from_iter([
            rustc_middle::ty::GenericArg::from(plan.k_ty),
            rustc_middle::ty::GenericArg::from(plan.v_ty),
        ].into_iter());
        let helper_sig = tcx.fn_sig(helper_def_id).instantiate(tcx, helper_substs);
        let helper_sig = tcx.instantiate_bound_regions_with_erased(helper_sig);
        if helper_sig.inputs().len() != 2 { continue }
        let dyn_ref_ty = tcx.erase_and_anonymize_regions(helper_sig.inputs()[1]);

        // closure as Place to take its ref
        let closure_place = match &plan.body_op {
            mir::Operand::Move(p) | mir::Operand::Copy(p) if p.projection.is_empty() => *p,
            // TODO fn-item Constant lacks Place, skipped
            _ => continue,
        };
        #[allow(rustc::usage_of_qualified_ty)]
        let closure_ref_ty = ty::Ty::new_imm_ref(
            tcx,
            tcx.lifetimes.re_erased,
            body.local_decls[closure_place.local].ty,
        );
        let ref_local = body.local_decls.push(mir::LocalDecl::new(
            closure_ref_ty,
            rustc_span::DUMMY_SP,
        ));
        let dyn_local = body.local_decls.push(mir::LocalDecl::new(
            dyn_ref_ty,
            rustc_span::DUMMY_SP,
        ));
        let ref_rvalue = mir::Rvalue::Ref(
            tcx.lifetimes.re_erased,
            mir::BorrowKind::Shared,
            closure_place,
        );
        let unsize_rvalue = mir::Rvalue::Cast(
            mir::CastKind::PointerCoercion(
                ty::adjustment::PointerCoercion::Unsize,
                mir::CoercionSource::Implicit,
            ),
            mir::Operand::Copy(mir::Place::from(ref_local)),
            dyn_ref_ty,
        );
        let stmts = vec![
            mir::Statement::new(
                mir::SourceInfo::outermost(rustc_span::DUMMY_SP),
                mir::StatementKind::Assign(Box::new((
                    mir::Place::from(ref_local),
                    ref_rvalue,
                ))),
            ),
            mir::Statement::new(
                mir::SourceInfo::outermost(rustc_span::DUMMY_SP),
                mir::StatementKind::Assign(Box::new((
                    mir::Place::from(dyn_local),
                    unsize_rvalue,
                ))),
            ),
        ];

        // helper Call with K, V generic args
        let helper_callee = mir::Operand::function_handle(
            tcx,
            helper_def_id,
            [
                rustc_middle::ty::GenericArg::from(plan.k_ty),
                rustc_middle::ty::GenericArg::from(plan.v_ty),
            ].into_iter(),
            rustc_span::DUMMY_SP,
        );
        let new_args: Box<[rustc_span::source_map::Spanned<mir::Operand<'tcx>>]> = Box::new([
            rustc_span::source_map::Spanned {
                node: plan.map_op,
                span: rustc_span::DUMMY_SP,
            },
            rustc_span::source_map::Spanned {
                node: mir::Operand::Move(mir::Place::from(dyn_local)),
                span: rustc_span::DUMMY_SP,
            },
        ]);
        let new_term = mir::TerminatorKind::Call {
            func: helper_callee,
            args: new_args,
            destination: plan.method_dest,
            target: Some(plan.bb_after),
            unwind: mir::UnwindAction::Unreachable,
            call_source: mir::CallSource::Misc,
            fn_span: rustc_span::DUMMY_SP,
        };
        let bb_data = &mut body.basic_blocks_mut()[plan.bb_iter];
        // Hoist bb_method's pre-terminator statements
        for s in plan.method_bb_stmts {
            bb_data.statements.push(s);
        }
        for s in stmts {
            bb_data.statements.push(s);
        }
        bb_data.terminator_mut().kind = new_term;
        par_dump!(
            tcx,
            "[PAR-IDIOM-HASHMAP-FOR-EACH-APPLIED] fn={} bb={} k_ty={:?} v_ty={:?}",
            fn_name,
            plan.bb_iter.index(),
            plan.k_ty,
            plan.v_ty,
        );
        count += 1;
    }
    count
}

pub(crate) fn try_transform_iterator_collect<'tcx>(
    tcx: TyCtxt<'tcx>,
    body: &mut mir::Body<'tcx>,
) -> usize {
    if !idiom_transform_enabled(tcx) {
        return 0;
    }
    // Never transform sysroot crate internals
    if is_in_sysroot_crate(tcx, body.source.def_id()) {
        return 0;
    }
    if body.basic_blocks.len() < 2 {
        return 0;
    }
    if !body.basic_blocks.iter().any(|bb| {
        matches!(bb.terminator().kind, mir::TerminatorKind::Call { .. })
    }) {
        return 0;
    }
    let fn_name = tcx.def_path_str(body.source.def_id());
    if fn_name.contains("parallel_runtime") || fn_name.contains("parallel_reduce_") {
        return 0;
    }

    fn lookup(tcx: TyCtxt<'_>, name: &str) -> Option<rustc_hir::def_id::DefId> {
        tcx.get_diagnostic_item(Symbol::intern(name))
    }
    if lookup(tcx, "parallel_runtime_parallel_collect_range_map_u64").is_none() {
        return 0;
    }

    fn range_collect_helper(tag: AccTyTag) -> Option<&'static str> {
        match tag {
            AccTyTag::U64 => Some("parallel_runtime_parallel_collect_range_map_u64"),
            AccTyTag::U32 => Some("parallel_runtime_parallel_collect_range_map_u32"),
            AccTyTag::I64 => Some("parallel_runtime_parallel_collect_range_map_i64"),
            AccTyTag::I32 => Some("parallel_runtime_parallel_collect_range_map_i32"),
            _ => None,
        }
    }
    fn slice_collect_helper(tag: AccTyTag) -> Option<&'static str> {
        match tag {
            AccTyTag::U64 => Some("parallel_runtime_parallel_collect_slice_map_u64"),
            AccTyTag::U32 => Some("parallel_runtime_parallel_collect_slice_map_u32"),
            AccTyTag::I64 => Some("parallel_runtime_parallel_collect_slice_map_i64"),
            AccTyTag::I32 => Some("parallel_runtime_parallel_collect_slice_map_i32"),
            _ => None,
        }
    }

    fn resolve_through_reborrow<'tcx>(
        body: &mir::Body<'tcx>,
        bb: mir::BasicBlock,
        op: &mir::Operand<'tcx>,
    ) -> mir::Operand<'tcx> {
        let local = match op {
            mir::Operand::Move(p) | mir::Operand::Copy(p)
                if p.projection.is_empty() => p.local,
            _ => return op.clone(),
        };
        for stmt in body.basic_blocks[bb].statements.iter() {
            if let mir::StatementKind::Assign(box (place, rvalue)) = &stmt.kind {
                if place.local == local && place.projection.is_empty() {
                    if let mir::Rvalue::Ref(_, _, src_place) = rvalue {
                        if src_place.projection.len() == 1
                            && matches!(
                                src_place.projection[0],
                                mir::ProjectionElem::Deref
                            )
                        {
                            return mir::Operand::Copy(mir::Place::from(src_place.local));
                        }
                    }
                }
            }
        }
        op.clone()
    }

    #[allow(rustc::usage_of_qualified_ty)]
    fn callee_coercion_kind<'tcx>(
        ty: ty::Ty<'tcx>,
    ) -> Option<ty::adjustment::PointerCoercion> {
        match ty.kind() {
            ty::FnDef(..) => Some(ty::adjustment::PointerCoercion::ReifyFnPointer(
                rustc_hir::Safety::Safe,
            )),
            ty::Closure(_, args) => {
                if args.as_closure().upvar_tys().is_empty() {
                    Some(ty::adjustment::PointerCoercion::ClosureFnPointer(
                        rustc_hir::Safety::Safe,
                    ))
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    /// end operand of zero-start Range aggregate
    #[allow(rustc::usage_of_qualified_ty)]
    fn find_range_end<'tcx>(
        body: &mir::Body<'tcx>,
        bb: mir::BasicBlock,
        target_local: mir::Local,
    ) -> Option<(mir::Operand<'tcx>, AccTyTag)> {
        for stmt in body.basic_blocks[bb].statements.iter() {
            if let mir::StatementKind::Assign(box (place, rvalue)) = &stmt.kind {
                if place.local != target_local || !place.projection.is_empty() {
                    continue;
                }
                if let mir::Rvalue::Aggregate(box agg_kind, fields) = rvalue {
                    let mir::AggregateKind::Adt(adt_def_id, _, gargs, _, _) = agg_kind else {
                        continue;
                    };
                    let path = body.local_decls[place.local].ty;
                    let ty::Adt(_, args) = path.kind() else { continue };
                    let _ = (adt_def_id, gargs);
                    let p = body.local_decls[place.local].ty.to_string();
                    if !p.contains("Range<") {
                        continue;
                    }
                    let elem_ty = args.types().next()?;
                    let tag = AccTyTag::from_ty(elem_ty)?;
                    if fields.len() != 2 { continue }
                    // Iterate the IndexVec to find index-0
                    let ops: Vec<&mir::Operand<'tcx>> = fields.iter().collect();
                    if ops.len() != 2 { continue }
                    let is_zero_start = match ops[0] {
                        mir::Operand::Constant(c) => {
                            c.const_.try_to_scalar_int()
                                .map(|s| s.to_uint(s.size()) == 0)
                                .unwrap_or(false)
                        }
                        _ => false,
                    };
                    if !is_zero_start { continue }
                    return Some((ops[1].clone(), tag));
                }
            }
        }
        None
    }

    #[allow(rustc::usage_of_qualified_ty)]
    enum CollectKind<'tcx> {
        Range {
            end_op: mir::Operand<'tcx>,
            elem_tag: AccTyTag,
        },
        Slice {
            slice_op: mir::Operand<'tcx>,
            elem_tag: AccTyTag,
        },
    }

    #[allow(rustc::usage_of_qualified_ty)]
    struct Plan<'tcx> {
        kind: CollectKind<'tcx>,
        bb_to_replace: mir::BasicBlock,
        bb_after: mir::BasicBlock,
        result_dest: mir::Place<'tcx>,
        body_op: mir::Operand<'tcx>,
        body_ty: ty::Ty<'tcx>,
    }

    let bbs: Vec<(mir::BasicBlock, mir::Terminator<'tcx>)> = body
        .basic_blocks
        .iter_enumerated()
        .map(|(b, d)| (b, d.terminator().clone()))
        .collect();

    let mut plans: Vec<Plan<'tcx>> = Vec::new();

    // range form: Range stmt, map, collect chain
    for (bb_map, term) in &bbs {
        let mir::TerminatorKind::Call {
            func: m_func,
            args: m_args,
            destination: map_dest,
            target: Some(bb_collect),
            ..
        } = &term.kind else { continue };
        let mir::Operand::Constant(mc) = m_func else { continue };
        let ty::FnDef(m_def_id, _) = mc.const_.ty().kind() else { continue };
        let m_path = tcx.def_path_str(*m_def_id);
        if m_path.rsplit("::").next() != Some("map") { continue }
        if !m_path.contains("Iterator") { continue }
        if m_args.len() != 2 { continue }
        // arg[0] is Move/Copy of Range aggregate local
        let iter_local = match &m_args[0].node {
            mir::Operand::Move(p) | mir::Operand::Copy(p) if p.projection.is_empty() => p.local,
            _ => continue,
        };
        // look for Range aggregate in this bb
        let range_info = find_range_end(body, *bb_map, iter_local);
        let body_op = m_args[1].node.clone();
        let body_ty = match &body_op {
            mir::Operand::Constant(c) => c.const_.ty(),
            mir::Operand::Move(p) | mir::Operand::Copy(p) => p.ty(&body.local_decls, tcx).ty,
            _ => continue,
        };
        if callee_coercion_kind(body_ty).is_none() { continue }

        // bb_collect: Call collect → bb_after
        let c_idx = bb_collect.as_usize();
        if c_idx >= bbs.len() { continue }
        let c_term = &bbs[c_idx].1;
        let mir::TerminatorKind::Call {
            func: c_func,
            args: c_args,
            destination: c_dest,
            target: Some(bb_after),
            ..
        } = &c_term.kind else { continue };
        let mir::Operand::Constant(cc) = c_func else { continue };
        let ty::FnDef(c_def_id, c_substs) = cc.const_.ty().kind() else { continue };
        let c_path = tcx.def_path_str(*c_def_id);
        if c_path.rsplit("::").next() != Some("collect") { continue }
        // require B == Vec<elem_tag>, avoids bitcast mismatch
        let target_ty = c_substs.iter().nth(1).and_then(|a| a.as_type());
        #[allow(rustc::usage_of_qualified_ty)] let target_inner: Option<rustc_middle::ty::Ty<'tcx>> = match target_ty {
            Some(t) => match t.kind() {
                ty::Adt(adt, args) => {
                    let path = tcx.def_path_str(adt.did());
                    if !path.contains("Vec") {
                        None
                    } else {
                        args.types().next()
                    }
                }
                _ => None,
            },
            None => None,
        };
        let target_inner = match target_inner {
            Some(t) => t,
            None => continue,
        };
        let target_tag = match AccTyTag::from_ty(target_inner) {
            Some(t) => t,
            None => continue,
        };
        if c_args.len() != 1 { continue }
        let c_arg_ok = match &c_args[0].node {
            mir::Operand::Move(p) | mir::Operand::Copy(p) => {
                p.projection.is_empty() && p.local == map_dest.local
            }
            _ => false,
        };
        if !c_arg_ok { continue }

        if let Some((end_op, tag)) = range_info {
            // Range helpers all take `start
            if tag != AccTyTag::U64 { continue }
            plans.push(Plan {
                kind: CollectKind::Range { end_op, elem_tag: target_tag },
                bb_to_replace: *bb_map,
                bb_after: *bb_after,
                result_dest: *c_dest,
                body_op,
                body_ty,
            });
            continue;
        }
    }

    // slice form: iter, map, collect Call chain
    for (bb_iter, term) in &bbs {
        let mir::TerminatorKind::Call {
            func: i_func,
            args: i_args,
            destination: iter_dest,
            target: Some(bb_map),
            ..
        } = &term.kind else { continue };
        if !iter_dest.projection.is_empty() { continue }
        let mir::Operand::Constant(ic) = i_func else { continue };
        let ty::FnDef(i_def_id, _) = ic.const_.ty().kind() else { continue };
        let i_path = tcx.def_path_str(*i_def_id);
        if i_path.rsplit("::").next() != Some("iter") || !i_path.contains("slice") { continue }
        if i_args.len() != 1 { continue }
        let slice_op = resolve_through_reborrow(body, *bb_iter, &i_args[0].node);
        let slice_ty = match &slice_op {
            mir::Operand::Copy(p) | mir::Operand::Move(p) => p.ty(&body.local_decls, tcx).ty,
            mir::Operand::Constant(c) => c.const_.ty(),
            _ => continue,
        };
        let elem_tag = match slice_ty.kind() {
            ty::Ref(_, inner, _) => match inner.kind() {
                ty::Slice(elem) => match AccTyTag::from_ty(*elem) { Some(t) => t, None => continue },
                _ => continue,
            },
            _ => continue,
        };
        // bb_map
        let m_idx = bb_map.as_usize();
        if m_idx >= bbs.len() { continue }
        let m_term = &bbs[m_idx].1;
        let mir::TerminatorKind::Call {
            func: m_func,
            args: m_args,
            destination: map_dest,
            target: Some(bb_collect),
            ..
        } = &m_term.kind else { continue };
        let mir::Operand::Constant(mc) = m_func else { continue };
        let ty::FnDef(m_def_id, _) = mc.const_.ty().kind() else { continue };
        let m_path = tcx.def_path_str(*m_def_id);
        if m_path.rsplit("::").next() != Some("map") { continue }
        if !m_path.contains("Iterator") { continue }
        if m_args.len() != 2 { continue }
        let iter_arg_ok = matches!(&m_args[0].node, mir::Operand::Move(p) | mir::Operand::Copy(p) if p.projection.is_empty() && p.local == iter_dest.local);
        if !iter_arg_ok { continue }
        let body_op = m_args[1].node.clone();
        let body_ty = match &body_op {
            mir::Operand::Constant(c) => c.const_.ty(),
            mir::Operand::Move(p) | mir::Operand::Copy(p) => p.ty(&body.local_decls, tcx).ty,
            _ => continue,
        };
        if callee_coercion_kind(body_ty).is_none() { continue }

        // bb_collect
        let c_idx = bb_collect.as_usize();
        if c_idx >= bbs.len() { continue }
        let c_term = &bbs[c_idx].1;
        let mir::TerminatorKind::Call {
            func: c_func,
            args: c_args,
            destination: c_dest,
            target: Some(bb_after),
            ..
        } = &c_term.kind else { continue };
        let mir::Operand::Constant(cc) = c_func else { continue };
        let ty::FnDef(c_def_id, c_substs) = cc.const_.ty().kind() else { continue };
        let c_path = tcx.def_path_str(*c_def_id);
        if c_path.rsplit("::").next() != Some("collect") { continue }
        // collect's target = Vec<T_out>
        let target_ty = c_substs.iter().nth(1).and_then(|a| a.as_type());
        #[allow(rustc::usage_of_qualified_ty)] let target_inner: Option<rustc_middle::ty::Ty<'tcx>> = match target_ty {
            Some(t) => match t.kind() {
                ty::Adt(adt, args) => {
                    if !tcx.def_path_str(adt.did()).contains("Vec") { None }
                    else { args.types().next() }
                }
                _ => None,
            },
            None => None,
        };
        let target_inner = match target_inner { Some(t) => t, None => continue };
        let target_tag = match AccTyTag::from_ty(target_inner) { Some(t) => t, None => continue };
        // slice form helper needs T_in == T_out
        if target_tag != elem_tag { continue }
        if c_args.len() != 1 { continue }
        let c_arg_ok = matches!(&c_args[0].node, mir::Operand::Move(p) | mir::Operand::Copy(p) if p.projection.is_empty() && p.local == map_dest.local);
        if !c_arg_ok { continue }

        plans.push(Plan {
            kind: CollectKind::Slice { slice_op, elem_tag },
            bb_to_replace: *bb_iter,
            bb_after: *bb_after,
            result_dest: *c_dest,
            body_op,
            body_ty,
        });
    }

    let mut count = 0usize;
    for plan in plans {
        let (helper_name, elem_tag, slice_or_end_op, is_range) = match plan.kind {
            CollectKind::Range { end_op, elem_tag } => {
                let h = match range_collect_helper(elem_tag) { Some(h) => h, None => continue };
                (h, elem_tag, end_op, true)
            }
            CollectKind::Slice { slice_op, elem_tag } => {
                let h = match slice_collect_helper(elem_tag) { Some(h) => h, None => continue };
                (h, elem_tag, slice_op, false)
            }
        };
        let Some(helper_def_id) = lookup(tcx, helper_name) else {
            par_dump!(tcx, "[PAR-IDIOM-COLLECT-NO-HELPER] fn={} helper={}", fn_name, helper_name);
            continue;
        };
        let elem_ty = match elem_tag {
            AccTyTag::U64 => tcx.types.u64,
            AccTyTag::U32 => tcx.types.u32,
            AccTyTag::I64 => tcx.types.i64,
            AccTyTag::I32 => tcx.types.i32,
            _ => continue,
        };
        // Build body fn-ptr type
        #[allow(rustc::usage_of_qualified_ty)]
        let body_ptr_ty = if is_range {
            ty::Ty::new_fn_ptr(
                tcx,
                ty::Binder::dummy(tcx.mk_fn_sig(
                    [tcx.types.u64],
                    elem_ty,
                    false,
                    rustc_hir::Safety::Safe,
                    rustc_abi::ExternAbi::Rust,
                )),
            )
        } else {
            let ref_elem = ty::Ty::new_imm_ref(tcx, tcx.lifetimes.re_erased, elem_ty);
            ty::Ty::new_fn_ptr(
                tcx,
                ty::Binder::dummy(tcx.mk_fn_sig(
                    [ref_elem],
                    elem_ty,
                    false,
                    rustc_hir::Safety::Safe,
                    rustc_abi::ExternAbi::Rust,
                )),
            )
        };
        let body_local = body.local_decls.push(mir::LocalDecl::new(
            body_ptr_ty,
            rustc_span::DUMMY_SP,
        ));
        let coercion = match callee_coercion_kind(plan.body_ty) {
            Some(c) => c,
            None => continue,
        };
        let cast_rvalue = mir::Rvalue::Cast(
            mir::CastKind::PointerCoercion(coercion, mir::CoercionSource::Implicit),
            plan.body_op,
            body_ptr_ty,
        );
        let cast_stmt = mir::Statement::new(
            mir::SourceInfo::outermost(rustc_span::DUMMY_SP),
            mir::StatementKind::Assign(Box::new((
                mir::Place::from(body_local),
                cast_rvalue,
            ))),
        );
        let helper_callee = mir::Operand::function_handle(
            tcx,
            helper_def_id,
            std::iter::empty(),
            rustc_span::DUMMY_SP,
        );
        let new_args: Box<[rustc_span::source_map::Spanned<mir::Operand<'tcx>>]> = if is_range {
            // emit start = 0, matcher requires 0..end
            let zero_const = mir::ConstOperand {
                span: rustc_span::DUMMY_SP,
                user_ty: None,
                const_: mir::Const::from_bits(
                    tcx,
                    0u128,
                    ty::TypingEnv::fully_monomorphized(),
                    tcx.types.u64,
                ),
            };
            Box::new([
                rustc_span::source_map::Spanned {
                    node: mir::Operand::Constant(Box::new(zero_const)),
                    span: rustc_span::DUMMY_SP,
                },
                rustc_span::source_map::Spanned {
                    node: slice_or_end_op,
                    span: rustc_span::DUMMY_SP,
                },
                rustc_span::source_map::Spanned {
                    node: mir::Operand::Move(mir::Place::from(body_local)),
                    span: rustc_span::DUMMY_SP,
                },
            ])
        } else {
            // helper sig: fn(slice: &[T], body) -> Vec<T>
            Box::new([
                rustc_span::source_map::Spanned {
                    node: slice_or_end_op,
                    span: rustc_span::DUMMY_SP,
                },
                rustc_span::source_map::Spanned {
                    node: mir::Operand::Move(mir::Place::from(body_local)),
                    span: rustc_span::DUMMY_SP,
                },
            ])
        };
        let new_term = mir::TerminatorKind::Call {
            func: helper_callee,
            args: new_args,
            destination: plan.result_dest,
            target: Some(plan.bb_after),
            unwind: mir::UnwindAction::Unreachable,
            call_source: mir::CallSource::Misc,
            fn_span: rustc_span::DUMMY_SP,
        };
        let bb_data = &mut body.basic_blocks_mut()[plan.bb_to_replace];
        bb_data.statements.push(cast_stmt);
        bb_data.terminator_mut().kind = new_term;
        par_dump!(
            tcx,
            "[PAR-IDIOM-COLLECT-APPLIED] fn={} bb={} elem_ty={:?} kind={}",
            fn_name,
            plan.bb_to_replace.index(),
            elem_tag,
            if is_range { "range" } else { "slice" },
        );
        count += 1;
    }
    count
}

// parallel_invoke matcher: two independent fn calls forked

/// min weighted body cost, ~500 ns break-even
const MIN_PARALLEL_INVOKE_BODY_COST: usize = 30;

/// Walk through any number of "passthrough" blocks
fn walk_passthrough_blocks<'tcx>(
    body: &mir::Body<'tcx>,
    start: mir::BasicBlock,
    tainted: mir::Local,
    forbidden: Option<mir::Local>,
    max_steps: usize,
    out_stmts: &mut Vec<mir::Statement<'tcx>>,
) -> Option<mir::BasicBlock> {
    let mut current = start;
    for _ in 0..max_steps {
        let bb_data = &body.basic_blocks[current];
        // Statements must not read `tainted`
        for stmt in &bb_data.statements {
            match &stmt.kind {
                mir::StatementKind::Assign(box (place, rvalue)) => {
                    if let Some(forbid) = forbidden {
                        if place.local == forbid {
                            return None;
                        }
                    }
                    if rvalue_reads_local(rvalue, tainted) {
                        return None;
                    }
                }
                mir::StatementKind::StorageLive(_)
                | mir::StatementKind::StorageDead(_)
                | mir::StatementKind::Nop => {}
                _ => return None,
            }
        }
        match &bb_data.terminator().kind {
            mir::TerminatorKind::Goto { target } => {
                // Passthrough block - accumulate statements and continue
                out_stmts.extend(bb_data.statements.iter().cloned());
                current = *target;
            }
            _ => {
                // real terminator, leave its stmts to bb_b
                return Some(current);
            }
        }
    }
    None
}

// per-thread in-progress bodies, rejects mutual-recursion cycles
thread_local! {
    static IN_PROGRESS_BODIES: std::cell::RefCell<Vec<rustc_hir::def_id::DefId>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

struct InProgressGuard {
    pushed: bool,
}
impl InProgressGuard {
    fn try_enter(did: rustc_hir::def_id::DefId) -> Option<Self> {
        IN_PROGRESS_BODIES.with(|s| {
            let mut v = s.borrow_mut();
            if v.contains(&did) {
                None
            } else {
                v.push(did);
                Some(InProgressGuard { pushed: true })
            }
        })
    }
}
impl Drop for InProgressGuard {
    fn drop(&mut self) {
        if self.pushed {
            IN_PROGRESS_BODIES.with(|s| { s.borrow_mut().pop(); });
        }
    }
}

/// Recursive "weighted cost" of a fn body
fn body_weighted_cost<'tcx>(
    tcx: TyCtxt<'tcx>,
    did: rustc_hir::def_id::DefId,
    depth: u32,
    body_did: rustc_hir::def_id::DefId,
) -> usize {
    if did == body_did {
        return 30;
    }
    let Some(_guard) = InProgressGuard::try_enter(did) else {
        // Mutual-recursion cycle: bail with a flat estimate.
        return 30;
    };
    if !tcx.is_mir_available(did) {
        // Cross-crate or unavailable - assume non-trivial
        return 50;
    }
    if depth > 2 {
        // Recursion guard.
        return 0;
    }
    let mir = tcx.optimized_mir(did);
    let mut cost = 0usize;
    // loop back-edge: flat count undercounts, weight up
    let mut has_loop = false;
    for (bb, data) in mir.basic_blocks.iter_enumerated() {
        for stmt in &data.statements {
            cost += match stmt.kind {
                mir::StatementKind::StorageLive(_)
                | mir::StatementKind::StorageDead(_)
                | mir::StatementKind::Nop => 0,
                _ => 1,
            };
        }
        if let mir::TerminatorKind::Call { func, .. } = &data.terminator().kind {
            // Base call overhead + recursive estimate (depth-bounded).
            cost = cost.saturating_add(10);
            if depth > 0 {
                if let mir::Operand::Constant(c) = func {
                    if let ty::FnDef(callee_did, _) = c.const_.ty().kind() {
                        cost = cost.saturating_add(
                            body_weighted_cost(tcx, *callee_did, depth - 1, body_did),
                        );
                    }
                }
            }
        }
        if !has_loop {
            for succ in data.terminator().successors() {
                if succ.index() <= bb.index() {
                    has_loop = true;
                    break;
                }
            }
        }
    }
    if has_loop {
        // Loop bodies execute many times
        cost = cost.saturating_mul(8);
    }
    cost
}

/// 2-way helper diagnostic name for arity 0..=20
fn invoke_helper_name_for_arity(arity: usize) -> Option<&'static str> {
    if arity > 20 { return None; }
    // static array avoids leaking String per call
    static NAMES: [&str; 21] = [
        "parallel_runtime_parallel_invoke_2_fn0",
        "parallel_runtime_parallel_invoke_2_fn1",
        "parallel_runtime_parallel_invoke_2_fn2",
        "parallel_runtime_parallel_invoke_2_fn3",
        "parallel_runtime_parallel_invoke_2_fn4",
        "parallel_runtime_parallel_invoke_2_fn5",
        "parallel_runtime_parallel_invoke_2_fn6",
        "parallel_runtime_parallel_invoke_2_fn7",
        "parallel_runtime_parallel_invoke_2_fn8",
        "parallel_runtime_parallel_invoke_2_fn9",
        "parallel_runtime_parallel_invoke_2_fn10",
        "parallel_runtime_parallel_invoke_2_fn11",
        "parallel_runtime_parallel_invoke_2_fn12",
        "parallel_runtime_parallel_invoke_2_fn13",
        "parallel_runtime_parallel_invoke_2_fn14",
        "parallel_runtime_parallel_invoke_2_fn15",
        "parallel_runtime_parallel_invoke_2_fn16",
        "parallel_runtime_parallel_invoke_2_fn17",
        "parallel_runtime_parallel_invoke_2_fn18",
        "parallel_runtime_parallel_invoke_2_fn19",
        "parallel_runtime_parallel_invoke_2_fn20",
    ];
    Some(NAMES[arity])
}

/// cheap pre-filter, skips bodies that cannot match
fn parallel_pass_quick_skip<'tcx>(
    tcx: TyCtxt<'tcx>,
    body: &mir::Body<'tcx>,
    min_blocks: usize,
    requires_call_terminator: bool,
) -> bool {
    // Block count check. O(1).
    if body.basic_blocks.len() < min_blocks {
        return true;
    }
    // Skip fns annotated #[inline]/#[inline(always)]
    let did = body.source.def_id();
    if did.is_local() {
        use rustc_hir::def::DefKind;
        if matches!(
            tcx.def_kind(did),
            DefKind::Fn | DefKind::AssocFn | DefKind::Closure | DefKind::Ctor(..)
        ) {
            let attrs = tcx.codegen_fn_attrs(did);
            use rustc_hir::attrs::InlineAttr;
            use rustc_middle::middle::codegen_fn_attrs::CodegenFnAttrFlags;
            if matches!(attrs.inline, InlineAttr::Always)
                || attrs.flags.contains(CodegenFnAttrFlags::NAKED)
            {
                return true;
            }
        }
    }
    // any Call terminator? walks terminators only
    if requires_call_terminator {
        let mut has_call = false;
        for bb in body.basic_blocks.iter() {
            if matches!(bb.terminator().kind, mir::TerminatorKind::Call { .. }) {
                has_call = true;
                break;
            }
        }
        if !has_call {
            return true;
        }
    }
    false
}

// direct 3/4-way invoke matcher, runs before 2-way

#[allow(rustc::usage_of_qualified_ty)]
struct NWayInvokePlan<'tcx> {
    bb_a: mir::BasicBlock,
    bb_after: mir::BasicBlock,
    callees: Vec<(rustc_hir::def_id::DefId, ty::GenericArgsRef<'tcx>)>,
    /// per-callee arg vectors, each exactly arity long
    arg_ops: Vec<Vec<mir::Operand<'tcx>>>,
    dest_locals: Vec<mir::Local>,
    dest_tys: Vec<ty::Ty<'tcx>>,
    /// Per-callee input type list
    input_tys: Vec<Vec<ty::Ty<'tcx>>>,
    intermediate_stmts: Vec<mir::Statement<'tcx>>,
    n_calls: usize, // 3..=16
    /// uniform callee arity M across chain
    arity: usize,
}

#[allow(rustc::usage_of_qualified_ty)]
pub(crate) fn try_transform_parallel_invoke_n_way<'tcx>(
    tcx: TyCtxt<'tcx>,
    body: &mut mir::Body<'tcx>,
) -> usize {
    if !idiom_transform_enabled(tcx) {
        return 0;
    }
    // Never transform sysroot crate internals
    if is_in_sysroot_crate(tcx, body.source.def_id()) {
        return 0;
    }
    let fn_name = tcx.def_path_str(body.source.def_id());
    if fn_name.contains("parallel_reduce_") || fn_name.contains("parallel_runtime")
        || fn_name.contains("parallel_invoke")
    {
        return 0;
    }
    if parallel_pass_quick_skip(tcx, body, 4, true) {
        return 0;
    }
    fn lookup(tcx: TyCtxt<'_>, name: &str) -> Option<rustc_hir::def_id::DefId> {
        tcx.get_diagnostic_item(Symbol::intern(name))
    }
    // helpers[N][M] = DefId for `parallel_invoke_<N>_fn<M>` if available
    const MAX_INVOKE_ARITY: usize = 20;
    let mut helpers: [[Option<rustc_hir::def_id::DefId>; MAX_INVOKE_ARITY + 1]; 14] =
        [[None; MAX_INVOKE_ARITY + 1]; 14];
    for n in 3usize..=16 {
        for m in 0usize..=MAX_INVOKE_ARITY {
            let name = format!("parallel_runtime_parallel_invoke_{n}_fn{m}");
            helpers[n - 3][m] = lookup(tcx, &name);
        }
    }
    let helper_for = |n: usize, m: usize| -> Option<rustc_hir::def_id::DefId> {
        if !(3..=16).contains(&n) || m > MAX_INVOKE_ARITY { return None; }
        helpers[n - 3][m]
    };
    // bail unless a 3/4-way helper exists
    let any_3_or_4 = (0..=MAX_INVOKE_ARITY)
        .any(|m| helper_for(3, m).is_some() || helper_for(4, m).is_some());
    if !any_3_or_4 {
        return 0;
    }
    /// max chain length, cascade nesting depth 4
    const MAX_INVOKE_CHAIN: usize = 16;

    /// Read a single Call-terminated block
    fn read_call<'tcx>(
        tcx: TyCtxt<'tcx>,
        body: &mir::Body<'tcx>,
        bb: mir::BasicBlock,
    ) -> Option<(
        rustc_hir::def_id::DefId,
        ty::GenericArgsRef<'tcx>,
        Vec<mir::Operand<'tcx>>,
        mir::Local,
        mir::BasicBlock,
        Vec<rustc_middle::ty::Ty<'tcx>>, // input tys (length = arity)
        rustc_middle::ty::Ty<'tcx>,      // output ty
    )> {
        let bb_data = &body.basic_blocks[bb];
        let mir::TerminatorKind::Call {
            func, args, destination, target: Some(target_bb), ..
        } = &bb_data.terminator().kind
        else {
            return None;
        };
        if !destination.projection.is_empty() {
            return None;
        }
        let mir::Operand::Constant(c) = func else { return None };
        let ty::FnDef(did, gargs) = c.const_.ty().kind() else { return None };
        if !did.is_local() {
            return None;
        }
        if is_in_sysroot_crate(tcx, *did) {
            return None;
        }
        let path = tcx.def_path_str(*did);
        if path.starts_with("core::") || path.starts_with("std::")
            || path.contains("::wrapping_") || path.contains("::rotate_")
            || path.contains("::min") || path.contains("::max")
            || path.contains("parallel_invoke") || path.contains("parallel_reduce")
        {
            return None;
        }
        // Runtime helpers exist for arity 0..=20
        if args.len() > 20 {
            return None;
        }
        let sig = tcx.fn_sig(*did).instantiate(tcx, gargs);
        let sig = tcx.instantiate_bound_regions_with_erased(sig);
        if sig.inputs().len() != args.len() {
            return None;
        }
        let arg_ops: Vec<mir::Operand<'tcx>> = args.iter().map(|a| a.node.clone()).collect();
        let input_tys: Vec<_> = sig
            .inputs()
            .iter()
            .map(|t| tcx.erase_and_anonymize_regions(*t))
            .collect();
        let r_ty = tcx.erase_and_anonymize_regions(sig.output());
        let dest_ty = tcx.erase_and_anonymize_regions(body.local_decls[destination.local].ty);
        if dest_ty != r_ty {
            return None;
        }
        Some((*did, gargs, arg_ops, destination.local, *target_bb, input_tys, r_ty))
    }

    let mut plans: Vec<NWayInvokePlan<'tcx>> = Vec::new();
    let bb_count = body.basic_blocks.len();
    let mut consumed_blocks: rustc_data_structures::fx::FxHashSet<mir::BasicBlock> =
        rustc_data_structures::fx::FxHashSet::default();

    for bb_idx in 0..bb_count {
        let bb_a = mir::BasicBlock::from_usize(bb_idx);
        if consumed_blocks.contains(&bb_a) {
            continue;
        }
        // Read call 1.
        let Some((did1, gargs1, arg1, dest1, target1, a1_tys, r1_ty)) =
            read_call(tcx, body, bb_a)
        else {
            continue;
        };

        // any op reads a prior_dests local?
        let args_read_any = |ops: &[mir::Operand<'tcx>], prior_dests: &[mir::Local]| -> bool {
            for op in ops {
                if let mir::Operand::Copy(p) | mir::Operand::Move(p) = op {
                    if prior_dests.iter().any(|&d| d == p.local) { return true; }
                }
            }
            false
        };
        // first callee sets arity, rest must match
        let arity = arg1.len();

        // Walk passthrough blocks to call 2's bb.
        let mut intermediate1 = Vec::new();
        let Some(bb_b) = walk_passthrough_blocks(
            body, target1, dest1, None, 4, &mut intermediate1,
        ) else { continue };
        let Some((did2, gargs2, arg2, dest2, target2, a2_tys, r2_ty)) =
            read_call(tcx, body, bb_b)
        else { continue };
        if arg2.len() != arity { continue; }
        if dest1 == dest2 { continue; }
        if args_read_any(&arg2, &[dest1]) { continue; }
        // capture bb_b stmts, must not touch dest1/dest2
        let bb_b_stmts: Vec<mir::Statement<'tcx>> = body.basic_blocks[bb_b].statements.clone();
        let mut bb_b_clean = true;
        for stmt in &bb_b_stmts {
            if let mir::StatementKind::Assign(box (place, rvalue)) = &stmt.kind {
                if place.local == dest1 || place.local == dest2 { bb_b_clean = false; break; }
                if rvalue_reads_local(rvalue, dest1) { bb_b_clean = false; break; }
            }
        }
        if !bb_b_clean { continue; }

        // Walk passthrough to call 3's bb.
        let mut intermediate2 = Vec::new();
        let Some(bb_c) = walk_passthrough_blocks(
            body, target2, dest2, None, 4, &mut intermediate2,
        ) else { continue };
        let Some((did3, gargs3, arg3, dest3, target3, a3_tys, r3_ty)) =
            read_call(tcx, body, bb_c)
        else { continue };
        if arg3.len() != arity { continue; }
        if dest1 == dest3 || dest2 == dest3 { continue; }
        if args_read_any(&arg3, &[dest1, dest2]) { continue; }
        let bb_c_stmts: Vec<mir::Statement<'tcx>> = body.basic_blocks[bb_c].statements.clone();
        let mut bb_c_clean = true;
        for stmt in &bb_c_stmts {
            if let mir::StatementKind::Assign(box (place, rvalue)) = &stmt.kind {
                if place.local == dest1 || place.local == dest2 || place.local == dest3 {
                    bb_c_clean = false; break;
                }
                if rvalue_reads_local(rvalue, dest1) || rvalue_reads_local(rvalue, dest2) {
                    bb_c_clean = false; break;
                }
            }
        }
        if !bb_c_clean { continue; }

        // relaxed purity; per-thread catch_unwind makes allocs/drops sound
        let body_did = body.source.def_id();
        if check_body_purity_for_invoke(tcx, did1, body_did).is_err() { continue; }
        if check_body_purity_for_invoke(tcx, did2, body_did).is_err() { continue; }
        if check_body_purity_for_invoke(tcx, did3, body_did).is_err() { continue; }
        let c1 = body_weighted_cost(tcx, did1, 1, body_did);
        let c2 = body_weighted_cost(tcx, did2, 1, body_did);
        let c3 = body_weighted_cost(tcx, did3, 1, body_did);
        if c1 < MIN_PARALLEL_INVOKE_BODY_COST
            || c2 < MIN_PARALLEL_INVOKE_BODY_COST
            || c3 < MIN_PARALLEL_INVOKE_BODY_COST
        { continue; }
        // imbalance reject on chain's largest vs smallest
        let mut sorted = [c1, c2, c3];
        sorted.sort();
        if sorted[2] > sorted[0].saturating_mul(10) { continue; }

        // Try to also extend to 4-way.
        let try_4 = || -> Option<(_, _, _, _, _, _, _, Vec<mir::Statement<'tcx>>, Vec<mir::Statement<'tcx>>, mir::BasicBlock)> {
            let mut intermediate3 = Vec::new();
            let bb_d = walk_passthrough_blocks(body, target3, dest3, None, 4, &mut intermediate3)?;
            let (did4, gargs4, arg4, dest4, target4, a4_tys, r4_ty) =
                read_call(tcx, body, bb_d)?;
            if arg4.len() != arity { return None; }
            if dest1 == dest4 || dest2 == dest4 || dest3 == dest4 { return None; }
            if args_read_any(&arg4, &[dest1, dest2, dest3]) { return None; }
            let bb_d_stmts: Vec<mir::Statement<'tcx>> = body.basic_blocks[bb_d].statements.clone();
            for stmt in &bb_d_stmts {
                if let mir::StatementKind::Assign(box (place, rvalue)) = &stmt.kind {
                    if place.local == dest1 || place.local == dest2
                        || place.local == dest3 || place.local == dest4
                    { return None; }
                    if rvalue_reads_local(rvalue, dest1)
                        || rvalue_reads_local(rvalue, dest2)
                        || rvalue_reads_local(rvalue, dest3)
                    { return None; }
                }
            }
            if check_body_purity_for_invoke(tcx, did4, body_did).is_err() { return None; }
            let c4 = body_weighted_cost(tcx, did4, 1, body_did);
            if c4 < MIN_PARALLEL_INVOKE_BODY_COST { return None; }
            let mut s = [c1, c2, c3, c4];
            s.sort();
            if s[3] > s[0].saturating_mul(10) { return None; }
            Some((did4, gargs4, arg4, dest4, target4, a4_tys, r4_ty, intermediate3, bb_d_stmts, bb_d))
        };

        let four_way_data = if helper_for(4, arity).is_some() { try_4() } else { None };

        // Build the plan.
        let mut all_intermediate: Vec<mir::Statement<'tcx>> = Vec::new();
        all_intermediate.extend(intermediate1);
        all_intermediate.extend(bb_b_stmts.iter().cloned());
        all_intermediate.extend(intermediate2);
        all_intermediate.extend(bb_c_stmts.iter().cloned());

        if let Some((did4, gargs4, arg4, dest4, target4, a4_tys, r4_ty, intermediate3, bb_d_stmts, bb_d)) = four_way_data {
            // 4-way base plan, greedily extend to 5..8
            all_intermediate.extend(intermediate3);
            all_intermediate.extend(bb_d_stmts);
            let dest4_ty = body.local_decls[dest4].ty;
            if dest4_ty != r4_ty { continue; }

            let mut callees: Vec<(_, _)> = vec![
                (did1, gargs1), (did2, gargs2), (did3, gargs3), (did4, gargs4),
            ];
            let arg4_v: Vec<mir::Operand<'tcx>> = arg4.clone();
            let mut arg_ops: Vec<Vec<mir::Operand<'tcx>>> =
                vec![arg1.clone(), arg2.clone(), arg3.clone(), arg4_v];
            let mut dests: Vec<mir::Local> = vec![dest1, dest2, dest3, dest4];
            #[allow(rustc::usage_of_qualified_ty)]
            let mut dest_tys: Vec<ty::Ty<'tcx>> = vec![r1_ty, r2_ty, r3_ty, r4_ty];
            #[allow(rustc::usage_of_qualified_ty)]
            let mut input_tys: Vec<Vec<ty::Ty<'tcx>>> = vec![a1_tys.clone(), a2_tys, a3_tys, a4_tys];
            let c4 = body_weighted_cost(tcx, did4, 1, body_did);
            let mut chain_costs: Vec<usize> = vec![c1, c2, c3, c4];
            let mut after_target = target4;
            let mut chain_bbs: Vec<mir::BasicBlock> = vec![bb_a, bb_b, bb_c, bb_d];

            while callees.len() < MAX_INVOKE_CHAIN {
                let mut extra_intermediate = Vec::new();
                let last_dest = *dests.last().unwrap();
                let next_bb = match walk_passthrough_blocks(
                    body, after_target, last_dest, None, 4, &mut extra_intermediate,
                ) { Some(b) => b, None => break };
                let (did_n, gargs_n, arg_n, dest_n, target_n, a_n_tys, r_n_ty) =
                    match read_call(tcx, body, next_bb) { Some(c) => c, None => break };
                if arg_n.len() != arity { break; }
                if dests.iter().any(|d| *d == dest_n) { break; }
                if args_read_any(&arg_n, &dests) { break; }
                let bb_n_stmts: Vec<mir::Statement<'tcx>> =
                    body.basic_blocks[next_bb].statements.clone();
                let mut bb_n_clean = true;
                for stmt in &bb_n_stmts {
                    if let mir::StatementKind::Assign(box (place, rvalue)) = &stmt.kind {
                        if dests.iter().any(|d| *d == place.local) || place.local == dest_n {
                            bb_n_clean = false; break;
                        }
                        if dests.iter().any(|d| rvalue_reads_local(rvalue, *d)) {
                            bb_n_clean = false; break;
                        }
                    }
                }
                if !bb_n_clean { break; }
                if check_body_purity_for_invoke(tcx, did_n, body_did).is_err() { break; }
                let c_n = body_weighted_cost(tcx, did_n, 1, body_did);
                if c_n < MIN_PARALLEL_INVOKE_BODY_COST { break; }
                let mut tentative_costs = chain_costs.clone();
                tentative_costs.push(c_n);
                let &min_c = tentative_costs.iter().min().unwrap();
                let &max_c = tentative_costs.iter().max().unwrap();
                if max_c > min_c.saturating_mul(10) { break; }

                if body.local_decls[dest_n].ty != r_n_ty { break; }

                callees.push((did_n, gargs_n));
                arg_ops.push(arg_n);
                dests.push(dest_n);
                dest_tys.push(r_n_ty);
                input_tys.push(a_n_tys);
                chain_costs.push(c_n);
                all_intermediate.extend(extra_intermediate);
                all_intermediate.extend(bb_n_stmts);
                chain_bbs.push(next_bb);
                after_target = target_n;
            }

            // largest N with helper, drop trailing entries
            let mut n = callees.len();
            while n >= 3 {
                if helper_for(n, arity).is_some() { break }
                n -= 1;
            }
            if n < 3 { continue; }
            callees.truncate(n);
            arg_ops.truncate(n);
            dests.truncate(n);
            dest_tys.truncate(n);
            input_tys.truncate(n);

            for &cb in &chain_bbs {
                consumed_blocks.insert(cb);
            }

            plans.push(NWayInvokePlan {
                bb_a,
                bb_after: after_target,
                callees,
                arg_ops,
                dest_locals: dests,
                dest_tys,
                input_tys,
                intermediate_stmts: all_intermediate,
                n_calls: n,
                arity,
            });
        } else if helper_for(3, arity).is_some() {
            // 3-way plan (no 4th call available).
            consumed_blocks.insert(bb_a);
            consumed_blocks.insert(bb_b);
            // bb_c consumed via target3
            plans.push(NWayInvokePlan {
                bb_a,
                bb_after: target3,
                callees: vec![(did1, gargs1), (did2, gargs2), (did3, gargs3)],
                arg_ops: vec![arg1, arg2, arg3],
                dest_locals: vec![dest1, dest2, dest3],
                dest_tys: vec![r1_ty, r2_ty, r3_ty],
                input_tys: vec![a1_tys, a2_tys, a3_tys],
                intermediate_stmts: all_intermediate,
                n_calls: 3,
                arity,
            });
        }
    }

    let mut count = 0usize;
    for plan in plans {
        let Some(helper_def_id) = helper_for(plan.n_calls, plan.arity) else { continue };
        par_dump!(tcx,
            "[PAR-IDIOM-NWAY-CANDIDATE] fn={} bb_a=bb{} n={} arity={} after=bb{} callees=[{}]",
            fn_name,
            plan.bb_a.index(),
            plan.n_calls,
            plan.arity,
            plan.bb_after.index(),
            plan.callees
                .iter()
                .map(|(did, _)| tcx.def_path_str(*did))
                .collect::<Vec<_>>()
                .join(", "),
        );
        if apply_n_way_invoke(tcx, body, &plan, helper_def_id) {
            par_dump!(tcx,
                "[PAR-IDIOM-NWAY-APPLIED] fn={} bb_a=bb{} n={} arity={}",
                fn_name, plan.bb_a.index(), plan.n_calls, plan.arity,
            );
            count += 1;
        }
    }
    count
}

#[allow(rustc::usage_of_qualified_ty)]
fn apply_n_way_invoke<'tcx>(
    tcx: TyCtxt<'tcx>,
    body: &mut mir::Body<'tcx>,
    plan: &NWayInvokePlan<'tcx>,
    helper_def_id: rustc_hir::def_id::DefId,
) -> bool {
    use rustc_middle::mir::*;
    use rustc_middle::ty::{GenericArg, Ty};
    use rustc_span::DUMMY_SP;
    use rustc_span::source_map::Spanned;

    let n = plan.n_calls;
    debug_assert!((3..=16).contains(&n));

    // Build (R1, R2, R3[, R4]) tuple destination.
    let tuple_ty = Ty::new_tup(tcx, &plan.dest_tys);
    let tuple_local = body.local_decls.push(LocalDecl::new(tuple_ty, DUMMY_SP));

    let m = plan.arity; // 0..=4

    // per callee: fn-pointer type plus ReifyFnPointer cast
    let mut fn_ptr_locals: Vec<Local> = Vec::with_capacity(n);
    let mut cast_stmts: Vec<Statement<'tcx>> = Vec::with_capacity(n);
    for i in 0..n {
        let (did, gargs) = plan.callees[i];
        debug_assert_eq!(plan.input_tys[i].len(), m);
        let fn_ptr_ty = Ty::new_fn_ptr(
            tcx,
            ty::Binder::dummy(tcx.mk_fn_sig(
                plan.input_tys[i].iter().copied(),
                plan.dest_tys[i],
                false,
                rustc_hir::Safety::Safe,
                rustc_abi::ExternAbi::Rust,
            )),
        );
        let local = body.local_decls.push(LocalDecl::new(fn_ptr_ty, DUMMY_SP));
        let fn_def_op = Operand::function_handle(tcx, did, gargs.iter(), DUMMY_SP);
        cast_stmts.push(Statement::new(
            SourceInfo::outermost(DUMMY_SP),
            StatementKind::Assign(Box::new((
                Place::from(local),
                Rvalue::Cast(
                    CastKind::PointerCoercion(
                        ty::adjustment::PointerCoercion::ReifyFnPointer(rustc_hir::Safety::Safe),
                        mir::CoercionSource::Implicit,
                    ),
                    fn_def_op,
                    fn_ptr_ty,
                ),
            ))),
        ));
        fn_ptr_locals.push(local);
    }

    // per-callee generic args: m inputs then return
    let mut generic_args: Vec<GenericArg<'tcx>> = Vec::with_capacity(n * (m + 1));
    for i in 0..n {
        for &ty in &plan.input_tys[i] {
            generic_args.push(GenericArg::from(ty));
        }
        generic_args.push(GenericArg::from(plan.dest_tys[i]));
    }
    let helper_callee =
        Operand::function_handle(tcx, helper_def_id, generic_args.into_iter(), DUMMY_SP);

    // per callee: fn_ptr then its m args
    let mut call_args: Vec<Spanned<Operand<'tcx>>> = Vec::with_capacity(n * (1 + m));
    for i in 0..n {
        call_args.push(Spanned {
            node: Operand::Move(Place::from(fn_ptr_locals[i])),
            span: DUMMY_SP,
        });
        debug_assert_eq!(plan.arg_ops[i].len(), m);
        for op in &plan.arg_ops[i] {
            call_args.push(Spanned { node: op.clone(), span: DUMMY_SP });
        }
    }

    // bb_unpack: dest_locals from tuple fields, goto bb_after
    let mut unpack_stmts: Vec<Statement<'tcx>> = Vec::with_capacity(n);
    for i in 0..n {
        unpack_stmts.push(Statement::new(
            SourceInfo::outermost(DUMMY_SP),
            StatementKind::Assign(Box::new((
                Place::from(plan.dest_locals[i]),
                Rvalue::Use(Operand::Move(Place {
                    local: tuple_local,
                    projection: tcx.mk_place_elems(&[
                        ProjectionElem::Field(
                            rustc_abi::FieldIdx::from_u32(i as u32),
                            plan.dest_tys[i],
                        ),
                    ]),
                })),
            ))),
        ));
    }
    let unpack_bb = body.basic_blocks_mut().push(BasicBlockData::new_stmts(
        unpack_stmts,
        Some(Terminator {
            source_info: SourceInfo::outermost(DUMMY_SP),
            kind: TerminatorKind::Goto { target: plan.bb_after },
        }),
        false,
    ));

    let new_terminator = Terminator {
        source_info: SourceInfo::outermost(DUMMY_SP),
        kind: TerminatorKind::Call {
            func: helper_callee,
            args: call_args.into_boxed_slice(),
            destination: Place::from(tuple_local),
            target: Some(unpack_bb),
            unwind: UnwindAction::Unreachable,
            call_source: CallSource::Misc,
            fn_span: DUMMY_SP,
        },
    };
    {
        let bb_a_data = &mut body.basic_blocks_mut()[plan.bb_a];
        bb_a_data.statements.extend(plan.intermediate_stmts.iter().cloned());
        bb_a_data.statements.extend(cast_stmts);
        bb_a_data.terminator = Some(new_terminator);
    }
    true
}

pub(crate) fn try_transform_parallel_invoke<'tcx>(
    tcx: TyCtxt<'tcx>,
    body: &mut mir::Body<'tcx>,
) -> usize {
    if !idiom_transform_enabled(tcx) {
        return 0;
    }
    // Never transform sysroot crate internals
    if is_in_sysroot_crate(tcx, body.source.def_id()) {
        return 0;
    }
    let fn_name = tcx.def_path_str(body.source.def_id());
    if fn_name.contains("parallel_reduce_") || fn_name.contains("parallel_runtime")
        || fn_name.contains("parallel_invoke")
    {
        return 0;
    }
    // pre-filter: need 3 blocks and a Call
    if parallel_pass_quick_skip(tcx, body, 3, true) {
        return 0;
    }
    fn lookup(tcx: TyCtxt<'_>, name: &str) -> Option<rustc_hir::def_id::DefId> {
        tcx.get_diagnostic_item(Symbol::intern(name))
    }
    // Pre-resolve every helper variant we might need.
    let helper_by_arity: [Option<rustc_hir::def_id::DefId>; 5] = [
        lookup(tcx, "parallel_runtime_parallel_invoke_2_fn0"),
        lookup(tcx, "parallel_runtime_parallel_invoke_2_fn1"),
        lookup(tcx, "parallel_runtime_parallel_invoke_2_fn2"),
        lookup(tcx, "parallel_runtime_parallel_invoke_2_fn3"),
        lookup(tcx, "parallel_runtime_parallel_invoke_2_fn4"),
    ];
    if helper_by_arity.iter().all(|h| h.is_none()) {
        return 0;
    }

    let mut plans: Vec<InvokePlanLike<'tcx>> = Vec::new();

    let bb_count = body.basic_blocks.len();
    'outer: for bb_a_idx in 0..bb_count {
        let bb_a = mir::BasicBlock::from_usize(bb_a_idx);
        let bb_a_data = &body.basic_blocks[bb_a];

        // bb_a's terminator must be a Call C1.
        let mir::TerminatorKind::Call {
            func: f1_func,
            args: f1_args_ops,
            destination: f1_dest,
            target: Some(bb_after_f1),
            ..
        } = &bb_a_data.terminator().kind
        else {
            continue;
        };
        if !f1_dest.projection.is_empty() {
            continue;
        }
        let mir::Operand::Constant(f1_c) = f1_func else { continue };
        let ty::FnDef(f1_def_id, f1_generic_args) = f1_c.const_.ty().kind() else { continue };
        // arity 0..=4 only, pack extras into tuple
        let arity = f1_args_ops.len();
        if arity > 4 {
            continue;
        }
        let arg1_ops: Vec<mir::Operand<'tcx>> =
            f1_args_ops.iter().map(|spanned| spanned.node.clone()).collect();
        let a_local = f1_dest.local;

        // skip Goto passthroughs to bb_b, capturing stmts
        let mut bb_intermediate_stmts: Vec<mir::Statement<'tcx>> = Vec::new();
        // We don't yet know b_local
        let Some(bb_b) = walk_passthrough_blocks(
            body,
            *bb_after_f1,
            a_local,
            None,
            8, // max passthrough chain length
            &mut bb_intermediate_stmts,
        ) else {
            continue;
        };

        // bb_b's terminator must be another Call C2.
        let bb_b_data = &body.basic_blocks[bb_b];
        let mir::TerminatorKind::Call {
            func: f2_func,
            args: f2_args_ops,
            destination: f2_dest,
            target: Some(bb_c),
            ..
        } = &bb_b_data.terminator().kind
        else {
            continue;
        };
        if !f2_dest.projection.is_empty() {
            continue;
        }
        let mir::Operand::Constant(f2_c) = f2_func else { continue };
        let ty::FnDef(f2_def_id, f2_generic_args) = f2_c.const_.ty().kind() else { continue };
        // same arity only; mixed needs 25 helpers
        if f2_args_ops.len() != arity {
            continue;
        }
        // Helper for this arity must exist.
        if helper_by_arity[arity].is_none() {
            continue;
        }
        let arg2_ops: Vec<mir::Operand<'tcx>> =
            f2_args_ops.iter().map(|spanned| spanned.node.clone()).collect();
        let b_local = f2_dest.local;
        if a_local == b_local {
            continue;
        }

        // re-check passthrough stmts against b_local
        for stmt in &bb_intermediate_stmts {
            if let mir::StatementKind::Assign(box (place, _)) = &stmt.kind {
                if place.local == b_local {
                    continue 'outer;
                }
            }
        }

        // reject trivial, cross-crate, impure, sysroot callees
        for &did in &[*f1_def_id, *f2_def_id] {
            if !did.is_local() {
                continue 'outer;
            }
            // crate-level sysroot guard, see is_in_sysroot_crate
            if is_in_sysroot_crate(tcx, did) {
                continue 'outer;
            }
            let path = tcx.def_path_str(did);
            if path.starts_with("core::")
                || path.starts_with("std::")
                || path.contains("::wrapping_")
                || path.contains("::rotate_")
                || path.contains("::min")
                || path.contains("::max")
            {
                continue 'outer;
            }
        }

        // independence: f2 args must not read _a
        for op in &arg2_ops {
            match op {
                mir::Operand::Copy(p) | mir::Operand::Move(p) => {
                    if p.local == a_local {
                        continue 'outer;
                    }
                }
                _ => {}
            }
        }

        // bb_b pre-call stmts, same independence rules
        let mut bb_b_stmts: Vec<mir::Statement<'tcx>> = Vec::new();
        let mut bad = false;
        for stmt in &bb_b_data.statements {
            match &stmt.kind {
                mir::StatementKind::Assign(box (place, rvalue)) => {
                    if place.local == b_local {
                        bad = true;
                        break;
                    }
                    if rvalue_reads_local(rvalue, a_local) {
                        bad = true;
                        break;
                    }
                    bb_b_stmts.push(stmt.clone());
                }
                mir::StatementKind::StorageLive(_)
                | mir::StatementKind::StorageDead(_)
                | mir::StatementKind::Nop => {
                    bb_b_stmts.push(stmt.clone());
                }
                _ => {
                    bad = true;
                    break;
                }
            }
        }
        if bad {
            continue;
        }

        // Purity check on both bodies
        let body_did = body.source.def_id();
        if check_body_purity_for_invoke(tcx, *f1_def_id, body_did).is_err() {
            continue;
        }
        if check_body_purity_for_invoke(tcx, *f2_def_id, body_did).is_err() {
            continue;
        }

        // cost gate, weighted estimator at depth 1
        let c1 = body_weighted_cost(tcx, *f1_def_id, 1, body_did);
        let c2 = body_weighted_cost(tcx, *f2_def_id, 1, body_did);
        if c1 < MIN_PARALLEL_INVOKE_BODY_COST || c2 < MIN_PARALLEL_INVOKE_BODY_COST {
            continue;
        }
        // imbalance reject, 10x ratio gains under 5%
        let (small, large) = if c1 < c2 { (c1, c2) } else { (c2, c1) };
        if large > small.saturating_mul(10) {
            continue;
        }

        // resolve fn sigs for helper generic args
        let f1_sig = tcx.fn_sig(*f1_def_id).instantiate(tcx, f1_generic_args);
        let f1_sig = tcx.instantiate_bound_regions_with_erased(f1_sig);
        let f2_sig = tcx.fn_sig(*f2_def_id).instantiate(tcx, f2_generic_args);
        let f2_sig = tcx.instantiate_bound_regions_with_erased(f2_sig);
        if f1_sig.inputs().len() != arity || f2_sig.inputs().len() != arity {
            continue;
        }
        // Bug 5B: erase regions, rationale in read_call
        #[allow(rustc::usage_of_qualified_ty)]
        let f1_input_tys: Vec<ty::Ty<'tcx>> =
            f1_sig.inputs().iter().map(|t| tcx.erase_and_anonymize_regions(*t)).collect();
        #[allow(rustc::usage_of_qualified_ty)]
        let f2_input_tys: Vec<ty::Ty<'tcx>> =
            f2_sig.inputs().iter().map(|t| tcx.erase_and_anonymize_regions(*t)).collect();
        let a_ty = tcx.erase_and_anonymize_regions(body.local_decls[a_local].ty);
        let b_ty = tcx.erase_and_anonymize_regions(body.local_decls[b_local].ty);
        let f1_out = tcx.erase_and_anonymize_regions(f1_sig.output());
        let f2_out = tcx.erase_and_anonymize_regions(f2_sig.output());
        if f1_out != a_ty || f2_out != b_ty {
            continue;
        }

        plans.push(InvokePlanLike {
            bb_a,
            bb_c: *bb_c,
            f1_def_id: *f1_def_id,
            f1_args: f1_generic_args,
            f2_def_id: *f2_def_id,
            f2_args: f2_generic_args,
            arg1_ops,
            arg2_ops,
            a_local,
            b_local,
            a_ty,
            b_ty,
            f1_input_tys,
            f2_input_tys,
            arity,
            bb_intermediate_stmts,
            bb_b_stmts,
        });
    }

    let mut count = 0usize;
    for plan in plans {
        let helper_name = match invoke_helper_name_for_arity(plan.arity) {
            Some(n) => n,
            None => continue,
        };
        let Some(helper_def_id) = lookup(tcx, helper_name) else { continue };

        par_dump!(tcx, 
            "[PAR-IDIOM-INVOKE-CANDIDATE] fn={} bb_a=bb{} bb_c=bb{} arity={} f1={} f2={}",
            fn_name,
            plan.bb_a.index(),
            plan.bb_c.index(),
            plan.arity,
            tcx.def_path_str(plan.f1_def_id),
            tcx.def_path_str(plan.f2_def_id),
        );

        if apply_parallel_invoke(tcx, body, &plan, helper_def_id) {
            par_dump!(tcx, 
                "[PAR-IDIOM-INVOKE-APPLIED] fn={} bb_a=bb{} arity={} f1={} f2={}",
                fn_name,
                plan.bb_a.index(),
                plan.arity,
                tcx.def_path_str(plan.f1_def_id),
                tcx.def_path_str(plan.f2_def_id),
            );
            count += 1;
        }
    }
    count
}

/// true if rvalue reads `target`
fn rvalue_reads_local<'tcx>(rvalue: &mir::Rvalue<'tcx>, target: mir::Local) -> bool {
    let mut found = false;
    let check_op = |op: &mir::Operand<'tcx>, found: &mut bool| match op {
        mir::Operand::Copy(p) | mir::Operand::Move(p) => {
            if p.local == target {
                *found = true;
            }
        }
        _ => {}
    };
    match rvalue {
        mir::Rvalue::Use(op) => check_op(op, &mut found),
        mir::Rvalue::Repeat(op, _) => check_op(op, &mut found),
        mir::Rvalue::Ref(_, _, p) => {
            if p.local == target {
                found = true;
            }
        }
        mir::Rvalue::RawPtr(_, p) => {
            if p.local == target {
                found = true;
            }
        }
        mir::Rvalue::Cast(_, op, _) => check_op(op, &mut found),
        mir::Rvalue::BinaryOp(_, box (a, b)) => {
            check_op(a, &mut found);
            check_op(b, &mut found);
        }
        mir::Rvalue::UnaryOp(_, op) => check_op(op, &mut found),
        mir::Rvalue::Aggregate(_, ops) => {
            for op in ops {
                check_op(op, &mut found);
            }
        }
        mir::Rvalue::Discriminant(p) => {
            if p.local == target {
                found = true;
            }
        }
        mir::Rvalue::CopyForDeref(p) => {
            if p.local == target {
                found = true;
            }
        }
        _ => {}
    }
    found
}

#[allow(rustc::usage_of_qualified_ty)]
fn apply_parallel_invoke<'tcx>(
    tcx: TyCtxt<'tcx>,
    body: &mut mir::Body<'tcx>,
    plan: &InvokePlanLike<'tcx>,
    helper_def_id: rustc_hir::def_id::DefId,
) -> bool {
    use rustc_middle::mir::*;
    use rustc_middle::ty::{GenericArg, Ty};
    use rustc_span::DUMMY_SP;
    use rustc_span::source_map::Spanned;

    // Build the result tuple type `(R1, R2)`.
    let tuple_ty = Ty::new_tup(tcx, &[plan.a_ty, plan.b_ty]);
    let tuple_local = body.local_decls.push(LocalDecl::new(tuple_ty, DUMMY_SP));

    // fn-pointer type per callee
    let mk_fn_ptr_ty = |inputs: &[Ty<'tcx>], output: Ty<'tcx>| -> Ty<'tcx> {
        Ty::new_fn_ptr(
            tcx,
            ty::Binder::dummy(tcx.mk_fn_sig(
                inputs.iter().copied(),
                output,
                false,
                rustc_hir::Safety::Safe,
                rustc_abi::ExternAbi::Rust,
            )),
        )
    };
    let f1_fn_ptr_ty = mk_fn_ptr_ty(&plan.f1_input_tys, plan.a_ty);
    let f2_fn_ptr_ty = mk_fn_ptr_ty(&plan.f2_input_tys, plan.b_ty);

    let f1_ptr_local = body.local_decls.push(LocalDecl::new(f1_fn_ptr_ty, DUMMY_SP));
    let f2_ptr_local = body.local_decls.push(LocalDecl::new(f2_fn_ptr_ty, DUMMY_SP));

    let f1_fn_def_operand =
        Operand::function_handle(tcx, plan.f1_def_id, plan.f1_args.iter(), DUMMY_SP);
    let f2_fn_def_operand =
        Operand::function_handle(tcx, plan.f2_def_id, plan.f2_args.iter(), DUMMY_SP);

    let f1_cast_stmt = Statement::new(
        SourceInfo::outermost(DUMMY_SP),
        StatementKind::Assign(Box::new((
            Place::from(f1_ptr_local),
            Rvalue::Cast(
                CastKind::PointerCoercion(
                    ty::adjustment::PointerCoercion::ReifyFnPointer(rustc_hir::Safety::Safe),
                    mir::CoercionSource::Implicit,
                ),
                f1_fn_def_operand,
                f1_fn_ptr_ty,
            ),
        ))),
    );
    let f2_cast_stmt = Statement::new(
        SourceInfo::outermost(DUMMY_SP),
        StatementKind::Assign(Box::new((
            Place::from(f2_ptr_local),
            Rvalue::Cast(
                CastKind::PointerCoercion(
                    ty::adjustment::PointerCoercion::ReifyFnPointer(rustc_hir::Safety::Safe),
                    mir::CoercionSource::Implicit,
                ),
                f2_fn_def_operand,
                f2_fn_ptr_ty,
            ),
        ))),
    );

    // generic args: A1.., R1, A2.., R2
    let mut generic_args: Vec<GenericArg<'tcx>> = Vec::with_capacity(2 * plan.arity + 2);
    for &t in &plan.f1_input_tys {
        generic_args.push(GenericArg::from(t));
    }
    generic_args.push(GenericArg::from(plan.a_ty));
    for &t in &plan.f2_input_tys {
        generic_args.push(GenericArg::from(t));
    }
    generic_args.push(GenericArg::from(plan.b_ty));

    let helper_callee =
        Operand::function_handle(tcx, helper_def_id, generic_args.into_iter(), DUMMY_SP);

    // call args: f1_ptr, a1.., f2_ptr, a2
    let mut call_args: Vec<Spanned<Operand<'tcx>>> = Vec::with_capacity(2 + 2 * plan.arity);
    call_args.push(Spanned {
        node: Operand::Move(Place::from(f1_ptr_local)),
        span: DUMMY_SP,
    });
    for op in &plan.arg1_ops {
        call_args.push(Spanned { node: op.clone(), span: DUMMY_SP });
    }
    call_args.push(Spanned {
        node: Operand::Move(Place::from(f2_ptr_local)),
        span: DUMMY_SP,
    });
    for op in &plan.arg2_ops {
        call_args.push(Spanned { node: op.clone(), span: DUMMY_SP });
    }
    let call_args: Box<[Spanned<Operand<'tcx>>]> = call_args.into_boxed_slice();

    // bb_unpack: _a, _b from tuple, goto bb_c
    let unpack_a = Statement::new(
        SourceInfo::outermost(DUMMY_SP),
        StatementKind::Assign(Box::new((
            Place::from(plan.a_local),
            Rvalue::Use(Operand::Move(Place {
                local: tuple_local,
                projection: tcx.mk_place_elems(&[
                    ProjectionElem::Field(rustc_abi::FieldIdx::from_u32(0), plan.a_ty),
                ]),
            })),
        ))),
    );
    let unpack_b = Statement::new(
        SourceInfo::outermost(DUMMY_SP),
        StatementKind::Assign(Box::new((
            Place::from(plan.b_local),
            Rvalue::Use(Operand::Move(Place {
                local: tuple_local,
                projection: tcx.mk_place_elems(&[
                    ProjectionElem::Field(rustc_abi::FieldIdx::from_u32(1), plan.b_ty),
                ]),
            })),
        ))),
    );
    let unpack_bb = body.basic_blocks_mut().push(BasicBlockData::new_stmts(
        vec![unpack_a, unpack_b],
        Some(Terminator {
            source_info: SourceInfo::outermost(DUMMY_SP),
            kind: TerminatorKind::Goto { target: plan.bb_c },
        }),
        false,
    ));

    // bb_a: passthrough, bb_b, cast stmts, invoke Call
    let new_terminator = Terminator {
        source_info: SourceInfo::outermost(DUMMY_SP),
        kind: TerminatorKind::Call {
            func: helper_callee,
            args: call_args,
            destination: Place::from(tuple_local),
            target: Some(unpack_bb),
            unwind: UnwindAction::Unreachable,
            call_source: CallSource::Misc,
            fn_span: DUMMY_SP,
        },
    };
    {
        let bb_a_data = &mut body.basic_blocks_mut()[plan.bb_a];
        bb_a_data.statements.extend(plan.bb_intermediate_stmts.iter().cloned());
        bb_a_data.statements.extend(plan.bb_b_stmts.iter().cloned());
        bb_a_data.statements.push(f1_cast_stmt);
        bb_a_data.statements.push(f2_cast_stmt);
        bb_a_data.terminator = Some(new_terminator);
    }

    true
}

// lifted out so apply_parallel_invoke can name it
#[allow(rustc::usage_of_qualified_ty)]
struct InvokePlanLike<'tcx> {
    bb_a: mir::BasicBlock,
    bb_c: mir::BasicBlock,
    f1_def_id: rustc_hir::def_id::DefId,
    f1_args: ty::GenericArgsRef<'tcx>,
    f2_def_id: rustc_hir::def_id::DefId,
    f2_args: ty::GenericArgsRef<'tcx>,
    arg1_ops: Vec<mir::Operand<'tcx>>,
    arg2_ops: Vec<mir::Operand<'tcx>>,
    a_local: mir::Local,
    b_local: mir::Local,
    a_ty: ty::Ty<'tcx>,
    b_ty: ty::Ty<'tcx>,
    f1_input_tys: Vec<ty::Ty<'tcx>>,
    f2_input_tys: Vec<ty::Ty<'tcx>>,
    arity: usize,
    /// stmts from Goto-only blocks between bb_a, bb_b
    bb_intermediate_stmts: Vec<mir::Statement<'tcx>>,
    /// bb_b pre-call stmts, moved into bb_a tail
    bb_b_stmts: Vec<mir::Statement<'tcx>>,
}

/// rewrite range sum/product to parallel range reduce
fn try_transform_range_iterator_methods<'tcx>(
    tcx: TyCtxt<'tcx>,
    body: &mut mir::Body<'tcx>,
    fn_name: &str,
) -> usize {
    fn lookup(tcx: TyCtxt<'_>, name: &str) -> Option<rustc_hir::def_id::DefId> {
        tcx.get_diagnostic_item(Symbol::intern(name))
    }

    fn range_helper_for(method: &str, tag: AccTyTag) -> Option<&'static str> {
        Some(match (method, tag) {
            ("sum", AccTyTag::U32) => "parallel_runtime_parallel_reduce_sum_range_u32",
            ("sum", AccTyTag::U64) => "parallel_runtime_parallel_reduce_sum_range_u64",
            ("sum", AccTyTag::I32) => "parallel_runtime_parallel_reduce_sum_range_i32",
            ("sum", AccTyTag::I64) => "parallel_runtime_parallel_reduce_sum_range_i64",
            ("sum", AccTyTag::Usize) => "parallel_runtime_parallel_reduce_sum_range_usize",
            ("sum", AccTyTag::Isize) => "parallel_runtime_parallel_reduce_sum_range_isize",
            ("product", AccTyTag::U32) => "parallel_runtime_parallel_reduce_mul_range_u32",
            ("product", AccTyTag::U64) => "parallel_runtime_parallel_reduce_mul_range_u64",
            ("product", AccTyTag::I32) => "parallel_runtime_parallel_reduce_mul_range_i32",
            ("product", AccTyTag::I64) => "parallel_runtime_parallel_reduce_mul_range_i64",
            ("product", AccTyTag::Usize) => "parallel_runtime_parallel_reduce_mul_range_usize",
            ("product", AccTyTag::Isize) => "parallel_runtime_parallel_reduce_mul_range_isize",
            _ => return None,
        })
    }

    struct RangePlan<'tcx> {
        bb: mir::BasicBlock,
        start_op: mir::Operand<'tcx>,
        end_op: mir::Operand<'tcx>,
        method_dest: mir::Place<'tcx>,
        after_bb: mir::BasicBlock,
        method: &'static str,
        elem_tag: AccTyTag,
    }

    let mut plans: Vec<RangePlan<'tcx>> = Vec::new();

    // Snapshot to allow mutation later.
    let bb_count = body.basic_blocks.len();
    for bb_idx in 0..bb_count {
        let bb = mir::BasicBlock::from_usize(bb_idx);
        let block = &body.basic_blocks[bb];

        // find Range::<T> aggregate in this block
        let mut found: Option<(mir::Local, mir::Operand<'tcx>, mir::Operand<'tcx>, AccTyTag)> = None;
        for stmt in &block.statements {
            let mir::StatementKind::Assign(box (place, rvalue)) = &stmt.kind else { continue };
            if !place.projection.is_empty() {
                continue;
            }
            let mir::Rvalue::Aggregate(agg_kind, fields) = rvalue else { continue };
            // Adt must be Range<T> with two fields
            let mir::AggregateKind::Adt(adt_def_id, _variant_idx, generic_args, _, _) = &**agg_kind
            else {
                continue;
            };
            if fields.len() != 2 {
                continue;
            }
            let adt_path = tcx.def_path_str(*adt_def_id);
            if !adt_path.ends_with("ops::Range") && !adt_path.ends_with("Range") {
                continue;
            }
            // Inspect generic args for the element type.
            let elem_ty = generic_args.types().next();
            let Some(elem_ty) = elem_ty else { continue };
            let Some(elem_tag) = AccTyTag::from_ty(elem_ty) else { continue };
            // field 0 start, field 1 end
            let start_op = fields[rustc_abi::FieldIdx::from_u32(0)].clone();
            let end_op = fields[rustc_abi::FieldIdx::from_u32(1)].clone();
            found = Some((place.local, start_op, end_op, elem_tag));
            // Only the LAST aggregate matters
        }
        let Some((range_local, start_op, end_op, elem_tag)) = found else { continue };

        // terminator must be Iterator sum/product on _range
        let term = block.terminator();
        let mir::TerminatorKind::Call {
            func,
            args,
            destination,
            target: Some(after_bb),
            ..
        } = &term.kind
        else {
            continue;
        };
        let mir::Operand::Constant(c) = func else { continue };
        let ty::FnDef(call_def_id, _) = c.const_.ty().kind() else { continue };
        let path = tcx.def_path_str(*call_def_id);
        let method = match path.rsplit("::").next().unwrap_or("") {
            "sum" => "sum",
            "product" => "product",
            _ => continue,
        };
        if args.len() != 1 {
            continue;
        }
        let arg_ok = match &args[0].node {
            mir::Operand::Copy(p) | mir::Operand::Move(p) => {
                p.projection.is_empty() && p.local == range_local
            }
            _ => false,
        };
        if !arg_ok {
            continue;
        }
        plans.push(RangePlan {
            bb,
            start_op,
            end_op,
            method_dest: *destination,
            after_bb: *after_bb,
            method,
            elem_tag,
        });
    }

    let mut count = 0usize;
    for plan in plans {
        let Some(helper_name) = range_helper_for(plan.method, plan.elem_tag) else {
            continue;
        };
        let Some(helper_def_id) = lookup(tcx, helper_name) else {
            par_dump!(tcx, 
                "[PAR-IDIOM-RANGE-NO-HELPER] fn={} method=.{}() elem_ty={:?} helper={}",
                fn_name, plan.method, plan.elem_tag, helper_name,
            );
            continue;
        };
        par_dump!(tcx, 
            "[PAR-IDIOM-RANGE-CANDIDATE] fn={} bb=bb{} method=.{}() elem_ty={:?} after=bb{}",
            fn_name,
            plan.bb.index(),
            plan.method,
            plan.elem_tag,
            plan.after_bb.index(),
        );

        let helper_callee = mir::Operand::function_handle(
            tcx,
            helper_def_id,
            std::iter::empty(),
            rustc_span::DUMMY_SP,
        );
        let new_args: Box<[rustc_span::source_map::Spanned<mir::Operand<'tcx>>]> = Box::new([
            rustc_span::source_map::Spanned {
                node: plan.start_op.clone(),
                span: rustc_span::DUMMY_SP,
            },
            rustc_span::source_map::Spanned {
                node: plan.end_op.clone(),
                span: rustc_span::DUMMY_SP,
            },
        ]);
        let new_term_kind = mir::TerminatorKind::Call {
            func: helper_callee,
            args: new_args,
            destination: plan.method_dest,
            target: Some(plan.after_bb),
            unwind: mir::UnwindAction::Unreachable,
            call_source: mir::CallSource::Misc,
            fn_span: rustc_span::DUMMY_SP,
        };
        body.basic_blocks_mut()[plan.bb].terminator_mut().kind = new_term_kind;

        par_dump!(tcx, 
            "[PAR-IDIOM-RANGE-APPLIED] fn={} bb=bb{} method=.{}() elem_ty={:?}",
            fn_name,
            plan.bb.index(),
            plan.method,
            plan.elem_tag,
        );
        count += 1;
    }
    count
}

// slice-loop u64 sum reduction to helper call

#[derive(Debug, Clone)]
struct SliceReducePlan {
    entry_bb: mir::BasicBlock,
    exit_bb: mir::BasicBlock,
    acc_local: mir::Local,
    /// The Local holding `&[u64]`
    slice_local: mir::Local,
}

fn analyze_slice_loop_for_transform<'tcx>(
    tcx: TyCtxt<'tcx>,
    body: &mir::Body<'tcx>,
    lp: &SimpleLoop,
) -> Option<SliceReducePlan> {
    let dbg = std::env::var("PARALLEL_TRANSFORM_REDUCE_DEBUG").ok().as_deref()
        == Some("1");
    macro_rules! reject {
        ($($arg:tt)*) => {
            if dbg { par_dump!(tcx, "[PAR-IDIOM-SLICE-REJECT] header=bb{}: {}", lp.header.index(), format!($($arg)*)); }
            return None;
        };
    }

    let header = lp.header;

    // header terminator must be Iterator::next(&mut _state)
    let header_term = body.basic_blocks[header].terminator();
    let mir::TerminatorKind::Call { func, args: _, destination, target, .. } =
        &header_term.kind
    else {
        reject!("header terminator is not a Call");
    };
    let Some(post_bb) = *target else {
        reject!("header Call has no return target");
    };
    if !destination.projection.is_empty() {
        reject!("Call destination has projection");
    }
    let opt_local = destination.local;

    let mir::Operand::Constant(callee_c) = func else {
        reject!("indirect Call in header");
    };
    let rustc_middle::ty::FnDef(callee_def_id, _) = callee_c.const_.ty().kind() else {
        reject!("Call callee is not FnDef");
    };
    let callee_path = tcx.def_path_str(*callee_def_id);
    if !callee_path.contains("Iterator") || !callee_path.ends_with("::next") {
        reject!("header Call is not Iterator::next ({})", callee_path);
    }

    // 2. post_bb must do switchInt(discriminant(opt_local))
    let post_block = &body.basic_blocks[post_bb];
    let mir::TerminatorKind::SwitchInt { discr, targets } = &post_block.terminator().kind
    else {
        reject!("post-Call BB doesn't switchInt");
    };
    let discr_local = match discr {
        mir::Operand::Move(p) | mir::Operand::Copy(p) if p.projection.is_empty() => p.local,
        _ => {
            reject!("switchInt discriminant is not a bare Local");
        }
    };
    // discr_local must be Discriminant(opt_local) in post_bb
    let mut found_disc = false;
    for stmt in &post_block.statements {
        let mir::StatementKind::Assign(box (place, rvalue)) = &stmt.kind else { continue };
        if !place.projection.is_empty() || place.local != discr_local {
            continue;
        }
        if let mir::Rvalue::Discriminant(disc_place) = rvalue {
            if disc_place.local == opt_local && disc_place.projection.is_empty() {
                found_disc = true;
                break;
            }
        }
    }
    if !found_disc {
        reject!("post-Call BB has no Discriminant({:?}) assignment to discr Local", opt_local);
    }

    // Determine exit + Some-arm targets
    let mut exit_bb: Option<mir::BasicBlock> = None;
    let mut some_arm: Option<mir::BasicBlock> = None;
    for (val, t) in targets.iter() {
        match val {
            0 => exit_bb = Some(t),
            1 => some_arm = Some(t),
            _ => {}
        }
    }
    let Some(exit_bb) = exit_bb else {
        reject!("switchInt has no 0 (None/exit) branch");
    };
    let Some(_some_arm) = some_arm else {
        reject!("switchInt has no 1 (Some/body) branch");
    };

    // entry: the single non-loop predecessor of header
    let preds = body.basic_blocks.predecessors();
    let mut entry: Option<mir::BasicBlock> = None;
    for &pred in preds[header].iter() {
        if !lp.body.contains(&pred) {
            if entry.replace(pred).is_some() {
                reject!("multiple out-of-loop predecessors of header");
            }
        }
    }
    let Some(entry_bb) = entry else {
        reject!("no out-of-loop predecessor of header");
    };

    // into_iter Call on &[T] gives slice operand
    let mut slice_local: Option<mir::Local> = None;
    for (_bb_idx, block) in body.basic_blocks.iter_enumerated() {
        let term = block.terminator();
        let mir::TerminatorKind::Call { func, args, .. } = &term.kind else { continue };
        let mir::Operand::Constant(c) = func else { continue };
        let rustc_middle::ty::FnDef(def_id, _) = c.const_.ty().kind() else { continue };
        let path = tcx.def_path_str(*def_id);
        if dbg {
            par_dump!(tcx, "[PAR-IDIOM-SLICE-DBG] candidate Call path={}", path);
        }
        // last-segment match, def_path_str may give bare into_iter
        let last = path.rsplit("::").next().unwrap_or("");
        if last != "into_iter" {
            continue;
        }
        // first arg: Place rooted at &[T] Local
        if args.is_empty() {
            continue;
        }
        let arg = &args[0].node;
        let arg_local = match arg {
            mir::Operand::Copy(p) | mir::Operand::Move(p) if p.projection.is_empty() => p.local,
            _ => continue,
        };
        // walk in-block copy chain past compiler temps
        let root = resolve_local_chain(block, arg_local);
        slice_local = Some(root);
        break;
    }
    let Some(mut slice_local) = slice_local else {
        reject!("no into_iter Call found in function");
    };

    // arg Local accepted, type check below verifies

    // verify &[u64]; Phase 1 handles u64 only
    let slice_ty = body.local_decls[slice_local].ty;
    if !is_ref_slice_u64(tcx, slice_ty) {
        // extra entry-block hop for reborrow temps
        let entry_block = &body.basic_blocks[entry_bb];
        let next = resolve_local_chain(entry_block, slice_local);
        let next_ty = body.local_decls[next].ty;
        if next != slice_local && is_ref_slice_u64(tcx, next_ty) {
            slice_local = next;
        } else {
            reject!(
                "slice operand type is not &[u64]: got {:?}",
                slice_ty
            );
        }
    }

    // find acc_local zeroed pre-loop, often in bb0
    let Some(acc_local) = find_u64_acc_init_zero_anywhere(body, &lp.body) else {
        reject!("no u64 Local initialised to const 0 before loop");
    };

    Some(SliceReducePlan { entry_bb, exit_bb, acc_local, slice_local })
}

/// true for `&[u64]` or `&mut [u64]`
#[allow(rustc::usage_of_qualified_ty)]
fn is_ref_slice_u64<'tcx>(
    tcx: TyCtxt<'tcx>,
    ty: rustc_middle::ty::Ty<'tcx>,
) -> bool {
    let rustc_middle::ty::Ref(_, inner, _) = ty.kind() else { return false };
    let rustc_middle::ty::Slice(elem) = inner.kind() else { return false };
    *elem == tcx.types.u64
}

/// first non-loop `_local = const 0_u64` assignment
fn find_u64_acc_init_zero_anywhere<'tcx>(
    body: &mir::Body<'tcx>,
    loop_body: &std::collections::BTreeSet<mir::BasicBlock>,
) -> Option<mir::Local> {
    for (bb_idx, block) in body.basic_blocks.iter_enumerated() {
        if loop_body.contains(&bb_idx) {
            continue;
        }
        for stmt in &block.statements {
            let mir::StatementKind::Assign(box (place, rvalue)) = &stmt.kind else { continue };
            if !place.projection.is_empty() {
                continue;
            }
            let mir::Rvalue::Use(mir::Operand::Constant(c)) = rvalue else { continue };
            if extract_u64_const_loose(&c.const_) != Some(0) {
                continue;
            }
            // no TyCtxt here, codegen catches type mismatch
            return Some(place.local);
        }
    }
    None
}

fn apply_slice_reduce_transform<'tcx>(
    tcx: TyCtxt<'tcx>,
    body: &mut mir::Body<'tcx>,
    plan: &SliceReducePlan,
    sum_def_id: rustc_hir::def_id::DefId,
) -> bool {
    use rustc_middle::mir::*;
    use rustc_span::DUMMY_SP;

    // Callee operand for parallel_reduce_sum_slice_u64.
    let sum_callee =
        Operand::function_handle(tcx, sum_def_id, std::iter::empty(), DUMMY_SP);

    // pass slice by Copy, &[u64] is Copy
    let slice_arg = Operand::Copy(Place::from(plan.slice_local));

    let call_term_kind = TerminatorKind::Call {
        func: sum_callee,
        args: Box::new([rustc_span::source_map::Spanned {
            node: slice_arg,
            span: DUMMY_SP,
        }]),
        destination: Place::from(plan.acc_local),
        target: Some(plan.exit_bb),
        unwind: UnwindAction::Unreachable,
        call_source: CallSource::Misc,
        fn_span: DUMMY_SP,
    };
    let new_bb = BasicBlockData::new_stmts(
        Vec::new(),
        Some(Terminator {
            source_info: SourceInfo::outermost(DUMMY_SP),
            kind: call_term_kind,
        }),
        false,
    );

    // find loop header the entry jumps to
    let header = {
        let mut found: Option<BasicBlock> = None;
        body.basic_blocks[plan.entry_bb].terminator().successors().for_each(|s| {
            if found.is_none() {
                found = Some(s);
            }
        });
        match found {
            Some(h) => h,
            None => return false,
        }
    };

    let new_bb_idx = body.basic_blocks_mut().push(new_bb);

    // rewire entry terminator from header to new_bb
    let entry_term = body.basic_blocks_mut()[plan.entry_bb].terminator_mut();
    entry_term.successors_mut(|succ| {
        if *succ == header {
            *succ = new_bb_idx;
        }
    });

    true
}

// AXPY matcher: y[i] += alpha*x[i] loop, opt-in

/// dst origin: &mut [f64] or &mut Vec<f64>
#[derive(Debug, Clone, Copy)]
enum SliceDst {
    Slice { local: mir::Local },
    VecRef { vec_ref: mir::Local },
}

#[derive(Debug, Clone)]
struct AxpyPlan<'tcx> {
    entry_bb: mir::BasicBlock,
    exit_bb: mir::BasicBlock,
    /// y destination, slice ref or Vec ref
    y_dst: SliceDst,
    /// Local of type `&[f{32,64}]` (the x operand).
    x_local: mir::Local,
    /// Local of type `f{32,64}` (the scalar alpha).
    alpha_local: mir::Local,
    /// element type; Vec form is f64 only
    fty: FloatTy,
    /// source op was Sub rather than Add
    is_sub: bool,
    _phantom: std::marker::PhantomData<&'tcx ()>,
}

fn analyze_axpy_loop_for_transform<'tcx>(
    tcx: TyCtxt<'tcx>,
    body: &mir::Body<'tcx>,
    lp: &SimpleLoop,
) -> Option<AxpyPlan<'tcx>> {
    use rustc_middle::mir::*;
    let dbg = std::env::var("PARALLEL_AXPY_DEBUG").ok().as_deref() == Some("1");
    macro_rules! reject {
        ($($arg:tt)*) => {{
            if dbg { eprintln!("[PAR-IDIOM-AXPY-REJECT-WHY] header=bb{}: {}", lp.header.index(), format!($($arg)*)); }
            return None;
        }};
    }

    let header = lp.header;

    // header terminator must be Iterator::next or Range::next
    let header_term = body.basic_blocks[header].terminator();
    let TerminatorKind::Call { func, destination, target, .. } = &header_term.kind
    else { reject!("header terminator is not a Call"); };
    let Some(target) = *target else { reject!("header Call has no return target"); };
    if !destination.projection.is_empty() { reject!("destination has projection"); }
    let opt_local = destination.local;
    let Operand::Constant(c) = func else { reject!("indirect Call in header"); };
    let rustc_middle::ty::FnDef(callee_did, _) = c.const_.ty().kind() else {
        reject!("Call callee not FnDef");
    };
    let callee_path = tcx.def_path_str(*callee_did);
    if !callee_path.ends_with("::next") { reject!("not Iterator::next ({})", callee_path); }

    // post block switches on discriminant(opt_local)
    let post_block = &body.basic_blocks[target];
    let TerminatorKind::SwitchInt { discr, targets } = &post_block.terminator().kind
    else { reject!("post not switchInt"); };
    let discr_local = match discr {
        Operand::Move(p) | Operand::Copy(p) if p.projection.is_empty() => p.local,
        _ => reject!("switchInt discr not bare Local"),
    };
    let mut found_disc = false;
    for stmt in &post_block.statements {
        let StatementKind::Assign(box (place, rvalue)) = &stmt.kind else { continue };
        if !place.projection.is_empty() || place.local != discr_local { continue; }
        if let Rvalue::Discriminant(dp) = rvalue {
            if dp.local == opt_local && dp.projection.is_empty() {
                found_disc = true; break;
            }
        }
    }
    if !found_disc { reject!("no discriminant assignment for opt"); }
    let mut exit_bb: Option<BasicBlock> = None;
    let mut some_arm: Option<BasicBlock> = None;
    for (val, t) in targets.iter() {
        match val { 0 => exit_bb = Some(t), 1 => some_arm = Some(t), _ => {} }
    }
    let Some(exit_bb) = exit_bb else { reject!("no exit branch"); };
    let Some(some_arm) = some_arm else { reject!("no some_arm branch"); };
    if !lp.body.contains(&some_arm) { reject!("some_arm bb{} not in loop body", some_arm.index()); }

    // find induction var extraction in some-arm block
    let some_block = &body.basic_blocks[some_arm];
    let mut i_local: Option<Local> = None;
    for stmt in &some_block.statements {
        let StatementKind::Assign(box (place, rvalue)) = &stmt.kind else { continue };
        if !place.projection.is_empty() { continue; }
        let Rvalue::Use(op) = rvalue else { continue };
        let src_place = match op {
            Operand::Copy(p) | Operand::Move(p) => p,
            _ => continue,
        };
        if src_place.local != opt_local { continue; }
        // Projection should be [Downcast(Some), Field(0)].
        if src_place.projection.len() != 2 { continue; }
        let is_downcast_some =
            matches!(src_place.projection[0], ProjectionElem::Downcast(_, _));
        let is_field0 = matches!(
            src_place.projection[1],
            ProjectionElem::Field(idx, _) if idx.as_u32() == 0
        );
        if !is_downcast_some || !is_field0 { continue; }
        i_local = Some(place.local);
        break;
    }
    let Some(i_local) = i_local else { reject!("no induction-var extraction in some_arm"); };

    // alias map, lowering copies i_local/alpha to temps
    let mut alias_root: std::collections::BTreeMap<Local, Local> = Default::default();
    for &bb in &lp.body {
        for stmt in &body.basic_blocks[bb].statements {
            let StatementKind::Assign(box (place, rvalue)) = &stmt.kind else { continue };
            if !place.projection.is_empty() { continue; }
            if let Rvalue::Use(op) = rvalue {
                if let Operand::Copy(p) | Operand::Move(p) = op {
                    if p.projection.is_empty() {
                        alias_root.insert(place.local, p.local);
                    }
                }
            }
            if let Rvalue::Ref(_, _, p) = rvalue {
                if p.projection.len() == 1
                    && matches!(p.projection[0], ProjectionElem::Deref)
                {
                    alias_root.insert(place.local, p.local);
                }
            }
        }
    }
    // Walk pre-loop blocks too
    for (bb_idx, block) in body.basic_blocks.iter_enumerated() {
        if lp.body.contains(&bb_idx) { continue; }
        for stmt in &block.statements {
            let StatementKind::Assign(box (place, rvalue)) = &stmt.kind else { continue };
            if !place.projection.is_empty() { continue; }
            if let Rvalue::Use(op) = rvalue {
                if let Operand::Copy(p) | Operand::Move(p) = op {
                    if p.projection.is_empty() {
                        alias_root.entry(place.local).or_insert(p.local);
                    }
                }
            }
            if let Rvalue::Ref(_, _, p) = rvalue {
                if p.projection.len() == 1
                    && matches!(p.projection[0], ProjectionElem::Deref)
                {
                    alias_root.entry(place.local).or_insert(p.local);
                }
            }
        }
    }
    // chase alias chain, capped at 16 hops
    let resolve = |l: Local| -> Local {
        let mut cur = l;
        for _ in 0..16 {
            match alias_root.get(&cur) {
                Some(&n) if n != cur => cur = n,
                _ => break,
            }
        }
        cur
    };
    let i_root = resolve(i_local);
    let same_as_i = |l: Local| -> bool { resolve(l) == i_root };

    // Walk EVERY block in the loop body

    let mut x_reads: Vec<(Local, Local)> = Vec::new();   // (xi_local, x_local)
    let mut muls: Vec<(Local, Local, Local)> = Vec::new(); // (mul_local, alpha_local, xi_local)
    // slice-form writes: (y_local, mul_local, is_sub)
    let mut writes: Vec<(Local, Local, bool)> = Vec::new();
    // Vec-form writes (vec_ref, mul, is_sub)
    let mut vec_writes: Vec<(Local, Local, bool)> = Vec::new();
    let mut indexed_writes: usize = 0;

    // Pass A, slice form, walk body BBs
    for &bb in &lp.body {
        if bb == header { continue; }
        let block = &body.basic_blocks[bb];
        for stmt in &block.statements {
            let StatementKind::Assign(box (place, rvalue)) = &stmt.kind else { continue };

            // (c) Indexed write through Deref
            if place.projection.len() >= 2
                && matches!(place.projection[0], ProjectionElem::Deref)
                && matches!(place.projection[1], ProjectionElem::Index(idx) if same_as_i(idx))
            {
                {
                    indexed_writes += 1;
                    let y_local = place.local;
                    if let Rvalue::BinaryOp(op, box (lhs, rhs)) = rvalue {
                        let is_add = matches!(op, BinOp::Add | BinOp::AddUnchecked);
                        let is_sub = matches!(op, BinOp::Sub | BinOp::SubUnchecked);
                        if !(is_add || is_sub) { continue; }
                        // One side must self-read (*_y)[_i]
                        let self_read = |op: &Operand<'tcx>| -> bool {
                            let p = match op {
                                Operand::Copy(p) | Operand::Move(p) => p,
                                _ => return false,
                            };
                            if p.local != y_local { return false; }
                            if p.projection.len() < 2 { return false; }
                            matches!(p.projection[0], ProjectionElem::Deref)
                                && matches!(
                                    p.projection[1],
                                    ProjectionElem::Index(idx) if same_as_i(idx)
                                )
                        };
                        let other_local = |op: &Operand<'tcx>| -> Option<Local> {
                            let p = match op {
                                Operand::Copy(p) | Operand::Move(p) => p,
                                _ => return None,
                            };
                            if !p.projection.is_empty() { return None; }
                            Some(p.local)
                        };
                        // Sub needs `y[i] - mul`, not reversed
                        if is_sub {
                            if self_read(lhs) {
                                if let Some(m) = other_local(rhs) {
                                    writes.push((y_local, m, true));
                                }
                            }
                        } else {
                            if self_read(lhs) {
                                if let Some(m) = other_local(rhs) {
                                    writes.push((y_local, m, false));
                                }
                            } else if self_read(rhs) {
                                if let Some(m) = other_local(lhs) {
                                    writes.push((y_local, m, false));
                                }
                            }
                        }
                    }
                    continue;
                }
            }

            // Indexed read or `_mul = Mul(alpha, xi)`
            if place.projection.is_empty() {
                if let Rvalue::Use(op) = rvalue {
                    if let Operand::Copy(p) | Operand::Move(p) = op {
                        if p.projection.len() >= 2
                            && matches!(p.projection[0], ProjectionElem::Deref)
                            && matches!(
                                p.projection[1],
                                ProjectionElem::Index(idx) if same_as_i(idx)
                            )
                        {
                            x_reads.push((place.local, p.local));
                            continue;
                        }
                    }
                }
                if let Rvalue::BinaryOp(op, box (lhs, rhs)) = rvalue {
                    if matches!(op, BinOp::Mul | BinOp::MulUnchecked) {
                        let local_op = |o: &Operand<'tcx>| -> Option<Local> {
                            let p = match o {
                                Operand::Copy(p) | Operand::Move(p) => p,
                                _ => return None,
                            };
                            if !p.projection.is_empty() { return None; }
                            Some(p.local)
                        };
                        if let (Some(la), Some(lb)) = (local_op(lhs), local_op(rhs)) {
                            // alpha/xi unknown, record both, unify later
                            muls.push((place.local, la, lb));
                            muls.push((place.local, lb, la));
                        }
                    }
                }
            }
        }
    }

    // Pass B, Vec form via index_mut Call
    for &bb in &lp.body {
        if bb == header { continue; }
        let block = &body.basic_blocks[bb];
        let TerminatorKind::Call { func, args, destination, target: Some(target_bb), .. } =
            &block.terminator().kind
        else { continue };
        let Operand::Constant(c) = func else { continue };
        let rustc_middle::ty::FnDef(callee_did, _) = c.const_.ty().kind() else { continue };
        let path = tcx.def_path_str(*callee_did);
        // Match `<Vec<T> as IndexMut<usize>>::index_mut`
        if !path.ends_with("::index_mut") { continue; }
        if args.len() != 2 { continue; }
        if !destination.projection.is_empty() { continue; }
        let t_local = destination.local;
        let arg0_local = match &args[0].node {
            Operand::Move(p) | Operand::Copy(p) if p.projection.is_empty() => p.local,
            _ => continue,
        };
        let arg1_local = match &args[1].node {
            Operand::Move(p) | Operand::Copy(p) if p.projection.is_empty() => p.local,
            _ => continue,
        };
        if !same_as_i(arg1_local) { continue; }
        let vec_ref_local = resolve(arg0_local);
        let vec_ty = body.local_decls[vec_ref_local].ty;
        if !is_ref_vec_f64(tcx, vec_ty, /*require_mut=*/true) { continue; }
        // Target BB adds `_mul` through `*_t`
        let next_block = &body.basic_blocks[*target_bb];
        for stmt in &next_block.statements {
            let StatementKind::Assign(box (place, rvalue)) = &stmt.kind else { continue };
            if place.local != t_local { continue; }
            if place.projection.len() != 1 { continue; }
            if !matches!(place.projection[0], ProjectionElem::Deref) { continue; }
            let Rvalue::BinaryOp(op, box (lhs, rhs)) = rvalue else { continue };
            let is_add = matches!(op, BinOp::Add | BinOp::AddUnchecked);
            let is_sub = matches!(op, BinOp::Sub | BinOp::SubUnchecked);
            if !(is_add || is_sub) { continue; }
            let self_read = |op: &Operand<'tcx>| -> bool {
                let p = match op {
                    Operand::Copy(p) | Operand::Move(p) => p,
                    _ => return false,
                };
                if p.local != t_local { return false; }
                if p.projection.len() != 1 { return false; }
                matches!(p.projection[0], ProjectionElem::Deref)
            };
            let other_local = |op: &Operand<'tcx>| -> Option<Local> {
                let p = match op {
                    Operand::Copy(p) | Operand::Move(p) => p,
                    _ => return None,
                };
                if !p.projection.is_empty() { return None; }
                Some(p.local)
            };
            indexed_writes += 1;
            if is_sub {
                if self_read(lhs) {
                    if let Some(m) = other_local(rhs) { vec_writes.push((vec_ref_local, m, true)); }
                }
            } else {
                if self_read(lhs) {
                    if let Some(m) = other_local(rhs) { vec_writes.push((vec_ref_local, m, false)); }
                } else if self_read(rhs) {
                    if let Some(m) = other_local(lhs) { vec_writes.push((vec_ref_local, m, false)); }
                }
            }
        }
    }

    if dbg { eprintln!("[PAR-IDIOM-AXPY-DBG] indexed_writes={} slice_writes={} vec_writes={} muls={} x_reads={}",
        indexed_writes, writes.len(), vec_writes.len(), muls.len(), x_reads.len()); }
    if indexed_writes != 1 { reject!("indexed_writes={}", indexed_writes); }
    // Exactly one form must have fired
    let (y_dst_local, mul_local, is_sub, is_vec_form) = match (writes.len(), vec_writes.len()) {
        (1, 0) => {
            let (y, m, s) = writes.pop().unwrap();
            (y, m, s, false)
        }
        (0, 1) => {
            let (vr, m, s) = vec_writes.pop().unwrap();
            (vr, m, s, true)
        }
        _ => reject!("write counts mismatch (slice={}, vec={})", writes.len(), vec_writes.len()),
    };
    let y_local = y_dst_local;

    // Match mul Local, find (alpha, xi), x-read
    let mut chosen: Option<(Local, Local, Local)> = None; // (alpha, xi, x_local)
    for &(m, alpha_l, xi_l) in &muls {
        if m != mul_local { continue; }
        for &(xi_seen, x_local) in &x_reads {
            if xi_seen == xi_l {
                chosen = Some((alpha_l, xi_l, x_local));
                break;
            }
        }
        if chosen.is_some() { break; }
    }
    let Some((alpha_local_raw, _xi_local, x_local_raw)) = chosen else {
        reject!("no consistent (mul, x_read) pair");
    };
    // Resolve aliases so helper args are entry-live
    let alpha_local = resolve(alpha_local_raw);
    let x_local = resolve(x_local_raw);
    let y_local = resolve(y_local);

    // Type checks; f32/f64 agree, Vec form f64-only
    let y_ty = body.local_decls[y_local].ty;
    let x_ty = body.local_decls[x_local].ty;
    let alpha_ty = body.local_decls[alpha_local].ty;
    let (y_dst, fty) = if is_vec_form {
        if !is_ref_vec_f64(tcx, y_ty, /*require_mut=*/true) {
            reject!("y type not &mut Vec<f64>: {:?}", y_ty);
        }
        (SliceDst::VecRef { vec_ref: y_local }, FloatTy::F64)
    } else {
        let Some(yfty) = ref_slice_float_ty(tcx, y_ty, /*require_mut=*/true) else {
            reject!("y type not &mut [f32/f64]: {:?}", y_ty);
        };
        (SliceDst::Slice { local: y_local }, yfty)
    };
    let Some(xfty) = ref_slice_float_ty(tcx, x_ty, /*require_mut=*/false) else {
        reject!("x type not &[f32/f64]/&mut [f32/f64]: {:?}", x_ty);
    };
    if xfty != fty { reject!("x type doesn't match y type ({:?} vs {:?})", xfty, fty); }
    let expected_alpha_ty = match fty {
        FloatTy::F64 => tcx.types.f64,
    };
    if alpha_ty != expected_alpha_ty {
        reject!("alpha type doesn't match y elem type: {:?} vs {:?}", alpha_ty, expected_alpha_ty);
    }
    if y_local == x_local { reject!("y and x are same Local"); }

    // Find unique out-of-loop predecessor of header
    let preds = body.basic_blocks.predecessors();
    let mut entry_bb: Option<BasicBlock> = None;
    for &pred in preds[header].iter() {
        if !lp.body.contains(&pred) {
            if entry_bb.replace(pred).is_some() { return None; }
        }
    }
    let Some(entry_bb) = entry_bb else { reject!("no out-of-loop predecessor of header"); };

    Some(AxpyPlan {
        entry_bb,
        exit_bb,
        y_dst,
        x_local,
        alpha_local,
        fty,
        is_sub,
        _phantom: std::marker::PhantomData,
    })
}

/// True iff `ty` is `&mut Vec<f64>`
#[allow(rustc::usage_of_qualified_ty)]
fn is_ref_vec_f64<'tcx>(
    tcx: TyCtxt<'tcx>,
    ty: rustc_middle::ty::Ty<'tcx>,
    require_mut: bool,
) -> bool {
    let rustc_middle::ty::Ref(_, inner, mutbl) = ty.kind() else { return false; };
    if require_mut && *mutbl != rustc_ast::Mutability::Mut { return false; }
    let rustc_middle::ty::Adt(adt_def, generic_args) = inner.kind() else { return false; };
    // Match Adt def_path ending in vec::Vec
    let path = tcx.def_path_str(adt_def.did());
    if !(path == "Vec" || path == "alloc::vec::Vec" || path == "std::vec::Vec"
        || path.ends_with("::Vec"))
    {
        return false;
    }
    // First generic arg must be f64.
    let Some(elem_ty) = generic_args.types().next() else { return false; };
    elem_ty == tcx.types.f64
}

/// True iff `ty` is `&[f64]`
#[allow(rustc::usage_of_qualified_ty)]
#[allow(dead_code)]
fn is_ref_slice_f64<'tcx>(
    tcx: TyCtxt<'tcx>,
    ty: rustc_middle::ty::Ty<'tcx>,
    require_mut: bool,
) -> bool {
    let rustc_middle::ty::Ref(_, inner, mutbl) = ty.kind() else { return false; };
    let rustc_middle::ty::Slice(elem) = inner.kind() else { return false; };
    if *elem != tcx.types.f64 { return false; }
    if require_mut && *mutbl != rustc_ast::Mutability::Mut { return false; }
    true
}

fn apply_axpy_transform<'tcx>(
    tcx: TyCtxt<'tcx>,
    body: &mut mir::Body<'tcx>,
    plan: &AxpyPlan<'tcx>,
    helper_def_id: rustc_hir::def_id::DefId,
) -> bool {
    use rustc_middle::mir::*;
    use rustc_span::DUMMY_SP;
    use rustc_span::source_map::Spanned;

    let elem_ty = match plan.fty {
        FloatTy::F64 => tcx.types.f64,
    };
    #[allow(rustc::usage_of_qualified_ty)]
    let slice_ty = rustc_middle::ty::Ty::new_slice(tcx, elem_ty);
    #[allow(rustc::usage_of_qualified_ty)]
    let mut_slice_ref_ty = rustc_middle::ty::Ty::new_mut_ref(
        tcx,
        tcx.lifetimes.re_erased,
        slice_ty,
    );

    // Build y operand; Vec form calls as_mut_slice
    let (entry_target, helper_y_op) = match plan.y_dst {
        SliceDst::Slice { local: y_local } => {
            // Reborrow `&mut *_y` into temp, move it
            let y_temp = body.local_decls.push(LocalDecl::new(mut_slice_ref_ty, DUMMY_SP));
            let reborrow_stmt = Statement::new(
                SourceInfo::outermost(DUMMY_SP),
                StatementKind::Assign(Box::new((
                    Place::from(y_temp),
                    Rvalue::Ref(
                        tcx.lifetimes.re_erased,
                        BorrowKind::Mut { kind: MutBorrowKind::Default },
                        Place {
                            local: y_local,
                            projection: tcx.mk_place_elems(&[ProjectionElem::Deref]),
                        },
                    ),
                ))),
            );
            (Either::Stmt(vec![reborrow_stmt]), Operand::Move(Place::from(y_temp)))
        }
        SliceDst::VecRef { vec_ref } => {
            // Separate BB calls as_mut_slice, then helper BB
            fn lookup_diag(tcx: TyCtxt<'_>, name: &str) -> Option<rustc_hir::def_id::DefId> {
                tcx.get_diagnostic_item(rustc_span::Symbol::intern(name))
            }
            let as_mut_slice_did = match lookup_diag(tcx, "vec_as_mut_slice") {
                Some(d) => d,
                None => return false,
            };
            // Reborrow keeps original ref live post-loop
            let vec_ref_ty = body.local_decls[vec_ref].ty;
            let vec_reborrow = body.local_decls.push(LocalDecl::new(vec_ref_ty, DUMMY_SP));
            let reborrow_stmt = Statement::new(
                SourceInfo::outermost(DUMMY_SP),
                StatementKind::Assign(Box::new((
                    Place::from(vec_reborrow),
                    Rvalue::Ref(
                        tcx.lifetimes.re_erased,
                        BorrowKind::Mut { kind: MutBorrowKind::Default },
                        Place {
                            local: vec_ref,
                            projection: tcx.mk_place_elems(&[ProjectionElem::Deref]),
                        },
                    ),
                ))),
            );
            // Generic args for Vec::as_mut_slice
            let global_alloc_did = match tcx.lang_items().global_alloc_ty() {
                Some(d) => d,
                None => return false,
            };
            #[allow(rustc::usage_of_qualified_ty)]
            let global_alloc_ty = rustc_middle::ty::Ty::new_adt(
                tcx,
                tcx.adt_def(global_alloc_did),
                tcx.mk_args(&[]),
            );
            // Vec form is f64-only
            let as_mut_slice_callee = Operand::function_handle(
                tcx,
                as_mut_slice_did,
                [
                    rustc_middle::ty::GenericArg::from(tcx.types.f64),
                    rustc_middle::ty::GenericArg::from(global_alloc_ty),
                ].into_iter(),
                DUMMY_SP,
            );
            let slice_temp = body.local_decls.push(LocalDecl::new(mut_slice_ref_ty, DUMMY_SP));
            // Target BB index patched after pushing helper
            (
                Either::CallDeref {
                    reborrow_stmt,
                    as_mut_slice_callee,
                    vec_reborrow,
                    slice_temp,
                },
                Operand::Move(Place::from(slice_temp)),
            )
        }
    };

    // alpha_arg: copy alpha, optionally negate.
    let mut alpha_prelude_stmts: Vec<Statement<'tcx>> = Vec::new();
    let alpha_op: Operand<'tcx> = if plan.is_sub {
        let neg_local = body.local_decls.push(LocalDecl::new(elem_ty, DUMMY_SP));
        alpha_prelude_stmts.push(Statement::new(
            SourceInfo::outermost(DUMMY_SP),
            StatementKind::Assign(Box::new((
                Place::from(neg_local),
                Rvalue::UnaryOp(UnOp::Neg, Operand::Copy(Place::from(plan.alpha_local))),
            ))),
        ));
        Operand::Move(Place::from(neg_local))
    } else {
        Operand::Copy(Place::from(plan.alpha_local))
    };

    let unit_ty = tcx.types.unit;
    let unit_dest = body.local_decls.push(LocalDecl::new(unit_ty, DUMMY_SP));

    let helper_callee = Operand::function_handle(
        tcx,
        helper_def_id,
        std::iter::empty(),
        DUMMY_SP,
    );
    let helper_term_kind = TerminatorKind::Call {
        func: helper_callee,
        args: Box::new([
            Spanned { node: helper_y_op, span: DUMMY_SP },
            Spanned { node: Operand::Copy(Place::from(plan.x_local)), span: DUMMY_SP },
            Spanned { node: alpha_op, span: DUMMY_SP },
        ]),
        destination: Place::from(unit_dest),
        target: Some(plan.exit_bb),
        unwind: UnwindAction::Unreachable,
        call_source: CallSource::Misc,
        fn_span: DUMMY_SP,
    };

    // Find loop header entry_bb jumps to
    let header = {
        let mut found: Option<BasicBlock> = None;
        body.basic_blocks[plan.entry_bb].terminator().successors().for_each(|s| {
            if found.is_none() { found = Some(s); }
        });
        match found { Some(h) => h, None => return false, }
    };

    // Build the helper-Call BB
    let helper_bb_stmts: Vec<Statement<'tcx>> = match &entry_target {
        Either::Stmt(stmts) => {
            let mut s = stmts.clone();
            s.extend(alpha_prelude_stmts);
            s
        }
        Either::CallDeref { .. } => alpha_prelude_stmts,
    };
    let helper_bb_data = BasicBlockData::new_stmts(
        helper_bb_stmts,
        Some(Terminator {
            source_info: SourceInfo::outermost(DUMMY_SP),
            kind: helper_term_kind,
        }),
        false,
    );
    let helper_bb_idx = body.basic_blocks_mut().push(helper_bb_data);

    let entry_target_bb = match entry_target {
        Either::Stmt(_) => helper_bb_idx,
        Either::CallDeref { reborrow_stmt, as_mut_slice_callee, vec_reborrow, slice_temp } => {
            // bb_deref reborrows, calls as_mut_slice, then helper_bb
            let deref_term = TerminatorKind::Call {
                func: as_mut_slice_callee,
                args: Box::new([Spanned {
                    node: Operand::Move(Place::from(vec_reborrow)),
                    span: DUMMY_SP,
                }]),
                destination: Place::from(slice_temp),
                target: Some(helper_bb_idx),
                unwind: UnwindAction::Unreachable,
                call_source: CallSource::Misc,
                fn_span: DUMMY_SP,
            };
            let deref_bb_data = BasicBlockData::new_stmts(
                vec![reborrow_stmt],
                Some(Terminator {
                    source_info: SourceInfo::outermost(DUMMY_SP),
                    kind: deref_term,
                }),
                false,
            );
            body.basic_blocks_mut().push(deref_bb_data)
        }
    };

    // Rewire entry_bb header edges to entry_target_bb
    let entry_term = body.basic_blocks_mut()[plan.entry_bb].terminator_mut();
    entry_term.successors_mut(|succ| {
        if *succ == header { *succ = entry_target_bb; }
    });
    true
}

/// Inline statements or deref-Call BB, for apply_axpy_transform
enum Either<'tcx> {
    Stmt(Vec<mir::Statement<'tcx>>),
    CallDeref {
        reborrow_stmt: mir::Statement<'tcx>,
        as_mut_slice_callee: mir::Operand<'tcx>,
        vec_reborrow: mir::Local,
        slice_temp: mir::Local,
    },
}

pub(crate) fn try_transform_slice_axpy_f64<'tcx>(
    tcx: TyCtxt<'tcx>,
    body: &mut mir::Body<'tcx>,
) -> usize {
    if !idiom_transform_enabled(tcx) { return 0; }
    if is_in_sysroot_crate(tcx, body.source.def_id()) { return 0; }
    let fn_name = tcx.def_path_str(body.source.def_id());
    if fn_name.contains("parallel_runtime") || fn_name.contains("parallel_reduce_")
        || fn_name.contains("parallel_axpy") || fn_name.contains("parallel_invoke")
    { return 0; }
    // AXPY needs associative-float opt-in; chunking changes rounding
    if !is_associative_float_opted_in(tcx, body.source.def_id()) { return 0; }
    if parallel_pass_quick_skip(tcx, body, 4, true) { return 0; }

    fn lookup(tcx: TyCtxt<'_>, name: &str) -> Option<rustc_hir::def_id::DefId> {
        tcx.get_diagnostic_item(rustc_span::Symbol::intern(name))
    }
    let helper_f64 = lookup(tcx, "parallel_runtime_parallel_axpy_f64");
    if helper_f64.is_none() { return 0; }

    let loops = find_simple_loops(body);
    let mut plans: Vec<AxpyPlan<'tcx>> = Vec::new();
    for lp in &loops {
        if let Some(p) = analyze_axpy_loop_for_transform(tcx, body, lp) {
            let (y_desc, y_idx) = match p.y_dst {
                SliceDst::Slice { local } => ("slice", local.index()),
                SliceDst::VecRef { vec_ref } => ("vec", vec_ref.index()),
            };
            par_dump!(tcx,
                "[PAR-IDIOM-AXPY-CANDIDATE] fn={} entry=bb{} exit=bb{} y={}=_{} x=_{} alpha=_{} sub={} fty={:?}",
                fn_name, p.entry_bb.index(), p.exit_bb.index(),
                y_desc, y_idx, p.x_local.index(), p.alpha_local.index(),
                p.is_sub, p.fty,
            );
            plans.push(p);
        }
    }
    let mut count = 0;
    for plan in plans {
        let helper_def_id = helper_f64;
        let Some(helper_def_id) = helper_def_id else { continue };
        if apply_axpy_transform(tcx, body, &plan, helper_def_id) {
            par_dump!(tcx,
                "[PAR-IDIOM-AXPY-APPLIED] fn={} entry=bb{} fty={:?}",
                fn_name, plan.entry_bb.index(), plan.fty,
            );
            count += 1;
        }
    }
    count
}

/// Does the enclosing fn carry `#[parallelize_associative_float]`?
fn is_associative_float_opted_in<'tcx>(
    tcx: TyCtxt<'tcx>,
    did: rustc_hir::def_id::DefId,
) -> bool {
    let mut attrs = tcx.get_attrs(did, rustc_span::sym::parallelize_associative_float);
    attrs.next().is_some()
}

// Common Range-loop detector shared by typed matchers

#[derive(Debug)]
struct LoopShape {
    entry_bb: mir::BasicBlock,
    exit_bb: mir::BasicBlock,
    i_root: mir::Local,
    alias_root: std::collections::BTreeMap<mir::Local, mir::Local>,
}

impl LoopShape {
    fn same_as_i(&self, l: mir::Local) -> bool {
        Self::resolve(&self.alias_root, l) == self.i_root
    }
    fn resolve(alias_root: &std::collections::BTreeMap<mir::Local, mir::Local>, l: mir::Local) -> mir::Local {
        let mut cur = l;
        for _ in 0..16 {
            match alias_root.get(&cur) {
                Some(&n) if n != cur => cur = n,
                _ => break,
            }
        }
        cur
    }
}

fn detect_range_loop_shape<'tcx>(
    tcx: TyCtxt<'tcx>,
    body: &mir::Body<'tcx>,
    lp: &SimpleLoop,
) -> Option<LoopShape> {
    use rustc_middle::mir::*;
    let header = lp.header;
    let header_term = body.basic_blocks[header].terminator();
    let TerminatorKind::Call { func, destination, target, .. } = &header_term.kind else { return None; };
    let target = (*target)?;
    if !destination.projection.is_empty() { return None; }
    let opt_local = destination.local;
    let Operand::Constant(c) = func else { return None; };
    let rustc_middle::ty::FnDef(callee_did, _) = c.const_.ty().kind() else { return None; };
    let callee_path = tcx.def_path_str(*callee_did);
    if !callee_path.ends_with("::next") { return None; }

    let post_block = &body.basic_blocks[target];
    let TerminatorKind::SwitchInt { discr, targets } = &post_block.terminator().kind else { return None; };
    let discr_local = match discr {
        Operand::Move(p) | Operand::Copy(p) if p.projection.is_empty() => p.local,
        _ => return None,
    };
    let mut found_disc = false;
    for stmt in &post_block.statements {
        let StatementKind::Assign(box (place, rvalue)) = &stmt.kind else { continue };
        if !place.projection.is_empty() || place.local != discr_local { continue; }
        if let Rvalue::Discriminant(dp) = rvalue {
            if dp.local == opt_local && dp.projection.is_empty() {
                found_disc = true; break;
            }
        }
    }
    if !found_disc { return None; }
    let mut exit_bb: Option<BasicBlock> = None;
    let mut some_arm: Option<BasicBlock> = None;
    for (val, t) in targets.iter() {
        match val { 0 => exit_bb = Some(t), 1 => some_arm = Some(t), _ => {} }
    }
    let exit_bb = exit_bb?;
    let some_arm = some_arm?;
    if !lp.body.contains(&some_arm) { return None; }

    // Induction var `_i = ((opt as Some).0)`
    let some_block = &body.basic_blocks[some_arm];
    let mut i_local: Option<Local> = None;
    for stmt in &some_block.statements {
        let StatementKind::Assign(box (place, rvalue)) = &stmt.kind else { continue };
        if !place.projection.is_empty() { continue; }
        let Rvalue::Use(op) = rvalue else { continue };
        let src_place = match op {
            Operand::Copy(p) | Operand::Move(p) => p,
            _ => continue,
        };
        if src_place.local != opt_local { continue; }
        if src_place.projection.len() != 2 { continue; }
        let is_downcast_some =
            matches!(src_place.projection[0], ProjectionElem::Downcast(_, _));
        let is_field0 = matches!(
            src_place.projection[1],
            ProjectionElem::Field(idx, _) if idx.as_u32() == 0
        );
        if !is_downcast_some || !is_field0 { continue; }
        i_local = Some(place.local);
        break;
    }
    let i_local = i_local?;

    // Build copy-alias map for resolution.
    let mut alias_root: std::collections::BTreeMap<Local, Local> = Default::default();
    for &bb in &lp.body {
        for stmt in &body.basic_blocks[bb].statements {
            let StatementKind::Assign(box (place, rvalue)) = &stmt.kind else { continue };
            if !place.projection.is_empty() { continue; }
            if let Rvalue::Use(op) = rvalue {
                if let Operand::Copy(p) | Operand::Move(p) = op {
                    if p.projection.is_empty() {
                        alias_root.insert(place.local, p.local);
                    }
                }
            }
            if let Rvalue::Ref(_, _, p) = rvalue {
                if p.projection.len() == 1
                    && matches!(p.projection[0], ProjectionElem::Deref)
                {
                    alias_root.insert(place.local, p.local);
                }
            }
        }
    }
    // Walk pre-loop blocks too
    for (bb_idx, block) in body.basic_blocks.iter_enumerated() {
        if lp.body.contains(&bb_idx) { continue; }
        for stmt in &block.statements {
            let StatementKind::Assign(box (place, rvalue)) = &stmt.kind else { continue };
            if !place.projection.is_empty() { continue; }
            if let Rvalue::Use(op) = rvalue {
                if let Operand::Copy(p) | Operand::Move(p) = op {
                    if p.projection.is_empty() {
                        alias_root.entry(place.local).or_insert(p.local);
                    }
                }
            }
            if let Rvalue::Ref(_, _, p) = rvalue {
                if p.projection.len() == 1
                    && matches!(p.projection[0], ProjectionElem::Deref)
                {
                    alias_root.entry(place.local).or_insert(p.local);
                }
            }
        }
    }
    let i_root = LoopShape::resolve(&alias_root, i_local);

    // Find the unique out-of-loop predecessor of header.
    let preds = body.basic_blocks.predecessors();
    let mut entry_bb: Option<BasicBlock> = None;
    for &pred in preds[header].iter() {
        if !lp.body.contains(&pred) {
            if entry_bb.replace(pred).is_some() { return None; }
        }
    }
    let entry_bb = entry_bb?;

    Some(LoopShape { entry_bb, exit_bb, i_root, alias_root })
}

// zip-sub-write matcher, `dst[i] = a[i] - b[i]`

#[derive(Debug, Clone)]
struct ZipSubWritePlan {
    entry_bb: mir::BasicBlock,
    exit_bb: mir::BasicBlock,
    dst_local: mir::Local,
    a_local: mir::Local,
    b_local: mir::Local,
    fty: FloatTy,
}

fn analyze_zip_sub_write_loop<'tcx>(
    tcx: TyCtxt<'tcx>,
    body: &mir::Body<'tcx>,
    lp: &SimpleLoop,
) -> Option<ZipSubWritePlan> {
    use rustc_middle::mir::*;
    let shape = detect_range_loop_shape(tcx, body, lp)?;
    let same_as_i = |l: Local| shape.same_as_i(l);
    let resolve = |l: Local| LoopShape::resolve(&shape.alias_root, l);

    // Two reads, one Sub, single indexed write
    let mut reads: Vec<(Local, Local)> = Vec::new();    // (vi_local, src_local)
    // Two-stmt or inline Sub; collect both, unify
    let mut subs: Vec<(Local, Local, Local)> = Vec::new(); // (diff_or_dst, la_local, lb_local)
    // One-stmt form, dst Local doubles as diff_local
    let mut writes: Vec<Local> = Vec::new();   // dst_local for each indexed write
    let mut indexed_writes = 0usize;
    // Per-write value Local, None means inline Sub
    let mut write_kind: Vec<(Local, Option<Local>)> = Vec::new(); // (dst, Some(value_local) | None for inline-sub)

    for &bb in &lp.body {
        if bb == lp.header { continue; }
        for stmt in &body.basic_blocks[bb].statements {
            let StatementKind::Assign(box (place, rvalue)) = &stmt.kind else { continue };
            // Indexed write: (*_dst)[_i] = ...
            if place.projection.len() >= 2
                && matches!(place.projection[0], ProjectionElem::Deref)
                && matches!(place.projection[1], ProjectionElem::Index(idx) if same_as_i(idx))
            {
                indexed_writes += 1;
                writes.push(place.local);
                let local_op = |o: &Operand<'tcx>| -> Option<Local> {
                    let p = match o {
                        Operand::Copy(p) | Operand::Move(p) => p,
                        _ => return None,
                    };
                    if !p.projection.is_empty() { return None; }
                    Some(p.local)
                };
                match rvalue {
                    Rvalue::Use(op) => {
                        if let Some(src) = local_op(op) {
                            write_kind.push((place.local, Some(src)));
                        }
                    }
                    Rvalue::BinaryOp(op, box (lhs, rhs))
                        if matches!(op, BinOp::Sub | BinOp::SubUnchecked) =>
                    {
                        if let (Some(la), Some(lb)) = (local_op(lhs), local_op(rhs)) {
                            subs.push((place.local, la, lb));
                            write_kind.push((place.local, None));
                        }
                    }
                    _ => {}
                }
                continue;
            }
            // Plain assign, indexed read or Sub
            if place.projection.is_empty() {
                if let Rvalue::Use(op) = rvalue {
                    if let Operand::Copy(p) | Operand::Move(p) = op {
                        if p.projection.len() >= 2
                            && matches!(p.projection[0], ProjectionElem::Deref)
                            && matches!(p.projection[1], ProjectionElem::Index(idx) if same_as_i(idx))
                        {
                            reads.push((place.local, p.local));
                            continue;
                        }
                    }
                }
                if let Rvalue::BinaryOp(op, box (lhs, rhs)) = rvalue {
                    if matches!(op, BinOp::Sub | BinOp::SubUnchecked) {
                        let local_op = |o: &Operand<'tcx>| -> Option<Local> {
                            let p = match o {
                                Operand::Copy(p) | Operand::Move(p) => p,
                                _ => return None,
                            };
                            if !p.projection.is_empty() { return None; }
                            Some(p.local)
                        };
                        if let (Some(la), Some(lb)) = (local_op(lhs), local_op(rhs)) {
                            subs.push((place.local, la, lb));
                        }
                    }
                }
            }
        }
    }

    if indexed_writes != 1 { return None; }
    if writes.len() != 1 { return None; }
    let dst_local = writes.pop()?;
    let (_, value_or_inline) = write_kind.into_iter().find(|(d, _)| *d == dst_local)?;

    // Diff tag, value_local two-stmt, dst_local one-stmt
    let diff_tag = match value_or_inline {
        Some(v) => v,
        None => dst_local,
    };
    // Find the (la, lb) pair from subs.
    let mut chosen: Option<(Local, Local)> = None;
    for &(d, la, lb) in &subs {
        if d == diff_tag {
            // Unwrap each through reads
            let lookup_read = |x: Local| -> Local {
                for &(vi, src) in &reads { if vi == x { return src; } }
                x
            };
            chosen = Some((lookup_read(la), lookup_read(lb)));
            break;
        }
    }
    let (a_local_raw, b_local_raw) = chosen?;
    let a_local = resolve(a_local_raw);
    let b_local = resolve(b_local_raw);
    let dst_local = resolve(dst_local);

    // All three operands must agree f32/f64
    let dst_ty = body.local_decls[dst_local].ty;
    let a_ty = body.local_decls[a_local].ty;
    let b_ty = body.local_decls[b_local].ty;
    let dst_fty = ref_slice_float_ty(tcx, dst_ty, true)?;
    let a_fty = ref_slice_float_ty(tcx, a_ty, false)?;
    let b_fty = ref_slice_float_ty(tcx, b_ty, false)?;
    if dst_fty != a_fty || dst_fty != b_fty { return None; }
    if dst_local == a_local || dst_local == b_local { return None; }

    Some(ZipSubWritePlan {
        entry_bb: shape.entry_bb,
        exit_bb: shape.exit_bb,
        dst_local,
        a_local,
        b_local,
        fty: dst_fty,
    })
}

fn apply_zip_sub_write_transform<'tcx>(
    tcx: TyCtxt<'tcx>,
    body: &mut mir::Body<'tcx>,
    plan: &ZipSubWritePlan,
    helper_def_id: rustc_hir::def_id::DefId,
) -> bool {
    use rustc_middle::mir::*;
    use rustc_span::DUMMY_SP;
    use rustc_span::source_map::Spanned;

    let elem_ty = match plan.fty {
        FloatTy::F64 => tcx.types.f64,
    };
    #[allow(rustc::usage_of_qualified_ty)]
    let slice_ty = rustc_middle::ty::Ty::new_slice(tcx, elem_ty);
    #[allow(rustc::usage_of_qualified_ty)]
    let mut_ref_ty = rustc_middle::ty::Ty::new_mut_ref(
        tcx,
        tcx.lifetimes.re_erased,
        slice_ty,
    );

    let mut prelude_stmts: Vec<Statement<'tcx>> = Vec::new();
    let dst_temp = body.local_decls.push(LocalDecl::new(mut_ref_ty, DUMMY_SP));
    prelude_stmts.push(Statement::new(
        SourceInfo::outermost(DUMMY_SP),
        StatementKind::Assign(Box::new((
            Place::from(dst_temp),
            Rvalue::Ref(
                tcx.lifetimes.re_erased,
                BorrowKind::Mut { kind: MutBorrowKind::Default },
                Place {
                    local: plan.dst_local,
                    projection: tcx.mk_place_elems(&[ProjectionElem::Deref]),
                },
            ),
        ))),
    ));
    let unit_dest = body.local_decls.push(LocalDecl::new(tcx.types.unit, DUMMY_SP));
    let helper_callee = Operand::function_handle(tcx, helper_def_id, std::iter::empty(), DUMMY_SP);
    let new_term_kind = TerminatorKind::Call {
        func: helper_callee,
        args: Box::new([
            Spanned { node: Operand::Move(Place::from(dst_temp)), span: DUMMY_SP },
            Spanned { node: Operand::Copy(Place::from(plan.a_local)), span: DUMMY_SP },
            Spanned { node: Operand::Copy(Place::from(plan.b_local)), span: DUMMY_SP },
        ]),
        destination: Place::from(unit_dest),
        target: Some(plan.exit_bb),
        unwind: UnwindAction::Unreachable,
        call_source: CallSource::Misc,
        fn_span: DUMMY_SP,
    };
    let new_bb_data = BasicBlockData::new_stmts(
        prelude_stmts,
        Some(Terminator { source_info: SourceInfo::outermost(DUMMY_SP), kind: new_term_kind }),
        false,
    );
    let header = {
        let mut found: Option<BasicBlock> = None;
        body.basic_blocks[plan.entry_bb].terminator().successors().for_each(|s| {
            if found.is_none() { found = Some(s); }
        });
        match found { Some(h) => h, None => return false }
    };
    let new_bb_idx = body.basic_blocks_mut().push(new_bb_data);
    let entry_term = body.basic_blocks_mut()[plan.entry_bb].terminator_mut();
    entry_term.successors_mut(|succ| {
        if *succ == header { *succ = new_bb_idx; }
    });
    true
}

pub(crate) fn try_transform_slice_zip_sub_write_f64<'tcx>(
    tcx: TyCtxt<'tcx>,
    body: &mut mir::Body<'tcx>,
) -> usize {
    if !idiom_transform_enabled(tcx) { return 0; }
    if is_in_sysroot_crate(tcx, body.source.def_id()) { return 0; }
    let fn_name = tcx.def_path_str(body.source.def_id());
    if fn_name.contains("parallel_runtime") || fn_name.contains("parallel_reduce_")
        || fn_name.contains("parallel_zip_") || fn_name.contains("parallel_axpy")
        || fn_name.contains("parallel_invoke")
    { return 0; }
    // No `#[parallelize_associative_float]` opt-in needed
    if parallel_pass_quick_skip(tcx, body, 4, true) { return 0; }
    fn lookup(tcx: TyCtxt<'_>, name: &str) -> Option<rustc_hir::def_id::DefId> {
        tcx.get_diagnostic_item(rustc_span::Symbol::intern(name))
    }
    let helper_f64 = lookup(tcx, "parallel_runtime_parallel_zip_sub_write_f64");
    if helper_f64.is_none() { return 0; }
    let loops = find_simple_loops(body);
    let mut plans: Vec<ZipSubWritePlan> = Vec::new();
    for lp in &loops {
        if let Some(p) = analyze_zip_sub_write_loop(tcx, body, lp) {
            par_dump!(tcx,
                "[PAR-IDIOM-ZIP-SUB-WRITE-CANDIDATE] fn={} entry=bb{} exit=bb{} dst=_{} a=_{} b=_{} fty={:?}",
                fn_name, p.entry_bb.index(), p.exit_bb.index(),
                p.dst_local.index(), p.a_local.index(), p.b_local.index(), p.fty,
            );
            plans.push(p);
        }
    }
    let mut count = 0;
    for plan in plans {
        let helper_did = helper_f64;
        let Some(helper_did) = helper_did else { continue };
        if apply_zip_sub_write_transform(tcx, body, &plan, helper_did) {
            par_dump!(tcx, "[PAR-IDIOM-ZIP-SUB-WRITE-APPLIED] fn={} entry=bb{} fty={:?}",
                fn_name, plan.entry_bb.index(), plan.fty);
            count += 1;
        }
    }
    count
}

// zip-diff-sq-add matcher, `dst[i] += (a[i] - b[i])^2`

#[derive(Debug, Clone)]
struct ZipDiffSqAddPlan {
    entry_bb: mir::BasicBlock,
    exit_bb: mir::BasicBlock,
    dst_local: mir::Local,
    a_local: mir::Local,
    b_local: mir::Local,
    fty: FloatTy,
}

fn analyze_zip_diff_sq_add_loop<'tcx>(
    tcx: TyCtxt<'tcx>,
    body: &mir::Body<'tcx>,
    lp: &SimpleLoop,
) -> Option<ZipDiffSqAddPlan> {
    use rustc_middle::mir::*;
    let shape = detect_range_loop_shape(tcx, body, lp)?;
    let same_as_i = |l: Local| shape.same_as_i(l);
    let resolve = |l: Local| LoopShape::resolve(&shape.alias_root, l);

    let mut reads: Vec<(Local, Local)> = Vec::new();    // (vi_local, src_local)
    let mut subs: Vec<(Local, Local, Local)> = Vec::new(); // (diff_local, lhs, rhs)
    let mut sqs: Vec<(Local, Local)> = Vec::new();  // (sq_local, diff_local) - Mul(diff, diff)
    let mut writes: Vec<(Local, Local)> = Vec::new();  // (dst_local, sq_local) from `dst[i] += sq`
    let mut indexed_writes = 0usize;

    for &bb in &lp.body {
        if bb == lp.header { continue; }
        for stmt in &body.basic_blocks[bb].statements {
            let StatementKind::Assign(box (place, rvalue)) = &stmt.kind else { continue };
            // Indexed write `(*_dst)[_i] = Add(copy (*_dst)[_i], _sq)`.
            if place.projection.len() >= 2
                && matches!(place.projection[0], ProjectionElem::Deref)
                && matches!(place.projection[1], ProjectionElem::Index(idx) if same_as_i(idx))
            {
                indexed_writes += 1;
                let dst_local = place.local;
                if let Rvalue::BinaryOp(op, box (lhs, rhs)) = rvalue {
                    if !matches!(op, BinOp::Add | BinOp::AddUnchecked) { continue; }
                    let self_read = |op: &Operand<'tcx>| -> bool {
                        let p = match op {
                            Operand::Copy(p) | Operand::Move(p) => p,
                            _ => return false,
                        };
                        if p.local != dst_local { return false; }
                        if p.projection.len() < 2 { return false; }
                        matches!(p.projection[0], ProjectionElem::Deref)
                            && matches!(p.projection[1], ProjectionElem::Index(idx) if same_as_i(idx))
                    };
                    let other_local = |op: &Operand<'tcx>| -> Option<Local> {
                        let p = match op {
                            Operand::Copy(p) | Operand::Move(p) => p,
                            _ => return None,
                        };
                        if !p.projection.is_empty() { return None; }
                        Some(p.local)
                    };
                    if self_read(lhs) {
                        if let Some(m) = other_local(rhs) { writes.push((dst_local, m)); }
                    } else if self_read(rhs) {
                        if let Some(m) = other_local(lhs) { writes.push((dst_local, m)); }
                    }
                }
                continue;
            }
            // Ignore other indexed writes (non-i index)

            // Reads / Sub / Mul
            if place.projection.is_empty() {
                if let Rvalue::Use(op) = rvalue {
                    if let Operand::Copy(p) | Operand::Move(p) = op {
                        if p.projection.len() >= 2
                            && matches!(p.projection[0], ProjectionElem::Deref)
                            && matches!(p.projection[1], ProjectionElem::Index(idx) if same_as_i(idx))
                        {
                            reads.push((place.local, p.local));
                            continue;
                        }
                    }
                }
                if let Rvalue::BinaryOp(op, box (lhs, rhs)) = rvalue {
                    let local_op = |o: &Operand<'tcx>| -> Option<Local> {
                        let p = match o {
                            Operand::Copy(p) | Operand::Move(p) => p,
                            _ => return None,
                        };
                        if !p.projection.is_empty() { return None; }
                        Some(p.local)
                    };
                    if matches!(op, BinOp::Sub | BinOp::SubUnchecked) {
                        if let (Some(la), Some(lb)) = (local_op(lhs), local_op(rhs)) {
                            subs.push((place.local, la, lb));
                        }
                    } else if matches!(op, BinOp::Mul | BinOp::MulUnchecked) {
                        if let (Some(la), Some(lb)) = (local_op(lhs), local_op(rhs)) {
                            // Square, Mul with same Local both sides
                            if resolve(la) == resolve(lb) {
                                sqs.push((place.local, la));
                            }
                        }
                    }
                }
            }
        }
    }

    if indexed_writes != 1 { return None; }
    if writes.len() != 1 { return None; }
    let (dst_local, sq_local) = writes.pop()?;

    // The Mul producing sq_local
    let sq_root = resolve(sq_local);
    let mut found_sq_diff: Option<Local> = None;
    for &(s, d) in &sqs {
        if resolve(s) == sq_root { found_sq_diff = Some(resolve(d)); break; }
    }
    let diff_root = found_sq_diff?;

    // Find Sub(va_op, vb_op) producing diff_root
    let mut found_sub: Option<(Local, Local)> = None;
    for &(d, la, lb) in &subs {
        if resolve(d) == diff_root { found_sub = Some((la, lb)); break; }
    }
    let (va_local, vb_local) = found_sub?;

    // Find indexed reads producing va and vb
    let va_root = resolve(va_local);
    let vb_root = resolve(vb_local);
    let mut a_local: Option<Local> = None;
    let mut b_local: Option<Local> = None;
    for &(vi, src) in &reads {
        if resolve(vi) == va_root { a_local = Some(src); }
        if resolve(vi) == vb_root { b_local = Some(src); }
    }
    let a_local = resolve(a_local?);
    let b_local = resolve(b_local?);
    let dst_local = resolve(dst_local);

    let dst_ty = body.local_decls[dst_local].ty;
    let a_ty = body.local_decls[a_local].ty;
    let b_ty = body.local_decls[b_local].ty;
    let dst_fty = ref_slice_float_ty(tcx, dst_ty, true)?;
    let a_fty = ref_slice_float_ty(tcx, a_ty, false)?;
    let b_fty = ref_slice_float_ty(tcx, b_ty, false)?;
    if dst_fty != a_fty || dst_fty != b_fty { return None; }
    if dst_local == a_local || dst_local == b_local { return None; }

    Some(ZipDiffSqAddPlan {
        entry_bb: shape.entry_bb,
        exit_bb: shape.exit_bb,
        dst_local,
        a_local,
        b_local,
        fty: dst_fty,
    })
}

fn apply_zip_diff_sq_add_transform<'tcx>(
    tcx: TyCtxt<'tcx>,
    body: &mut mir::Body<'tcx>,
    plan: &ZipDiffSqAddPlan,
    helper_def_id: rustc_hir::def_id::DefId,
) -> bool {
    use rustc_middle::mir::*;
    use rustc_span::DUMMY_SP;
    use rustc_span::source_map::Spanned;

    let elem_ty = match plan.fty {
        FloatTy::F64 => tcx.types.f64,
    };
    #[allow(rustc::usage_of_qualified_ty)]
    let slice_ty = rustc_middle::ty::Ty::new_slice(tcx, elem_ty);
    #[allow(rustc::usage_of_qualified_ty)]
    let mut_ref_ty = rustc_middle::ty::Ty::new_mut_ref(
        tcx,
        tcx.lifetimes.re_erased,
        slice_ty,
    );

    let mut prelude_stmts: Vec<Statement<'tcx>> = Vec::new();
    let dst_temp = body.local_decls.push(LocalDecl::new(mut_ref_ty, DUMMY_SP));
    prelude_stmts.push(Statement::new(
        SourceInfo::outermost(DUMMY_SP),
        StatementKind::Assign(Box::new((
            Place::from(dst_temp),
            Rvalue::Ref(
                tcx.lifetimes.re_erased,
                BorrowKind::Mut { kind: MutBorrowKind::Default },
                Place {
                    local: plan.dst_local,
                    projection: tcx.mk_place_elems(&[ProjectionElem::Deref]),
                },
            ),
        ))),
    ));
    let unit_dest = body.local_decls.push(LocalDecl::new(tcx.types.unit, DUMMY_SP));
    let helper_callee = Operand::function_handle(tcx, helper_def_id, std::iter::empty(), DUMMY_SP);
    let new_term_kind = TerminatorKind::Call {
        func: helper_callee,
        args: Box::new([
            Spanned { node: Operand::Move(Place::from(dst_temp)), span: DUMMY_SP },
            Spanned { node: Operand::Copy(Place::from(plan.a_local)), span: DUMMY_SP },
            Spanned { node: Operand::Copy(Place::from(plan.b_local)), span: DUMMY_SP },
        ]),
        destination: Place::from(unit_dest),
        target: Some(plan.exit_bb),
        unwind: UnwindAction::Unreachable,
        call_source: CallSource::Misc,
        fn_span: DUMMY_SP,
    };
    let new_bb_data = BasicBlockData::new_stmts(
        prelude_stmts,
        Some(Terminator { source_info: SourceInfo::outermost(DUMMY_SP), kind: new_term_kind }),
        false,
    );
    let header = {
        let mut found: Option<BasicBlock> = None;
        body.basic_blocks[plan.entry_bb].terminator().successors().for_each(|s| {
            if found.is_none() { found = Some(s); }
        });
        match found { Some(h) => h, None => return false }
    };
    let new_bb_idx = body.basic_blocks_mut().push(new_bb_data);
    let entry_term = body.basic_blocks_mut()[plan.entry_bb].terminator_mut();
    entry_term.successors_mut(|succ| {
        if *succ == header { *succ = new_bb_idx; }
    });
    true
}

pub(crate) fn try_transform_slice_zip_diff_sq_add_f64<'tcx>(
    tcx: TyCtxt<'tcx>,
    body: &mut mir::Body<'tcx>,
) -> usize {
    if !idiom_transform_enabled(tcx) { return 0; }
    if is_in_sysroot_crate(tcx, body.source.def_id()) { return 0; }
    let fn_name = tcx.def_path_str(body.source.def_id());
    if fn_name.contains("parallel_runtime") || fn_name.contains("parallel_reduce_")
        || fn_name.contains("parallel_zip_") || fn_name.contains("parallel_axpy")
        || fn_name.contains("parallel_invoke")
    { return 0; }
    // No `#[parallelize_associative_float]` opt-in needed
    if parallel_pass_quick_skip(tcx, body, 4, true) { return 0; }
    fn lookup(tcx: TyCtxt<'_>, name: &str) -> Option<rustc_hir::def_id::DefId> {
        tcx.get_diagnostic_item(rustc_span::Symbol::intern(name))
    }
    let helper_f64 = lookup(tcx, "parallel_runtime_parallel_zip_diff_sq_add_f64");
    if helper_f64.is_none() { return 0; }
    let loops = find_simple_loops(body);
    let mut plans: Vec<ZipDiffSqAddPlan> = Vec::new();
    for lp in &loops {
        if let Some(p) = analyze_zip_diff_sq_add_loop(tcx, body, lp) {
            par_dump!(tcx,
                "[PAR-IDIOM-ZIP-DIFFSQADD-CANDIDATE] fn={} entry=bb{} exit=bb{} dst=_{} a=_{} b=_{} fty={:?}",
                fn_name, p.entry_bb.index(), p.exit_bb.index(),
                p.dst_local.index(), p.a_local.index(), p.b_local.index(), p.fty,
            );
            plans.push(p);
        }
    }
    let mut count = 0;
    for plan in plans {
        let helper_did = helper_f64;
        let Some(helper_did) = helper_did else { continue };
        if apply_zip_diff_sq_add_transform(tcx, body, &plan, helper_did) {
            par_dump!(tcx, "[PAR-IDIOM-ZIP-DIFFSQADD-APPLIED] fn={} entry=bb{} fty={:?}",
                fn_name, plan.entry_bb.index(), plan.fty);
            count += 1;
        }
    }
    count
}

// Unified element-wise write matcher

#[derive(Debug, Clone)]
enum ElemwiseShape {
    /// `dst[i] = <const float>` - compile-time constant
    FillConst { c: f64 },
    /// `dst[i] = <runtime float Local>`
    FillRuntime { src: mir::Local },
    /// `dst[i] = src[i]` - copy_from_slice
    Copy { src: mir::Local },
    /// `dst[i] = a[i] + b[i]`
    ZipAdd { a: mir::Local, b: mir::Local },
    /// `dst[i] = a[i] * b[i]`
    ZipMul { a: mir::Local, b: mir::Local },
    /// `dst[i] = a[i] / b[i]`
    ZipDiv { a: mir::Local, b: mir::Local },
}

#[derive(Debug, Clone)]
struct ElemwisePlan {
    entry_bb: mir::BasicBlock,
    exit_bb: mir::BasicBlock,
    dst_local: mir::Local,
    shape: ElemwiseShape,
    fty: FloatTy,
}

fn analyze_elemwise_write_loop<'tcx>(
    tcx: TyCtxt<'tcx>,
    body: &mir::Body<'tcx>,
    lp: &SimpleLoop,
) -> Option<ElemwisePlan> {
    use rustc_middle::mir::*;
    let shape = detect_range_loop_shape(tcx, body, lp)?;
    let same_as_i = |l: Local| shape.same_as_i(l);
    let resolve = |l: Local| LoopShape::resolve(&shape.alias_root, l);

    // Find one indexed write, collect indexed reads
    let mut reads: Vec<(Local, Local)> = Vec::new();
    let mut indexed_writes: usize = 0;
    let mut found: Option<(Local /*dst*/, Rvalue<'tcx> /*the rvalue*/)> = None;

    for &bb in &lp.body {
        if bb == lp.header { continue; }
        for stmt in &body.basic_blocks[bb].statements {
            let StatementKind::Assign(box (place, rvalue)) = &stmt.kind else { continue };
            // Indexed read tracking
            if place.projection.is_empty() {
                if let Rvalue::Use(op) = rvalue {
                    if let Operand::Copy(p) | Operand::Move(p) = op {
                        if p.projection.len() >= 2
                            && matches!(p.projection[0], ProjectionElem::Deref)
                            && matches!(p.projection[1], ProjectionElem::Index(idx) if same_as_i(idx))
                        {
                            reads.push((place.local, p.local));
                        }
                    }
                }
            }
            // Indexed write
            if place.projection.len() >= 2
                && matches!(place.projection[0], ProjectionElem::Deref)
                && matches!(place.projection[1], ProjectionElem::Index(idx) if same_as_i(idx))
            {
                indexed_writes += 1;
                found = Some((place.local, rvalue.clone()));
            }
        }
    }
    if indexed_writes != 1 { return None; }
    let (dst_local, write_rvalue) = found?;

    // Resolve Operand to Constant f64 or Local
    let local_of = |o: &Operand<'tcx>| -> Option<Local> {
        let p = match o {
            Operand::Copy(p) | Operand::Move(p) => p,
            _ => return None,
        };
        if !p.projection.is_empty() { return None; }
        Some(p.local)
    };
    // Map indexed-read result Local to source slice
    let lookup_read_src = |l: Local| -> Option<Local> {
        for &(vi, src) in &reads {
            if vi == l || resolve(vi) == resolve(l) { return Some(resolve(src)); }
        }
        None
    };
    // Operand may itself be indexed-read place
    let direct_indexed_src = |o: &Operand<'tcx>| -> Option<Local> {
        let p = match o {
            Operand::Copy(p) | Operand::Move(p) => p,
            _ => return None,
        };
        if p.projection.len() >= 2
            && matches!(p.projection[0], ProjectionElem::Deref)
            && matches!(p.projection[1], ProjectionElem::Index(idx) if same_as_i(idx))
        {
            return Some(resolve(p.local));
        }
        None
    };

    // Get dst float type for later checks
    let dst_resolved = resolve(dst_local);
    let dst_ty = body.local_decls[dst_resolved].ty;
    let fty = ref_slice_float_ty(tcx, dst_ty, /*require_mut=*/true)?;
    let expected_elem_ty = match fty {
        FloatTy::F64 => tcx.types.f64,
    };

    // Pattern-match on the rvalue:
    let plan_shape = match &write_rvalue {
        // FILL with compile-time constant: `dst[i] = <Constant>`.
        Rvalue::Use(op) if matches!(op, Operand::Constant(_)) => {
            // Const bits, valid for f32 and f64
            let Operand::Constant(c) = op else { return None };
            let bits = eval_const_bits(tcx, &c.const_)?;
            let c_val = {
                let _ = fty;
                if bits > u64::MAX as u128 { return None; }
                f64::from_bits(bits as u64)
            };
            ElemwiseShape::FillConst { c: c_val }
        }
        // Use of a Local
        Rvalue::Use(op) => {
            // Try indexed-read source first.
            if let Some(src) = direct_indexed_src(op) {
                ElemwiseShape::Copy { src }
            } else if let Some(src_local) = local_of(op).and_then(lookup_read_src) {
                ElemwiseShape::Copy { src: src_local }
            } else if let Some(scalar_local) = local_of(op) {
                let resolved = resolve(scalar_local);
                if body.local_decls[resolved].ty != expected_elem_ty {
                    return None;
                }
                // Entry-live check, reject if assigned in loop
                let mut assigned_in_loop = false;
                for &bb in &lp.body {
                    let bb_data = &body.basic_blocks[bb];
                    for stmt in &bb_data.statements {
                        if let StatementKind::Assign(box (place, _)) = &stmt.kind {
                            if place.projection.is_empty() && place.local == resolved {
                                assigned_in_loop = true; break;
                            }
                        }
                    }
                    if assigned_in_loop { break; }
                    if let TerminatorKind::Call { destination, .. } = &bb_data.terminator().kind {
                        if destination.projection.is_empty() && destination.local == resolved {
                            assigned_in_loop = true; break;
                        }
                    }
                }
                if assigned_in_loop {
                    // Also check via aliases
                    return None;
                }
                ElemwiseShape::FillRuntime { src: resolved }
            } else {
                return None;
            }
        }
        // Zip add/mul/div write of two indexed reads
        Rvalue::BinaryOp(op, box (lhs, rhs)) => {
            let lhs_src = direct_indexed_src(lhs).or_else(|| local_of(lhs).and_then(lookup_read_src))?;
            let rhs_src = direct_indexed_src(rhs).or_else(|| local_of(rhs).and_then(lookup_read_src))?;
            // Reject self-modify (would be AXPY's domain).
            if lhs_src == dst_local || rhs_src == dst_local { return None; }
            match op {
                BinOp::Add | BinOp::AddUnchecked => {
                    ElemwiseShape::ZipAdd { a: lhs_src, b: rhs_src }
                }
                BinOp::Mul | BinOp::MulUnchecked => {
                    ElemwiseShape::ZipMul { a: lhs_src, b: rhs_src }
                }
                BinOp::Div => {
                    ElemwiseShape::ZipDiv { a: lhs_src, b: rhs_src }
                }
                _ => return None,
            }
        }
        _ => return None,
    };

    // Type checks on source slices
    match &plan_shape {
        ElemwiseShape::FillConst { .. } | ElemwiseShape::FillRuntime { .. } => {}
        ElemwiseShape::Copy { src } => {
            let st = body.local_decls[*src].ty;
            if ref_slice_float_ty(tcx, st, false) != Some(fty) { return None; }
            if dst_resolved == *src { return None; }
        }
        ElemwiseShape::ZipAdd { a, b }
        | ElemwiseShape::ZipMul { a, b }
        | ElemwiseShape::ZipDiv { a, b } => {
            let at = body.local_decls[*a].ty;
            let bt = body.local_decls[*b].ty;
            if ref_slice_float_ty(tcx, at, false) != Some(fty) { return None; }
            if ref_slice_float_ty(tcx, bt, false) != Some(fty) { return None; }
            if dst_resolved == *a || dst_resolved == *b { return None; }
        }
    }

    Some(ElemwisePlan {
        entry_bb: shape.entry_bb,
        exit_bb: shape.exit_bb,
        dst_local: dst_resolved,
        shape: plan_shape,
        fty,
    })
}

fn apply_elemwise_write_transform<'tcx>(
    tcx: TyCtxt<'tcx>,
    body: &mut mir::Body<'tcx>,
    plan: &ElemwisePlan,
    fill_did: Option<rustc_hir::def_id::DefId>,
    copy_did: Option<rustc_hir::def_id::DefId>,
    add_did: Option<rustc_hir::def_id::DefId>,
    mul_did: Option<rustc_hir::def_id::DefId>,
    div_did: Option<rustc_hir::def_id::DefId>,
) -> bool {
    use rustc_middle::mir::*;
    use rustc_span::DUMMY_SP;
    use rustc_span::source_map::Spanned;

    let elem_ty = match plan.fty {
        FloatTy::F64 => tcx.types.f64,
    };
    #[allow(rustc::usage_of_qualified_ty)]
    let slice_ty = rustc_middle::ty::Ty::new_slice(tcx, elem_ty);
    #[allow(rustc::usage_of_qualified_ty)]
    let mut_slice_ref_ty = rustc_middle::ty::Ty::new_mut_ref(
        tcx, tcx.lifetimes.re_erased, slice_ty);

    let mut prelude_stmts: Vec<Statement<'tcx>> = Vec::new();
    let dst_temp = body.local_decls.push(LocalDecl::new(mut_slice_ref_ty, DUMMY_SP));
    prelude_stmts.push(Statement::new(
        SourceInfo::outermost(DUMMY_SP),
        StatementKind::Assign(Box::new((
            Place::from(dst_temp),
            Rvalue::Ref(
                tcx.lifetimes.re_erased,
                BorrowKind::Mut { kind: MutBorrowKind::Default },
                Place {
                    local: plan.dst_local,
                    projection: tcx.mk_place_elems(&[ProjectionElem::Deref]),
                },
            ),
        ))),
    ));

    // Per-shape helper args and DefId
    let (helper_did, helper_args): (rustc_hir::def_id::DefId, Box<[Spanned<Operand<'tcx>>]>) =
        match &plan.shape {
            ElemwiseShape::FillConst { c } => {
                let did = match fill_did { Some(d) => d, None => return false };
                // Build Constant operand sized by float type
                let (bits_u128, size_bytes) = match plan.fty {
                    FloatTy::F64 => (c.to_bits() as u128, 8u64),
                };
                let scalar_int = rustc_middle::ty::ScalarInt::try_from_uint(
                    bits_u128,
                    rustc_abi::Size::from_bytes(size_bytes),
                ).expect("float bits fit in u128");
                let const_op = Operand::Constant(Box::new(ConstOperand {
                    span: DUMMY_SP,
                    user_ty: None,
                    const_: rustc_middle::mir::Const::Val(
                        rustc_middle::mir::ConstValue::Scalar(
                            rustc_middle::mir::interpret::Scalar::Int(scalar_int),
                        ),
                        elem_ty,
                    ),
                }));
                let args: Box<[_]> = Box::new([
                    Spanned { node: Operand::Move(Place::from(dst_temp)), span: DUMMY_SP },
                    Spanned { node: const_op, span: DUMMY_SP },
                ]);
                (did, args)
            }
            ElemwiseShape::FillRuntime { src } => {
                let did = match fill_did { Some(d) => d, None => return false };
                let args: Box<[_]> = Box::new([
                    Spanned { node: Operand::Move(Place::from(dst_temp)), span: DUMMY_SP },
                    Spanned { node: Operand::Copy(Place::from(*src)), span: DUMMY_SP },
                ]);
                (did, args)
            }
            ElemwiseShape::Copy { src } => {
                let did = match copy_did { Some(d) => d, None => return false };
                let args: Box<[_]> = Box::new([
                    Spanned { node: Operand::Move(Place::from(dst_temp)), span: DUMMY_SP },
                    Spanned { node: Operand::Copy(Place::from(*src)), span: DUMMY_SP },
                ]);
                (did, args)
            }
            ElemwiseShape::ZipAdd { a, b } => {
                let did = match add_did { Some(d) => d, None => return false };
                let args: Box<[_]> = Box::new([
                    Spanned { node: Operand::Move(Place::from(dst_temp)), span: DUMMY_SP },
                    Spanned { node: Operand::Copy(Place::from(*a)), span: DUMMY_SP },
                    Spanned { node: Operand::Copy(Place::from(*b)), span: DUMMY_SP },
                ]);
                (did, args)
            }
            ElemwiseShape::ZipMul { a, b } => {
                let did = match mul_did { Some(d) => d, None => return false };
                let args: Box<[_]> = Box::new([
                    Spanned { node: Operand::Move(Place::from(dst_temp)), span: DUMMY_SP },
                    Spanned { node: Operand::Copy(Place::from(*a)), span: DUMMY_SP },
                    Spanned { node: Operand::Copy(Place::from(*b)), span: DUMMY_SP },
                ]);
                (did, args)
            }
            ElemwiseShape::ZipDiv { a, b } => {
                let did = match div_did { Some(d) => d, None => return false };
                let args: Box<[_]> = Box::new([
                    Spanned { node: Operand::Move(Place::from(dst_temp)), span: DUMMY_SP },
                    Spanned { node: Operand::Copy(Place::from(*a)), span: DUMMY_SP },
                    Spanned { node: Operand::Copy(Place::from(*b)), span: DUMMY_SP },
                ]);
                (did, args)
            }
        };

    let unit_dest = body.local_decls.push(LocalDecl::new(tcx.types.unit, DUMMY_SP));
    let helper_callee = Operand::function_handle(tcx, helper_did, std::iter::empty(), DUMMY_SP);
    let new_term_kind = TerminatorKind::Call {
        func: helper_callee,
        args: helper_args,
        destination: Place::from(unit_dest),
        target: Some(plan.exit_bb),
        unwind: UnwindAction::Unreachable,
        call_source: CallSource::Misc,
        fn_span: DUMMY_SP,
    };
    let new_bb_data = BasicBlockData::new_stmts(
        prelude_stmts,
        Some(Terminator { source_info: SourceInfo::outermost(DUMMY_SP), kind: new_term_kind }),
        false,
    );
    let header = {
        let mut found: Option<BasicBlock> = None;
        body.basic_blocks[plan.entry_bb].terminator().successors().for_each(|s| {
            if found.is_none() { found = Some(s); }
        });
        match found { Some(h) => h, None => return false }
    };
    let new_bb_idx = body.basic_blocks_mut().push(new_bb_data);
    let entry_term = body.basic_blocks_mut()[plan.entry_bb].terminator_mut();
    entry_term.successors_mut(|succ| {
        if *succ == header { *succ = new_bb_idx; }
    });
    true
}

pub(crate) fn try_transform_slice_elemwise_write_f64<'tcx>(
    tcx: TyCtxt<'tcx>,
    body: &mut mir::Body<'tcx>,
) -> usize {
    if !idiom_transform_enabled(tcx) { return 0; }
    if is_in_sysroot_crate(tcx, body.source.def_id()) { return 0; }
    let fn_name = tcx.def_path_str(body.source.def_id());
    if fn_name.contains("parallel_runtime") || fn_name.contains("parallel_reduce_")
        || fn_name.contains("parallel_zip_") || fn_name.contains("parallel_axpy")
        || fn_name.contains("parallel_fill") || fn_name.contains("parallel_copy_from_slice")
        || fn_name.contains("parallel_invoke") || fn_name.contains("parallel_scale")
    { return 0; }
    if parallel_pass_quick_skip(tcx, body, 4, true) { return 0; }
    fn lookup(tcx: TyCtxt<'_>, name: &str) -> Option<rustc_hir::def_id::DefId> {
        tcx.get_diagnostic_item(rustc_span::Symbol::intern(name))
    }
    let fill_f64 = lookup(tcx, "parallel_runtime_parallel_fill_f64");
    let copy_f64 = lookup(tcx, "parallel_runtime_parallel_copy_from_slice_f64");
    let add_f64 = lookup(tcx, "parallel_runtime_parallel_zip_add_write_f64");
    let mul_f64 = lookup(tcx, "parallel_runtime_parallel_zip_mul_write_f64");
    let div_f64 = lookup(tcx, "parallel_runtime_parallel_zip_div_write_f64");

    let loops = find_simple_loops(body);
    let mut plans: Vec<ElemwisePlan> = Vec::new();
    for lp in &loops {
        if let Some(p) = analyze_elemwise_write_loop(tcx, body, lp) {
            par_dump!(tcx,
                "[PAR-IDIOM-ELEMWISE-WRITE-CANDIDATE] fn={} entry=bb{} exit=bb{} dst=_{} shape={:?} fty={:?}",
                fn_name, p.entry_bb.index(), p.exit_bb.index(), p.dst_local.index(), p.shape, p.fty,
            );
            plans.push(p);
        }
    }
    let mut count = 0;
    for plan in plans {
        let (fill_did, copy_did, add_did, mul_did, div_did) = match plan.fty {
            FloatTy::F64 => (fill_f64, copy_f64, add_f64, mul_f64, div_f64),
        };
        if apply_elemwise_write_transform(tcx, body, &plan, fill_did, copy_did, add_did, mul_did, div_did) {
            par_dump!(tcx, "[PAR-IDIOM-ELEMWISE-WRITE-APPLIED] fn={} entry=bb{} shape={:?} fty={:?}",
                fn_name, plan.entry_bb.index(), plan.shape, plan.fty);
            count += 1;
        }
    }
    count
}

// Unary-apply matcher, `dst[i] = src[i].METHOD()`

#[derive(Debug, Clone)]
struct UnaryApplyPlan {
    entry_bb: mir::BasicBlock,
    exit_bb: mir::BasicBlock,
    dst_local: mir::Local,
    src_local: mir::Local,
    method: &'static str,
    fty: FloatTy,
}

/// Method name plus FloatTy to diagnostic-item name
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FloatTy { F64 }

fn unary_helper_name(method: &str, fty: FloatTy) -> Option<&'static str> {
    Some(match (method, fty) {
        ("abs",   FloatTy::F64) => "parallel_runtime_parallel_apply_abs_f64",
        ("sqrt",  FloatTy::F64) => "parallel_runtime_parallel_apply_sqrt_f64",
        ("exp",   FloatTy::F64) => "parallel_runtime_parallel_apply_exp_f64",
        ("ln",    FloatTy::F64) => "parallel_runtime_parallel_apply_ln_f64",
        ("tanh",  FloatTy::F64) => "parallel_runtime_parallel_apply_tanh_f64",
        ("sin",   FloatTy::F64) => "parallel_runtime_parallel_apply_sin_f64",
        ("cos",   FloatTy::F64) => "parallel_runtime_parallel_apply_cos_f64",
        ("recip", FloatTy::F64) => "parallel_runtime_parallel_apply_recip_f64",
        ("floor", FloatTy::F64) => "parallel_runtime_parallel_apply_floor_f64",
        ("ceil",  FloatTy::F64) => "parallel_runtime_parallel_apply_ceil_f64",
        ("round", FloatTy::F64) => "parallel_runtime_parallel_apply_round_f64",
        _ => return None,
    })
}

/// True iff `ty` is `&[T]`
#[allow(rustc::usage_of_qualified_ty)]
fn ref_slice_float_ty<'tcx>(
    tcx: TyCtxt<'tcx>,
    ty: rustc_middle::ty::Ty<'tcx>,
    require_mut: bool,
) -> Option<FloatTy> {
    let rustc_middle::ty::Ref(_, inner, mutbl) = ty.kind() else { return None; };
    let rustc_middle::ty::Slice(elem) = inner.kind() else { return None; };
    if require_mut && *mutbl != rustc_ast::Mutability::Mut { return None; }
    if *elem == tcx.types.f64 { Some(FloatTy::F64) }
    else { None }
}

fn analyze_unary_apply_loop<'tcx>(
    tcx: TyCtxt<'tcx>,
    body: &mir::Body<'tcx>,
    lp: &SimpleLoop,
) -> Option<UnaryApplyPlan> {
    use rustc_middle::mir::*;
    let shape = detect_range_loop_shape(tcx, body, lp)?;
    let same_as_i = |l: Local| shape.same_as_i(l);
    let resolve = |l: Local| LoopShape::resolve(&shape.alias_root, l);

    // Indexed write of _result, then producing Call
    let mut indexed_writes = 0usize;
    let mut chosen: Option<(Local /*dst*/, Local /*result*/)> = None;
    for &bb in &lp.body {
        if bb == lp.header { continue; }
        for stmt in &body.basic_blocks[bb].statements {
            let StatementKind::Assign(box (place, rvalue)) = &stmt.kind else { continue };
            if !(place.projection.len() >= 2
                && matches!(place.projection[0], ProjectionElem::Deref)
                && matches!(place.projection[1], ProjectionElem::Index(idx) if same_as_i(idx)))
            {
                continue;
            }
            indexed_writes += 1;
            if let Rvalue::Use(op) = rvalue {
                if let Operand::Copy(p) | Operand::Move(p) = op {
                    if p.projection.is_empty() {
                        chosen = Some((place.local, p.local));
                    }
                }
            }
        }
    }
    if indexed_writes != 1 { return None; }
    let (dst_local, result_local) = chosen?;

    // Find the Call terminator producing result_local
    let mut method_name: Option<String> = None;
    let mut src_local: Option<Local> = None;
    for &bb in &lp.body {
        if bb == lp.header { continue; }
        let block = &body.basic_blocks[bb];
        let TerminatorKind::Call { func, args, destination, .. } = &block.terminator().kind
        else { continue };
        if destination.projection.is_empty()
            && resolve(destination.local) == resolve(result_local)
        {
            // Callee must be known unary float method
            let Operand::Constant(c) = func else { return None };
            let rustc_middle::ty::FnDef(callee_did, _) = c.const_.ty().kind() else { return None };
            let path = tcx.def_path_str(*callee_did);
            let last = path.rsplit("::").next().unwrap_or("");
            // Known iff f64 helper exists; f32 mirrors
            unary_helper_name(last, FloatTy::F64)?;
            method_name = Some(last.to_string());

            // Verify single arg.
            if args.len() != 1 { return None; }
            // Arg is direct indexed read or alias
            let arg_local = match &args[0].node {
                Operand::Copy(p) | Operand::Move(p) => {
                    if p.projection.len() >= 2
                        && matches!(p.projection[0], ProjectionElem::Deref)
                        && matches!(p.projection[1], ProjectionElem::Index(idx) if same_as_i(idx))
                    {
                        // Direct: arg IS (*_src)[_i].
                        src_local = Some(resolve(p.local));
                        break;
                    }
                    if !p.projection.is_empty() { return None; }
                    p.local
                }
                _ => return None,
            };
            // Arg is Local, find upstream indexed-read source
            for &abb in &lp.body {
                for stmt in &body.basic_blocks[abb].statements {
                    let StatementKind::Assign(box (place, rvalue)) = &stmt.kind else { continue };
                    if !place.projection.is_empty() || resolve(place.local) != resolve(arg_local) {
                        continue;
                    }
                    if let Rvalue::Use(op) = rvalue {
                        if let Operand::Copy(p) | Operand::Move(p) = op {
                            if p.projection.len() >= 2
                                && matches!(p.projection[0], ProjectionElem::Deref)
                                && matches!(p.projection[1], ProjectionElem::Index(idx) if same_as_i(idx))
                            {
                                src_local = Some(resolve(p.local));
                            }
                        }
                    }
                }
            }
            break;
        }
    }
    let method_name = method_name?;
    let src_local = src_local?;

    // dst, src must share one float type
    let dst_resolved = resolve(dst_local);
    let dst_ty = body.local_decls[dst_resolved].ty;
    let src_ty = body.local_decls[src_local].ty;
    let dst_fty = ref_slice_float_ty(tcx, dst_ty, true)?;
    let src_fty = ref_slice_float_ty(tcx, src_ty, false)?;
    if dst_fty != src_fty { return None; }
    if dst_resolved == src_local { return None; }

    let method_static: &'static str = match method_name.as_str() {
        "abs" => "abs", "sqrt" => "sqrt", "exp" => "exp", "ln" => "ln",
        "tanh" => "tanh", "sin" => "sin", "cos" => "cos", "recip" => "recip",
        "floor" => "floor", "ceil" => "ceil", "round" => "round",
        _ => return None,
    };

    Some(UnaryApplyPlan {
        entry_bb: shape.entry_bb,
        exit_bb: shape.exit_bb,
        dst_local: dst_resolved,
        src_local,
        method: method_static,
        fty: dst_fty,
    })
}

fn apply_unary_apply_transform<'tcx>(
    tcx: TyCtxt<'tcx>,
    body: &mut mir::Body<'tcx>,
    plan: &UnaryApplyPlan,
    helper_def_id: rustc_hir::def_id::DefId,
) -> bool {
    use rustc_middle::mir::*;
    use rustc_span::DUMMY_SP;
    use rustc_span::source_map::Spanned;

    let elem_ty = match plan.fty {
        FloatTy::F64 => tcx.types.f64,
    };
    #[allow(rustc::usage_of_qualified_ty)]
    let slice_ty = rustc_middle::ty::Ty::new_slice(tcx, elem_ty);
    #[allow(rustc::usage_of_qualified_ty)]
    let mut_slice_ref_ty = rustc_middle::ty::Ty::new_mut_ref(
        tcx, tcx.lifetimes.re_erased, slice_ty);
    let dst_temp = body.local_decls.push(LocalDecl::new(mut_slice_ref_ty, DUMMY_SP));
    let prelude = vec![Statement::new(
        SourceInfo::outermost(DUMMY_SP),
        StatementKind::Assign(Box::new((
            Place::from(dst_temp),
            Rvalue::Ref(
                tcx.lifetimes.re_erased,
                BorrowKind::Mut { kind: MutBorrowKind::Default },
                Place {
                    local: plan.dst_local,
                    projection: tcx.mk_place_elems(&[ProjectionElem::Deref]),
                },
            ),
        ))),
    )];
    let unit_dest = body.local_decls.push(LocalDecl::new(tcx.types.unit, DUMMY_SP));
    let helper_callee = Operand::function_handle(tcx, helper_def_id, std::iter::empty(), DUMMY_SP);
    let new_term_kind = TerminatorKind::Call {
        func: helper_callee,
        args: Box::new([
            Spanned { node: Operand::Move(Place::from(dst_temp)), span: DUMMY_SP },
            Spanned { node: Operand::Copy(Place::from(plan.src_local)), span: DUMMY_SP },
        ]),
        destination: Place::from(unit_dest),
        target: Some(plan.exit_bb),
        unwind: UnwindAction::Unreachable,
        call_source: CallSource::Misc,
        fn_span: DUMMY_SP,
    };
    let new_bb_data = BasicBlockData::new_stmts(
        prelude,
        Some(Terminator { source_info: SourceInfo::outermost(DUMMY_SP), kind: new_term_kind }),
        false,
    );
    let header = {
        let mut found: Option<BasicBlock> = None;
        body.basic_blocks[plan.entry_bb].terminator().successors().for_each(|s| {
            if found.is_none() { found = Some(s); }
        });
        match found { Some(h) => h, None => return false }
    };
    let new_bb_idx = body.basic_blocks_mut().push(new_bb_data);
    let entry_term = body.basic_blocks_mut()[plan.entry_bb].terminator_mut();
    entry_term.successors_mut(|succ| {
        if *succ == header { *succ = new_bb_idx; }
    });
    true
}

pub(crate) fn try_transform_slice_unary_apply_f64<'tcx>(
    tcx: TyCtxt<'tcx>,
    body: &mut mir::Body<'tcx>,
) -> usize {
    if !idiom_transform_enabled(tcx) { return 0; }
    if is_in_sysroot_crate(tcx, body.source.def_id()) { return 0; }
    let fn_name = tcx.def_path_str(body.source.def_id());
    if fn_name.contains("parallel_runtime") || fn_name.contains("parallel_apply_")
        || fn_name.contains("parallel_invoke")
    { return 0; }
    if parallel_pass_quick_skip(tcx, body, 4, true) { return 0; }
    fn lookup(tcx: TyCtxt<'_>, name: &str) -> Option<rustc_hir::def_id::DefId> {
        tcx.get_diagnostic_item(rustc_span::Symbol::intern(name))
    }
    let loops = find_simple_loops(body);
    let mut plans: Vec<UnaryApplyPlan> = Vec::new();
    for lp in &loops {
        if let Some(p) = analyze_unary_apply_loop(tcx, body, lp) {
            par_dump!(tcx,
                "[PAR-IDIOM-UNARY-APPLY-CANDIDATE] fn={} entry=bb{} exit=bb{} dst=_{} src=_{} method={}",
                fn_name, p.entry_bb.index(), p.exit_bb.index(),
                p.dst_local.index(), p.src_local.index(), p.method,
            );
            plans.push(p);
        }
    }
    let mut count = 0;
    for plan in plans {
        let helper_name = unary_helper_name(plan.method, plan.fty).unwrap();
        let Some(helper_def_id) = lookup(tcx, helper_name) else { continue };
        if apply_unary_apply_transform(tcx, body, &plan, helper_def_id) {
            par_dump!(tcx, "[PAR-IDIOM-UNARY-APPLY-APPLIED] fn={} entry=bb{} method={}",
                fn_name, plan.entry_bb.index(), plan.method);
            count += 1;
        }
    }
    count
}

// Binary-apply matcher, `dst[i] = a[i].METHOD(b[i])`

#[derive(Debug, Clone)]
struct BinaryApplyPlan {
    entry_bb: mir::BasicBlock,
    exit_bb: mir::BasicBlock,
    dst_local: mir::Local,
    a_local: mir::Local,
    b_local: mir::Local,
    method: &'static str,
    fty: FloatTy,
}

fn binary_helper_name(method: &str, fty: FloatTy) -> Option<&'static str> {
    Some(match (method, fty) {
        ("max",      FloatTy::F64) => "parallel_runtime_parallel_apply_max_f64",
        ("min",      FloatTy::F64) => "parallel_runtime_parallel_apply_min_f64",
        ("copysign", FloatTy::F64) => "parallel_runtime_parallel_apply_copysign_f64",
        ("hypot",    FloatTy::F64) => "parallel_runtime_parallel_apply_hypot_f64",
        ("atan2",    FloatTy::F64) => "parallel_runtime_parallel_apply_atan2_f64",
        ("powf",     FloatTy::F64) => "parallel_runtime_parallel_apply_powf_f64",
        _ => return None,
    })
}

fn analyze_binary_apply_loop<'tcx>(
    tcx: TyCtxt<'tcx>,
    body: &mir::Body<'tcx>,
    lp: &SimpleLoop,
) -> Option<BinaryApplyPlan> {
    use rustc_middle::mir::*;
    let shape = detect_range_loop_shape(tcx, body, lp)?;
    let same_as_i = |l: Local| shape.same_as_i(l);
    let resolve = |l: Local| LoopShape::resolve(&shape.alias_root, l);

    // Find indexed write `(*_dst)[_i] = move _result`.
    let mut indexed_writes = 0usize;
    let mut chosen: Option<(Local, Local)> = None;
    for &bb in &lp.body {
        if bb == lp.header { continue; }
        for stmt in &body.basic_blocks[bb].statements {
            let StatementKind::Assign(box (place, rvalue)) = &stmt.kind else { continue };
            if !(place.projection.len() >= 2
                && matches!(place.projection[0], ProjectionElem::Deref)
                && matches!(place.projection[1], ProjectionElem::Index(idx) if same_as_i(idx)))
            {
                continue;
            }
            indexed_writes += 1;
            if let Rvalue::Use(op) = rvalue {
                if let Operand::Copy(p) | Operand::Move(p) = op {
                    if p.projection.is_empty() {
                        chosen = Some((place.local, p.local));
                    }
                }
            }
        }
    }
    if indexed_writes != 1 { return None; }
    let (dst_local, result_local) = chosen?;

    // Find the Call producing result_local
    let mut method_static: Option<&'static str> = None;
    let mut a_local: Option<Local> = None;
    let mut b_local: Option<Local> = None;

    let resolve_arg_to_indexed_src = |arg: &Operand<'tcx>| -> Option<Local> {
        let p = match arg {
            Operand::Copy(p) | Operand::Move(p) => p,
            _ => return None,
        };
        // Direct: arg IS (*_x)[_i].
        if p.projection.len() >= 2
            && matches!(p.projection[0], ProjectionElem::Deref)
            && matches!(p.projection[1], ProjectionElem::Index(idx) if same_as_i(idx))
        {
            return Some(resolve(p.local));
        }
        // Indirect arg Local, find upstream indexed read
        if !p.projection.is_empty() { return None; }
        let arg_local = p.local;
        for &abb in &lp.body {
            for stmt in &body.basic_blocks[abb].statements {
                let StatementKind::Assign(box (place, rvalue)) = &stmt.kind else { continue };
                if !place.projection.is_empty() || resolve(place.local) != resolve(arg_local) {
                    continue;
                }
                if let Rvalue::Use(op) = rvalue {
                    if let Operand::Copy(p) | Operand::Move(p) = op {
                        if p.projection.len() >= 2
                            && matches!(p.projection[0], ProjectionElem::Deref)
                            && matches!(p.projection[1], ProjectionElem::Index(idx) if same_as_i(idx))
                        {
                            return Some(resolve(p.local));
                        }
                    }
                }
            }
        }
        None
    };

    for &bb in &lp.body {
        if bb == lp.header { continue; }
        let block = &body.basic_blocks[bb];
        let TerminatorKind::Call { func, args, destination, .. } = &block.terminator().kind
        else { continue };
        if !(destination.projection.is_empty()
            && resolve(destination.local) == resolve(result_local))
        { continue; }
        let Operand::Constant(c) = func else { return None };
        let rustc_middle::ty::FnDef(callee_did, _) = c.const_.ty().kind() else { return None };
        let path = tcx.def_path_str(*callee_did);
        let last = path.rsplit("::").next().unwrap_or("");
        let method = match last {
            "max" => "max", "min" => "min", "copysign" => "copysign",
            "hypot" => "hypot", "atan2" => "atan2", "powf" => "powf",
            _ => return None,
        };
        // Method known iff f64 variant exists
        binary_helper_name(method, FloatTy::F64)?;
        method_static = Some(method);
        if args.len() != 2 { return None; }
        a_local = resolve_arg_to_indexed_src(&args[0].node);
        b_local = resolve_arg_to_indexed_src(&args[1].node);
        break;
    }
    let method = method_static?;
    let a_local = a_local?;
    let b_local = b_local?;

    let dst_resolved = resolve(dst_local);
    let dst_ty = body.local_decls[dst_resolved].ty;
    let a_ty = body.local_decls[a_local].ty;
    let b_ty = body.local_decls[b_local].ty;
    let dst_fty = ref_slice_float_ty(tcx, dst_ty, true)?;
    let a_fty = ref_slice_float_ty(tcx, a_ty, false)?;
    let b_fty = ref_slice_float_ty(tcx, b_ty, false)?;
    if dst_fty != a_fty || dst_fty != b_fty { return None; }
    if dst_resolved == a_local || dst_resolved == b_local { return None; }

    Some(BinaryApplyPlan {
        entry_bb: shape.entry_bb,
        exit_bb: shape.exit_bb,
        dst_local: dst_resolved,
        a_local,
        b_local,
        method,
        fty: dst_fty,
    })
}

fn apply_binary_apply_transform<'tcx>(
    tcx: TyCtxt<'tcx>,
    body: &mut mir::Body<'tcx>,
    plan: &BinaryApplyPlan,
    helper_def_id: rustc_hir::def_id::DefId,
) -> bool {
    use rustc_middle::mir::*;
    use rustc_span::DUMMY_SP;
    use rustc_span::source_map::Spanned;

    let elem_ty = match plan.fty {
        FloatTy::F64 => tcx.types.f64,
    };
    #[allow(rustc::usage_of_qualified_ty)]
    let slice_ty = rustc_middle::ty::Ty::new_slice(tcx, elem_ty);
    #[allow(rustc::usage_of_qualified_ty)]
    let mut_slice_ref_ty = rustc_middle::ty::Ty::new_mut_ref(
        tcx, tcx.lifetimes.re_erased, slice_ty);
    let dst_temp = body.local_decls.push(LocalDecl::new(mut_slice_ref_ty, DUMMY_SP));
    let prelude = vec![Statement::new(
        SourceInfo::outermost(DUMMY_SP),
        StatementKind::Assign(Box::new((
            Place::from(dst_temp),
            Rvalue::Ref(
                tcx.lifetimes.re_erased,
                BorrowKind::Mut { kind: MutBorrowKind::Default },
                Place {
                    local: plan.dst_local,
                    projection: tcx.mk_place_elems(&[ProjectionElem::Deref]),
                },
            ),
        ))),
    )];
    let unit_dest = body.local_decls.push(LocalDecl::new(tcx.types.unit, DUMMY_SP));
    let helper_callee = Operand::function_handle(tcx, helper_def_id, std::iter::empty(), DUMMY_SP);
    let new_term_kind = TerminatorKind::Call {
        func: helper_callee,
        args: Box::new([
            Spanned { node: Operand::Move(Place::from(dst_temp)), span: DUMMY_SP },
            Spanned { node: Operand::Copy(Place::from(plan.a_local)), span: DUMMY_SP },
            Spanned { node: Operand::Copy(Place::from(plan.b_local)), span: DUMMY_SP },
        ]),
        destination: Place::from(unit_dest),
        target: Some(plan.exit_bb),
        unwind: UnwindAction::Unreachable,
        call_source: CallSource::Misc,
        fn_span: DUMMY_SP,
    };
    let new_bb_data = BasicBlockData::new_stmts(
        prelude,
        Some(Terminator { source_info: SourceInfo::outermost(DUMMY_SP), kind: new_term_kind }),
        false,
    );
    let header = {
        let mut found: Option<BasicBlock> = None;
        body.basic_blocks[plan.entry_bb].terminator().successors().for_each(|s| {
            if found.is_none() { found = Some(s); }
        });
        match found { Some(h) => h, None => return false }
    };
    let new_bb_idx = body.basic_blocks_mut().push(new_bb_data);
    let entry_term = body.basic_blocks_mut()[plan.entry_bb].terminator_mut();
    entry_term.successors_mut(|succ| {
        if *succ == header { *succ = new_bb_idx; }
    });
    true
}

pub(crate) fn try_transform_slice_binary_apply_f64<'tcx>(
    tcx: TyCtxt<'tcx>,
    body: &mut mir::Body<'tcx>,
) -> usize {
    if !idiom_transform_enabled(tcx) { return 0; }
    if is_in_sysroot_crate(tcx, body.source.def_id()) { return 0; }
    let fn_name = tcx.def_path_str(body.source.def_id());
    if fn_name.contains("parallel_runtime") || fn_name.contains("parallel_apply_")
        || fn_name.contains("parallel_invoke")
    { return 0; }
    if parallel_pass_quick_skip(tcx, body, 4, true) { return 0; }
    fn lookup(tcx: TyCtxt<'_>, name: &str) -> Option<rustc_hir::def_id::DefId> {
        tcx.get_diagnostic_item(rustc_span::Symbol::intern(name))
    }
    let loops = find_simple_loops(body);
    let mut plans: Vec<BinaryApplyPlan> = Vec::new();
    for lp in &loops {
        if let Some(p) = analyze_binary_apply_loop(tcx, body, lp) {
            par_dump!(tcx,
                "[PAR-IDIOM-BINARY-APPLY-CANDIDATE] fn={} entry=bb{} exit=bb{} dst=_{} a=_{} b=_{} method={}",
                fn_name, p.entry_bb.index(), p.exit_bb.index(),
                p.dst_local.index(), p.a_local.index(), p.b_local.index(), p.method,
            );
            plans.push(p);
        }
    }
    let mut count = 0;
    for plan in plans {
        let helper_name = binary_helper_name(plan.method, plan.fty).unwrap();
        let Some(helper_def_id) = lookup(tcx, helper_name) else { continue };
        if apply_binary_apply_transform(tcx, body, &plan, helper_def_id) {
            par_dump!(tcx, "[PAR-IDIOM-BINARY-APPLY-APPLIED] fn={} entry=bb{} method={}",
                fn_name, plan.entry_bb.index(), plan.method);
            count += 1;
        }
    }
    count
}

// abs-diff matcher, `dst[i] = (a[i] - b[i]).abs()`

#[derive(Debug, Clone)]
struct AbsDiffPlan {
    entry_bb: mir::BasicBlock,
    exit_bb: mir::BasicBlock,
    dst_local: mir::Local,
    a_local: mir::Local,
    b_local: mir::Local,
    fty: FloatTy,
}

fn analyze_abs_diff_loop<'tcx>(
    tcx: TyCtxt<'tcx>,
    body: &mir::Body<'tcx>,
    lp: &SimpleLoop,
) -> Option<AbsDiffPlan> {
    use rustc_middle::mir::*;
    let shape = detect_range_loop_shape(tcx, body, lp)?;
    let same_as_i = |l: Local| shape.same_as_i(l);
    let resolve = |l: Local| LoopShape::resolve(&shape.alias_root, l);

    // Trace write through abs Call, Sub, reads
    let mut indexed_writes = 0usize;
    let mut written_result: Option<(Local, Local)> = None; // (dst, result)
    for &bb in &lp.body {
        if bb == lp.header { continue; }
        for stmt in &body.basic_blocks[bb].statements {
            let StatementKind::Assign(box (place, rvalue)) = &stmt.kind else { continue };
            if !(place.projection.len() >= 2
                && matches!(place.projection[0], ProjectionElem::Deref)
                && matches!(place.projection[1], ProjectionElem::Index(idx) if same_as_i(idx)))
            {
                continue;
            }
            indexed_writes += 1;
            if let Rvalue::Use(op) = rvalue {
                if let Operand::Copy(p) | Operand::Move(p) = op {
                    if p.projection.is_empty() {
                        written_result = Some((place.local, p.local));
                    }
                }
            }
        }
    }
    if indexed_writes != 1 { return None; }
    let (dst_local, result_local) = written_result?;

    // Find Call to `f64::abs`
    let mut abs_arg_local: Option<Local> = None;
    for &bb in &lp.body {
        if bb == lp.header { continue; }
        let block = &body.basic_blocks[bb];
        let TerminatorKind::Call { func, args, destination, .. } = &block.terminator().kind
        else { continue };
        if !(destination.projection.is_empty()
            && resolve(destination.local) == resolve(result_local))
        { continue; }
        let Operand::Constant(c) = func else { return None };
        let rustc_middle::ty::FnDef(callee_did, _) = c.const_.ty().kind() else { return None };
        let path = tcx.def_path_str(*callee_did);
        if !path.ends_with("::abs") { return None; }
        if args.len() != 1 { return None; }
        // Arg must be a non-projection Local.
        if let Operand::Copy(p) | Operand::Move(p) = &args[0].node {
            if p.projection.is_empty() { abs_arg_local = Some(p.local); }
        }
        break;
    }
    let abs_arg_local = abs_arg_local?;

    // Find `_diff = Sub(_va_op, _vb_op)` producing `abs_arg_local`.
    let abs_arg_root = resolve(abs_arg_local);
    let mut sub_args: Option<(Local, Local)> = None;
    for &bb in &lp.body {
        if bb == lp.header { continue; }
        for stmt in &body.basic_blocks[bb].statements {
            let StatementKind::Assign(box (place, rvalue)) = &stmt.kind else { continue };
            if !place.projection.is_empty() { continue; }
            if resolve(place.local) != abs_arg_root { continue; }
            let Rvalue::BinaryOp(op, box (lhs, rhs)) = rvalue else { continue };
            if !matches!(op, BinOp::Sub | BinOp::SubUnchecked) { continue; }
            let local_op = |o: &Operand<'tcx>| -> Option<Local> {
                let p = match o {
                    Operand::Copy(p) | Operand::Move(p) => p,
                    _ => return None,
                };
                if !p.projection.is_empty() { return None; }
                Some(p.local)
            };
            // Handle direct indexed-read operands too.
            let direct_indexed = |o: &Operand<'tcx>| -> Option<Local> {
                let p = match o {
                    Operand::Copy(p) | Operand::Move(p) => p,
                    _ => return None,
                };
                if p.projection.len() >= 2
                    && matches!(p.projection[0], ProjectionElem::Deref)
                    && matches!(p.projection[1], ProjectionElem::Index(idx) if same_as_i(idx))
                {
                    return Some(resolve(p.local));
                }
                None
            };
            // Resolve each side to its source slice
            let resolve_side = |o: &Operand<'tcx>| -> Option<Local> {
                if let Some(s) = direct_indexed(o) { return Some(s); }
                let l = local_op(o)?;
                // Walk body for `l = (*_x)[_i]`.
                for &abb in &lp.body {
                    for s in &body.basic_blocks[abb].statements {
                        let StatementKind::Assign(box (pl, rv)) = &s.kind else { continue };
                        if !pl.projection.is_empty() || resolve(pl.local) != resolve(l) { continue; }
                        let Rvalue::Use(op2) = rv else { continue };
                        if let Operand::Copy(p) | Operand::Move(p) = op2 {
                            if p.projection.len() >= 2
                                && matches!(p.projection[0], ProjectionElem::Deref)
                                && matches!(p.projection[1],
                                    ProjectionElem::Index(idx) if same_as_i(idx))
                            {
                                return Some(resolve(p.local));
                            }
                        }
                    }
                }
                None
            };
            if let (Some(a), Some(b)) = (resolve_side(lhs), resolve_side(rhs)) {
                sub_args = Some((a, b));
            }
        }
    }
    let (a_local, b_local) = sub_args?;

    let dst_resolved = resolve(dst_local);
    let dst_ty = body.local_decls[dst_resolved].ty;
    let a_ty = body.local_decls[a_local].ty;
    let b_ty = body.local_decls[b_local].ty;
    let fty = ref_slice_float_ty(tcx, dst_ty, true)?;
    if ref_slice_float_ty(tcx, a_ty, false) != Some(fty) { return None; }
    if ref_slice_float_ty(tcx, b_ty, false) != Some(fty) { return None; }
    if dst_resolved == a_local || dst_resolved == b_local { return None; }

    Some(AbsDiffPlan {
        entry_bb: shape.entry_bb,
        exit_bb: shape.exit_bb,
        dst_local: dst_resolved,
        a_local,
        b_local,
        fty,
    })
}

fn apply_abs_diff_transform<'tcx>(
    tcx: TyCtxt<'tcx>,
    body: &mut mir::Body<'tcx>,
    plan: &AbsDiffPlan,
    helper_def_id: rustc_hir::def_id::DefId,
) -> bool {
    use rustc_middle::mir::*;
    use rustc_span::DUMMY_SP;
    use rustc_span::source_map::Spanned;

    let elem_ty = match plan.fty {
        FloatTy::F64 => tcx.types.f64,
    };
    #[allow(rustc::usage_of_qualified_ty)]
    let slice_ty = rustc_middle::ty::Ty::new_slice(tcx, elem_ty);
    #[allow(rustc::usage_of_qualified_ty)]
    let mut_slice_ref_ty = rustc_middle::ty::Ty::new_mut_ref(
        tcx, tcx.lifetimes.re_erased, slice_ty);
    let dst_temp = body.local_decls.push(LocalDecl::new(mut_slice_ref_ty, DUMMY_SP));
    let prelude = vec![Statement::new(
        SourceInfo::outermost(DUMMY_SP),
        StatementKind::Assign(Box::new((
            Place::from(dst_temp),
            Rvalue::Ref(
                tcx.lifetimes.re_erased,
                BorrowKind::Mut { kind: MutBorrowKind::Default },
                Place {
                    local: plan.dst_local,
                    projection: tcx.mk_place_elems(&[ProjectionElem::Deref]),
                },
            ),
        ))),
    )];
    let unit_dest = body.local_decls.push(LocalDecl::new(tcx.types.unit, DUMMY_SP));
    let helper_callee = Operand::function_handle(tcx, helper_def_id, std::iter::empty(), DUMMY_SP);
    let new_term_kind = TerminatorKind::Call {
        func: helper_callee,
        args: Box::new([
            Spanned { node: Operand::Move(Place::from(dst_temp)), span: DUMMY_SP },
            Spanned { node: Operand::Copy(Place::from(plan.a_local)), span: DUMMY_SP },
            Spanned { node: Operand::Copy(Place::from(plan.b_local)), span: DUMMY_SP },
        ]),
        destination: Place::from(unit_dest),
        target: Some(plan.exit_bb),
        unwind: UnwindAction::Unreachable,
        call_source: CallSource::Misc,
        fn_span: DUMMY_SP,
    };
    let new_bb_data = BasicBlockData::new_stmts(
        prelude,
        Some(Terminator { source_info: SourceInfo::outermost(DUMMY_SP), kind: new_term_kind }),
        false,
    );
    let header = {
        let mut found: Option<BasicBlock> = None;
        body.basic_blocks[plan.entry_bb].terminator().successors().for_each(|s| {
            if found.is_none() { found = Some(s); }
        });
        match found { Some(h) => h, None => return false }
    };
    let new_bb_idx = body.basic_blocks_mut().push(new_bb_data);
    let entry_term = body.basic_blocks_mut()[plan.entry_bb].terminator_mut();
    entry_term.successors_mut(|succ| {
        if *succ == header { *succ = new_bb_idx; }
    });
    true
}

pub(crate) fn try_transform_slice_abs_diff_f64<'tcx>(
    tcx: TyCtxt<'tcx>,
    body: &mut mir::Body<'tcx>,
) -> usize {
    if !idiom_transform_enabled(tcx) { return 0; }
    if is_in_sysroot_crate(tcx, body.source.def_id()) { return 0; }
    let fn_name = tcx.def_path_str(body.source.def_id());
    if fn_name.contains("parallel_runtime") || fn_name.contains("parallel_apply_")
        || fn_name.contains("parallel_invoke") || fn_name.contains("parallel_abs_diff")
    { return 0; }
    if parallel_pass_quick_skip(tcx, body, 4, true) { return 0; }
    fn lookup(tcx: TyCtxt<'_>, name: &str) -> Option<rustc_hir::def_id::DefId> {
        tcx.get_diagnostic_item(rustc_span::Symbol::intern(name))
    }
    let helper_f64 = lookup(tcx, "parallel_runtime_parallel_abs_diff_write_f64");
    if helper_f64.is_none() { return 0; }
    let loops = find_simple_loops(body);
    let mut plans: Vec<AbsDiffPlan> = Vec::new();
    for lp in &loops {
        if let Some(p) = analyze_abs_diff_loop(tcx, body, lp) {
            par_dump!(tcx,
                "[PAR-IDIOM-ABS-DIFF-CANDIDATE] fn={} entry=bb{} exit=bb{} dst=_{} a=_{} b=_{} fty={:?}",
                fn_name, p.entry_bb.index(), p.exit_bb.index(),
                p.dst_local.index(), p.a_local.index(), p.b_local.index(), p.fty,
            );
            plans.push(p);
        }
    }
    let mut count = 0;
    for plan in plans {
        let helper_did = helper_f64;
        let Some(helper_did) = helper_did else { continue };
        if apply_abs_diff_transform(tcx, body, &plan, helper_did) {
            par_dump!(tcx, "[PAR-IDIOM-ABS-DIFF-APPLIED] fn={} entry=bb{} fty={:?}",
                fn_name, plan.entry_bb.index(), plan.fty);
            count += 1;
        }
    }
    count
}

// FMA-write matcher, `dst[i] = a[i]*b[i] + c[i]`

#[derive(Debug, Clone)]
struct FmaWritePlan {
    entry_bb: mir::BasicBlock,
    exit_bb: mir::BasicBlock,
    dst_local: mir::Local,
    a_local: mir::Local,
    b_local: mir::Local,
    c_local: mir::Local,
    fty: FloatTy,
}

fn analyze_fma_write_loop<'tcx>(
    tcx: TyCtxt<'tcx>,
    body: &mir::Body<'tcx>,
    lp: &SimpleLoop,
) -> Option<FmaWritePlan> {
    use rustc_middle::mir::*;
    let shape = detect_range_loop_shape(tcx, body, lp)?;
    let same_as_i = |l: Local| shape.same_as_i(l);
    let resolve = |l: Local| LoopShape::resolve(&shape.alias_root, l);

    // Helper closures - same as in elemwise
    let direct_indexed = |o: &Operand<'tcx>| -> Option<Local> {
        let p = match o {
            Operand::Copy(p) | Operand::Move(p) => p,
            _ => return None,
        };
        if p.projection.len() >= 2
            && matches!(p.projection[0], ProjectionElem::Deref)
            && matches!(p.projection[1], ProjectionElem::Index(idx) if same_as_i(idx))
        {
            return Some(resolve(p.local));
        }
        None
    };
    let local_of = |o: &Operand<'tcx>| -> Option<Local> {
        let p = match o {
            Operand::Copy(p) | Operand::Move(p) => p,
            _ => return None,
        };
        if !p.projection.is_empty() { return None; }
        Some(p.local)
    };
    let resolve_side = |o: &Operand<'tcx>| -> Option<Local> {
        if let Some(s) = direct_indexed(o) { return Some(s); }
        let l = local_of(o)?;
        for &abb in &lp.body {
            for stmt in &body.basic_blocks[abb].statements {
                let StatementKind::Assign(box (pl, rv)) = &stmt.kind else { continue };
                if !pl.projection.is_empty() || resolve(pl.local) != resolve(l) { continue; }
                let Rvalue::Use(op2) = rv else { continue };
                if let Operand::Copy(p) | Operand::Move(p) = op2 {
                    if p.projection.len() >= 2
                        && matches!(p.projection[0], ProjectionElem::Deref)
                        && matches!(p.projection[1],
                            ProjectionElem::Index(idx) if same_as_i(idx))
                    {
                        return Some(resolve(p.local));
                    }
                }
            }
        }
        None
    };

    // Find write `Add(mul_result, indexed_read)`, either order
    let mut indexed_writes = 0usize;
    let mut chosen: Option<(Local /*dst*/, Local /*mul_result*/, Local /*c_src*/)> = None;
    for &bb in &lp.body {
        if bb == lp.header { continue; }
        for stmt in &body.basic_blocks[bb].statements {
            let StatementKind::Assign(box (place, rvalue)) = &stmt.kind else { continue };
            if !(place.projection.len() >= 2
                && matches!(place.projection[0], ProjectionElem::Deref)
                && matches!(place.projection[1], ProjectionElem::Index(idx) if same_as_i(idx)))
            {
                continue;
            }
            indexed_writes += 1;
            let Rvalue::BinaryOp(op, box (lhs, rhs)) = rvalue else { continue };
            if !matches!(op, BinOp::Add | BinOp::AddUnchecked) { continue; }
            // For each ordering, try (mul-result, indexed-c).
            let try_split = |mul_op: &Operand<'tcx>, c_op: &Operand<'tcx>| -> Option<(Local, Local)> {
                let mul_l = local_of(mul_op)?;
                let c_src = resolve_side(c_op)?;
                Some((mul_l, c_src))
            };
            if let Some((mul_l, c_src)) = try_split(lhs, rhs).or_else(|| try_split(rhs, lhs)) {
                chosen = Some((place.local, mul_l, c_src));
            }
        }
    }
    if indexed_writes != 1 { return None; }
    let (dst_local, mul_local, c_local) = chosen?;

    // Find Mul feeding mul_local, resolve indexed sources
    let mul_root = resolve(mul_local);
    let mut mul_args: Option<(Local, Local)> = None;
    for &bb in &lp.body {
        if bb == lp.header { continue; }
        for stmt in &body.basic_blocks[bb].statements {
            let StatementKind::Assign(box (place, rvalue)) = &stmt.kind else { continue };
            if !place.projection.is_empty() { continue; }
            if resolve(place.local) != mul_root { continue; }
            let Rvalue::BinaryOp(op, box (lhs, rhs)) = rvalue else { continue };
            if !matches!(op, BinOp::Mul | BinOp::MulUnchecked) { continue; }
            if let (Some(a), Some(b)) = (resolve_side(lhs), resolve_side(rhs)) {
                mul_args = Some((a, b));
            }
        }
    }
    let (a_local, b_local) = mul_args?;

    let dst_resolved = resolve(dst_local);
    let dst_ty = body.local_decls[dst_resolved].ty;
    let fty = ref_slice_float_ty(tcx, dst_ty, true)?;
    if ref_slice_float_ty(tcx, body.local_decls[a_local].ty, false) != Some(fty) { return None; }
    if ref_slice_float_ty(tcx, body.local_decls[b_local].ty, false) != Some(fty) { return None; }
    if ref_slice_float_ty(tcx, body.local_decls[c_local].ty, false) != Some(fty) { return None; }
    if dst_resolved == a_local || dst_resolved == b_local || dst_resolved == c_local {
        return None;
    }

    Some(FmaWritePlan {
        entry_bb: shape.entry_bb,
        exit_bb: shape.exit_bb,
        dst_local: dst_resolved,
        a_local,
        b_local,
        c_local,
        fty,
    })
}

fn apply_fma_write_transform<'tcx>(
    tcx: TyCtxt<'tcx>,
    body: &mut mir::Body<'tcx>,
    plan: &FmaWritePlan,
    helper_def_id: rustc_hir::def_id::DefId,
) -> bool {
    use rustc_middle::mir::*;
    use rustc_span::DUMMY_SP;
    use rustc_span::source_map::Spanned;

    let elem_ty = match plan.fty {
        FloatTy::F64 => tcx.types.f64,
    };
    #[allow(rustc::usage_of_qualified_ty)]
    let slice_ty = rustc_middle::ty::Ty::new_slice(tcx, elem_ty);
    #[allow(rustc::usage_of_qualified_ty)]
    let mut_slice_ref_ty = rustc_middle::ty::Ty::new_mut_ref(
        tcx, tcx.lifetimes.re_erased, slice_ty);
    let dst_temp = body.local_decls.push(LocalDecl::new(mut_slice_ref_ty, DUMMY_SP));
    let prelude = vec![Statement::new(
        SourceInfo::outermost(DUMMY_SP),
        StatementKind::Assign(Box::new((
            Place::from(dst_temp),
            Rvalue::Ref(
                tcx.lifetimes.re_erased,
                BorrowKind::Mut { kind: MutBorrowKind::Default },
                Place {
                    local: plan.dst_local,
                    projection: tcx.mk_place_elems(&[ProjectionElem::Deref]),
                },
            ),
        ))),
    )];
    let unit_dest = body.local_decls.push(LocalDecl::new(tcx.types.unit, DUMMY_SP));
    let helper_callee = Operand::function_handle(tcx, helper_def_id, std::iter::empty(), DUMMY_SP);
    let new_term_kind = TerminatorKind::Call {
        func: helper_callee,
        args: Box::new([
            Spanned { node: Operand::Move(Place::from(dst_temp)), span: DUMMY_SP },
            Spanned { node: Operand::Copy(Place::from(plan.a_local)), span: DUMMY_SP },
            Spanned { node: Operand::Copy(Place::from(plan.b_local)), span: DUMMY_SP },
            Spanned { node: Operand::Copy(Place::from(plan.c_local)), span: DUMMY_SP },
        ]),
        destination: Place::from(unit_dest),
        target: Some(plan.exit_bb),
        unwind: UnwindAction::Unreachable,
        call_source: CallSource::Misc,
        fn_span: DUMMY_SP,
    };
    let new_bb_data = BasicBlockData::new_stmts(
        prelude,
        Some(Terminator { source_info: SourceInfo::outermost(DUMMY_SP), kind: new_term_kind }),
        false,
    );
    let header = {
        let mut found: Option<BasicBlock> = None;
        body.basic_blocks[plan.entry_bb].terminator().successors().for_each(|s| {
            if found.is_none() { found = Some(s); }
        });
        match found { Some(h) => h, None => return false }
    };
    let new_bb_idx = body.basic_blocks_mut().push(new_bb_data);
    let entry_term = body.basic_blocks_mut()[plan.entry_bb].terminator_mut();
    entry_term.successors_mut(|succ| {
        if *succ == header { *succ = new_bb_idx; }
    });
    true
}

pub(crate) fn try_transform_slice_fma_write_f64<'tcx>(
    tcx: TyCtxt<'tcx>,
    body: &mut mir::Body<'tcx>,
) -> usize {
    if !idiom_transform_enabled(tcx) { return 0; }
    if is_in_sysroot_crate(tcx, body.source.def_id()) { return 0; }
    let fn_name = tcx.def_path_str(body.source.def_id());
    if fn_name.contains("parallel_runtime") || fn_name.contains("parallel_apply_")
        || fn_name.contains("parallel_invoke") || fn_name.contains("parallel_fma_")
    { return 0; }
    if parallel_pass_quick_skip(tcx, body, 4, true) { return 0; }
    fn lookup(tcx: TyCtxt<'_>, name: &str) -> Option<rustc_hir::def_id::DefId> {
        tcx.get_diagnostic_item(rustc_span::Symbol::intern(name))
    }
    let helper_f64 = lookup(tcx, "parallel_runtime_parallel_fma_write_f64");
    if helper_f64.is_none() { return 0; }
    let loops = find_simple_loops(body);
    let mut plans: Vec<FmaWritePlan> = Vec::new();
    for lp in &loops {
        if let Some(p) = analyze_fma_write_loop(tcx, body, lp) {
            par_dump!(tcx,
                "[PAR-IDIOM-FMA-WRITE-CANDIDATE] fn={} entry=bb{} exit=bb{} dst=_{} a=_{} b=_{} c=_{} fty={:?}",
                fn_name, p.entry_bb.index(), p.exit_bb.index(),
                p.dst_local.index(), p.a_local.index(), p.b_local.index(), p.c_local.index(), p.fty,
            );
            plans.push(p);
        }
    }
    let mut count = 0;
    for plan in plans {
        let helper_did = helper_f64;
        let Some(helper_did) = helper_did else { continue };
        if apply_fma_write_transform(tcx, body, &plan, helper_did) {
            par_dump!(tcx, "[PAR-IDIOM-FMA-WRITE-APPLIED] fn={} entry=bb{} fty={:?}",
                fn_name, plan.entry_bb.index(), plan.fty);
            count += 1;
        }
    }
    count
}

// Clamp matcher, `dst[i] = a[i].clamp(lo, hi)`

#[derive(Debug, Clone)]
struct ClampPlan {
    entry_bb: mir::BasicBlock,
    exit_bb: mir::BasicBlock,
    dst_local: mir::Local,
    src_local: mir::Local,
    lo_local: mir::Local,
    hi_local: mir::Local,
    fty: FloatTy,
}

fn analyze_clamp_loop<'tcx>(
    tcx: TyCtxt<'tcx>,
    body: &mir::Body<'tcx>,
    lp: &SimpleLoop,
) -> Option<ClampPlan> {
    use rustc_middle::mir::*;
    let dbg = std::env::var("PARALLEL_CLAMP_DEBUG").ok().as_deref() == Some("1");
    macro_rules! reject {
        ($($arg:tt)*) => {{
            if dbg { eprintln!("[PAR-IDIOM-CLAMP-REJECT] header=bb{}: {}", lp.header.index(), format!($($arg)*)); }
            return None;
        }};
    }
    let shape = match detect_range_loop_shape(tcx, body, lp) {
        Some(s) => s,
        None => reject!("loop shape not detected"),
    };
    let same_as_i = |l: Local| shape.same_as_i(l);
    let resolve = |l: Local| LoopShape::resolve(&shape.alias_root, l);

    // Find indexed write `(*_dst)[_i] = move _result`.
    let mut indexed_writes = 0usize;
    let mut chosen: Option<(Local, Local)> = None;
    for &bb in &lp.body {
        if bb == lp.header { continue; }
        for stmt in &body.basic_blocks[bb].statements {
            let StatementKind::Assign(box (place, rvalue)) = &stmt.kind else { continue };
            if !(place.projection.len() >= 2
                && matches!(place.projection[0], ProjectionElem::Deref)
                && matches!(place.projection[1], ProjectionElem::Index(idx) if same_as_i(idx)))
            {
                continue;
            }
            indexed_writes += 1;
            if let Rvalue::Use(op) = rvalue {
                if let Operand::Copy(p) | Operand::Move(p) = op {
                    if p.projection.is_empty() {
                        chosen = Some((place.local, p.local));
                    }
                }
            }
        }
    }
    if dbg { eprintln!("[PAR-IDIOM-CLAMP-DBG] indexed_writes={}", indexed_writes); }
    if indexed_writes != 1 { reject!("indexed_writes={}", indexed_writes); }
    let Some((dst_local, result_local)) = chosen else { reject!("no Use indexed write"); };
    if dbg { eprintln!("[PAR-IDIOM-CLAMP-DBG] dst_local=_{} result_local=_{}", dst_local.index(), result_local.index()); }

    // Find Call to `f64::clamp`
    let mut src_local: Option<Local> = None;
    let mut lo_local: Option<Local> = None;
    let mut hi_local: Option<Local> = None;
    let mut found_call = false;
    for &bb in &lp.body {
        if bb == lp.header { continue; }
        let block = &body.basic_blocks[bb];
        let TerminatorKind::Call { func, args, destination, .. } = &block.terminator().kind
        else { continue };
        if !(destination.projection.is_empty()
            && resolve(destination.local) == resolve(result_local))
        { continue; }
        found_call = true;
        let Operand::Constant(c) = func else { reject!("func not Constant"); };
        let rustc_middle::ty::FnDef(callee_did, _) = c.const_.ty().kind() else {
            reject!("func not FnDef");
        };
        let path = tcx.def_path_str(*callee_did);
        if dbg { eprintln!("[PAR-IDIOM-CLAMP-DBG] callee path={}", path); }
        if !path.ends_with("::clamp") { reject!("not ::clamp ({})", path); }
        if args.len() != 3 { reject!("args.len={}", args.len()); }

        // Arg 0, indexed read or upstream alias
        let arg_to_indexed_src = |o: &Operand<'tcx>| -> Option<Local> {
            let p = match o {
                Operand::Copy(p) | Operand::Move(p) => p,
                _ => return None,
            };
            if p.projection.len() >= 2
                && matches!(p.projection[0], ProjectionElem::Deref)
                && matches!(p.projection[1], ProjectionElem::Index(idx) if same_as_i(idx))
            {
                return Some(resolve(p.local));
            }
            if !p.projection.is_empty() { return None; }
            // Walk reads chain.
            for &abb in &lp.body {
                for stmt in &body.basic_blocks[abb].statements {
                    let StatementKind::Assign(box (pl, rv)) = &stmt.kind else { continue };
                    if !pl.projection.is_empty() || resolve(pl.local) != resolve(p.local) { continue; }
                    let Rvalue::Use(op2) = rv else { continue };
                    if let Operand::Copy(p2) | Operand::Move(p2) = op2 {
                        if p2.projection.len() >= 2
                            && matches!(p2.projection[0], ProjectionElem::Deref)
                            && matches!(p2.projection[1],
                                ProjectionElem::Index(idx) if same_as_i(idx))
                        {
                            return Some(resolve(p2.local));
                        }
                    }
                }
            }
            None
        };
        // Args 1,2 scalar Locals, entry-live required
        let scalar_arg = |o: &Operand<'tcx>| -> Option<Local> {
            let p = match o {
                Operand::Copy(p) | Operand::Move(p) => p,
                _ => return None,
            };
            if !p.projection.is_empty() { return None; }
            Some(resolve(p.local))
        };
        src_local = arg_to_indexed_src(&args[0].node);
        lo_local = scalar_arg(&args[1].node);
        hi_local = scalar_arg(&args[2].node);
        break;
    }
    if !found_call { reject!("no Call producing result_local"); }
    if dbg { eprintln!("[PAR-IDIOM-CLAMP-DBG] src={:?} lo={:?} hi={:?}", src_local, lo_local, hi_local); }
    let Some(src_local) = src_local else { reject!("src not resolved"); };
    let Some(lo_local) = lo_local else { reject!("lo not resolved"); };
    let Some(hi_local) = hi_local else { reject!("hi not resolved"); };

    // lo/hi unassigned in loop; aliases aren't writes
    let assigned_in_loop = |l: Local| -> bool {
        for &bb in &lp.body {
            for stmt in &body.basic_blocks[bb].statements {
                if let StatementKind::Assign(box (pl, _)) = &stmt.kind {
                    if pl.projection.is_empty() && pl.local == l { return true; }
                }
            }
            if let TerminatorKind::Call { destination, .. }
                = &body.basic_blocks[bb].terminator().kind
            {
                if destination.projection.is_empty() && destination.local == l { return true; }
            }
        }
        false
    };
    if assigned_in_loop(lo_local) { return None; }
    if assigned_in_loop(hi_local) { return None; }

    let dst_resolved = resolve(dst_local);
    let dst_ty = body.local_decls[dst_resolved].ty;
    let fty = ref_slice_float_ty(tcx, dst_ty, true)?;
    if ref_slice_float_ty(tcx, body.local_decls[src_local].ty, false) != Some(fty) { return None; }
    let scalar_ty = tcx.types.f64;
    if body.local_decls[lo_local].ty != scalar_ty { return None; }
    if body.local_decls[hi_local].ty != scalar_ty { return None; }
    if dst_resolved == src_local { return None; }

    Some(ClampPlan {
        entry_bb: shape.entry_bb,
        exit_bb: shape.exit_bb,
        dst_local: dst_resolved,
        src_local,
        lo_local,
        hi_local,
        fty,
    })
}

fn apply_clamp_transform<'tcx>(
    tcx: TyCtxt<'tcx>,
    body: &mut mir::Body<'tcx>,
    plan: &ClampPlan,
    helper_def_id: rustc_hir::def_id::DefId,
) -> bool {
    use rustc_middle::mir::*;
    use rustc_span::DUMMY_SP;
    use rustc_span::source_map::Spanned;

    let elem_ty = match plan.fty {
        FloatTy::F64 => tcx.types.f64,
    };
    #[allow(rustc::usage_of_qualified_ty)]
    let slice_ty = rustc_middle::ty::Ty::new_slice(tcx, elem_ty);
    #[allow(rustc::usage_of_qualified_ty)]
    let mut_slice_ref_ty = rustc_middle::ty::Ty::new_mut_ref(
        tcx, tcx.lifetimes.re_erased, slice_ty);
    let dst_temp = body.local_decls.push(LocalDecl::new(mut_slice_ref_ty, DUMMY_SP));
    let prelude = vec![Statement::new(
        SourceInfo::outermost(DUMMY_SP),
        StatementKind::Assign(Box::new((
            Place::from(dst_temp),
            Rvalue::Ref(
                tcx.lifetimes.re_erased,
                BorrowKind::Mut { kind: MutBorrowKind::Default },
                Place {
                    local: plan.dst_local,
                    projection: tcx.mk_place_elems(&[ProjectionElem::Deref]),
                },
            ),
        ))),
    )];
    let unit_dest = body.local_decls.push(LocalDecl::new(tcx.types.unit, DUMMY_SP));
    let helper_callee = Operand::function_handle(tcx, helper_def_id, std::iter::empty(), DUMMY_SP);
    let new_term_kind = TerminatorKind::Call {
        func: helper_callee,
        args: Box::new([
            Spanned { node: Operand::Move(Place::from(dst_temp)), span: DUMMY_SP },
            Spanned { node: Operand::Copy(Place::from(plan.src_local)), span: DUMMY_SP },
            Spanned { node: Operand::Copy(Place::from(plan.lo_local)), span: DUMMY_SP },
            Spanned { node: Operand::Copy(Place::from(plan.hi_local)), span: DUMMY_SP },
        ]),
        destination: Place::from(unit_dest),
        target: Some(plan.exit_bb),
        unwind: UnwindAction::Unreachable,
        call_source: CallSource::Misc,
        fn_span: DUMMY_SP,
    };
    let new_bb_data = BasicBlockData::new_stmts(
        prelude,
        Some(Terminator { source_info: SourceInfo::outermost(DUMMY_SP), kind: new_term_kind }),
        false,
    );
    let header = {
        let mut found: Option<BasicBlock> = None;
        body.basic_blocks[plan.entry_bb].terminator().successors().for_each(|s| {
            if found.is_none() { found = Some(s); }
        });
        match found { Some(h) => h, None => return false }
    };
    let new_bb_idx = body.basic_blocks_mut().push(new_bb_data);
    let entry_term = body.basic_blocks_mut()[plan.entry_bb].terminator_mut();
    entry_term.successors_mut(|succ| {
        if *succ == header { *succ = new_bb_idx; }
    });
    true
}

pub(crate) fn try_transform_slice_clamp_f64<'tcx>(
    tcx: TyCtxt<'tcx>,
    body: &mut mir::Body<'tcx>,
) -> usize {
    if !idiom_transform_enabled(tcx) { return 0; }
    if is_in_sysroot_crate(tcx, body.source.def_id()) { return 0; }
    let fn_name = tcx.def_path_str(body.source.def_id());
    if fn_name.contains("parallel_runtime") || fn_name.contains("parallel_clamp")
        || fn_name.contains("parallel_invoke") || fn_name.contains("parallel_apply_")
    { return 0; }
    if parallel_pass_quick_skip(tcx, body, 4, true) { return 0; }
    fn lookup(tcx: TyCtxt<'_>, name: &str) -> Option<rustc_hir::def_id::DefId> {
        tcx.get_diagnostic_item(rustc_span::Symbol::intern(name))
    }
    let helper_f64 = lookup(tcx, "parallel_runtime_parallel_clamp_f64");
    if helper_f64.is_none() { return 0; }
    let loops = find_simple_loops(body);
    let mut plans: Vec<ClampPlan> = Vec::new();
    for lp in &loops {
        if let Some(p) = analyze_clamp_loop(tcx, body, lp) {
            par_dump!(tcx,
                "[PAR-IDIOM-CLAMP-CANDIDATE] fn={} entry=bb{} dst=_{} src=_{} lo=_{} hi=_{} fty={:?}",
                fn_name, p.entry_bb.index(),
                p.dst_local.index(), p.src_local.index(),
                p.lo_local.index(), p.hi_local.index(), p.fty,
            );
            plans.push(p);
        }
    }
    let mut count = 0;
    for plan in plans {
        let helper_did = helper_f64;
        let Some(helper_did) = helper_did else { continue };
        if apply_clamp_transform(tcx, body, &plan, helper_did) {
            par_dump!(tcx, "[PAR-IDIOM-CLAMP-APPLIED] fn={} entry=bb{} fty={:?}",
                fn_name, plan.entry_bb.index(), plan.fty);
            count += 1;
        }
    }
    count
}

// Multi-AXPY matcher, `dst[i] = alpha*x[i] + beta*z[i]`

#[derive(Debug, Clone)]
struct MultiAxpyPlan {
    entry_bb: mir::BasicBlock,
    exit_bb: mir::BasicBlock,
    dst_local: mir::Local,
    x_local: mir::Local,
    alpha_local: mir::Local,
    z_local: mir::Local,
    beta_local: mir::Local,
    fty: FloatTy,
}

fn analyze_multi_axpy_loop<'tcx>(
    tcx: TyCtxt<'tcx>,
    body: &mir::Body<'tcx>,
    lp: &SimpleLoop,
) -> Option<MultiAxpyPlan> {
    use rustc_middle::mir::*;
    let shape = detect_range_loop_shape(tcx, body, lp)?;
    let same_as_i = |l: Local| shape.same_as_i(l);
    let resolve = |l: Local| LoopShape::resolve(&shape.alias_root, l);

    let direct_indexed = |o: &Operand<'tcx>| -> Option<Local> {
        let p = match o {
            Operand::Copy(p) | Operand::Move(p) => p,
            _ => return None,
        };
        if p.projection.len() >= 2
            && matches!(p.projection[0], ProjectionElem::Deref)
            && matches!(p.projection[1], ProjectionElem::Index(idx) if same_as_i(idx))
        {
            return Some(resolve(p.local));
        }
        None
    };
    let local_of = |o: &Operand<'tcx>| -> Option<Local> {
        let p = match o {
            Operand::Copy(p) | Operand::Move(p) => p,
            _ => return None,
        };
        if !p.projection.is_empty() { return None; }
        Some(p.local)
    };
    let resolve_indexed = |o: &Operand<'tcx>| -> Option<Local> {
        if let Some(s) = direct_indexed(o) { return Some(s); }
        let l = local_of(o)?;
        for &abb in &lp.body {
            for stmt in &body.basic_blocks[abb].statements {
                let StatementKind::Assign(box (pl, rv)) = &stmt.kind else { continue };
                if !pl.projection.is_empty() || resolve(pl.local) != resolve(l) { continue; }
                let Rvalue::Use(op2) = rv else { continue };
                if let Operand::Copy(p) | Operand::Move(p) = op2 {
                    if p.projection.len() >= 2
                        && matches!(p.projection[0], ProjectionElem::Deref)
                        && matches!(p.projection[1],
                            ProjectionElem::Index(idx) if same_as_i(idx))
                    {
                        return Some(resolve(p.local));
                    }
                }
            }
        }
        None
    };

    // Indexed write `Add(_t1, _t2)`, both Mul results
    let mut indexed_writes = 0usize;
    let mut chosen: Option<(Local, Local, Local)> = None; // (dst, t1, t2)
    for &bb in &lp.body {
        if bb == lp.header { continue; }
        for stmt in &body.basic_blocks[bb].statements {
            let StatementKind::Assign(box (place, rvalue)) = &stmt.kind else { continue };
            if !(place.projection.len() >= 2
                && matches!(place.projection[0], ProjectionElem::Deref)
                && matches!(place.projection[1], ProjectionElem::Index(idx) if same_as_i(idx)))
            { continue; }
            indexed_writes += 1;
            let Rvalue::BinaryOp(op, box (lhs, rhs)) = rvalue else { continue };
            if !matches!(op, BinOp::Add | BinOp::AddUnchecked) { continue; }
            if let (Some(a), Some(b)) = (local_of(lhs), local_of(rhs)) {
                chosen = Some((place.local, a, b));
            }
        }
    }
    if indexed_writes != 1 { return None; }
    let (dst_local, t1_local, t2_local) = chosen?;

    // For each of t1, t2
    let resolve_mul = |target: Local| -> Option<(Local /*scalar*/, Local /*indexed_src*/)> {
        let target_root = resolve(target);
        for &bb in &lp.body {
            if bb == lp.header { continue; }
            for stmt in &body.basic_blocks[bb].statements {
                let StatementKind::Assign(box (pl, rv)) = &stmt.kind else { continue };
                if !pl.projection.is_empty() || resolve(pl.local) != target_root { continue; }
                let Rvalue::BinaryOp(op, box (lhs, rhs)) = rv else { continue };
                if !matches!(op, BinOp::Mul | BinOp::MulUnchecked) { continue; }
                // One side is scalar
                let try_split = |scalar_op: &Operand<'tcx>, idx_op: &Operand<'tcx>|
                    -> Option<(Local, Local)>
                {
                    let s = local_of(scalar_op)?;
                    let i = resolve_indexed(idx_op)?;
                    Some((resolve(s), i))
                };
                if let Some(p) = try_split(lhs, rhs).or_else(|| try_split(rhs, lhs)) {
                    return Some(p);
                }
            }
        }
        None
    };

    let (alpha_local, x_local) = resolve_mul(t1_local)?;
    let (beta_local, z_local) = resolve_mul(t2_local)?;

    // Slices and scalars must share float type
    let dst_resolved = resolve(dst_local);
    let dst_ty = body.local_decls[dst_resolved].ty;
    let fty = ref_slice_float_ty(tcx, dst_ty, true)?;
    if ref_slice_float_ty(tcx, body.local_decls[x_local].ty, false) != Some(fty) { return None; }
    if ref_slice_float_ty(tcx, body.local_decls[z_local].ty, false) != Some(fty) { return None; }
    let scalar_ty = tcx.types.f64;
    if body.local_decls[alpha_local].ty != scalar_ty { return None; }
    if body.local_decls[beta_local].ty != scalar_ty { return None; }
    if dst_resolved == x_local || dst_resolved == z_local { return None; }
    if x_local == z_local { return None; }  // collapses to single AXPY

    Some(MultiAxpyPlan {
        entry_bb: shape.entry_bb,
        exit_bb: shape.exit_bb,
        dst_local: dst_resolved,
        x_local,
        alpha_local,
        z_local,
        beta_local,
        fty,
    })
}

fn apply_multi_axpy_transform<'tcx>(
    tcx: TyCtxt<'tcx>,
    body: &mut mir::Body<'tcx>,
    plan: &MultiAxpyPlan,
    helper_def_id: rustc_hir::def_id::DefId,
) -> bool {
    use rustc_middle::mir::*;
    use rustc_span::DUMMY_SP;
    use rustc_span::source_map::Spanned;

    let elem_ty = match plan.fty {
        FloatTy::F64 => tcx.types.f64,
    };
    #[allow(rustc::usage_of_qualified_ty)]
    let slice_ty = rustc_middle::ty::Ty::new_slice(tcx, elem_ty);
    #[allow(rustc::usage_of_qualified_ty)]
    let mut_slice_ref_ty = rustc_middle::ty::Ty::new_mut_ref(
        tcx, tcx.lifetimes.re_erased, slice_ty);
    let dst_temp = body.local_decls.push(LocalDecl::new(mut_slice_ref_ty, DUMMY_SP));
    let prelude = vec![Statement::new(
        SourceInfo::outermost(DUMMY_SP),
        StatementKind::Assign(Box::new((
            Place::from(dst_temp),
            Rvalue::Ref(
                tcx.lifetimes.re_erased,
                BorrowKind::Mut { kind: MutBorrowKind::Default },
                Place {
                    local: plan.dst_local,
                    projection: tcx.mk_place_elems(&[ProjectionElem::Deref]),
                },
            ),
        ))),
    )];
    let unit_dest = body.local_decls.push(LocalDecl::new(tcx.types.unit, DUMMY_SP));
    let helper_callee = Operand::function_handle(tcx, helper_def_id, std::iter::empty(), DUMMY_SP);
    // Helper signature: parallel_multi_axpy(dst, x, alpha, z, beta).
    let new_term_kind = TerminatorKind::Call {
        func: helper_callee,
        args: Box::new([
            Spanned { node: Operand::Move(Place::from(dst_temp)), span: DUMMY_SP },
            Spanned { node: Operand::Copy(Place::from(plan.x_local)), span: DUMMY_SP },
            Spanned { node: Operand::Copy(Place::from(plan.alpha_local)), span: DUMMY_SP },
            Spanned { node: Operand::Copy(Place::from(plan.z_local)), span: DUMMY_SP },
            Spanned { node: Operand::Copy(Place::from(plan.beta_local)), span: DUMMY_SP },
        ]),
        destination: Place::from(unit_dest),
        target: Some(plan.exit_bb),
        unwind: UnwindAction::Unreachable,
        call_source: CallSource::Misc,
        fn_span: DUMMY_SP,
    };
    let new_bb_data = BasicBlockData::new_stmts(
        prelude,
        Some(Terminator { source_info: SourceInfo::outermost(DUMMY_SP), kind: new_term_kind }),
        false,
    );
    let header = {
        let mut found: Option<BasicBlock> = None;
        body.basic_blocks[plan.entry_bb].terminator().successors().for_each(|s| {
            if found.is_none() { found = Some(s); }
        });
        match found { Some(h) => h, None => return false }
    };
    let new_bb_idx = body.basic_blocks_mut().push(new_bb_data);
    let entry_term = body.basic_blocks_mut()[plan.entry_bb].terminator_mut();
    entry_term.successors_mut(|succ| {
        if *succ == header { *succ = new_bb_idx; }
    });
    true
}

pub(crate) fn try_transform_slice_multi_axpy_f64<'tcx>(
    tcx: TyCtxt<'tcx>,
    body: &mut mir::Body<'tcx>,
) -> usize {
    if !idiom_transform_enabled(tcx) { return 0; }
    if is_in_sysroot_crate(tcx, body.source.def_id()) { return 0; }
    let fn_name = tcx.def_path_str(body.source.def_id());
    if fn_name.contains("parallel_runtime") || fn_name.contains("parallel_multi_axpy")
        || fn_name.contains("parallel_invoke") || fn_name.contains("parallel_axpy")
    { return 0; }
    if parallel_pass_quick_skip(tcx, body, 4, true) { return 0; }
    fn lookup(tcx: TyCtxt<'_>, name: &str) -> Option<rustc_hir::def_id::DefId> {
        tcx.get_diagnostic_item(rustc_span::Symbol::intern(name))
    }
    let helper_f64 = lookup(tcx, "parallel_runtime_parallel_multi_axpy_f64");
    if helper_f64.is_none() { return 0; }
    let loops = find_simple_loops(body);
    let mut plans: Vec<MultiAxpyPlan> = Vec::new();
    for lp in &loops {
        if let Some(p) = analyze_multi_axpy_loop(tcx, body, lp) {
            par_dump!(tcx,
                "[PAR-IDIOM-MULTI-AXPY-CANDIDATE] fn={} entry=bb{} dst=_{} x=_{} α=_{} z=_{} β=_{} fty={:?}",
                fn_name, p.entry_bb.index(),
                p.dst_local.index(), p.x_local.index(), p.alpha_local.index(),
                p.z_local.index(), p.beta_local.index(), p.fty,
            );
            plans.push(p);
        }
    }
    let mut count = 0;
    for plan in plans {
        let helper_did = helper_f64;
        let Some(helper_did) = helper_did else { continue };
        if apply_multi_axpy_transform(tcx, body, &plan, helper_did) {
            par_dump!(tcx, "[PAR-IDIOM-MULTI-AXPY-APPLIED] fn={} entry=bb{} fty={:?}",
                fn_name, plan.entry_bb.index(), plan.fty);
            count += 1;
        }
    }
    count
}

// Matvec matcher, nested loops, `y[i] += a[i*n+j]*x[j]`

#[derive(Debug, Clone)]
struct MatvecPlan<'tcx> {
    outer_entry_bb: mir::BasicBlock,
    outer_exit_bb: mir::BasicBlock,
    y_local: mir::Local,
    a_local: mir::Local,
    x_local: mir::Local,
    /// Outer Range end `m`, from Range Aggregate
    m_op: mir::Operand<'tcx>,
    /// Inner Range end `n`, multiplier in `i*n+j`
    n_op: mir::Operand<'tcx>,
    fty: FloatTy,
}

fn analyze_matvec_loops<'tcx>(
    tcx: TyCtxt<'tcx>,
    body: &mir::Body<'tcx>,
    outer_lp: &SimpleLoop,
    inner_lp: &SimpleLoop,
) -> Option<MatvecPlan<'tcx>> {
    use rustc_middle::mir::*;

    // Containment, inner body within outer, headers differ
    if outer_lp.header == inner_lp.header { return None; }
    if !outer_lp.body.contains(&inner_lp.header) { return None; }

    let outer_shape = detect_range_loop_shape(tcx, body, outer_lp)?;
    let inner_shape = detect_range_loop_shape(tcx, body, inner_lp)?;

    let outer_resolve = |l: Local| LoopShape::resolve(&outer_shape.alias_root, l);
    let inner_resolve = |l: Local| LoopShape::resolve(&inner_shape.alias_root, l);
    let same_as_outer_i = |l: Local| outer_shape.same_as_i(l);
    let same_as_inner_j = |l: Local| inner_shape.same_as_i(l);

    // Re-derive outer some-arm from switchInt 1-target
    let outer_some_arm = {
        let header = outer_lp.header;
        let TerminatorKind::Call { target: Some(post), .. } =
            &body.basic_blocks[header].terminator().kind
        else { return None; };
        let TerminatorKind::SwitchInt { targets, .. } =
            &body.basic_blocks[*post].terminator().kind
        else { return None; };
        let mut some: Option<BasicBlock> = None;
        for (val, t) in targets.iter() { if val == 1 { some = Some(t); } }
        some?
    };

    // Find acc const-zero init in outer some-arm
    let mut acc_local: Option<Local> = None;
    for stmt in &body.basic_blocks[outer_some_arm].statements {
        let StatementKind::Assign(box (place, rvalue)) = &stmt.kind else { continue };
        if !place.projection.is_empty() { continue; }
        let Rvalue::Use(op) = rvalue else { continue };
        let Operand::Constant(c) = op else { continue };
        let bits = match eval_const_bits(tcx, &c.const_) {
            Some(b) => b, None => continue,
        };
        // Bits 0 for both widths, type disambiguates
        if bits != 0 { continue; }
        let lty = body.local_decls[place.local].ty;
        if lty == tcx.types.f64 {
            acc_local = Some(place.local);
        }
    }
    let acc_local = acc_local?;
    let acc_ty = body.local_decls[acc_local].ty;
    let fty = if acc_ty == tcx.types.f64 { FloatTy::F64 }
        else { return None; };

    // Match `acc += a[i*n+j] * x[j]` chain

    let local_op = |o: &Operand<'tcx>| -> Option<Local> {
        let p = match o {
            Operand::Copy(p) | Operand::Move(p) => p,
            _ => return None,
        };
        if !p.projection.is_empty() { return None; }
        Some(p.local)
    };

    // Step A, Mul(i, n) in inner body
    let mut t_mul_local: Option<Local> = None;
    let mut n_op_found: Option<Operand<'tcx>> = None;
    'find_mul: for &bb in &inner_lp.body {
        if bb == inner_lp.header { continue; }
        for stmt in &body.basic_blocks[bb].statements {
            let StatementKind::Assign(box (place, rvalue)) = &stmt.kind else { continue };
            if !place.projection.is_empty() { continue; }
            let Rvalue::BinaryOp(op, box (lhs, rhs)) = rvalue else { continue };
            if !matches!(op, BinOp::Mul | BinOp::MulUnchecked) { continue; }
            // Try both orderings for (i, n).
            let try_ord = |a_op: &Operand<'tcx>, b_op: &Operand<'tcx>|
                -> Option<Operand<'tcx>>
            {
                let l = local_op(a_op)?;
                if !same_as_outer_i(outer_resolve(l)) { return None; }
                // Other side is `n`
                Some(b_op.clone())
            };
            if let Some(n_op) = try_ord(lhs, rhs).or_else(|| try_ord(rhs, lhs)) {
                t_mul_local = Some(place.local);
                n_op_found = Some(n_op);
                break 'find_mul;
            }
        }
    }
    let t_mul_local = t_mul_local?;
    let n_op = n_op_found?;

    // Step B, Add(_t_mul, _j) gives _idx, commutative
    let mut t_idx_local: Option<Local> = None;
    'find_add: for &bb in &inner_lp.body {
        if bb == inner_lp.header { continue; }
        for stmt in &body.basic_blocks[bb].statements {
            let StatementKind::Assign(box (place, rvalue)) = &stmt.kind else { continue };
            if !place.projection.is_empty() { continue; }
            let Rvalue::BinaryOp(op, box (lhs, rhs)) = rvalue else { continue };
            if !matches!(op, BinOp::Add | BinOp::AddUnchecked) { continue; }
            let try_ord = |mul_op: &Operand<'tcx>, j_op: &Operand<'tcx>| -> bool {
                let mul_l = match local_op(mul_op) { Some(l) => l, None => return false };
                if inner_resolve(mul_l) != inner_resolve(t_mul_local) { return false; }
                let j_l = match local_op(j_op) { Some(l) => l, None => return false };
                same_as_inner_j(inner_resolve(j_l))
            };
            if try_ord(lhs, rhs) || try_ord(rhs, lhs) {
                t_idx_local = Some(place.local);
                break 'find_add;
            }
        }
    }
    let t_idx_local = t_idx_local?;

    // Step C, indexed read `_va = (*_a)[_idx]`
    let same_as_idx = |l: Local| inner_resolve(l) == inner_resolve(t_idx_local);
    let mut va_local: Option<Local> = None;
    let mut a_local: Option<Local> = None;
    'find_va: for &bb in &inner_lp.body {
        if bb == inner_lp.header { continue; }
        for stmt in &body.basic_blocks[bb].statements {
            let StatementKind::Assign(box (place, rvalue)) = &stmt.kind else { continue };
            if !place.projection.is_empty() { continue; }
            let Rvalue::Use(op) = rvalue else { continue };
            if let Operand::Copy(p) | Operand::Move(p) = op {
                if p.projection.len() >= 2
                    && matches!(p.projection[0], ProjectionElem::Deref)
                    && matches!(p.projection[1], ProjectionElem::Index(idx) if same_as_idx(idx))
                {
                    va_local = Some(place.local);
                    a_local = Some(inner_resolve(p.local));
                    break 'find_va;
                }
            }
        }
    }
    let va_local = va_local?;
    let a_local = a_local?;

    // Step D, j-indexed read `_vx = (*_x)[_j]`
    let mut vx_local: Option<Local> = None;
    let mut x_local: Option<Local> = None;
    'find_vx: for &bb in &inner_lp.body {
        if bb == inner_lp.header { continue; }
        for stmt in &body.basic_blocks[bb].statements {
            let StatementKind::Assign(box (place, rvalue)) = &stmt.kind else { continue };
            if !place.projection.is_empty() { continue; }
            let Rvalue::Use(op) = rvalue else { continue };
            if let Operand::Copy(p) | Operand::Move(p) = op {
                if p.projection.len() >= 2
                    && matches!(p.projection[0], ProjectionElem::Deref)
                    && matches!(p.projection[1],
                        ProjectionElem::Index(idx) if same_as_inner_j(inner_resolve(idx)))
                {
                    let src = inner_resolve(p.local);
                    if src == a_local { continue; }  // skip a's own indexed read (same i)
                    vx_local = Some(place.local);
                    x_local = Some(src);
                    break 'find_vx;
                }
            }
        }
    }
    let vx_local = vx_local?;
    let x_local = x_local?;

    // Step E, Mul(_va, _vx) gives _t_prod, commutative
    let mut t_prod_local: Option<Local> = None;
    'find_prod: for &bb in &inner_lp.body {
        if bb == inner_lp.header { continue; }
        for stmt in &body.basic_blocks[bb].statements {
            let StatementKind::Assign(box (place, rvalue)) = &stmt.kind else { continue };
            if !place.projection.is_empty() { continue; }
            let Rvalue::BinaryOp(op, box (lhs, rhs)) = rvalue else { continue };
            if !matches!(op, BinOp::Mul | BinOp::MulUnchecked) { continue; }
            let pair_match = |x: &Operand<'tcx>, y: &Operand<'tcx>| -> bool {
                let lx = match local_op(x) { Some(l) => l, None => return false };
                let ly = match local_op(y) { Some(l) => l, None => return false };
                inner_resolve(lx) == inner_resolve(va_local)
                    && inner_resolve(ly) == inner_resolve(vx_local)
            };
            if pair_match(lhs, rhs) || pair_match(rhs, lhs) {
                t_prod_local = Some(place.local);
                break 'find_prod;
            }
        }
    }
    let t_prod_local = t_prod_local?;

    // Step F, `_acc = Add(_acc, _t_prod)`, commutative
    let mut found_acc_update = false;
    'find_acc_upd: for &bb in &inner_lp.body {
        if bb == inner_lp.header { continue; }
        for stmt in &body.basic_blocks[bb].statements {
            let StatementKind::Assign(box (place, rvalue)) = &stmt.kind else { continue };
            if !place.projection.is_empty() { continue; }
            // Target must be acc
            if outer_resolve(place.local) != outer_resolve(acc_local) { continue; }
            let Rvalue::BinaryOp(op, box (lhs, rhs)) = rvalue else { continue };
            if !matches!(op, BinOp::Add | BinOp::AddUnchecked) { continue; }
            let try_ord = |a_op: &Operand<'tcx>, b_op: &Operand<'tcx>| -> bool {
                let la = match local_op(a_op) { Some(l) => l, None => return false };
                let lb = match local_op(b_op) { Some(l) => l, None => return false };
                outer_resolve(la) == outer_resolve(acc_local)
                    && inner_resolve(lb) == inner_resolve(t_prod_local)
            };
            if try_ord(lhs, rhs) || try_ord(rhs, lhs) {
                found_acc_update = true;
                break 'find_acc_upd;
            }
        }
    }
    if !found_acc_update { return None; }

    // After inner exit find `(*_y)[i] = _acc`
    let mut y_local: Option<Local> = None;
    'find_y: for &bb in &outer_lp.body {
        if inner_lp.body.contains(&bb) { continue; }
        if bb == outer_lp.header { continue; }
        for stmt in &body.basic_blocks[bb].statements {
            let StatementKind::Assign(box (place, rvalue)) = &stmt.kind else { continue };
            if !(place.projection.len() >= 2
                && matches!(place.projection[0], ProjectionElem::Deref)
                && matches!(place.projection[1],
                    ProjectionElem::Index(idx) if same_as_outer_i(outer_resolve(idx))))
            { continue; }
            // Rvalue must be Use of acc
            let Rvalue::Use(op) = rvalue else { continue };
            if let Operand::Copy(p) | Operand::Move(p) = op {
                if p.projection.is_empty()
                    && outer_resolve(p.local) == outer_resolve(acc_local)
                {
                    y_local = Some(outer_resolve(place.local));
                    break 'find_y;
                }
            }
        }
    }
    let y_local = y_local?;

    // y/a/x must be `fty` slice refs
    if ref_slice_float_ty(tcx, body.local_decls[y_local].ty, true) != Some(fty) { return None; }
    if ref_slice_float_ty(tcx, body.local_decls[a_local].ty, false) != Some(fty) { return None; }
    if ref_slice_float_ty(tcx, body.local_decls[x_local].ty, false) != Some(fty) { return None; }
    if y_local == a_local || y_local == x_local || a_local == x_local { return None; }

    // Find the unique out-of-loop predecessor of outer.header
    let preds = body.basic_blocks.predecessors();
    let mut outer_entry_bb: Option<BasicBlock> = None;
    for &pred in preds[outer_lp.header].iter() {
        if !outer_lp.body.contains(&pred) {
            if outer_entry_bb.replace(pred).is_some() { return None; }
        }
    }
    let outer_entry_bb = outer_entry_bb?;

    // Outer exit_bb comes from the shape
    let outer_exit_bb = outer_shape.exit_bb;

    // Recover m from Range Aggregate before preheader
    let mut pre_preheader: Option<BasicBlock> = None;
    for &pred in preds[outer_entry_bb].iter() {
        if outer_lp.body.contains(&pred) { continue; }
        if pre_preheader.replace(pred).is_some() { return None; }
    }
    let pre_preheader = pre_preheader?;
    let mut m_op: Option<Operand<'tcx>> = None;
    for stmt in &body.basic_blocks[pre_preheader].statements {
        let StatementKind::Assign(box (_, rvalue)) = &stmt.kind else { continue };
        let Rvalue::Aggregate(agg_kind, fields) = rvalue else { continue };
        let AggregateKind::Adt(adt_def_id, _, _, _, _) = &**agg_kind else { continue };
        if !tcx.def_path_str(*adt_def_id).ends_with("Range") { continue; }
        if fields.len() != 2 { continue; }
        let raw_end = fields[rustc_abi::FieldIdx::from_u32(1)].clone();
        // Resolve aliases to upstream live source
        let resolved = match &raw_end {
            Operand::Move(p) | Operand::Copy(p) if p.projection.is_empty() => {
                Operand::Copy(Place::from(outer_resolve(p.local)))
            }
            Operand::Constant(_) => raw_end.clone(),
            _ => raw_end,
        };
        m_op = Some(resolved);
    }
    let m_op = m_op?;
    // Same resolution for n_op
    let n_op = match n_op {
        Operand::Move(p) | Operand::Copy(p) if p.projection.is_empty() => {
            let root = outer_resolve(p.local);
            Operand::Copy(Place::from(root))
        }
        other => other,
    };

    Some(MatvecPlan {
        outer_entry_bb,
        outer_exit_bb,
        y_local,
        a_local,
        x_local,
        m_op,
        n_op,
        fty,
    })
}

// block_index_of removed; m_op uses bb_idx from iter_enumerated

fn apply_matvec_transform<'tcx>(
    tcx: TyCtxt<'tcx>,
    body: &mut mir::Body<'tcx>,
    plan: &MatvecPlan<'tcx>,
    helper_def_id: rustc_hir::def_id::DefId,
) -> bool {
    use rustc_middle::mir::*;
    use rustc_span::DUMMY_SP;
    use rustc_span::source_map::Spanned;

    let elem_ty = match plan.fty {
        FloatTy::F64 => tcx.types.f64,
    };
    #[allow(rustc::usage_of_qualified_ty)]
    let slice_ty = rustc_middle::ty::Ty::new_slice(tcx, elem_ty);
    #[allow(rustc::usage_of_qualified_ty)]
    let mut_slice_ref_ty = rustc_middle::ty::Ty::new_mut_ref(
        tcx, tcx.lifetimes.re_erased, slice_ty);

    // Reborrow y as `&mut [T]`.
    let y_temp = body.local_decls.push(LocalDecl::new(mut_slice_ref_ty, DUMMY_SP));
    let prelude = vec![Statement::new(
        SourceInfo::outermost(DUMMY_SP),
        StatementKind::Assign(Box::new((
            Place::from(y_temp),
            Rvalue::Ref(
                tcx.lifetimes.re_erased,
                BorrowKind::Mut { kind: MutBorrowKind::Default },
                Place {
                    local: plan.y_local,
                    projection: tcx.mk_place_elems(&[ProjectionElem::Deref]),
                },
            ),
        ))),
    )];

    let unit_dest = body.local_decls.push(LocalDecl::new(tcx.types.unit, DUMMY_SP));
    let helper_callee = Operand::function_handle(tcx, helper_def_id, std::iter::empty(), DUMMY_SP);
    // Helper signature: parallel_matvec(y, a, x, m, n).
    let new_term_kind = TerminatorKind::Call {
        func: helper_callee,
        args: Box::new([
            Spanned { node: Operand::Move(Place::from(y_temp)), span: DUMMY_SP },
            Spanned { node: Operand::Copy(Place::from(plan.a_local)), span: DUMMY_SP },
            Spanned { node: Operand::Copy(Place::from(plan.x_local)), span: DUMMY_SP },
            Spanned { node: plan.m_op.clone(), span: DUMMY_SP },
            Spanned { node: plan.n_op.clone(), span: DUMMY_SP },
        ]),
        destination: Place::from(unit_dest),
        target: Some(plan.outer_exit_bb),
        unwind: UnwindAction::Unreachable,
        call_source: CallSource::Misc,
        fn_span: DUMMY_SP,
    };
    let new_bb_data = BasicBlockData::new_stmts(
        prelude,
        Some(Terminator { source_info: SourceInfo::outermost(DUMMY_SP), kind: new_term_kind }),
        false,
    );
    let header = {
        let mut found: Option<BasicBlock> = None;
        body.basic_blocks[plan.outer_entry_bb].terminator().successors().for_each(|s| {
            if found.is_none() { found = Some(s); }
        });
        match found { Some(h) => h, None => return false }
    };
    let new_bb_idx = body.basic_blocks_mut().push(new_bb_data);
    let entry_term = body.basic_blocks_mut()[plan.outer_entry_bb].terminator_mut();
    entry_term.successors_mut(|succ| {
        if *succ == header { *succ = new_bb_idx; }
    });
    true
}

pub(crate) fn try_transform_slice_matvec_f64<'tcx>(
    tcx: TyCtxt<'tcx>,
    body: &mut mir::Body<'tcx>,
) -> usize {
    if !idiom_transform_enabled(tcx) { return 0; }
    if is_in_sysroot_crate(tcx, body.source.def_id()) { return 0; }
    let fn_name = tcx.def_path_str(body.source.def_id());
    if fn_name.contains("parallel_runtime") || fn_name.contains("parallel_matvec")
        || fn_name.contains("parallel_invoke") || fn_name.contains("parallel_axpy")
        || fn_name.contains("parallel_apply_")
    { return 0; }
    // Matvec gated on associative-float opt-in, safer contract
    if !is_associative_float_opted_in(tcx, body.source.def_id()) { return 0; }
    if parallel_pass_quick_skip(tcx, body, 6, true) { return 0; }
    fn lookup(tcx: TyCtxt<'_>, name: &str) -> Option<rustc_hir::def_id::DefId> {
        tcx.get_diagnostic_item(rustc_span::Symbol::intern(name))
    }
    let helper_f64 = lookup(tcx, "parallel_runtime_parallel_matvec_f64");
    if helper_f64.is_none() { return 0; }
    let loops = find_simple_loops(body);
    let mut plans: Vec<MatvecPlan<'tcx>> = Vec::new();
    // Try every (outer, inner) contained pair
    for (i, outer_lp) in loops.iter().enumerate() {
        for (j, inner_lp) in loops.iter().enumerate() {
            if i == j { continue; }
            if outer_lp.header == inner_lp.header { continue; }
            // Header-only containment; body sets miss late blocks
            if !outer_lp.body.contains(&inner_lp.header) { continue; }
            if let Some(p) = analyze_matvec_loops(tcx, body, outer_lp, inner_lp) {
                par_dump!(tcx,
                    "[PAR-IDIOM-MATVEC-CANDIDATE] fn={} outer_entry=bb{} y=_{} a=_{} x=_{} fty={:?}",
                    fn_name, p.outer_entry_bb.index(),
                    p.y_local.index(), p.a_local.index(), p.x_local.index(), p.fty,
                );
                plans.push(p);
                break; // one plan per outer
            }
        }
    }
    let mut count = 0;
    for plan in plans {
        let helper_did = helper_f64;
        let Some(helper_did) = helper_did else { continue };
        if apply_matvec_transform(tcx, body, &plan, helper_did) {
            par_dump!(tcx, "[PAR-IDIOM-MATVEC-APPLIED] fn={} outer_entry=bb{} fty={:?}",
                fn_name, plan.outer_entry_bb.index(), plan.fty);
            count += 1;
        }
    }
    count
}

// Multi-dim matchers (broadcast/axis-reduce/transpose) on matvec infra

#[derive(Debug, Clone)]
struct BroadcastPlan<'tcx> {
    outer_entry_bb: mir::BasicBlock,
    outer_exit_bb: mir::BasicBlock,
    a_local: mir::Local,
    b_local: mir::Local,
    m_op: mir::Operand<'tcx>,
    n_op: mir::Operand<'tcx>,
    fty: FloatTy,
}

fn analyze_broadcast_loops<'tcx>(
    tcx: TyCtxt<'tcx>,
    body: &mir::Body<'tcx>,
    outer_lp: &SimpleLoop,
    inner_lp: &SimpleLoop,
) -> Option<BroadcastPlan<'tcx>> {
    use rustc_middle::mir::*;
    let dbg = std::env::var("PARALLEL_BROADCAST_DEBUG").ok().as_deref() == Some("1");
    macro_rules! reject { ($($a:tt)*) => {{ if dbg { eprintln!("[BC-REJ] outer=bb{} inner=bb{}: {}", outer_lp.header.index(), inner_lp.header.index(), format!($($a)*)); } return None; }}; }
    if outer_lp.header == inner_lp.header { reject!("same header"); }
    if !outer_lp.body.contains(&inner_lp.header) { reject!("inner not nested"); }
    let outer_shape = match detect_range_loop_shape(tcx, body, outer_lp) {
        Some(s) => s, None => reject!("outer not range"),
    };
    let inner_shape = match detect_range_loop_shape(tcx, body, inner_lp) {
        Some(s) => s, None => reject!("inner not range"),
    };
    let outer_resolve = |l: Local| LoopShape::resolve(&outer_shape.alias_root, l);
    let inner_resolve = |l: Local| LoopShape::resolve(&inner_shape.alias_root, l);
    let same_as_outer_i = |l: Local| outer_shape.same_as_i(l);
    let same_as_inner_j = |l: Local| inner_shape.same_as_i(l);

    let local_op = |o: &Operand<'tcx>| -> Option<Local> {
        let p = match o {
            Operand::Copy(p) | Operand::Move(p) => p,
            _ => return None,
        };
        if !p.projection.is_empty() { return None; }
        Some(p.local)
    };

    // Detect i*n+j, matvec Steps A and B
    let mut t_mul_local: Option<Local> = None;
    let mut n_op_found: Option<Operand<'tcx>> = None;
    'find_mul: for &bb in &inner_lp.body {
        if bb == inner_lp.header { continue; }
        for stmt in &body.basic_blocks[bb].statements {
            let StatementKind::Assign(box (place, rvalue)) = &stmt.kind else { continue };
            if !place.projection.is_empty() { continue; }
            let Rvalue::BinaryOp(op, box (lhs, rhs)) = rvalue else { continue };
            if !matches!(op, BinOp::Mul | BinOp::MulUnchecked) { continue; }
            let try_ord = |a_op: &Operand<'tcx>, b_op: &Operand<'tcx>| -> Option<Operand<'tcx>> {
                let l = local_op(a_op)?;
                if !same_as_outer_i(outer_resolve(l)) { return None; }
                Some(b_op.clone())
            };
            if let Some(n_op) = try_ord(lhs, rhs).or_else(|| try_ord(rhs, lhs)) {
                t_mul_local = Some(place.local);
                n_op_found = Some(n_op);
                break 'find_mul;
            }
        }
    }
    let t_mul_local = t_mul_local?;
    let n_op = n_op_found?;
    let mut t_idx_local: Option<Local> = None;
    'find_add_idx: for &bb in &inner_lp.body {
        if bb == inner_lp.header { continue; }
        for stmt in &body.basic_blocks[bb].statements {
            let StatementKind::Assign(box (place, rvalue)) = &stmt.kind else { continue };
            if !place.projection.is_empty() { continue; }
            let Rvalue::BinaryOp(op, box (lhs, rhs)) = rvalue else { continue };
            if !matches!(op, BinOp::Add | BinOp::AddUnchecked) { continue; }
            let try_ord = |mul_op: &Operand<'tcx>, j_op: &Operand<'tcx>| -> bool {
                let mul_l = match local_op(mul_op) { Some(l) => l, None => return false };
                if inner_resolve(mul_l) != inner_resolve(t_mul_local) { return false; }
                let j_l = match local_op(j_op) { Some(l) => l, None => return false };
                same_as_inner_j(inner_resolve(j_l))
            };
            if try_ord(lhs, rhs) || try_ord(rhs, lhs) {
                t_idx_local = Some(place.local);
                break 'find_add_idx;
            }
        }
    }
    let t_idx_local = t_idx_local?;
    let same_as_idx = |l: Local| inner_resolve(l) == inner_resolve(t_idx_local);

    // Body pattern `a[i*n+j] += b[j]`
    let mut a_local: Option<Local> = None;
    let mut vb_local: Option<Local> = None;
    'find_aw: for &bb in &inner_lp.body {
        if bb == inner_lp.header { continue; }
        for stmt in &body.basic_blocks[bb].statements {
            let StatementKind::Assign(box (place, rvalue)) = &stmt.kind else { continue };
            if !(place.projection.len() >= 2
                && matches!(place.projection[0], ProjectionElem::Deref)
                && matches!(place.projection[1], ProjectionElem::Index(idx) if same_as_idx(idx)))
            { continue; }
            let Rvalue::BinaryOp(op, box (lhs, rhs)) = rvalue else { continue };
            if !matches!(op, BinOp::Add | BinOp::AddUnchecked) { continue; }
            let self_read = |o: &Operand<'tcx>, dst: Local| -> bool {
                let p = match o {
                    Operand::Copy(p) | Operand::Move(p) => p,
                    _ => return false,
                };
                if p.local != dst { return false; }
                if p.projection.len() < 2 { return false; }
                matches!(p.projection[0], ProjectionElem::Deref)
                    && matches!(p.projection[1], ProjectionElem::Index(idx) if same_as_idx(idx))
            };
            let other = |o: &Operand<'tcx>| -> Option<Local> { local_op(o) };
            let a_l = place.local;
            if self_read(lhs, a_l) {
                if let Some(vb) = other(rhs) {
                    a_local = Some(a_l);
                    vb_local = Some(vb);
                    break 'find_aw;
                }
            } else if self_read(rhs, a_l) {
                if let Some(vb) = other(lhs) {
                    a_local = Some(a_l);
                    vb_local = Some(vb);
                    break 'find_aw;
                }
            }
        }
    }
    let a_local = a_local?;
    let vb_local = vb_local?;

    // Find indexed read for vb_local from b[j].
    let vb_root = inner_resolve(vb_local);
    let mut b_local: Option<Local> = None;
    'find_bread: for &bb in &inner_lp.body {
        if bb == inner_lp.header { continue; }
        for stmt in &body.basic_blocks[bb].statements {
            let StatementKind::Assign(box (place, rvalue)) = &stmt.kind else { continue };
            if !place.projection.is_empty() { continue; }
            if inner_resolve(place.local) != vb_root { continue; }
            let Rvalue::Use(op) = rvalue else { continue };
            if let Operand::Copy(p) | Operand::Move(p) = op {
                if p.projection.len() >= 2
                    && matches!(p.projection[0], ProjectionElem::Deref)
                    && matches!(p.projection[1],
                        ProjectionElem::Index(idx) if same_as_inner_j(inner_resolve(idx)))
                {
                    b_local = Some(inner_resolve(p.local));
                    break 'find_bread;
                }
            }
        }
    }
    let b_local = b_local?;

    let a_resolved = outer_resolve(a_local);
    let a_ty = body.local_decls[a_resolved].ty;
    let fty = ref_slice_float_ty(tcx, a_ty, true)?;
    if ref_slice_float_ty(tcx, body.local_decls[b_local].ty, false) != Some(fty) { return None; }
    if a_resolved == b_local { return None; }

    let preds = body.basic_blocks.predecessors();
    let mut outer_entry_bb: Option<BasicBlock> = None;
    for &pred in preds[outer_lp.header].iter() {
        if !outer_lp.body.contains(&pred) {
            if outer_entry_bb.replace(pred).is_some() { return None; }
        }
    }
    let outer_entry_bb = outer_entry_bb?;
    let outer_exit_bb = outer_shape.exit_bb;

    // m extraction (same logic as matvec).
    let mut m_op: Option<Operand<'tcx>> = None;
    for (bb_idx, block) in body.basic_blocks.iter_enumerated() {
        if outer_lp.body.contains(&bb_idx) { continue; }
        for stmt in &block.statements {
            let StatementKind::Assign(box (_, rvalue)) = &stmt.kind else { continue };
            let Rvalue::Aggregate(agg_kind, fields) = rvalue else { continue };
            let AggregateKind::Adt(adt_def_id, _, _, _, _) = &**agg_kind else { continue };
            if !tcx.def_path_str(*adt_def_id).ends_with("Range") { continue; }
            if fields.len() != 2 { continue; }
            let raw_end = fields[rustc_abi::FieldIdx::from_u32(1)].clone();
            let resolved = match &raw_end {
                Operand::Move(p) | Operand::Copy(p) if p.projection.is_empty() => {
                    Operand::Copy(Place::from(outer_resolve(p.local)))
                }
                _ => raw_end,
            };
            m_op = Some(resolved);
        }
    }
    let m_op = m_op?;
    let n_op = match n_op {
        Operand::Move(p) | Operand::Copy(p) if p.projection.is_empty() => {
            Operand::Copy(Place::from(outer_resolve(p.local)))
        }
        other => other,
    };

    Some(BroadcastPlan {
        outer_entry_bb, outer_exit_bb,
        a_local: a_resolved, b_local, m_op, n_op, fty,
    })
}

fn apply_3arg_dim_transform<'tcx>(
    tcx: TyCtxt<'tcx>,
    body: &mut mir::Body<'tcx>,
    outer_entry_bb: mir::BasicBlock,
    outer_exit_bb: mir::BasicBlock,
    primary_local: mir::Local,
    primary_is_mut: bool,
    other_local: mir::Local,
    other_is_mut: bool,
    m_op: mir::Operand<'tcx>,
    n_op: mir::Operand<'tcx>,
    _fty: FloatTy,
    helper_def_id: rustc_hir::def_id::DefId,
) -> bool {
    use rustc_middle::mir::*;
    use rustc_span::DUMMY_SP;
    use rustc_span::source_map::Spanned;

    let elem_ty = tcx.types.f64;
    #[allow(rustc::usage_of_qualified_ty)]
    let slice_ty = rustc_middle::ty::Ty::new_slice(tcx, elem_ty);
    #[allow(rustc::usage_of_qualified_ty)]
    let mk_ref_ty = |is_mut: bool| {
        if is_mut {
            rustc_middle::ty::Ty::new_mut_ref(tcx, tcx.lifetimes.re_erased, slice_ty)
        } else {
            rustc_middle::ty::Ty::new_imm_ref(tcx, tcx.lifetimes.re_erased, slice_ty)
        }
    };
    let primary_ref_ty = mk_ref_ty(primary_is_mut);
    let primary_temp = body.local_decls.push(LocalDecl::new(primary_ref_ty, DUMMY_SP));
    let primary_borrow_kind = if primary_is_mut {
        BorrowKind::Mut { kind: MutBorrowKind::Default }
    } else {
        BorrowKind::Shared
    };
    let prelude = vec![Statement::new(
        SourceInfo::outermost(DUMMY_SP),
        StatementKind::Assign(Box::new((
            Place::from(primary_temp),
            Rvalue::Ref(
                tcx.lifetimes.re_erased,
                primary_borrow_kind,
                Place {
                    local: primary_local,
                    projection: tcx.mk_place_elems(&[ProjectionElem::Deref]),
                },
            ),
        ))),
    )];
    let _ = other_is_mut;
    let unit_dest = body.local_decls.push(LocalDecl::new(tcx.types.unit, DUMMY_SP));
    let helper_callee = Operand::function_handle(tcx, helper_def_id, std::iter::empty(), DUMMY_SP);
    let new_term_kind = TerminatorKind::Call {
        func: helper_callee,
        args: Box::new([
            Spanned { node: Operand::Move(Place::from(primary_temp)), span: DUMMY_SP },
            Spanned { node: Operand::Copy(Place::from(other_local)), span: DUMMY_SP },
            Spanned { node: m_op, span: DUMMY_SP },
            Spanned { node: n_op, span: DUMMY_SP },
        ]),
        destination: Place::from(unit_dest),
        target: Some(outer_exit_bb),
        unwind: UnwindAction::Unreachable,
        call_source: CallSource::Misc,
        fn_span: DUMMY_SP,
    };
    let new_bb_data = BasicBlockData::new_stmts(
        prelude,
        Some(Terminator { source_info: SourceInfo::outermost(DUMMY_SP), kind: new_term_kind }),
        false,
    );
    let header = {
        let mut found: Option<BasicBlock> = None;
        body.basic_blocks[outer_entry_bb].terminator().successors().for_each(|s| {
            if found.is_none() { found = Some(s); }
        });
        match found { Some(h) => h, None => return false }
    };
    let new_bb_idx = body.basic_blocks_mut().push(new_bb_data);
    let entry_term = body.basic_blocks_mut()[outer_entry_bb].terminator_mut();
    entry_term.successors_mut(|succ| {
        if *succ == header { *succ = new_bb_idx; }
    });
    true
}

pub(crate) fn try_transform_slice_broadcast_add_f64<'tcx>(
    tcx: TyCtxt<'tcx>,
    body: &mut mir::Body<'tcx>,
) -> usize {
    if !idiom_transform_enabled(tcx) { return 0; }
    if is_in_sysroot_crate(tcx, body.source.def_id()) { return 0; }
    let fn_name = tcx.def_path_str(body.source.def_id());
    if fn_name.contains("parallel_runtime") || fn_name.contains("parallel_broadcast")
        || fn_name.contains("parallel_invoke") || fn_name.contains("parallel_matvec")
    { return 0; }
    if !is_associative_float_opted_in(tcx, body.source.def_id()) { return 0; }
    if parallel_pass_quick_skip(tcx, body, 6, true) { return 0; }
    fn lookup(tcx: TyCtxt<'_>, name: &str) -> Option<rustc_hir::def_id::DefId> {
        tcx.get_diagnostic_item(rustc_span::Symbol::intern(name))
    }
    let helper_f64 = lookup(tcx, "parallel_runtime_parallel_broadcast_add_f64");
    if helper_f64.is_none() { return 0; }
    let loops = find_simple_loops(body);
    let mut plans: Vec<BroadcastPlan<'tcx>> = Vec::new();
    for (i, outer_lp) in loops.iter().enumerate() {
        for (j, inner_lp) in loops.iter().enumerate() {
            if i == j { continue; }
            if outer_lp.header == inner_lp.header { continue; }
            // Header-only containment; body sets miss late blocks
            if !outer_lp.body.contains(&inner_lp.header) { continue; }
            if let Some(p) = analyze_broadcast_loops(tcx, body, outer_lp, inner_lp) {
                par_dump!(tcx,
                    "[PAR-IDIOM-BROADCAST-CANDIDATE] fn={} outer_entry=bb{} a=_{} b=_{} fty={:?}",
                    fn_name, p.outer_entry_bb.index(), p.a_local.index(), p.b_local.index(), p.fty);
                plans.push(p);
                break;
            }
        }
    }
    let mut count = 0;
    for plan in plans {
        let helper_did = helper_f64;
        let Some(helper_did) = helper_did else { continue };
        if apply_3arg_dim_transform(tcx, body,
            plan.outer_entry_bb, plan.outer_exit_bb,
            plan.a_local, true, plan.b_local, false,
            plan.m_op, plan.n_op, plan.fty, helper_did)
        {
            par_dump!(tcx, "[PAR-IDIOM-BROADCAST-APPLIED] fn={} fty={:?}",
                fn_name, plan.fty);
            count += 1;
        }
    }
    count
}

#[derive(Debug, Clone)]
struct AxisReducePlan<'tcx> {
    outer_entry_bb: mir::BasicBlock,
    outer_exit_bb: mir::BasicBlock,
    out_local: mir::Local,
    a_local: mir::Local,
    m_op: mir::Operand<'tcx>,
    n_op: mir::Operand<'tcx>,
    fty: FloatTy,
}

fn analyze_axis_reduce_loops<'tcx>(
    tcx: TyCtxt<'tcx>,
    body: &mir::Body<'tcx>,
    outer_lp: &SimpleLoop,
    inner_lp: &SimpleLoop,
) -> Option<AxisReducePlan<'tcx>> {
    use rustc_middle::mir::*;
    if outer_lp.header == inner_lp.header { return None; }
    if !outer_lp.body.contains(&inner_lp.header) { return None; }
    let outer_shape = detect_range_loop_shape(tcx, body, outer_lp)?;
    let inner_shape = detect_range_loop_shape(tcx, body, inner_lp)?;
    let outer_resolve = |l: Local| LoopShape::resolve(&outer_shape.alias_root, l);
    let inner_resolve = |l: Local| LoopShape::resolve(&inner_shape.alias_root, l);
    let same_as_outer_i = |l: Local| outer_shape.same_as_i(l);
    let same_as_inner_j = |l: Local| inner_shape.same_as_i(l);

    let outer_some_arm = {
        let header = outer_lp.header;
        let TerminatorKind::Call { target: Some(post), .. } = &body.basic_blocks[header].terminator().kind
        else { return None; };
        let TerminatorKind::SwitchInt { targets, .. } = &body.basic_blocks[*post].terminator().kind
        else { return None; };
        let mut some: Option<BasicBlock> = None;
        for (val, t) in targets.iter() { if val == 1 { some = Some(t); } }
        some?
    };

    // acc init in outer some-arm
    let mut acc_local: Option<Local> = None;
    for stmt in &body.basic_blocks[outer_some_arm].statements {
        let StatementKind::Assign(box (place, rvalue)) = &stmt.kind else { continue };
        if !place.projection.is_empty() { continue; }
        let Rvalue::Use(op) = rvalue else { continue };
        let Operand::Constant(c) = op else { continue };
        let bits = match eval_const_bits(tcx, &c.const_) { Some(b) => b, None => continue };
        if bits != 0 { continue; }
        let lty = body.local_decls[place.local].ty;
        if lty == tcx.types.f64 {
            acc_local = Some(place.local);
        }
    }
    let acc_local = acc_local?;
    let acc_ty = body.local_decls[acc_local].ty;
    let fty = if acc_ty == tcx.types.f64 { FloatTy::F64 }
        else { return None; };

    let local_op = |o: &Operand<'tcx>| -> Option<Local> {
        let p = match o { Operand::Copy(p) | Operand::Move(p) => p, _ => return None };
        if !p.projection.is_empty() { return None; }
        Some(p.local)
    };

    // i*n+j detection (same as matvec).
    let mut t_mul_local: Option<Local> = None;
    let mut n_op_found: Option<Operand<'tcx>> = None;
    'find_mul: for &bb in &inner_lp.body {
        if bb == inner_lp.header { continue; }
        for stmt in &body.basic_blocks[bb].statements {
            let StatementKind::Assign(box (place, rvalue)) = &stmt.kind else { continue };
            if !place.projection.is_empty() { continue; }
            let Rvalue::BinaryOp(op, box (lhs, rhs)) = rvalue else { continue };
            if !matches!(op, BinOp::Mul | BinOp::MulUnchecked) { continue; }
            let try_ord = |a_op: &Operand<'tcx>, b_op: &Operand<'tcx>| -> Option<Operand<'tcx>> {
                let l = local_op(a_op)?;
                if !same_as_outer_i(outer_resolve(l)) { return None; }
                Some(b_op.clone())
            };
            if let Some(n_op) = try_ord(lhs, rhs).or_else(|| try_ord(rhs, lhs)) {
                t_mul_local = Some(place.local);
                n_op_found = Some(n_op);
                break 'find_mul;
            }
        }
    }
    let t_mul_local = t_mul_local?;
    let n_op = n_op_found?;
    let mut t_idx_local: Option<Local> = None;
    'find_add_idx: for &bb in &inner_lp.body {
        if bb == inner_lp.header { continue; }
        for stmt in &body.basic_blocks[bb].statements {
            let StatementKind::Assign(box (place, rvalue)) = &stmt.kind else { continue };
            if !place.projection.is_empty() { continue; }
            let Rvalue::BinaryOp(op, box (lhs, rhs)) = rvalue else { continue };
            if !matches!(op, BinOp::Add | BinOp::AddUnchecked) { continue; }
            let try_ord = |mul_op: &Operand<'tcx>, j_op: &Operand<'tcx>| -> bool {
                let mul_l = match local_op(mul_op) { Some(l) => l, None => return false };
                if inner_resolve(mul_l) != inner_resolve(t_mul_local) { return false; }
                let j_l = match local_op(j_op) { Some(l) => l, None => return false };
                same_as_inner_j(inner_resolve(j_l))
            };
            if try_ord(lhs, rhs) || try_ord(rhs, lhs) {
                t_idx_local = Some(place.local);
                break 'find_add_idx;
            }
        }
    }
    let t_idx_local = t_idx_local?;
    let same_as_idx = |l: Local| inner_resolve(l) == inner_resolve(t_idx_local);

    // Find indexed read `_va = (*_a)[idx]`.
    let mut va_local: Option<Local> = None;
    let mut a_local: Option<Local> = None;
    'find_va: for &bb in &inner_lp.body {
        if bb == inner_lp.header { continue; }
        for stmt in &body.basic_blocks[bb].statements {
            let StatementKind::Assign(box (place, rvalue)) = &stmt.kind else { continue };
            if !place.projection.is_empty() { continue; }
            let Rvalue::Use(op) = rvalue else { continue };
            if let Operand::Copy(p) | Operand::Move(p) = op {
                if p.projection.len() >= 2
                    && matches!(p.projection[0], ProjectionElem::Deref)
                    && matches!(p.projection[1], ProjectionElem::Index(idx) if same_as_idx(idx))
                {
                    va_local = Some(place.local);
                    a_local = Some(inner_resolve(p.local));
                    break 'find_va;
                }
            }
        }
    }
    let va_local = va_local?;
    let a_local = a_local?;

    // Find acc reduction `_acc = Add(_acc, _va)`
    let mut found = false;
    'find_acc: for &bb in &inner_lp.body {
        if bb == inner_lp.header { continue; }
        for stmt in &body.basic_blocks[bb].statements {
            let StatementKind::Assign(box (place, rvalue)) = &stmt.kind else { continue };
            if !place.projection.is_empty() { continue; }
            if outer_resolve(place.local) != outer_resolve(acc_local) { continue; }
            let Rvalue::BinaryOp(op, box (lhs, rhs)) = rvalue else { continue };
            if !matches!(op, BinOp::Add | BinOp::AddUnchecked) { continue; }
            let try_ord = |a_op: &Operand<'tcx>, b_op: &Operand<'tcx>| -> bool {
                let la = match local_op(a_op) { Some(l) => l, None => return false };
                let lb = match local_op(b_op) { Some(l) => l, None => return false };
                outer_resolve(la) == outer_resolve(acc_local)
                    && inner_resolve(lb) == inner_resolve(va_local)
            };
            if try_ord(lhs, rhs) || try_ord(rhs, lhs) {
                found = true;
                break 'find_acc;
            }
        }
    }
    if !found { return None; }

    // Find `(*_out)[i] = _acc` after inner exit.
    let mut out_local: Option<Local> = None;
    'find_out: for &bb in &outer_lp.body {
        if inner_lp.body.contains(&bb) { continue; }
        if bb == outer_lp.header { continue; }
        for stmt in &body.basic_blocks[bb].statements {
            let StatementKind::Assign(box (place, rvalue)) = &stmt.kind else { continue };
            if !(place.projection.len() >= 2
                && matches!(place.projection[0], ProjectionElem::Deref)
                && matches!(place.projection[1],
                    ProjectionElem::Index(idx) if same_as_outer_i(outer_resolve(idx))))
            { continue; }
            let Rvalue::Use(op) = rvalue else { continue };
            if let Operand::Copy(p) | Operand::Move(p) = op {
                if p.projection.is_empty()
                    && outer_resolve(p.local) == outer_resolve(acc_local)
                {
                    out_local = Some(outer_resolve(place.local));
                    break 'find_out;
                }
            }
        }
    }
    let out_local = out_local?;

    if ref_slice_float_ty(tcx, body.local_decls[out_local].ty, true) != Some(fty) { return None; }
    if ref_slice_float_ty(tcx, body.local_decls[a_local].ty, false) != Some(fty) { return None; }
    if out_local == a_local { return None; }

    let preds = body.basic_blocks.predecessors();
    let mut outer_entry_bb: Option<BasicBlock> = None;
    for &pred in preds[outer_lp.header].iter() {
        if !outer_lp.body.contains(&pred) {
            if outer_entry_bb.replace(pred).is_some() { return None; }
        }
    }
    let outer_entry_bb = outer_entry_bb?;
    let outer_exit_bb = outer_shape.exit_bb;

    let mut m_op: Option<Operand<'tcx>> = None;
    for (bb_idx, block) in body.basic_blocks.iter_enumerated() {
        if outer_lp.body.contains(&bb_idx) { continue; }
        for stmt in &block.statements {
            let StatementKind::Assign(box (_, rvalue)) = &stmt.kind else { continue };
            let Rvalue::Aggregate(agg_kind, fields) = rvalue else { continue };
            let AggregateKind::Adt(adt_def_id, _, _, _, _) = &**agg_kind else { continue };
            if !tcx.def_path_str(*adt_def_id).ends_with("Range") { continue; }
            if fields.len() != 2 { continue; }
            let raw_end = fields[rustc_abi::FieldIdx::from_u32(1)].clone();
            let resolved = match &raw_end {
                Operand::Move(p) | Operand::Copy(p) if p.projection.is_empty() => {
                    Operand::Copy(Place::from(outer_resolve(p.local)))
                }
                _ => raw_end,
            };
            m_op = Some(resolved);
        }
    }
    let m_op = m_op?;
    let n_op = match n_op {
        Operand::Move(p) | Operand::Copy(p) if p.projection.is_empty() => {
            Operand::Copy(Place::from(outer_resolve(p.local)))
        }
        other => other,
    };

    Some(AxisReducePlan {
        outer_entry_bb, outer_exit_bb,
        out_local, a_local, m_op, n_op, fty,
    })
}

pub(crate) fn try_transform_slice_axis_reduce_sum_f64<'tcx>(
    tcx: TyCtxt<'tcx>,
    body: &mut mir::Body<'tcx>,
) -> usize {
    if !idiom_transform_enabled(tcx) { return 0; }
    if is_in_sysroot_crate(tcx, body.source.def_id()) { return 0; }
    let fn_name = tcx.def_path_str(body.source.def_id());
    if fn_name.contains("parallel_runtime") || fn_name.contains("parallel_axis_reduce")
        || fn_name.contains("parallel_invoke") || fn_name.contains("parallel_matvec")
    { return 0; }
    if !is_associative_float_opted_in(tcx, body.source.def_id()) { return 0; }
    if parallel_pass_quick_skip(tcx, body, 6, true) { return 0; }
    fn lookup(tcx: TyCtxt<'_>, name: &str) -> Option<rustc_hir::def_id::DefId> {
        tcx.get_diagnostic_item(rustc_span::Symbol::intern(name))
    }
    let helper_f64 = lookup(tcx, "parallel_runtime_parallel_axis_reduce_sum_f64");
    if helper_f64.is_none() { return 0; }
    let loops = find_simple_loops(body);
    let mut plans: Vec<AxisReducePlan<'tcx>> = Vec::new();
    for (i, outer_lp) in loops.iter().enumerate() {
        for (j, inner_lp) in loops.iter().enumerate() {
            if i == j { continue; }
            if outer_lp.header == inner_lp.header { continue; }
            // Header-only containment; body sets miss late blocks
            if !outer_lp.body.contains(&inner_lp.header) { continue; }
            if let Some(p) = analyze_axis_reduce_loops(tcx, body, outer_lp, inner_lp) {
                par_dump!(tcx,
                    "[PAR-IDIOM-AXIS-REDUCE-CANDIDATE] fn={} outer_entry=bb{} out=_{} a=_{} fty={:?}",
                    fn_name, p.outer_entry_bb.index(),
                    p.out_local.index(), p.a_local.index(), p.fty);
                plans.push(p);
                break;
            }
        }
    }
    let mut count = 0;
    for plan in plans {
        let helper_did = helper_f64;
        let Some(helper_did) = helper_did else { continue };
        if apply_3arg_dim_transform(tcx, body,
            plan.outer_entry_bb, plan.outer_exit_bb,
            plan.out_local, true, plan.a_local, false,
            plan.m_op, plan.n_op, plan.fty, helper_did)
        {
            par_dump!(tcx, "[PAR-IDIOM-AXIS-REDUCE-APPLIED] fn={} fty={:?}",
                fn_name, plan.fty);
            count += 1;
        }
    }
    count
}

#[derive(Debug, Clone)]
struct TransposePlan<'tcx> {
    outer_entry_bb: mir::BasicBlock,
    outer_exit_bb: mir::BasicBlock,
    b_local: mir::Local,
    a_local: mir::Local,
    m_op: mir::Operand<'tcx>,
    n_op: mir::Operand<'tcx>,
    fty: FloatTy,
}

fn analyze_transpose_loops<'tcx>(
    tcx: TyCtxt<'tcx>,
    body: &mir::Body<'tcx>,
    outer_lp: &SimpleLoop,
    inner_lp: &SimpleLoop,
) -> Option<TransposePlan<'tcx>> {
    use rustc_middle::mir::*;
    if outer_lp.header == inner_lp.header { return None; }
    if !outer_lp.body.contains(&inner_lp.header) { return None; }
    let outer_shape = detect_range_loop_shape(tcx, body, outer_lp)?;
    let inner_shape = detect_range_loop_shape(tcx, body, inner_lp)?;
    let outer_resolve = |l: Local| LoopShape::resolve(&outer_shape.alias_root, l);
    let inner_resolve = |l: Local| LoopShape::resolve(&inner_shape.alias_root, l);
    let same_as_outer_i = |l: Local| outer_shape.same_as_i(l);
    let same_as_inner_j = |l: Local| inner_shape.same_as_i(l);

    let local_op = |o: &Operand<'tcx>| -> Option<Local> {
        let p = match o { Operand::Copy(p) | Operand::Move(p) => p, _ => return None };
        if !p.projection.is_empty() { return None; }
        Some(p.local)
    };

    // Read index `_idx_r = i*n + j`
    let mut t_mul_in: Option<Local> = None;
    let mut n_op_found: Option<Operand<'tcx>> = None;
    'find_mul_in: for &bb in &inner_lp.body {
        if bb == inner_lp.header { continue; }
        for stmt in &body.basic_blocks[bb].statements {
            let StatementKind::Assign(box (place, rvalue)) = &stmt.kind else { continue };
            if !place.projection.is_empty() { continue; }
            let Rvalue::BinaryOp(op, box (lhs, rhs)) = rvalue else { continue };
            if !matches!(op, BinOp::Mul | BinOp::MulUnchecked) { continue; }
            let try_ord = |a_op: &Operand<'tcx>, b_op: &Operand<'tcx>| -> Option<Operand<'tcx>> {
                let l = local_op(a_op)?;
                if !same_as_outer_i(outer_resolve(l)) { return None; }
                Some(b_op.clone())
            };
            if let Some(n_op) = try_ord(lhs, rhs).or_else(|| try_ord(rhs, lhs)) {
                t_mul_in = Some(place.local);
                n_op_found = Some(n_op);
                break 'find_mul_in;
            }
        }
    }
    let t_mul_in = t_mul_in?;
    let n_op = n_op_found?;
    let mut t_idx_r: Option<Local> = None;
    'find_add_idx_r: for &bb in &inner_lp.body {
        if bb == inner_lp.header { continue; }
        for stmt in &body.basic_blocks[bb].statements {
            let StatementKind::Assign(box (place, rvalue)) = &stmt.kind else { continue };
            if !place.projection.is_empty() { continue; }
            let Rvalue::BinaryOp(op, box (lhs, rhs)) = rvalue else { continue };
            if !matches!(op, BinOp::Add | BinOp::AddUnchecked) { continue; }
            let try_ord = |mul_op: &Operand<'tcx>, j_op: &Operand<'tcx>| -> bool {
                let mul_l = match local_op(mul_op) { Some(l) => l, None => return false };
                if inner_resolve(mul_l) != inner_resolve(t_mul_in) { return false; }
                let j_l = match local_op(j_op) { Some(l) => l, None => return false };
                same_as_inner_j(inner_resolve(j_l))
            };
            if try_ord(lhs, rhs) || try_ord(rhs, lhs) {
                t_idx_r = Some(place.local);
                break 'find_add_idx_r;
            }
        }
    }
    let t_idx_r = t_idx_r?;

    // Write index `_idx_w = j*m + i`
    let mut t_mul_w: Option<Local> = None;
    let mut m_op_found: Option<Operand<'tcx>> = None;
    'find_mul_w: for &bb in &inner_lp.body {
        if bb == inner_lp.header { continue; }
        for stmt in &body.basic_blocks[bb].statements {
            let StatementKind::Assign(box (place, rvalue)) = &stmt.kind else { continue };
            if !place.projection.is_empty() { continue; }
            let Rvalue::BinaryOp(op, box (lhs, rhs)) = rvalue else { continue };
            if !matches!(op, BinOp::Mul | BinOp::MulUnchecked) { continue; }
            let try_ord = |a_op: &Operand<'tcx>, b_op: &Operand<'tcx>| -> Option<Operand<'tcx>> {
                let l = local_op(a_op)?;
                if !same_as_inner_j(inner_resolve(l)) { return None; }
                Some(b_op.clone())
            };
            if let Some(m_op) = try_ord(lhs, rhs).or_else(|| try_ord(rhs, lhs)) {
                t_mul_w = Some(place.local);
                m_op_found = Some(m_op);
                break 'find_mul_w;
            }
        }
    }
    let t_mul_w = t_mul_w?;
    let m_op_inner = m_op_found?;
    let mut t_idx_w: Option<Local> = None;
    'find_add_idx_w: for &bb in &inner_lp.body {
        if bb == inner_lp.header { continue; }
        for stmt in &body.basic_blocks[bb].statements {
            let StatementKind::Assign(box (place, rvalue)) = &stmt.kind else { continue };
            if !place.projection.is_empty() { continue; }
            let Rvalue::BinaryOp(op, box (lhs, rhs)) = rvalue else { continue };
            if !matches!(op, BinOp::Add | BinOp::AddUnchecked) { continue; }
            let try_ord = |mul_op: &Operand<'tcx>, i_op: &Operand<'tcx>| -> bool {
                let mul_l = match local_op(mul_op) { Some(l) => l, None => return false };
                if inner_resolve(mul_l) != inner_resolve(t_mul_w) { return false; }
                let i_l = match local_op(i_op) { Some(l) => l, None => return false };
                same_as_outer_i(outer_resolve(i_l))
            };
            if try_ord(lhs, rhs) || try_ord(rhs, lhs) {
                t_idx_w = Some(place.local);
                break 'find_add_idx_w;
            }
        }
    }
    let t_idx_w = t_idx_w?;
    let same_as_idx_r = |l: Local| inner_resolve(l) == inner_resolve(t_idx_r);
    let same_as_idx_w = |l: Local| inner_resolve(l) == inner_resolve(t_idx_w);

    // Find `b[idx_w] = _va`, `_va = a[idx_r]`
    let mut va_local: Option<Local> = None;
    let mut a_local: Option<Local> = None;
    'find_va: for &bb in &inner_lp.body {
        if bb == inner_lp.header { continue; }
        for stmt in &body.basic_blocks[bb].statements {
            let StatementKind::Assign(box (place, rvalue)) = &stmt.kind else { continue };
            if !place.projection.is_empty() { continue; }
            let Rvalue::Use(op) = rvalue else { continue };
            if let Operand::Copy(p) | Operand::Move(p) = op {
                if p.projection.len() >= 2
                    && matches!(p.projection[0], ProjectionElem::Deref)
                    && matches!(p.projection[1], ProjectionElem::Index(idx) if same_as_idx_r(idx))
                {
                    va_local = Some(place.local);
                    a_local = Some(inner_resolve(p.local));
                    break 'find_va;
                }
            }
        }
    }
    let va_local = va_local?;
    let a_local = a_local?;

    let mut b_local: Option<Local> = None;
    'find_bw: for &bb in &inner_lp.body {
        if bb == inner_lp.header { continue; }
        for stmt in &body.basic_blocks[bb].statements {
            let StatementKind::Assign(box (place, rvalue)) = &stmt.kind else { continue };
            if !(place.projection.len() >= 2
                && matches!(place.projection[0], ProjectionElem::Deref)
                && matches!(place.projection[1], ProjectionElem::Index(idx) if same_as_idx_w(idx)))
            { continue; }
            let Rvalue::Use(op) = rvalue else { continue };
            if let Operand::Copy(p) | Operand::Move(p) = op {
                if p.projection.is_empty()
                    && inner_resolve(p.local) == inner_resolve(va_local)
                {
                    b_local = Some(outer_resolve(place.local));
                    break 'find_bw;
                }
            }
        }
    }
    let b_local = b_local?;

    let fty = ref_slice_float_ty(tcx, body.local_decls[b_local].ty, true)?;
    if ref_slice_float_ty(tcx, body.local_decls[a_local].ty, false) != Some(fty) { return None; }
    if b_local == a_local { return None; }

    let preds = body.basic_blocks.predecessors();
    let mut outer_entry_bb: Option<BasicBlock> = None;
    for &pred in preds[outer_lp.header].iter() {
        if !outer_lp.body.contains(&pred) {
            if outer_entry_bb.replace(pred).is_some() { return None; }
        }
    }
    let outer_entry_bb = outer_entry_bb?;
    let outer_exit_bb = outer_shape.exit_bb;

    // Transpose m is inner Mul's other operand
    let m_op = match m_op_inner {
        Operand::Move(p) | Operand::Copy(p) if p.projection.is_empty() => {
            Operand::Copy(Place::from(outer_resolve(p.local)))
        }
        other => other,
    };
    let n_op = match n_op {
        Operand::Move(p) | Operand::Copy(p) if p.projection.is_empty() => {
            Operand::Copy(Place::from(outer_resolve(p.local)))
        }
        other => other,
    };

    Some(TransposePlan {
        outer_entry_bb, outer_exit_bb,
        b_local, a_local, m_op, n_op, fty,
    })
}

pub(crate) fn try_transform_slice_transpose_f64<'tcx>(
    tcx: TyCtxt<'tcx>,
    body: &mut mir::Body<'tcx>,
) -> usize {
    if !idiom_transform_enabled(tcx) { return 0; }
    if is_in_sysroot_crate(tcx, body.source.def_id()) { return 0; }
    let fn_name = tcx.def_path_str(body.source.def_id());
    if fn_name.contains("parallel_runtime") || fn_name.contains("parallel_transpose")
        || fn_name.contains("parallel_invoke") || fn_name.contains("parallel_matvec")
    { return 0; }
    // Bit-identical, but gate for multi-dim consistency
    if !is_associative_float_opted_in(tcx, body.source.def_id()) { return 0; }
    if parallel_pass_quick_skip(tcx, body, 6, true) { return 0; }
    fn lookup(tcx: TyCtxt<'_>, name: &str) -> Option<rustc_hir::def_id::DefId> {
        tcx.get_diagnostic_item(rustc_span::Symbol::intern(name))
    }
    let helper_f64 = lookup(tcx, "parallel_runtime_parallel_transpose_f64");
    if helper_f64.is_none() { return 0; }
    let loops = find_simple_loops(body);
    let mut plans: Vec<TransposePlan<'tcx>> = Vec::new();
    for (i, outer_lp) in loops.iter().enumerate() {
        for (j, inner_lp) in loops.iter().enumerate() {
            if i == j { continue; }
            if outer_lp.header == inner_lp.header { continue; }
            // Header-only containment; body sets miss late blocks
            if !outer_lp.body.contains(&inner_lp.header) { continue; }
            if let Some(p) = analyze_transpose_loops(tcx, body, outer_lp, inner_lp) {
                par_dump!(tcx,
                    "[PAR-IDIOM-TRANSPOSE-CANDIDATE] fn={} outer_entry=bb{} b=_{} a=_{} fty={:?}",
                    fn_name, p.outer_entry_bb.index(),
                    p.b_local.index(), p.a_local.index(), p.fty);
                plans.push(p);
                break;
            }
        }
    }
    let mut count = 0;
    for plan in plans {
        let helper_did = helper_f64;
        let Some(helper_did) = helper_did else { continue };
        if apply_3arg_dim_transform(tcx, body,
            plan.outer_entry_bb, plan.outer_exit_bb,
            plan.b_local, true, plan.a_local, false,
            plan.m_op, plan.n_op, plan.fty, helper_did)
        {
            par_dump!(tcx, "[PAR-IDIOM-TRANSPOSE-APPLIED] fn={} fty={:?}",
                fn_name, plan.fty);
            count += 1;
        }
    }
    count
}

// Cross-fn matvec, `y[i] = USER_FN(&x[i*d..], w)`

#[derive(Debug, Clone)]
struct OuterDotCallPlan<'tcx> {
    outer_entry_bb: mir::BasicBlock,
    outer_exit_bb: mir::BasicBlock,
    y_local: mir::Local,
    x_local: mir::Local,
    w_local: mir::Local,
    m_op: mir::Operand<'tcx>,
    d_op: mir::Operand<'tcx>,
    fty: FloatTy,
}

fn fn_body_looks_like_dot<'tcx>(tcx: TyCtxt<'tcx>, did: rustc_hir::def_id::DefId) -> bool {
    use rustc_middle::mir::*;
    if !did.is_local() { return false; }
    if !tcx.is_mir_available(did) { return false; }
    let body = tcx.optimized_mir(did);
    for (_, block) in body.basic_blocks.iter_enumerated() {
        // Strong signal, zip-mul matcher already fired
        if let TerminatorKind::Call { func, .. } = &block.terminator().kind {
            if let Operand::Constant(c) = func {
                if let rustc_middle::ty::FnDef(callee_did, _) = c.const_.ty().kind() {
                    let path = tcx.def_path_str(*callee_did);
                    if path.contains("parallel_reduce_sum_slice_zip_mul")
                        || path.contains("parallel_reduce_sum_slice_zip_map")
                    {
                        return true;
                    }
                }
            }
        }
        // Weaker signal, any Mul BinaryOp in body
        for stmt in &block.statements {
            let StatementKind::Assign(box (_, rvalue)) = &stmt.kind else { continue };
            if let Rvalue::BinaryOp(op, _) = rvalue {
                if matches!(op, BinOp::Mul | BinOp::MulUnchecked) { return true; }
            }
        }
    }
    false
}

fn analyze_outer_dot_call_loop<'tcx>(
    tcx: TyCtxt<'tcx>,
    body: &mir::Body<'tcx>,
    outer_lp: &SimpleLoop,
) -> Option<OuterDotCallPlan<'tcx>> {
    use rustc_middle::mir::*;
    let outer_shape = detect_range_loop_shape(tcx, body, outer_lp)?;
    let outer_resolve = |l: Local| LoopShape::resolve(&outer_shape.alias_root, l);
    let same_as_outer_i = |l: Local| outer_shape.same_as_i(l);

    // Find user fn Call with dot-product signature
    for &bb in &outer_lp.body {
        if bb == outer_lp.header { continue; }
        let block = &body.basic_blocks[bb];
        let TerminatorKind::Call { func, args, destination, target: Some(target_bb), .. } =
            &block.terminator().kind
        else { continue };
        let Operand::Constant(c) = func else { continue };
        let rustc_middle::ty::FnDef(callee_did, gargs) = c.const_.ty().kind() else { continue };
        if !callee_did.is_local() { continue; }
        let path = tcx.def_path_str(*callee_did);
        // Filter out non-user fns
        if path.contains("Index") || path.contains("Iterator")
            || path.contains("IntoIterator") || path.contains("parallel_runtime")
            || path.contains("parallel_reduce") || path.contains("parallel_invoke")
            || path.contains("parallel_apply")
        { continue; }
        // Signature, 2 slice-ref args, float return
        let sig = tcx.fn_sig(*callee_did).instantiate(tcx, gargs);
        let sig = tcx.instantiate_bound_regions_with_erased(sig);
        if sig.inputs().len() < 2 { continue; }
        let ret_ty = sig.output();
        let fty = if ret_ty == tcx.types.f64 { FloatTy::F64 }
            else { continue };
        let arg0_ty = sig.inputs()[0];
        let arg1_ty = sig.inputs()[1];
        if ref_slice_float_ty(tcx, arg0_ty, false) != Some(fty) { continue; }
        if ref_slice_float_ty(tcx, arg1_ty, false) != Some(fty) { continue; }
        // Verify body looks like a dot product.
        if !fn_body_looks_like_dot(tcx, *callee_did) { continue; }
        // Extract the two args' source Locals.
        let arg0_local = match &args[0].node {
            Operand::Move(p) | Operand::Copy(p) if p.projection.is_empty() => p.local,
            _ => continue,
        };
        let arg1_local = match &args[1].node {
            Operand::Move(p) | Operand::Copy(p) if p.projection.is_empty() => p.local,
            _ => continue,
        };
        // arg0 row slice `x[i*d..]`, arg1 reborrowed w
        let row_root = outer_resolve(arg0_local);
        let w_root = outer_resolve(arg1_local);
        // Find the Index::index Call producing row_root
        let local_op_of = |o: &Operand<'tcx>| -> Option<Local> {
            let p = match o {
                Operand::Copy(p) | Operand::Move(p) => p,
                _ => return None,
            };
            if !p.projection.is_empty() { return None; }
            Some(p.local)
        };
        let mut x_local: Option<Local> = None;
        let mut d_op: Option<Operand<'tcx>> = None;
        let mut found_index_call = false;
        for &cbb in &outer_lp.body {
            let cblock = &body.basic_blocks[cbb];
            let TerminatorKind::Call { func: cfunc, args: cargs, destination: cdest, .. } =
                &cblock.terminator().kind
            else { continue };
            if !cdest.projection.is_empty() { continue; }
            // dest must trace forward to row_root.
            if outer_resolve(cdest.local) != row_root { continue; }
            let Operand::Constant(cc) = cfunc else { continue };
            let rustc_middle::ty::FnDef(cdid, _) = cc.const_.ty().kind() else { continue };
            let cpath = tcx.def_path_str(*cdid);
            // Match `<[T] as Index<Range<...>>>::index`.
            if !cpath.contains("Index") || !cpath.ends_with("::index") { continue; }
            if cargs.len() != 2 { continue; }
            // arg0 reborrow of x, arg1 the Range
            let x_arg = local_op_of(&cargs[0].node)?;
            let range_arg = local_op_of(&cargs[1].node)?;
            x_local = Some(outer_resolve(x_arg));
            // Find Range Aggregate producing range_arg in cblock
            let range_root = outer_resolve(range_arg);
            for stmt in &cblock.statements {
                let StatementKind::Assign(box (place, rvalue)) = &stmt.kind else { continue };
                if !place.projection.is_empty() { continue; }
                if outer_resolve(place.local) != range_root { continue; }
                let Rvalue::Aggregate(agg_kind, fields) = rvalue else { continue };
                let AggregateKind::Adt(adt_def_id, _, _, _, _) = &**agg_kind else { continue };
                if !tcx.def_path_str(*adt_def_id).ends_with("Range") { continue; }
                if fields.len() != 2 { continue; }
                // Range start = Mul(i, d). Find it.
                let start_op = fields[rustc_abi::FieldIdx::from_u32(0)].clone();
                let start_local = local_op_of(&start_op)?;
                // Trace start_local back to Mul(i, d)
                for s2 in &cblock.statements {
                    let StatementKind::Assign(box (p2, r2)) = &s2.kind else { continue };
                    if !p2.projection.is_empty() { continue; }
                    if outer_resolve(p2.local) != outer_resolve(start_local) { continue; }
                    let Rvalue::BinaryOp(op, box (lhs, rhs)) = r2 else { continue };
                    if !matches!(op, BinOp::Mul | BinOp::MulUnchecked) { continue; }
                    let try_ord = |a: &Operand<'tcx>, b: &Operand<'tcx>| -> Option<Operand<'tcx>> {
                        let l = local_op_of(a)?;
                        if !same_as_outer_i(outer_resolve(l)) { return None; }
                        Some(b.clone())
                    };
                    if let Some(d_o) = try_ord(lhs, rhs).or_else(|| try_ord(rhs, lhs)) {
                        // Resolve the d operand through outer aliases.
                        let resolved_d = match &d_o {
                            Operand::Move(p) | Operand::Copy(p) if p.projection.is_empty() => {
                                Operand::Copy(Place::from(outer_resolve(p.local)))
                            }
                            _ => d_o,
                        };
                        d_op = Some(resolved_d);
                    }
                }
            }
            found_index_call = true;
            break;
        }
        if !found_index_call { continue; }
        let x_local = x_local?;
        let d_op = d_op?;
        let w_local = w_root;
        // Find `(*_y)[_i] = result` from target_bb onward
        let dest_local = destination.local;
        let mut y_local: Option<Local> = None;
        // Walk forward from target_bb within outer body
        let mut to_visit = vec![*target_bb];
        let mut visited = std::collections::BTreeSet::<BasicBlock>::new();
        while let Some(vbb) = to_visit.pop() {
            if !outer_lp.body.contains(&vbb) { continue; }
            if !visited.insert(vbb) { continue; }
            let vblock = &body.basic_blocks[vbb];
            for stmt in &vblock.statements {
                let StatementKind::Assign(box (place, rvalue)) = &stmt.kind else { continue };
                if !(place.projection.len() >= 2
                    && matches!(place.projection[0], ProjectionElem::Deref)
                    && matches!(place.projection[1], ProjectionElem::Index(idx) if same_as_outer_i(outer_resolve(idx))))
                { continue; }
                let Rvalue::Use(op) = rvalue else { continue };
                if let Operand::Copy(p) | Operand::Move(p) = op {
                    if p.projection.is_empty()
                        && outer_resolve(p.local) == outer_resolve(dest_local)
                    {
                        y_local = Some(outer_resolve(place.local));
                    }
                }
            }
            if y_local.is_some() { break; }
            for s in vblock.terminator().successors() {
                if outer_lp.body.contains(&s) { to_visit.push(s); }
            }
        }
        let y_local = y_local?;
        // Type checks.
        if ref_slice_float_ty(tcx, body.local_decls[y_local].ty, true) != Some(fty) { continue; }
        if ref_slice_float_ty(tcx, body.local_decls[x_local].ty, false) != Some(fty) { continue; }
        if ref_slice_float_ty(tcx, body.local_decls[w_local].ty, false) != Some(fty) { continue; }
        if y_local == x_local || y_local == w_local || x_local == w_local { continue; }
        // m_op from outer Range bound before outer_entry
        let preds = body.basic_blocks.predecessors();
        let mut outer_entry_bb: Option<BasicBlock> = None;
        for &pred in preds[outer_lp.header].iter() {
            if !outer_lp.body.contains(&pred) {
                if outer_entry_bb.replace(pred).is_some() { return None; }
            }
        }
        let outer_entry_bb = outer_entry_bb?;
        let outer_exit_bb = outer_shape.exit_bb;
        let mut pre_preheader: Option<BasicBlock> = None;
        for &pred in preds[outer_entry_bb].iter() {
            if outer_lp.body.contains(&pred) { continue; }
            if pre_preheader.replace(pred).is_some() { return None; }
        }
        let pre_preheader = pre_preheader?;
        let mut m_op: Option<Operand<'tcx>> = None;
        for stmt in &body.basic_blocks[pre_preheader].statements {
            let StatementKind::Assign(box (_, rvalue)) = &stmt.kind else { continue };
            let Rvalue::Aggregate(agg_kind, fields) = rvalue else { continue };
            let AggregateKind::Adt(adt_def_id, _, _, _, _) = &**agg_kind else { continue };
            if !tcx.def_path_str(*adt_def_id).ends_with("Range") { continue; }
            if fields.len() != 2 { continue; }
            let raw_end = fields[rustc_abi::FieldIdx::from_u32(1)].clone();
            let resolved = match &raw_end {
                Operand::Move(p) | Operand::Copy(p) if p.projection.is_empty() => {
                    Operand::Copy(Place::from(outer_resolve(p.local)))
                }
                _ => raw_end,
            };
            m_op = Some(resolved);
        }
        let m_op = m_op?;
        return Some(OuterDotCallPlan {
            outer_entry_bb, outer_exit_bb,
            y_local, x_local, w_local,
            m_op, d_op, fty,
        });
    }
    None
}

pub(crate) fn try_transform_outer_dot_call_matvec_f64<'tcx>(
    tcx: TyCtxt<'tcx>,
    body: &mut mir::Body<'tcx>,
) -> usize {
    if !idiom_transform_enabled(tcx) { return 0; }
    if is_in_sysroot_crate(tcx, body.source.def_id()) { return 0; }
    let fn_name = tcx.def_path_str(body.source.def_id());
    if fn_name.contains("parallel_runtime") || fn_name.contains("parallel_matvec")
        || fn_name.contains("parallel_invoke") || fn_name.contains("predict_dot")
    { return 0; }
    if !is_associative_float_opted_in(tcx, body.source.def_id()) { return 0; }
    if parallel_pass_quick_skip(tcx, body, 4, true) { return 0; }
    fn lookup(tcx: TyCtxt<'_>, name: &str) -> Option<rustc_hir::def_id::DefId> {
        tcx.get_diagnostic_item(rustc_span::Symbol::intern(name))
    }
    let helper_f64 = lookup(tcx, "parallel_runtime_parallel_matvec_f64");
    if helper_f64.is_none() { return 0; }
    let loops = find_simple_loops(body);
    let mut plans: Vec<OuterDotCallPlan<'tcx>> = Vec::new();
    for outer_lp in &loops {
        if let Some(p) = analyze_outer_dot_call_loop(tcx, body, outer_lp) {
            par_dump!(tcx,
                "[PAR-IDIOM-OUTER-DOT-CALL-CANDIDATE] fn={} outer_entry=bb{} y=_{} x=_{} w=_{} fty={:?}",
                fn_name, p.outer_entry_bb.index(),
                p.y_local.index(), p.x_local.index(), p.w_local.index(), p.fty);
            plans.push(p);
        }
    }
    let mut count = 0;
    for plan in plans {
        let helper_did = helper_f64;
        let Some(helper_did) = helper_did else { continue };
        // Reuse matvec's apply path. Build a MatvecPlan-equivalent.
        let mvp = MatvecPlan {
            outer_entry_bb: plan.outer_entry_bb,
            outer_exit_bb: plan.outer_exit_bb,
            y_local: plan.y_local,
            a_local: plan.x_local,
            x_local: plan.w_local,
            m_op: plan.m_op,
            n_op: plan.d_op,
            fty: plan.fty,
        };
        if apply_matvec_transform(tcx, body, &mvp, helper_did) {
            par_dump!(tcx, "[PAR-IDIOM-OUTER-DOT-CALL-APPLIED] fn={} fty={:?}",
                fn_name, plan.fty);
            count += 1;
        }
    }
    count
}

// Conv1d matcher, `y[i] += x[i+k] * w[k]`

#[derive(Debug, Clone)]
struct Conv1dPlan<'tcx> {
    outer_entry_bb: mir::BasicBlock,
    outer_exit_bb: mir::BasicBlock,
    y_local: mir::Local,
    x_local: mir::Local,
    w_local: mir::Local,
    /// `n` = full input length
    #[allow(dead_code)]
    n_op: mir::Operand<'tcx>,
    ks_op: mir::Operand<'tcx>,
    fty: FloatTy,
}

fn analyze_conv1d_loops<'tcx>(
    tcx: TyCtxt<'tcx>,
    body: &mir::Body<'tcx>,
    outer_lp: &SimpleLoop,
    inner_lp: &SimpleLoop,
) -> Option<Conv1dPlan<'tcx>> {
    use rustc_middle::mir::*;
    if outer_lp.header == inner_lp.header { return None; }
    if !outer_lp.body.contains(&inner_lp.header) { return None; }
    let outer_shape = detect_range_loop_shape(tcx, body, outer_lp)?;
    let inner_shape = detect_range_loop_shape(tcx, body, inner_lp)?;
    let outer_resolve = |l: Local| LoopShape::resolve(&outer_shape.alias_root, l);
    let inner_resolve = |l: Local| LoopShape::resolve(&inner_shape.alias_root, l);
    let same_as_outer_i = |l: Local| outer_shape.same_as_i(l);
    let same_as_inner_k = |l: Local| inner_shape.same_as_i(l);
    let outer_some_arm = {
        let TerminatorKind::Call { target: Some(post), .. } = &body.basic_blocks[outer_lp.header].terminator().kind else { return None; };
        let TerminatorKind::SwitchInt { targets, .. } = &body.basic_blocks[*post].terminator().kind else { return None; };
        let mut some: Option<BasicBlock> = None;
        for (val, t) in targets.iter() { if val == 1 { some = Some(t); } }
        some?
    };
    // acc init in outer some-arm
    let mut acc_local: Option<Local> = None;
    for stmt in &body.basic_blocks[outer_some_arm].statements {
        let StatementKind::Assign(box (place, rvalue)) = &stmt.kind else { continue };
        if !place.projection.is_empty() { continue; }
        let Rvalue::Use(op) = rvalue else { continue };
        let Operand::Constant(c) = op else { continue };
        let Some(bits) = eval_const_bits(tcx, &c.const_) else { continue };
        if bits != 0 { continue; }
        let lty = body.local_decls[place.local].ty;
        if lty == tcx.types.f64 { acc_local = Some(place.local); }
    }
    let acc_local = acc_local?;
    let acc_ty = body.local_decls[acc_local].ty;
    let fty = if acc_ty == tcx.types.f64 { FloatTy::F64 } else { return None; };
    let local_op = |o: &Operand<'tcx>| -> Option<Local> {
        let p = match o { Operand::Copy(p) | Operand::Move(p) => p, _ => return None };
        if !p.projection.is_empty() { return None; } Some(p.local)
    };
    // Find Add(i_alias, k_alias) → t_idx
    let mut t_idx_local: Option<Local> = None;
    'find_add: for &bb in &inner_lp.body {
        if bb == inner_lp.header { continue; }
        for stmt in &body.basic_blocks[bb].statements {
            let StatementKind::Assign(box (place, rvalue)) = &stmt.kind else { continue };
            if !place.projection.is_empty() { continue; }
            let Rvalue::BinaryOp(op, box (lhs, rhs)) = rvalue else { continue };
            if !matches!(op, BinOp::Add | BinOp::AddUnchecked) { continue; }
            let try_ord = |i_op: &Operand<'tcx>, k_op: &Operand<'tcx>| -> bool {
                let il = match local_op(i_op) { Some(l) => l, None => return false };
                if !same_as_outer_i(outer_resolve(il)) { return false; }
                let kl = match local_op(k_op) { Some(l) => l, None => return false };
                same_as_inner_k(inner_resolve(kl))
            };
            if try_ord(lhs, rhs) || try_ord(rhs, lhs) {
                t_idx_local = Some(place.local);
                break 'find_add;
            }
        }
    }
    let t_idx_local = t_idx_local?;
    let same_as_idx = |l: Local| inner_resolve(l) == inner_resolve(t_idx_local);
    // Find _vx = (*_x)[t_idx]
    let mut vx_local: Option<Local> = None;
    let mut x_local: Option<Local> = None;
    'find_vx: for &bb in &inner_lp.body {
        if bb == inner_lp.header { continue; }
        for stmt in &body.basic_blocks[bb].statements {
            let StatementKind::Assign(box (place, rvalue)) = &stmt.kind else { continue };
            if !place.projection.is_empty() { continue; }
            let Rvalue::Use(op) = rvalue else { continue };
            if let Operand::Copy(p) | Operand::Move(p) = op {
                if p.projection.len() >= 2
                    && matches!(p.projection[0], ProjectionElem::Deref)
                    && matches!(p.projection[1], ProjectionElem::Index(idx) if same_as_idx(idx))
                {
                    vx_local = Some(place.local);
                    x_local = Some(inner_resolve(p.local));
                    break 'find_vx;
                }
            }
        }
    }
    let vx_local = vx_local?;
    let x_local = x_local?;
    // Find _vw = (*_w)[k]
    let mut vw_local: Option<Local> = None;
    let mut w_local: Option<Local> = None;
    'find_vw: for &bb in &inner_lp.body {
        if bb == inner_lp.header { continue; }
        for stmt in &body.basic_blocks[bb].statements {
            let StatementKind::Assign(box (place, rvalue)) = &stmt.kind else { continue };
            if !place.projection.is_empty() { continue; }
            let Rvalue::Use(op) = rvalue else { continue };
            if let Operand::Copy(p) | Operand::Move(p) = op {
                if p.projection.len() >= 2
                    && matches!(p.projection[0], ProjectionElem::Deref)
                    && matches!(p.projection[1], ProjectionElem::Index(idx) if same_as_inner_k(inner_resolve(idx)))
                {
                    let src = inner_resolve(p.local);
                    // Skip x slice read, want w
                    if x_local == src { continue; }
                    vw_local = Some(place.local);
                    w_local = Some(src);
                    break 'find_vw;
                }
            }
        }
    }
    let vw_local = vw_local?;
    let w_local = w_local?;
    // Find Mul(_vx, _vw) → t_prod
    let mut t_prod: Option<Local> = None;
    'find_prod: for &bb in &inner_lp.body {
        if bb == inner_lp.header { continue; }
        for stmt in &body.basic_blocks[bb].statements {
            let StatementKind::Assign(box (place, rvalue)) = &stmt.kind else { continue };
            if !place.projection.is_empty() { continue; }
            let Rvalue::BinaryOp(op, box (lhs, rhs)) = rvalue else { continue };
            if !matches!(op, BinOp::Mul | BinOp::MulUnchecked) { continue; }
            let pair = |x: &Operand<'tcx>, y: &Operand<'tcx>| -> bool {
                let lx = match local_op(x) { Some(l) => l, None => return false };
                let ly = match local_op(y) { Some(l) => l, None => return false };
                inner_resolve(lx) == inner_resolve(vx_local)
                    && inner_resolve(ly) == inner_resolve(vw_local)
            };
            if pair(lhs, rhs) || pair(rhs, lhs) { t_prod = Some(place.local); break 'find_prod; }
        }
    }
    let t_prod = t_prod?;
    // Verify Add(_acc, _t_prod) writes back to _acc
    let mut found_acc_upd = false;
    'find_acc_upd: for &bb in &inner_lp.body {
        if bb == inner_lp.header { continue; }
        for stmt in &body.basic_blocks[bb].statements {
            let StatementKind::Assign(box (place, rvalue)) = &stmt.kind else { continue };
            if !place.projection.is_empty() { continue; }
            if outer_resolve(place.local) != outer_resolve(acc_local) { continue; }
            let Rvalue::BinaryOp(op, box (lhs, rhs)) = rvalue else { continue };
            if !matches!(op, BinOp::Add | BinOp::AddUnchecked) { continue; }
            let pair = |a: &Operand<'tcx>, b: &Operand<'tcx>| -> bool {
                let la = match local_op(a) { Some(l) => l, None => return false };
                let lb = match local_op(b) { Some(l) => l, None => return false };
                outer_resolve(la) == outer_resolve(acc_local)
                    && inner_resolve(lb) == inner_resolve(t_prod)
            };
            if pair(lhs, rhs) || pair(rhs, lhs) { found_acc_upd = true; break 'find_acc_upd; }
        }
    }
    if !found_acc_upd { return None; }
    // After inner exit: (*_y)[i] = _acc
    let mut y_local: Option<Local> = None;
    'find_y: for &bb in &outer_lp.body {
        if inner_lp.body.contains(&bb) { continue; }
        if bb == outer_lp.header { continue; }
        for stmt in &body.basic_blocks[bb].statements {
            let StatementKind::Assign(box (place, rvalue)) = &stmt.kind else { continue };
            if !(place.projection.len() >= 2
                && matches!(place.projection[0], ProjectionElem::Deref)
                && matches!(place.projection[1], ProjectionElem::Index(idx) if same_as_outer_i(outer_resolve(idx))))
            { continue; }
            let Rvalue::Use(op) = rvalue else { continue };
            if let Operand::Copy(p) | Operand::Move(p) = op {
                if p.projection.is_empty() && outer_resolve(p.local) == outer_resolve(acc_local) {
                    y_local = Some(outer_resolve(place.local));
                    break 'find_y;
                }
            }
        }
    }
    let y_local = y_local?;
    if ref_slice_float_ty(tcx, body.local_decls[y_local].ty, true) != Some(fty) { return None; }
    if ref_slice_float_ty(tcx, body.local_decls[x_local].ty, false) != Some(fty) { return None; }
    if ref_slice_float_ty(tcx, body.local_decls[w_local].ty, false) != Some(fty) { return None; }
    if y_local == x_local || y_local == w_local || x_local == w_local { return None; }
    let preds = body.basic_blocks.predecessors();
    let mut outer_entry_bb: Option<BasicBlock> = None;
    for &pred in preds[outer_lp.header].iter() {
        if !outer_lp.body.contains(&pred) {
            if outer_entry_bb.replace(pred).is_some() { return None; }
        }
    }
    let outer_entry_bb = outer_entry_bb?;
    let outer_exit_bb = outer_shape.exit_bb;
    // Don't decode outer bound; pass runtime lens
    let mut ks_op: Option<Operand<'tcx>> = None;
    for (bb_idx, block) in body.basic_blocks.iter_enumerated() {
        if !outer_lp.body.contains(&bb_idx) { continue; }
        // INNER Range is constructed inside outer body
        if inner_lp.body.contains(&bb_idx) { continue; }
        for stmt in &block.statements {
            let StatementKind::Assign(box (_, rvalue)) = &stmt.kind else { continue };
            let Rvalue::Aggregate(agg_kind, fields) = rvalue else { continue };
            let AggregateKind::Adt(adt_def_id, _, _, _, _) = &**agg_kind else { continue };
            if !tcx.def_path_str(*adt_def_id).ends_with("Range") { continue; }
            if fields.len() != 2 { continue; }
            let raw_end = fields[rustc_abi::FieldIdx::from_u32(1)].clone();
            let resolved = match &raw_end {
                Operand::Move(p) | Operand::Copy(p) if p.projection.is_empty() => {
                    Operand::Copy(Place::from(outer_resolve(p.local)))
                }
                _ => raw_end,
            };
            ks_op = Some(resolved);
        }
    }
    let ks_op = ks_op?;
    // n_op is x.len() via PtrMetadata in apply
    Some(Conv1dPlan {
        outer_entry_bb, outer_exit_bb,
        y_local, x_local, w_local,
        n_op: Operand::Copy(Place::from(x_local)),  // placeholder; apply will compute x.len()
        ks_op,
        fty,
    })
}

fn apply_conv1d_transform<'tcx>(
    tcx: TyCtxt<'tcx>,
    body: &mut mir::Body<'tcx>,
    plan: &Conv1dPlan<'tcx>,
    helper_def_id: rustc_hir::def_id::DefId,
) -> bool {
    use rustc_middle::mir::*;
    use rustc_span::DUMMY_SP;
    use rustc_span::source_map::Spanned;
    let _ = plan.fty;
    let elem_ty = tcx.types.f64;
    #[allow(rustc::usage_of_qualified_ty)]
    let slice_ty = rustc_middle::ty::Ty::new_slice(tcx, elem_ty);
    #[allow(rustc::usage_of_qualified_ty)]
    let mut_slice_ref_ty = rustc_middle::ty::Ty::new_mut_ref(tcx, tcx.lifetimes.re_erased, slice_ty);
    let y_temp = body.local_decls.push(LocalDecl::new(mut_slice_ref_ty, DUMMY_SP));
    let n_temp = body.local_decls.push(LocalDecl::new(tcx.types.usize, DUMMY_SP));
    let prelude = vec![
        Statement::new(SourceInfo::outermost(DUMMY_SP), StatementKind::Assign(Box::new((
            Place::from(y_temp),
            Rvalue::Ref(tcx.lifetimes.re_erased, BorrowKind::Mut { kind: MutBorrowKind::Default },
                Place { local: plan.y_local, projection: tcx.mk_place_elems(&[ProjectionElem::Deref]) }),
        )))),
        // n = x.len() via PtrMetadata(x).
        Statement::new(SourceInfo::outermost(DUMMY_SP), StatementKind::Assign(Box::new((
            Place::from(n_temp),
            Rvalue::UnaryOp(UnOp::PtrMetadata, Operand::Copy(Place::from(plan.x_local))),
        )))),
    ];
    let unit_dest = body.local_decls.push(LocalDecl::new(tcx.types.unit, DUMMY_SP));
    let helper_callee = Operand::function_handle(tcx, helper_def_id, std::iter::empty(), DUMMY_SP);
    let new_term_kind = TerminatorKind::Call {
        func: helper_callee,
        args: Box::new([
            Spanned { node: Operand::Move(Place::from(y_temp)), span: DUMMY_SP },
            Spanned { node: Operand::Copy(Place::from(plan.x_local)), span: DUMMY_SP },
            Spanned { node: Operand::Copy(Place::from(plan.w_local)), span: DUMMY_SP },
            Spanned { node: Operand::Move(Place::from(n_temp)), span: DUMMY_SP },
            Spanned { node: plan.ks_op.clone(), span: DUMMY_SP },
        ]),
        destination: Place::from(unit_dest),
        target: Some(plan.outer_exit_bb),
        unwind: UnwindAction::Unreachable,
        call_source: CallSource::Misc,
        fn_span: DUMMY_SP,
    };
    let new_bb_data = BasicBlockData::new_stmts(prelude, Some(Terminator { source_info: SourceInfo::outermost(DUMMY_SP), kind: new_term_kind }), false);
    let header = {
        let mut found: Option<BasicBlock> = None;
        body.basic_blocks[plan.outer_entry_bb].terminator().successors().for_each(|s| {
            if found.is_none() { found = Some(s); }
        });
        match found { Some(h) => h, None => return false }
    };
    let new_bb_idx = body.basic_blocks_mut().push(new_bb_data);
    let entry_term = body.basic_blocks_mut()[plan.outer_entry_bb].terminator_mut();
    entry_term.successors_mut(|succ| { if *succ == header { *succ = new_bb_idx; } });
    true
}

pub(crate) fn try_transform_slice_conv1d_f64<'tcx>(
    tcx: TyCtxt<'tcx>,
    body: &mut mir::Body<'tcx>,
) -> usize {
    if !idiom_transform_enabled(tcx) { return 0; }
    if is_in_sysroot_crate(tcx, body.source.def_id()) { return 0; }
    let fn_name = tcx.def_path_str(body.source.def_id());
    if fn_name.contains("parallel_runtime") || fn_name.contains("parallel_conv1d")
        || fn_name.contains("parallel_invoke") || fn_name.contains("parallel_matvec")
    { return 0; }
    if !is_associative_float_opted_in(tcx, body.source.def_id()) { return 0; }
    if parallel_pass_quick_skip(tcx, body, 6, true) { return 0; }
    fn lookup(tcx: TyCtxt<'_>, name: &str) -> Option<rustc_hir::def_id::DefId> {
        tcx.get_diagnostic_item(rustc_span::Symbol::intern(name))
    }
    let helper_f64 = lookup(tcx, "parallel_runtime_parallel_conv1d_f64");
    if helper_f64.is_none() { return 0; }
    let loops = find_simple_loops(body);
    let mut plans: Vec<Conv1dPlan<'tcx>> = Vec::new();
    for (i, outer_lp) in loops.iter().enumerate() {
        for (j, inner_lp) in loops.iter().enumerate() {
            if i == j { continue; }
            if outer_lp.header == inner_lp.header { continue; }
            if !outer_lp.body.contains(&inner_lp.header) { continue; }
            if let Some(p) = analyze_conv1d_loops(tcx, body, outer_lp, inner_lp) {
                par_dump!(tcx, "[PAR-IDIOM-CONV1D-CANDIDATE] fn={} y=_{} x=_{} w=_{} fty={:?}",
                    fn_name, p.y_local.index(), p.x_local.index(), p.w_local.index(), p.fty);
                plans.push(p);
                break;
            }
        }
    }
    let mut count = 0;
    for plan in plans {
        let helper_did = helper_f64;
        let Some(helper_did) = helper_did else { continue };
        if apply_conv1d_transform(tcx, body, &plan, helper_did) {
            par_dump!(tcx, "[PAR-IDIOM-CONV1D-APPLIED] fn={} fty={:?}", fn_name, plan.fty);
            count += 1;
        }
    }
    count
}

// Slice scale matcher, `dst[i] *= alpha`, self-modifying

#[derive(Debug, Clone)]
struct ScalePlan {
    entry_bb: mir::BasicBlock,
    exit_bb: mir::BasicBlock,
    dst_local: mir::Local,
    alpha_local: mir::Local,
    fty: FloatTy,
}

fn analyze_scale_loop<'tcx>(
    tcx: TyCtxt<'tcx>,
    body: &mir::Body<'tcx>,
    lp: &SimpleLoop,
) -> Option<ScalePlan> {
    use rustc_middle::mir::*;
    let shape = detect_range_loop_shape(tcx, body, lp)?;
    let same_as_i = |l: Local| shape.same_as_i(l);
    let resolve = |l: Local| LoopShape::resolve(&shape.alias_root, l);

    let mut indexed_writes = 0usize;
    let mut chosen: Option<(Local /*dst*/, Local /*alpha*/)> = None;

    for &bb in &lp.body {
        if bb == lp.header { continue; }
        for stmt in &body.basic_blocks[bb].statements {
            let StatementKind::Assign(box (place, rvalue)) = &stmt.kind else { continue };
            if !(place.projection.len() >= 2
                && matches!(place.projection[0], ProjectionElem::Deref)
                && matches!(place.projection[1], ProjectionElem::Index(idx) if same_as_i(idx)))
            {
                continue;
            }
            indexed_writes += 1;
            let dst_local = place.local;
            let Rvalue::BinaryOp(op, box (lhs, rhs)) = rvalue else { continue };
            if !matches!(op, BinOp::Mul | BinOp::MulUnchecked) { continue; }
            // One side is a self-read
            let self_read = |op: &Operand<'tcx>| -> bool {
                let p = match op {
                    Operand::Copy(p) | Operand::Move(p) => p,
                    _ => return false,
                };
                if p.local != dst_local { return false; }
                if p.projection.len() < 2 { return false; }
                matches!(p.projection[0], ProjectionElem::Deref)
                    && matches!(p.projection[1], ProjectionElem::Index(idx) if same_as_i(idx))
            };
            let scalar_local = |op: &Operand<'tcx>| -> Option<Local> {
                let p = match op {
                    Operand::Copy(p) | Operand::Move(p) => p,
                    _ => return None,
                };
                if !p.projection.is_empty() { return None; }
                Some(p.local)
            };
            let pair = if self_read(lhs) {
                scalar_local(rhs).map(|a| (dst_local, a))
            } else if self_read(rhs) {
                scalar_local(lhs).map(|a| (dst_local, a))
            } else {
                None
            };
            if let Some(p) = pair { chosen = Some(p); }
        }
    }
    if indexed_writes != 1 { return None; }
    let (dst_local, alpha_local) = chosen?;
    let dst_local = resolve(dst_local);
    let alpha_local = resolve(alpha_local);
    let dst_ty = body.local_decls[dst_local].ty;
    let alpha_ty = body.local_decls[alpha_local].ty;
    let fty = ref_slice_float_ty(tcx, dst_ty, true)?;
    let expected_alpha_ty = match fty {
        FloatTy::F64 => tcx.types.f64,
    };
    if alpha_ty != expected_alpha_ty { return None; }
    Some(ScalePlan {
        entry_bb: shape.entry_bb,
        exit_bb: shape.exit_bb,
        dst_local,
        alpha_local,
        fty,
    })
}

fn apply_scale_transform<'tcx>(
    tcx: TyCtxt<'tcx>,
    body: &mut mir::Body<'tcx>,
    plan: &ScalePlan,
    helper_def_id: rustc_hir::def_id::DefId,
) -> bool {
    use rustc_middle::mir::*;
    use rustc_span::DUMMY_SP;
    use rustc_span::source_map::Spanned;

    let elem_ty = match plan.fty {
        FloatTy::F64 => tcx.types.f64,
    };
    #[allow(rustc::usage_of_qualified_ty)]
    let slice_f64_ty = rustc_middle::ty::Ty::new_slice(tcx, elem_ty);
    #[allow(rustc::usage_of_qualified_ty)]
    let mut_slice_ref_ty = rustc_middle::ty::Ty::new_mut_ref(
        tcx, tcx.lifetimes.re_erased, slice_f64_ty);
    let dst_temp = body.local_decls.push(LocalDecl::new(mut_slice_ref_ty, DUMMY_SP));
    let prelude = vec![Statement::new(
        SourceInfo::outermost(DUMMY_SP),
        StatementKind::Assign(Box::new((
            Place::from(dst_temp),
            Rvalue::Ref(
                tcx.lifetimes.re_erased,
                BorrowKind::Mut { kind: MutBorrowKind::Default },
                Place {
                    local: plan.dst_local,
                    projection: tcx.mk_place_elems(&[ProjectionElem::Deref]),
                },
            ),
        ))),
    )];
    let unit_dest = body.local_decls.push(LocalDecl::new(tcx.types.unit, DUMMY_SP));
    let helper_callee = Operand::function_handle(tcx, helper_def_id, std::iter::empty(), DUMMY_SP);
    let new_term_kind = TerminatorKind::Call {
        func: helper_callee,
        args: Box::new([
            Spanned { node: Operand::Move(Place::from(dst_temp)), span: DUMMY_SP },
            Spanned { node: Operand::Copy(Place::from(plan.alpha_local)), span: DUMMY_SP },
        ]),
        destination: Place::from(unit_dest),
        target: Some(plan.exit_bb),
        unwind: UnwindAction::Unreachable,
        call_source: CallSource::Misc,
        fn_span: DUMMY_SP,
    };
    let new_bb_data = BasicBlockData::new_stmts(
        prelude,
        Some(Terminator { source_info: SourceInfo::outermost(DUMMY_SP), kind: new_term_kind }),
        false,
    );
    let header = {
        let mut found: Option<BasicBlock> = None;
        body.basic_blocks[plan.entry_bb].terminator().successors().for_each(|s| {
            if found.is_none() { found = Some(s); }
        });
        match found { Some(h) => h, None => return false }
    };
    let new_bb_idx = body.basic_blocks_mut().push(new_bb_data);
    let entry_term = body.basic_blocks_mut()[plan.entry_bb].terminator_mut();
    entry_term.successors_mut(|succ| {
        if *succ == header { *succ = new_bb_idx; }
    });
    true
}

pub(crate) fn try_transform_slice_scale_f64<'tcx>(
    tcx: TyCtxt<'tcx>,
    body: &mut mir::Body<'tcx>,
) -> usize {
    if !idiom_transform_enabled(tcx) { return 0; }
    if is_in_sysroot_crate(tcx, body.source.def_id()) { return 0; }
    let fn_name = tcx.def_path_str(body.source.def_id());
    if fn_name.contains("parallel_runtime") || fn_name.contains("parallel_reduce_")
        || fn_name.contains("parallel_zip_") || fn_name.contains("parallel_axpy")
        || fn_name.contains("parallel_fill") || fn_name.contains("parallel_copy_from_slice")
        || fn_name.contains("parallel_invoke") || fn_name.contains("parallel_scale")
    { return 0; }
    // Scale bit-identical (no accumulation), no opt-in
    if parallel_pass_quick_skip(tcx, body, 4, true) { return 0; }
    fn lookup(tcx: TyCtxt<'_>, name: &str) -> Option<rustc_hir::def_id::DefId> {
        tcx.get_diagnostic_item(rustc_span::Symbol::intern(name))
    }
    let helper_f64 = lookup(tcx, "parallel_runtime_parallel_scale_f64");
    if helper_f64.is_none() { return 0; }
    let loops = find_simple_loops(body);
    let mut plans: Vec<ScalePlan> = Vec::new();
    for lp in &loops {
        if let Some(p) = analyze_scale_loop(tcx, body, lp) {
            par_dump!(tcx,
                "[PAR-IDIOM-SCALE-CANDIDATE] fn={} entry=bb{} exit=bb{} dst=_{} alpha=_{} fty={:?}",
                fn_name, p.entry_bb.index(), p.exit_bb.index(),
                p.dst_local.index(), p.alpha_local.index(), p.fty,
            );
            plans.push(p);
        }
    }
    let mut count = 0;
    for plan in plans {
        let helper_did = helper_f64;
        let Some(helper_did) = helper_did else { continue };
        if apply_scale_transform(tcx, body, &plan, helper_did) {
            par_dump!(tcx, "[PAR-IDIOM-SCALE-APPLIED] fn={} entry=bb{} fty={:?}",
                fn_name, plan.entry_bb.index(), plan.fty);
            count += 1;
        }
    }
    count
}

/// Conservative MIR purity check on reduction body
fn check_body_purity<'tcx>(
    tcx: TyCtxt<'tcx>,
    def_id: rustc_hir::def_id::DefId,
    // Body under optimization; optimized_mir on it cycles
    body_did: rustc_hir::def_id::DefId,
) -> Result<(), String> {
    if def_id == body_did {
        return Err(format!(
            "self-recursive callee {} — cannot inspect own optimized_mir mid-pass",
            tcx.def_path_str(def_id)
        ));
    }
    let Some(_guard) = InProgressGuard::try_enter(def_id) else {
        return Err(format!(
            "mutual-recursion cycle reaching {} — cannot inspect mid-pass",
            tcx.def_path_str(def_id)
        ));
    };
    // Bug 5A reject, see check_body_purity_for_invoke
    if fn_sig_has_problematic_regions(tcx, def_id) {
        return Err(format!(
            "fn signature contains free/bound regions {} — would leak into synthesized MIR",
            tcx.def_path_str(def_id)
        ));
    }
    // Bug 4A part 2, body-level proc_macro reject
    if body_calls_proc_macro(tcx, def_id) {
        return Err(format!(
            "body of {} calls into proc_macro/proc_macro2 — unsafe to run on parallel worker",
            tcx.def_path_str(def_id)
        ));
    }
    // We need the body's MIR
    if !def_id.is_local() {
        return Err(format!(
            "cross-crate body {} — purity not analysable from this crate",
            tcx.def_path_str(def_id)
        ));
    }
    if tcx.is_mir_available(def_id) == false {
        return Err(format!("MIR unavailable for {}", tcx.def_path_str(def_id)));
    }
    let mir = tcx.optimized_mir(def_id);

    for (bb_idx, bb) in mir.basic_blocks.iter_enumerated() {
        for (stmt_idx, stmt) in bb.statements.iter().enumerate() {
            if let Err(reason) = check_stmt_purity(tcx, stmt) {
                return Err(format!(
                    "bb{}:{} {}",
                    bb_idx.index(),
                    stmt_idx,
                    reason
                ));
            }
        }
        if let Err(reason) = check_terminator_purity(tcx, bb.terminator()) {
            return Err(format!("bb{} terminator {}", bb_idx.index(), reason));
        }
    }
    Ok(())
}

fn check_stmt_purity<'tcx>(
    _tcx: TyCtxt<'tcx>,
    stmt: &mir::Statement<'tcx>,
) -> Result<(), String> {
    match &stmt.kind {
        mir::StatementKind::Assign(box (place, rvalue)) => {
            // LHS bare Local; local-frame projections OK
            if !place.projection.is_empty() {
                // Field/Index OK; Deref could write shared memory
                for elem in place.projection.iter() {
                    if let mir::ProjectionElem::Deref = elem {
                        return Err("write through pointer (Deref projection)".to_string());
                    }
                }
            }
            check_rvalue_purity(rvalue)
        }
        mir::StatementKind::StorageLive(_) | mir::StatementKind::StorageDead(_) => Ok(()),
        mir::StatementKind::FakeRead(_) => Ok(()),
        mir::StatementKind::Nop => Ok(()),
        mir::StatementKind::PlaceMention(_) => Ok(()),
        mir::StatementKind::Retag(..) => Ok(()),
        mir::StatementKind::AscribeUserType(..) => Ok(()),
        mir::StatementKind::Coverage(_) => Ok(()),
        mir::StatementKind::SetDiscriminant { .. }
        | mir::StatementKind::Intrinsic(_)
        | mir::StatementKind::ConstEvalCounter
        | mir::StatementKind::BackwardIncompatibleDropHint { .. } => {
            Err(format!("statement kind not whitelisted: {:?}", stmt.kind.name()))
        }
    }
}

fn check_rvalue_purity<'tcx>(rvalue: &mir::Rvalue<'tcx>) -> Result<(), String> {
    use mir::Rvalue;
    match rvalue {
        Rvalue::Use(_)
        | Rvalue::Repeat(_, _)
        | Rvalue::Cast(_, _, _)
        | Rvalue::BinaryOp(_, _)
        | Rvalue::UnaryOp(_, _)
        | Rvalue::Discriminant(_)
        | Rvalue::Aggregate(_, _)
        | Rvalue::CopyForDeref(_) => Ok(()),
        Rvalue::Ref(_, kind, _) => {
            match kind {
                mir::BorrowKind::Shared | mir::BorrowKind::Fake(_) => Ok(()),
                mir::BorrowKind::Mut { .. } => {
                    Err("mutable borrow created in body".to_string())
                }
            }
        }
        Rvalue::RawPtr(_, _) => Err("raw pointer creation in body".to_string()),
        Rvalue::ThreadLocalRef(_) => Err("ThreadLocalRef in body".to_string()),
        Rvalue::ShallowInitBox(_, _) => Err("Box init (heap allocation) in body".to_string()),
        Rvalue::WrapUnsafeBinder(..) => Err("unsafe binder in body".to_string()),
    }
}

fn check_terminator_purity<'tcx>(
    tcx: TyCtxt<'tcx>,
    terminator: &mir::Terminator<'tcx>,
) -> Result<(), String> {
    use mir::TerminatorKind;
    match &terminator.kind {
        TerminatorKind::Goto { .. }
        | TerminatorKind::SwitchInt { .. }
        | TerminatorKind::Return
        | TerminatorKind::Unreachable
        | TerminatorKind::Assert { .. }
        | TerminatorKind::FalseEdge { .. }
        | TerminatorKind::FalseUnwind { .. } => Ok(()),
        TerminatorKind::Drop { .. } => {
            // Drop is conservatively impure
            Err("Drop terminator (Drop impl could have side effects)".to_string())
        }
        TerminatorKind::Call { func, .. } => {
            // Only allowlisted pure intrinsics; reject everything else
            let mir::Operand::Constant(c) = func else {
                return Err("indirect Call (function pointer)".to_string());
            };
            let ty = c.const_.ty();
            let rustc_middle::ty::FnDef(callee_def_id, _) = ty.kind() else {
                return Err("Call target is not a FnDef".to_string());
            };
            let path = tcx.def_path_str(*callee_def_id);
            if is_known_pure_callee(&path) {
                Ok(())
            } else {
                Err(format!("Call to non-allowlisted fn: {}", path))
            }
        }
        TerminatorKind::TailCall { .. } => Err("TailCall in body".to_string()),
        TerminatorKind::Yield { .. } => Err("Yield (generator) in body".to_string()),
        TerminatorKind::CoroutineDrop => Err("CoroutineDrop in body".to_string()),
        TerminatorKind::InlineAsm { .. } => Err("InlineAsm in body".to_string()),
        TerminatorKind::UnwindResume | TerminatorKind::UnwindTerminate(_) => {
            Err("Unwind path in body".to_string())
        }
    }
}

/// Relaxed purity check used **only** by `try_transform_parallel_invoke`
fn fn_sig_has_problematic_regions<'tcx>(
    tcx: TyCtxt<'tcx>,
    did: rustc_hir::def_id::DefId,
) -> bool {
    use rustc_middle::ty::TypeVisitableExt;
    let sig = tcx.fn_sig(did).instantiate_identity();
    // First mirror what `read_call` does
    let sig = tcx.instantiate_bound_regions_with_erased(sig);
    sig.has_free_regions()
}

/// Transitive proc-macro reject via memoized call-graph walk
const MAX_PROC_MACRO_WALK_DEPTH: u32 = 8;

thread_local! {
    static PROC_MACRO_CALL_CACHE: std::cell::RefCell<
        rustc_data_structures::fx::FxHashMap<rustc_hir::def_id::DefId, bool>
    > = const { std::cell::RefCell::new(rustc_data_structures::fx::FxHashMap::with_hasher(rustc_data_structures::fx::FxBuildHasher)) };

    static PROC_MACRO_CALL_PATH: std::cell::RefCell<Vec<rustc_hir::def_id::DefId>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

fn body_calls_proc_macro<'tcx>(tcx: TyCtxt<'tcx>, def_id: rustc_hir::def_id::DefId) -> bool {
    body_calls_proc_macro_rec(tcx, def_id, 0)
}

fn body_calls_proc_macro_rec<'tcx>(
    tcx: TyCtxt<'tcx>,
    def_id: rustc_hir::def_id::DefId,
    depth: u32,
) -> bool {
    // Direct hit, callee lives in proc_macro crates
    let crate_name = tcx.crate_name(def_id.krate);
    let s = crate_name.as_str();
    if s == "proc_macro" || s == "proc_macro2" {
        return true;
    }

    // Depth limit - conservative true
    if depth >= MAX_PROC_MACRO_WALK_DEPTH {
        return true;
    }

    // Memoized; in-cycle results are never cached
    if let Some(cached) = PROC_MACRO_CALL_CACHE.with(|c| c.borrow().get(&def_id).copied()) {
        return cached;
    }

    // Cycle detection on the current recursion path.
    let on_path =
        PROC_MACRO_CALL_PATH.with(|p| p.borrow().iter().any(|d| *d == def_id));
    if on_path {
        return false;
    }

    // No MIR cross-crate, assume false
    if !tcx.is_mir_available(def_id) {
        PROC_MACRO_CALL_CACHE.with(|c| {
            c.borrow_mut().insert(def_id, false);
        });
        return false;
    }

    // Guard only when depth>0; caller holds root
    let _inflight_guard = if depth > 0 {
        match InProgressGuard::try_enter(def_id) {
            Some(g) => Some(g),
            None => return true,
        }
    } else {
        None
    };

    PROC_MACRO_CALL_PATH.with(|p| p.borrow_mut().push(def_id));

    let mir = tcx.optimized_mir(def_id);
    let mut found = false;
    'outer: for bb in mir.basic_blocks.iter() {
        if let mir::TerminatorKind::Call { func, .. } = &bb.terminator().kind {
            if let mir::Operand::Constant(c) = func {
                if let ty::FnDef(callee_did, _) = c.const_.ty().kind() {
                    if body_calls_proc_macro_rec(tcx, *callee_did, depth + 1) {
                        found = true;
                        break 'outer;
                    }
                }
            }
        }
    }

    PROC_MACRO_CALL_PATH.with(|p| {
        p.borrow_mut().pop();
    });

    PROC_MACRO_CALL_CACHE.with(|c| {
        c.borrow_mut().insert(def_id, found);
    });

    found
}

/// Legacy direct-call version, kept for documentation
#[allow(dead_code)]
fn _body_calls_proc_macro_direct_only_legacy<'tcx>(
    tcx: TyCtxt<'tcx>,
    def_id: rustc_hir::def_id::DefId,
) -> bool {
    if !tcx.is_mir_available(def_id) {
        return false;
    }
    let mir = tcx.optimized_mir(def_id);
    for bb in mir.basic_blocks.iter() {
        if let mir::TerminatorKind::Call { func, .. } = &bb.terminator().kind {
            if let mir::Operand::Constant(c) = func {
                if let ty::FnDef(callee_did, _) = c.const_.ty().kind() {
                    let n = tcx.crate_name(callee_did.krate);
                    let s = n.as_str();
                    if s == "proc_macro" || s == "proc_macro2" {
                        return true;
                    }
                }
            }
        }
    }
    false
}

fn check_body_purity_for_invoke<'tcx>(
    tcx: TyCtxt<'tcx>,
    def_id: rustc_hir::def_id::DefId,
    // Same query-cycle guard as `check_body_purity`
    body_did: rustc_hir::def_id::DefId,
) -> Result<(), String> {
    if def_id == body_did {
        return Err(format!(
            "self-recursive callee {} — cannot inspect own optimized_mir mid-pass",
            tcx.def_path_str(def_id)
        ));
    }
    let Some(_guard) = InProgressGuard::try_enter(def_id) else {
        return Err(format!(
            "mutual-recursion cycle reaching {} — cannot inspect mid-pass",
            tcx.def_path_str(def_id)
        ));
    };
    // Bug 5A, un-erased regions fail MIR validation
    if fn_sig_has_problematic_regions(tcx, def_id) {
        return Err(format!(
            "fn signature contains free/bound regions {} — would leak into synthesized MIR",
            tcx.def_path_str(def_id)
        ));
    }
    // Bug 4A, proc_macro callers need bridge TLS
    if body_calls_proc_macro(tcx, def_id) {
        return Err(format!(
            "body of {} calls into proc_macro/proc_macro2 — unsafe to run on parallel worker",
            tcx.def_path_str(def_id)
        ));
    }
    if !def_id.is_local() {
        return Err(format!(
            "cross-crate body {} — purity not analysable from this crate",
            tcx.def_path_str(def_id)
        ));
    }
    if !tcx.is_mir_available(def_id) {
        return Err(format!("MIR unavailable for {}", tcx.def_path_str(def_id)));
    }
    let mir = tcx.optimized_mir(def_id);

    for (bb_idx, bb) in mir.basic_blocks.iter_enumerated() {
        for (stmt_idx, stmt) in bb.statements.iter().enumerate() {
            if let Err(reason) = check_stmt_purity_for_invoke(stmt) {
                return Err(format!("bb{}:{} {}", bb_idx.index(), stmt_idx, reason));
            }
        }
        if let Err(reason) = check_terminator_purity_for_invoke(bb.terminator()) {
            return Err(format!("bb{} terminator {}", bb_idx.index(), reason));
        }
    }
    Ok(())
}

fn check_stmt_purity_for_invoke<'tcx>(
    stmt: &mir::Statement<'tcx>,
) -> Result<(), String> {
    match &stmt.kind {
        mir::StatementKind::Assign(box (_, rvalue)) => {
            // Allow Deref writes; Send/Sync guards owned heap
            check_rvalue_purity_for_invoke(rvalue)
        }
        mir::StatementKind::StorageLive(_)
        | mir::StatementKind::StorageDead(_)
        | mir::StatementKind::FakeRead(_)
        | mir::StatementKind::Nop
        | mir::StatementKind::PlaceMention(_)
        | mir::StatementKind::Retag(..)
        | mir::StatementKind::AscribeUserType(..)
        | mir::StatementKind::Coverage(_)
        | mir::StatementKind::SetDiscriminant { .. } => Ok(()),
        mir::StatementKind::Intrinsic(_) => {
            // Admit MIR intrinsics, no user-visible effects
            Ok(())
        }
        mir::StatementKind::ConstEvalCounter
        | mir::StatementKind::BackwardIncompatibleDropHint { .. } => Ok(()),
    }
}

fn check_rvalue_purity_for_invoke<'tcx>(rvalue: &mir::Rvalue<'tcx>) -> Result<(), String> {
    use mir::Rvalue;
    match rvalue {
        Rvalue::Use(_)
        | Rvalue::Repeat(_, _)
        | Rvalue::Cast(_, _, _)
        | Rvalue::BinaryOp(_, _)
        | Rvalue::UnaryOp(_, _)
        | Rvalue::Discriminant(_)
        | Rvalue::Aggregate(_, _)
        | Rvalue::CopyForDeref(_) => Ok(()),
        // Admit Box init, allocator is thread-safe
        Rvalue::ShallowInitBox(_, _) => Ok(()),
        // Admit all borrows; borrowck prevents cross-closure aliasing
        Rvalue::Ref(_, _, _) => Ok(()),
        // Still reject; admitting caused downstream MIR ICE
        Rvalue::RawPtr(_, _) => Err("raw pointer creation in body".to_string()),
        // Reject TLS; workers see their own slots
        Rvalue::ThreadLocalRef(_) => Err("ThreadLocalRef in body".to_string()),
        Rvalue::WrapUnsafeBinder(..) => Err("unsafe binder in body".to_string()),
    }
}

fn check_terminator_purity_for_invoke<'tcx>(
    terminator: &mir::Terminator<'tcx>,
) -> Result<(), String> {
    use mir::TerminatorKind;
    match &terminator.kind {
        TerminatorKind::Goto { .. }
        | TerminatorKind::SwitchInt { .. }
        | TerminatorKind::Return
        | TerminatorKind::Unreachable
        | TerminatorKind::Assert { .. }
        | TerminatorKind::FalseEdge { .. }
        | TerminatorKind::FalseUnwind { .. } => Ok(()),
        // Admit Drop; closures own what they drop
        TerminatorKind::Drop { .. } => Ok(()),
        // Admit any FnDef Call
        TerminatorKind::Call { func, .. } => {
            let mir::Operand::Constant(c) = func else {
                return Err("indirect Call (function pointer)".to_string());
            };
            let ty = c.const_.ty();
            let rustc_middle::ty::FnDef(_, _) = ty.kind() else {
                return Err("Call target is not a FnDef".to_string());
            };
            Ok(())
        }
        TerminatorKind::TailCall { .. } => Err("TailCall in body".to_string()),
        TerminatorKind::Yield { .. } => Err("Yield (generator) in body".to_string()),
        TerminatorKind::CoroutineDrop => Err("CoroutineDrop in body".to_string()),
        TerminatorKind::InlineAsm { .. } => Err("InlineAsm in body".to_string()),
        TerminatorKind::UnwindResume | TerminatorKind::UnwindTerminate(_) => {
            Err("Unwind path in body".to_string())
        }
    }
}

/// Pure core::num allowlist, matched on last segment
fn is_known_pure_callee(path: &str) -> bool {
    let last = match path.rsplit("::").next() {
        Some(s) => s,
        None => return false,
    };
    matches!(
        last,
        "wrapping_add"
            | "wrapping_sub"
            | "wrapping_mul"
            | "wrapping_div"
            | "wrapping_rem"
            | "wrapping_neg"
            | "wrapping_shl"
            | "wrapping_shr"
            | "saturating_add"
            | "saturating_sub"
            | "saturating_mul"
            | "checked_add"
            | "checked_sub"
            | "checked_mul"
            | "overflowing_add"
            | "overflowing_sub"
            | "overflowing_mul"
            | "rotate_left"
            | "rotate_right"
            | "swap_bytes"
            | "reverse_bits"
            | "leading_zeros"
            | "trailing_zeros"
            | "leading_ones"
            | "trailing_ones"
            | "count_ones"
            | "count_zeros"
            | "abs"
            | "abs_diff"
            | "max"
            | "min"
            | "clamp"
            | "pow"
            | "isqrt"
            | "ilog2"
            | "ilog10"
            | "is_power_of_two"
            | "next_power_of_two"
            | "from_le_bytes"
            | "from_be_bytes"
            | "from_ne_bytes"
            | "to_le_bytes"
            | "to_be_bytes"
            | "to_ne_bytes"
            | "from_le"
            | "from_be"
            | "to_le"
            | "to_be"
    )
}

/// Match canonical reduction loop, return a plan
fn analyze_loop_for_transform<'tcx>(
    tcx: TyCtxt<'tcx>,
    body: &mir::Body<'tcx>,
    lp: &SimpleLoop,
) -> Option<ReducePlan<'tcx>> {
    let dbg = std::env::var("PARALLEL_TRANSFORM_REDUCE_DEBUG").ok().as_deref() == Some("1");
    macro_rules! reject {
        ($($arg:tt)*) => {
            if dbg { par_dump!(tcx, "[PAR-IDIOM-TX-REJECT] header=bb{}: {}", lp.header.index(), format!($($arg)*)); }
            return None;
        };
    }

    let header = lp.header;
    let preds = body.basic_blocks.predecessors();

    let mut entry: Option<mir::BasicBlock> = None;
    for &pred in preds[header].iter() {
        if !lp.body.contains(&pred) {
            if entry.replace(pred).is_some() {
                reject!("multiple out-of-loop predecessors of header");
            }
        }
    }
    let Some(entry_bb) = entry else {
        reject!("no out-of-loop predecessor (entry) found for header");
    };

    // Header must terminate with `switchInt(_cmp) -> [false
    let header_term = body.basic_blocks[header].terminator();
    let (cmp_local, exit_bb) = match &header_term.kind {
        mir::TerminatorKind::SwitchInt { discr, targets } => {
            let local = match discr {
                mir::Operand::Move(p) | mir::Operand::Copy(p) if p.projection.is_empty() => {
                    p.local
                }
                _ => {
                    reject!("switchInt discriminant is not a bare Local");
                }
            };
            let mut exit: Option<mir::BasicBlock> = None;
            for (_val, t) in targets.iter() {
                if !lp.body.contains(&t) {
                    exit = Some(t);
                }
            }
            let otherwise = targets.otherwise();
            if !lp.body.contains(&otherwise) && exit.is_none() {
                exit = Some(otherwise);
            }
            let Some(exit_bb) = exit else {
                reject!("switchInt has no out-of-loop branch (no exit)");
            };
            (local, exit_bb)
        }
        other => {
            reject!("header terminator is not SwitchInt: {:?}", other.name());
        }
    };

    // Header assigns cmp_local `Lt`; trace operand temps
    let header_block = &body.basic_blocks[header];
    let mut iter_local: Option<mir::Local> = None;
    let mut end_bound: Option<LoopBound> = None;
    for stmt in &header_block.statements {
        let mir::StatementKind::Assign(box (place, rvalue)) = &stmt.kind else { continue };
        if !place.projection.is_empty() || place.local != cmp_local {
            continue;
        }
        let mir::Rvalue::BinaryOp(mir::BinOp::Lt, box (a, b)) = rvalue else { continue };
        let i_temp = match a {
            mir::Operand::Copy(p) | mir::Operand::Move(p) if p.projection.is_empty() => p.local,
            _ => continue,
        };
        let bound = match b {
            mir::Operand::Constant(c) => match extract_u64_const(tcx, &c.const_) {
                Some(n) => LoopBound::Const(n),
                None => continue,
            },
            mir::Operand::Copy(p) | mir::Operand::Move(p) if p.projection.is_empty() => {
                // trace temp to local via copy chains
                LoopBound::Local(resolve_local_chain(header_block, p.local))
            }
            _ => continue,
        };
        // Trace iter temp back to real Local
        iter_local = Some(resolve_local_chain(header_block, i_temp));
        end_bound = Some(bound);
        break;
    }
    let Some(iter_local) = iter_local else {
        reject!("could not locate iter Local from Lt comparison in header");
    };
    let Some(end_bound) = end_bound else {
        reject!("could not locate end bound from Lt comparison in header");
    };

    // Single pass finds body call, op, acc
    let Some((body_def_id, body_args, reduce_op, acc_local)) =
        find_reduction_in_loop(tcx, body, lp, iter_local, entry_bb)
    else {
        reject!(
            "no reduction op-statement found connecting acc to body(iter) in loop"
        );
    };

    // Detect the accumulator type
    let acc_ty = body.local_decls[acc_local].ty;
    let Some(acc_ty_tag) = AccTyTag::from_ty(acc_ty) else {
        reject!("unsupported accumulator type {:?}", acc_ty.kind());
    };
    let width_bits = acc_ty_tag.width_bits();

    // Foldable iter init becomes `start`; nonzero fine
    let Some(iter_start_bits) = find_init_bits(tcx, body, entry_bb, iter_local, width_bits) else {
        reject!(
            "iter _{} init in bb{} is not a foldable constant",
            iter_local.index(), entry_bb.index()
        );
    };

    // Body fn takes and returns acc_ty
    {
        let body_sig = tcx.fn_sig(body_def_id).instantiate(tcx, body_args);
        let body_sig = tcx.instantiate_bound_regions_with_erased(body_sig);
        let inputs = body_sig.inputs();
        if inputs.len() != 1 || inputs[0] != acc_ty || body_sig.output() != acc_ty {
            reject!(
                "body fn signature {:?} does not match fn({:?}) -> {:?}",
                body_sig, acc_ty, acc_ty
            );
        }
    }

    // Iter type must equal acc_ty for helper
    let iter_ty = body.local_decls[iter_local].ty;
    if iter_ty != acc_ty {
        reject!(
            "iter type {:?} does not match acc type {:?}",
            iter_ty, acc_ty
        );
    }

    // Verify acc init matches the op's identity
    let expected_init = reduce_op.identity_bits(acc_ty_tag);
    if !is_local_initialised_to_bits(tcx, body, entry_bb, acc_local, expected_init, width_bits) {
        reject!(
            "acc _{} init in bb{} doesn't match identity {:#x} for op {:?}",
            acc_local.index(), entry_bb.index(), expected_init, reduce_op
        );
    }

    Some(ReducePlan {
        entry_bb,
        exit_bb,
        acc_local,
        body_def_id,
        body_args,
        end_bound,
        acc_ty,
        acc_ty_tag,
        reduce_op,
        iter_start_bits,
    })
}

fn extract_u64_const<'tcx>(tcx: TyCtxt<'tcx>, c: &mir::Const<'tcx>) -> Option<u64> {
    let val = c.try_eval_scalar_int(tcx, ty::TypingEnv::fully_monomorphized())?;
    val.to_uint(val.size()).try_into().ok()
}

/// `mir::Const` to raw bits, fast or evaluated
fn eval_const_bits<'tcx>(tcx: TyCtxt<'tcx>, c: &mir::Const<'tcx>) -> Option<u128> {
    if let Some(b) = extract_u128_const_loose(c) {
        return Some(b);
    }
    let val = c.try_eval_scalar_int(tcx, ty::TypingEnv::fully_monomorphized())?;
    if val.size().bits() <= 128 {
        Some(val.to_bits_unchecked())
    } else {
        None
    }
}

/// Foldable init bits of `target` in `bb`
fn find_init_bits<'tcx>(
    tcx: TyCtxt<'tcx>,
    body: &mir::Body<'tcx>,
    bb: mir::BasicBlock,
    target: mir::Local,
    width_bits: u32,
) -> Option<u128> {
    let mask: u128 = if width_bits >= 128 { u128::MAX } else { (1u128 << width_bits) - 1 };
    for stmt in &body.basic_blocks[bb].statements {
        let mir::StatementKind::Assign(box (place, rvalue)) = &stmt.kind else { continue };
        if !place.projection.is_empty() || place.local != target {
            continue;
        }
        let folded: Option<u128> = match rvalue {
            mir::Rvalue::Use(mir::Operand::Constant(c)) => {
                eval_const_bits(tcx, &c.const_).map(|v| v & mask)
            }
            mir::Rvalue::UnaryOp(mir::UnOp::Not, mir::Operand::Constant(c)) => {
                eval_const_bits(tcx, &c.const_).map(|v| (!v) & mask)
            }
            mir::Rvalue::UnaryOp(mir::UnOp::Neg, mir::Operand::Constant(c)) => {
                eval_const_bits(tcx, &c.const_).map(|v| ((!v).wrapping_add(1)) & mask)
            }
            _ => None,
        };
        if let Some(bits) = folded {
            return Some(bits);
        }
    }
    None
}

/// Convenience wrapper for call-site readability
fn is_local_initialised_to_bits<'tcx>(
    tcx: TyCtxt<'tcx>,
    body: &mir::Body<'tcx>,
    bb: mir::BasicBlock,
    target: mir::Local,
    expected_bits: u128,
    width_bits: u32,
) -> bool {
    let mask: u128 = if width_bits >= 128 { u128::MAX } else { (1u128 << width_bits) - 1 };
    find_init_bits(tcx, body, bb, target, width_bits).map(|b| b == (expected_bits & mask))
        .unwrap_or(false)
}

#[allow(dead_code)]
fn is_local_initialised_to_zero<'tcx>(
    body: &mir::Body<'tcx>,
    bb: mir::BasicBlock,
    target: mir::Local,
) -> bool {
    for stmt in &body.basic_blocks[bb].statements {
        let mir::StatementKind::Assign(box (place, rvalue)) = &stmt.kind else { continue };
        if !place.projection.is_empty() || place.local != target {
            continue;
        }
        let mir::Rvalue::Use(mir::Operand::Constant(c)) = rvalue else { continue };
        if let Some(0) = extract_u64_const_loose(&c.const_) {
            return true;
        }
    }
    false
}

/// Cheap u64 extractor that doesn't need tcx
fn extract_u64_const_loose<'tcx>(c: &mir::Const<'tcx>) -> Option<u64> {
    if let mir::Const::Val(val, _ty) = c {
        if let mir::ConstValue::Scalar(s) = val {
            // Try a sequence of progressively-narrower conversions
            if let Ok(scalar_int) = s.try_to_scalar_int() {
                let bits = scalar_int.size().bits();
                if bits <= 64 {
                    let raw_u128: u128 = scalar_int.to_bits_unchecked();
                    if raw_u128 <= u64::MAX as u128 {
                        return Some(raw_u128 as u64);
                    }
                }
            }
        }
    }
    None
}

/// Like extract_u64_const_loose but full u128 bits
fn extract_u128_const_loose<'tcx>(c: &mir::Const<'tcx>) -> Option<u128> {
    if let mir::Const::Val(val, _ty) = c {
        if let mir::ConstValue::Scalar(s) = val {
            if let Ok(scalar_int) = s.try_to_scalar_int() {
                if scalar_int.size().bits() <= 128 {
                    return Some(scalar_int.to_bits_unchecked());
                }
            }
        }
    }
    None
}

/// Single-pass body call and reduction op finder
fn find_reduction_in_loop<'tcx>(
    tcx: TyCtxt<'tcx>,
    body: &mir::Body<'tcx>,
    lp: &SimpleLoop,
    iter_local: mir::Local,
    entry_bb: mir::BasicBlock,
) -> Option<(rustc_hir::def_id::DefId, ty::GenericArgsRef<'tcx>, ReduceOp, mir::Local)> {
    // Step 1: collect candidate body fn calls.
    let mut body_candidates: Vec<(rustc_hir::def_id::DefId, ty::GenericArgsRef<'tcx>, mir::Local)> =
        Vec::new();
    for &bb in &lp.body {
        let block = &body.basic_blocks[bb];
        let term = block.terminator();
        let mir::TerminatorKind::Call { func, args, destination, .. } = &term.kind else {
            continue;
        };
        if !destination.projection.is_empty() {
            continue;
        }
        let mir::Operand::Constant(callee_c) = func else { continue };
        let ty::FnDef(def_id, generic_args) = callee_c.const_.ty().kind() else { continue };
        let path = tcx.def_path_str(*def_id);
        // Skip op-statement Calls and iter increment
        if path.starts_with("core::num::")
            || path.contains("wrapping_add")
            || path.contains("wrapping_sub")
            || path.contains("wrapping_mul")
            || path.contains("wrapping_div")
            || path.contains("rotate_left")
            || path.contains("rotate_right")
        {
            continue;
        }
        if args.len() != 1 {
            continue;
        }
        let arg_local = match &args[0].node {
            mir::Operand::Copy(p) | mir::Operand::Move(p) if p.projection.is_empty() => p.local,
            _ => continue,
        };
        if !traces_to_local(block, arg_local, iter_local) {
            continue;
        }
        body_candidates.push((*def_id, generic_args, destination.local));
    }

    // Step 2, find op-statement using body_dest_local
    let operand_local = |op: &mir::Operand<'tcx>| -> Option<mir::Local> {
        match op {
            mir::Operand::Copy(p) | mir::Operand::Move(p) if p.projection.is_empty() => {
                Some(p.local)
            }
            _ => None,
        }
    };
    let bin_op_to_reduce = |bo: mir::BinOp| -> Option<ReduceOp> {
        match bo {
            mir::BinOp::Add | mir::BinOp::AddUnchecked | mir::BinOp::AddWithOverflow => {
                Some(ReduceOp::Add)
            }
            mir::BinOp::Mul | mir::BinOp::MulUnchecked | mir::BinOp::MulWithOverflow => {
                Some(ReduceOp::Mul)
            }
            mir::BinOp::BitAnd => Some(ReduceOp::BitAnd),
            mir::BinOp::BitOr => Some(ReduceOp::BitOr),
            mir::BinOp::BitXor => Some(ReduceOp::BitXor),
            _ => None,
        }
    };

    for &(def_id, generic_args, body_dest_local) in &body_candidates {
        for &bb in &lp.body {
            let block = &body.basic_blocks[bb];

            // 2a. BinaryOp Rvalue inside an Assign statement.
            for (stmt_idx, stmt) in block.statements.iter().enumerate() {
                let mir::StatementKind::Assign(box (_place, rvalue)) = &stmt.kind else { continue };
                let (op, a, b) = match rvalue {
                    mir::Rvalue::BinaryOp(bo, box (a, b)) => {
                        let Some(op) = bin_op_to_reduce(*bo) else { continue };
                        (op, a, b)
                    }
                    _ => continue,
                };
                let (Some(la), Some(lb)) = (operand_local(a), operand_local(b)) else { continue };
                let acc_operand_local = if la == body_dest_local {
                    lb
                } else if lb == body_dest_local {
                    la
                } else {
                    continue;
                };
                // Block-local resolve avoids writeback; else cross-block walk
                let block_local_resolved =
                    resolve_local_chain_before(block, acc_operand_local, stmt_idx);
                let acc_local = if block_local_resolved == acc_operand_local {
                    resolve_local_chain_xblock_to_acc(body, lp, entry_bb, acc_operand_local)
                } else {
                    block_local_resolved
                };
                return Some((def_id, generic_args, op, acc_local));
            }

            // 2b. Call to wrapping_add / wrapping_mul (terminator).
            let term = block.terminator();
            let mir::TerminatorKind::Call { func, args, destination, .. } = &term.kind else {
                continue;
            };
            if !destination.projection.is_empty() || args.len() != 2 {
                continue;
            }
            let mir::Operand::Constant(callee_c) = func else { continue };
            let ty::FnDef(call_def_id, _) = callee_c.const_.ty().kind() else { continue };
            let path = tcx.def_path_str(*call_def_id);
            // Trailing path segment identifies the reduction op
            let last_seg = path.rsplit("::").next().unwrap_or("");
            let op = if path.contains("wrapping_add") {
                ReduceOp::Add
            } else if path.contains("wrapping_mul") {
                ReduceOp::Mul
            } else if last_seg == "min" {
                ReduceOp::Min
            } else if last_seg == "max" {
                ReduceOp::Max
            } else {
                continue;
            };
            let (Some(la), Some(lb)) = (operand_local(&args[0].node), operand_local(&args[1].node))
            else {
                continue;
            };
            let acc_operand_local = if la == body_dest_local {
                lb
            } else if lb == body_dest_local {
                la
            } else {
                continue;
            };
            // Terminator is conceptually after all statements
            let block_local_resolved = resolve_local_chain_before(
                block,
                acc_operand_local,
                block.statements.len(),
            );
            let acc_local = if block_local_resolved == acc_operand_local {
                resolve_local_chain_xblock_to_acc(body, lp, entry_bb, acc_operand_local)
            } else {
                block_local_resolved
            };
            return Some((def_id, generic_args, op, acc_local));
        }
    }
    None
}

/// Cross-block copy-chain walk, stops at entry_bb-assigned Local
fn resolve_local_chain_xblock_to_acc<'tcx>(
    body: &mir::Body<'tcx>,
    lp: &SimpleLoop,
    entry_bb: mir::BasicBlock,
    start: mir::Local,
) -> mir::Local {
    let mut current = start;
    let mut iters = 0u32;
    loop {
        if iters > 32 {
            return current;
        }
        iters += 1;
        // Stop at entry_bb init, else chases writeback
        let assigned_in_entry = body.basic_blocks[entry_bb].statements.iter().any(|s| {
            matches!(
                &s.kind,
                mir::StatementKind::Assign(box (p, _))
                if p.projection.is_empty() && p.local == current
            )
        });
        if assigned_in_entry {
            return current;
        }
        let mut next: Option<mir::Local> = None;
        'outer: for &bb in &lp.body {
            let block = &body.basic_blocks[bb];
            for stmt in &block.statements {
                let mir::StatementKind::Assign(box (place, rvalue)) = &stmt.kind else { continue };
                if !place.projection.is_empty() || place.local != current {
                    continue;
                }
                if let mir::Rvalue::Use(mir::Operand::Copy(p))
                | mir::Rvalue::Use(mir::Operand::Move(p)) = rvalue
                {
                    if p.projection.is_empty() {
                        next = Some(p.local);
                        break 'outer;
                    }
                }
            }
        }
        match next {
            Some(n) if n != current => current = n,
            _ => return current,
        }
    }
}

/// In-block copy-chain walk over statements before `before_idx`
fn resolve_local_chain_before<'tcx>(
    block: &mir::BasicBlockData<'tcx>,
    start: mir::Local,
    before_idx: usize,
) -> mir::Local {
    let mut current = start;
    let mut iter_count = 0u32;
    loop {
        if iter_count > 16 {
            return current;
        }
        iter_count += 1;
        let mut next: Option<mir::Local> = None;
        let upper = before_idx.min(block.statements.len());
        for stmt in block.statements[..upper].iter().rev() {
            let mir::StatementKind::Assign(box (place, rvalue)) = &stmt.kind else { continue };
            if place.projection.is_empty() && place.local == current {
                if let mir::Rvalue::Use(mir::Operand::Copy(p))
                | mir::Rvalue::Use(mir::Operand::Move(p)) = rvalue
                {
                    if p.projection.is_empty() {
                        next = Some(p.local);
                    }
                }
                break;
            }
        }
        match next {
            Some(n) if n != current => current = n,
            _ => return current,
        }
    }
}

/// In-block copy-chain walk to deepest source Local
fn resolve_local_chain<'tcx>(
    block: &mir::BasicBlockData<'tcx>,
    start: mir::Local,
) -> mir::Local {
    let mut current = start;
    let mut iter_count = 0u32;
    loop {
        if iter_count > 16 {
            return current;  // pathological chain - give up
        }
        iter_count += 1;
        let mut next: Option<mir::Local> = None;
        for stmt in block.statements.iter().rev() {
            let mir::StatementKind::Assign(box (place, rvalue)) = &stmt.kind else { continue };
            if place.projection.is_empty() && place.local == current {
                if let mir::Rvalue::Use(mir::Operand::Copy(p))
                | mir::Rvalue::Use(mir::Operand::Move(p)) = rvalue
                {
                    if p.projection.is_empty() {
                        next = Some(p.local);
                    }
                }
                break;
            }
        }
        match next {
            Some(n) if n != current => current = n,
            _ => return current,
        }
    }
}

/// Whether `start` copy-chains from `target` in `block`
fn traces_to_local<'tcx>(
    block: &mir::BasicBlockData<'tcx>,
    start: mir::Local,
    target: mir::Local,
) -> bool {
    let mut current = start;
    if current == target {
        return true;
    }
    // Walk the statements backwards
    for stmt in block.statements.iter().rev() {
        let mir::StatementKind::Assign(box (place, rvalue)) = &stmt.kind else { continue };
        if place.projection.is_empty() && place.local == current {
            match rvalue {
                mir::Rvalue::Use(mir::Operand::Copy(p))
                | mir::Rvalue::Use(mir::Operand::Move(p))
                    if p.projection.is_empty() =>
                {
                    current = p.local;
                    if current == target {
                        return true;
                    }
                }
                _ => return false,
            }
        }
    }
    current == target
}

/// Surgically rewrite the loop
fn apply_reduce_transform<'tcx>(
    tcx: TyCtxt<'tcx>,
    body: &mut mir::Body<'tcx>,
    plan: &ReducePlan<'tcx>,
    reduce_def_id: rustc_hir::def_id::DefId,
) -> bool {
    use rustc_middle::mir::*;
    use rustc_middle::ty::Ty;
    use rustc_span::DUMMY_SP;

    let acc_ty = plan.acc_ty;

    // Callee operand for the typed parallel_reduce_<op>_<T> primitive.
    let reduce_callee =
        Operand::function_handle(tcx, reduce_def_id, std::iter::empty(), DUMMY_SP);

    // FnDef ZST operand, cast to fn-pointer below
    let body_def_operand = Operand::function_handle(
        tcx,
        plan.body_def_id,
        plan.body_args.iter(),
        DUMMY_SP,
    );

    // start/end consts as acc_ty; from_bits keeps negatives
    let typing_env = ty::TypingEnv::fully_monomorphized();
    let mk_acc_const_bits = |v: u128| -> Operand<'tcx> {
        Operand::Constant(Box::new(ConstOperand {
            span: DUMMY_SP,
            user_ty: None,
            const_: Const::from_bits(tcx, v, typing_env, acc_ty),
        }))
    };
    let start_op = mk_acc_const_bits(plan.iter_start_bits);
    let end_op = match plan.end_bound {
        LoopBound::Const(n) => mk_acc_const_bits(n as u128),
        LoopBound::Local(l) => Operand::Copy(Place::from(l)),
    };

    // Construct fn-pointer type `fn(acc_ty) -> acc_ty`.
    let fn_ptr_ty = Ty::new_fn_ptr(
        tcx,
        ty::Binder::dummy(tcx.mk_fn_sig(
            [acc_ty],
            acc_ty,
            false,
            rustc_hir::Safety::Safe,
            rustc_abi::ExternAbi::Rust,
        )),
    );

    // Fresh fn-pointer Local for cast result
    let fn_ptr_local = body.local_decls.push(LocalDecl::new(fn_ptr_ty, DUMMY_SP));

    // Cast body FnDef to fn pointer
    let cast_rvalue = Rvalue::Cast(
        CastKind::PointerCoercion(
            ty::adjustment::PointerCoercion::ReifyFnPointer(rustc_hir::Safety::Safe),
            mir::CoercionSource::Implicit,
        ),
        body_def_operand,
        fn_ptr_ty,
    );
    let cast_stmt = Statement::new(
        SourceInfo::outermost(DUMMY_SP),
        StatementKind::Assign(Box::new((Place::from(fn_ptr_local), cast_rvalue))),
    );

    // Call terminator, `_acc = parallel_reduce_*(start, end, fn_ptr)`
    let call_args: Box<[rustc_span::source_map::Spanned<Operand<'tcx>>]> = Box::new([
        rustc_span::source_map::Spanned { node: start_op, span: DUMMY_SP },
        rustc_span::source_map::Spanned { node: end_op, span: DUMMY_SP },
        rustc_span::source_map::Spanned {
            node: Operand::Move(Place::from(fn_ptr_local)),
            span: DUMMY_SP,
        },
    ]);
    let call_term_kind = TerminatorKind::Call {
        func: reduce_callee,
        args: call_args,
        destination: Place::from(plan.acc_local),
        target: Some(plan.exit_bb),
        unwind: UnwindAction::Unreachable,
        call_source: CallSource::Misc,
        fn_span: DUMMY_SP,
    };
    let new_bb_data = BasicBlockData::new_stmts(
        vec![cast_stmt],
        Some(Terminator {
            source_info: SourceInfo::outermost(DUMMY_SP),
            kind: call_term_kind,
        }),
        false,
    );

    // Find the loop header
    let header = {
        let mut found: Option<BasicBlock> = None;
        body.basic_blocks[plan.entry_bb].terminator().successors().for_each(|succ| {
            if found.is_none() {
                found = Some(succ);
            }
        });
        match found {
            Some(h) => h,
            None => return false,
        }
    };

    // Push the new BB.
    let new_bb_idx = body.basic_blocks_mut().push(new_bb_data);

    // Rewire entry block, `header` edges to `new_bb_idx`
    let entry_term = body.basic_blocks_mut()[plan.entry_bb].terminator_mut();
    entry_term.successors_mut(|succ| {
        if *succ == header {
            *succ = new_bb_idx;
        }
    });

    true
}
