// `j2`: the J2 programming language compiler / runner.
//
// Usage:
//   j2 FILE.j2                  Build and run.
//   j2 build FILE.j2 -o OUT     Build to a binary, don't run.
//   j2 emit-native FILE.j2        Print the lowered backend source to stdout.
//
// Internally:
//   1. Read FILE.j2.
//   2. Lex + parse + lower → backend source.
//   3. Write a temp .rs file plus a small Cargo project that depends on j2_runtime.
//   4. Invoke `cargo build --release` (or equivalent) to produce the binary.
//   5. If running, exec the binary.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};

mod interp;

fn main() -> ExitCode {
    let mut args: Vec<String> = env::args().collect();
    // Capability flags are consumed here (not forwarded to the program) and
    // turned into env vars that the frontend (J2_TRUSTED → enables `rust{}`)
    // and the runtime (J2_ALLOW_FS/PROC/NET → enable fs/proc/http) read. The
    // built binary is spawned as a child and inherits this env. Everything is
    // deny-by-default: untrusted source cannot touch the host or run raw backend.
    args.retain(|a| match a.as_str() {
        "--allow-unsafe" => { env::set_var("J2_TRUSTED", "1"); false }
        "--allow-fs" => { env::set_var("J2_ALLOW_FS", "1"); false }
        "--allow-proc" => { env::set_var("J2_ALLOW_PROC", "1"); false }
        "--allow-net" => { env::set_var("J2_ALLOW_NET", "1"); false }
        "--allow-all" => {
            for k in ["J2_TRUSTED", "J2_ALLOW_FS", "J2_ALLOW_PROC", "J2_ALLOW_NET"] {
                env::set_var(k, "1");
            }
            false
        }
        _ => true,
    });
    // Version / help - checked only in the subcommand position (args[1]) so a
    // program argument that happens to be "version"/"-h" isn't intercepted.
    match args.get(1).map(|s| s.as_str()) {
        Some("--version" | "-V" | "version") => {
            println!("j2 {}", env!("CARGO_PKG_VERSION"));
            return ExitCode::SUCCESS;
        }
        Some("--help" | "-h" | "help") => {
            print_usage();
            return ExitCode::SUCCESS;
        }
        _ => {}
    }
    if args.len() < 2 {
        print_usage();
        return ExitCode::from(2);
    }

    match args[1].as_str() {
        "emit-native" => {
            if args.len() < 3 {
                eprintln!("j2 emit-native: missing file");
                return ExitCode::from(2);
            }
            let src = match fs::read_to_string(&args[2]) {
                Ok(s) => s,
                Err(e) => { eprintln!("error reading {}: {}", args[2], e); return ExitCode::from(1); }
            };
            let path = Path::new(&args[2]);
            match j2_compiler::compile_to_rust_with_path(&src, path) {
                Ok(rs) => { print!("{}", rs); ExitCode::SUCCESS }
                Err(e) => { eprint!("{}", e.render(&src, &args[2])); ExitCode::from(1) }
            }
        }
        "build" => {
            let file = match args.get(2) {
                Some(f) => f.clone(),
                None => { eprintln!("j2 build: missing file"); return ExitCode::from(2); }
            };
            let mut out: Option<String> = None;
            let mut i = 3;
            while i < args.len() {
                if args[i] == "-o" && i + 1 < args.len() {
                    out = Some(args[i + 1].clone());
                    i += 2;
                } else {
                    i += 1;
                }
            }
            let out = out.unwrap_or_else(|| {
                Path::new(&file).file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "a.out".into())
            });
            match build(&file, &out, false) {
                Ok(_) => ExitCode::SUCCESS,
                Err(e) => { eprintln!("{}", e); ExitCode::from(1) }
            }
        }
        "repl" => repl(),
        "run" => match args.get(2) {
            Some(f) => run_interp_or_compile(f, &[]),
            None => { eprintln!("j2 run: missing FILE.j2"); ExitCode::from(2) }
        },
        "test" => run_tests(args.get(2).map(|s| s.as_str()).unwrap_or("j2-tests")),
        "fmt" => {
            // `j2 fmt FILE`        → print formatted source
            // `j2 fmt -w FILE`     → rewrite the file in place
            let write = args.iter().any(|a| a == "-w");
            let file = args.iter().skip(2).find(|a| a.ends_with(".j2"));
            match file {
                None => { eprintln!("j2 fmt: missing FILE.j2"); ExitCode::from(2) }
                Some(f) => match fs::read_to_string(f) {
                    Err(e) => { eprintln!("j2 fmt: {}: {}", f, e); ExitCode::from(1) }
                    Ok(src) => {
                        let formatted = format_source(&src);
                        if write {
                            match fs::write(f, &formatted) {
                                Ok(_) => { eprintln!("formatted {}", f); ExitCode::SUCCESS }
                                Err(e) => { eprintln!("j2 fmt: write {}: {}", f, e); ExitCode::from(1) }
                            }
                        } else { print!("{}", formatted); ExitCode::SUCCESS }
                    }
                },
            }
        }
        file if file.ends_with(".j2") => {
            // Default run: interpreter-first (instant, no backend touched), with a
            // hidden native-compile fallback. `j2 build` / J2_FORCE_NATIVE force
            // native+parallel for speed-critical kernels.
            let extra: Vec<String> = args.iter().skip(2).cloned().collect();
            run_interp_or_compile(file, &extra)
        }
        other => {
            eprintln!("j: unknown subcommand {:?}", other);
            ExitCode::from(2)
        }
    }
}

/// Print the `j` usage summary (shown for `j`, `j --help`, `j -h`).
fn print_usage() {
    println!("j2 {}: the J2 programming language", env!("CARGO_PKG_VERSION"));
    println!();
    println!("usage:");
    println!("  j2 FILE.j2 [args...]      run a program (interpreter-first, instant)");
    println!("  j2 run FILE.j2            run through the interpreter");
    println!("  j2 build FILE.j2 -o OUT   compile to a native, auto-parallelized binary");
    println!("  j2 emit-native FILE.j2      print the lowered backend source");
    println!("  j2 fmt [-w] FILE.j2       format a source file (-w rewrites in place)");
    println!("  j2 repl                  start an interactive session");
    println!("  j2 test [DIR]            run every *.j2 under DIR (default j2-tests)");
    println!("  j2 --version | --help    print version / this help");
    println!();
    println!("capabilities (deny-by-default; grant per run):");
    println!("  --allow-fs  --allow-proc  --allow-net  --allow-all  --allow-unsafe");
}

/// Heuristic: does a REPL line establish persistent state (a definition or
/// binding) rather than an expression to evaluate-and-show?
fn is_repl_definition(t: &str) -> bool {
    if t.starts_with("func ") || t.starts_with("global ") {
        return true;
    }
    // `NAME = …` or `NAME := …` (a bare-identifier/index LHS), not `==`/`!=`/…
    if let Some(eq) = t.find('=') {
        if eq == 0 || matches!(t.as_bytes().get(eq + 1), Some(b'=')) {
            return false;
        }
        if matches!(t.as_bytes().get(eq.wrapping_sub(1)), Some(b'!' | b'<' | b'>')) {
            return false;
        }
        let lhs = t[..eq].trim_end().trim_end_matches(':').trim();
        return !lhs.is_empty()
            && lhs.chars().next().map(|c| c.is_alphabetic() || c == '_').unwrap_or(false)
            && lhs.chars().all(|c| c.is_alphanumeric() || "_.[] ".contains(c));
    }
    false
}

/// The default run path (and `j2 run`): try the fast AST interpreter (no
/// compile → instant, and no backend toolchain touched at all), falling back to a
/// hidden native compile for anything it doesn't support. The interpreter is
/// byte-identical to the compiled path (gated by the differential harness), so
/// this only changes speed/startup, never behavior.
///
/// Compilation is FORCED (interpreter skipped) when: `J2_NO_NATIVE=1` (the
/// dynamic-compile oracle), `J2_FORCE_NATIVE=1` (native AOT semantics: also
/// what `j2 build` uses), or program args are forwarded (the interpreter has no
/// program argv). For maximum speed on heavy kernels, use `j2 build` (native +
/// auto-parallel) or set `J2_FORCE_NATIVE=1`.
fn run_interp_or_compile(file: &str, extra: &[String]) -> ExitCode {
    let force_compile =
        env::var("J2_NO_NATIVE").is_ok() || env::var("J2_FORCE_NATIVE").is_ok();
    if !force_compile && extra.is_empty() {
        if let Ok(src) = fs::read_to_string(file) {
            let path = Path::new(file);
            // Same front of the pipeline as the compiler: resolve imports → lex
            // → parse. Only on full success do we attempt interpretation.
            if let Ok(expanded) = j2_compiler::imports::preprocess(&src, Some(path)) {
                if let Ok(toks) = j2_compiler::lexer::tokenize(&expanded) {
                    if let Ok(prog) = j2_compiler::parser::parse(&toks) {
                        // Only interpret a program the interpreter FULLY
                        // supports - never partially interpret (its side
                        // effects would be duplicated by the compile fallback).
                        // A supported program runs to completion or errors as a
                        // genuine J error; it never falls through after output.
                        if interp::supported(&prog) {
                            if let Some(code) = interp::try_run(&prog) {
                                return ExitCode::from(code as u8);
                            }
                        }
                    }
                }
            }
        }
    }
    // Fallback / forced: full compilation (handles everything the interpreter
    // doesn't, forwards program args, and reports rich parse/lex diagnostics).
    // Each invocation owns its executable. Concurrent runs of the same source
    // may use different compiler modes; overwriting a running executable can
    // also terminate that process on macOS.
    let executable = env::temp_dir().join(format!("j2_run_{}", std::process::id()));
    let result = build_and_run(file, &executable.to_string_lossy(), extra);
    let _ = fs::remove_file(&executable);
    match result {
        Ok(code) => ExitCode::from(code),
        Err(e) => { eprintln!("{}", e); ExitCode::from(1) }
    }
}

/// `j2 test [dir]`: run every `*.j2` under `dir` (default `j2-tests`) and report
/// a pass/fail summary. Each file is run by re-invoking this binary on it (so
/// it inherits the full build+cache+run path) with all capabilities granted
/// and stdin closed; a test passes iff it exits 0 (a failed `assert` or any
/// runtime error exits non-zero). Output is captured and only shown for
/// failures.
fn run_tests(dir: &str) -> ExitCode {
    let me = match env::current_exe() {
        Ok(p) => p,
        Err(e) => { eprintln!("j2 test: cannot find self: {}", e); return ExitCode::from(2); }
    };
    let mut files: Vec<PathBuf> = match fs::read_dir(dir) {
        Ok(rd) => rd.filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().map(|x| x == "j2").unwrap_or(false))
            .collect(),
        Err(e) => { eprintln!("j2 test: cannot read {}: {}", dir, e); return ExitCode::from(2); }
    };
    files.sort();
    if files.is_empty() {
        eprintln!("j2 test: no .j2 files in {}", dir);
        return ExitCode::from(2);
    }
    let mut passed = 0usize;
    let mut failed: Vec<String> = Vec::new();
    for f in &files {
        let name = f.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
        let out = Command::new(&me)
            .arg(f)
            .env("J2_TRUSTED", "1")
            .env("J2_ALLOW_FS", "1")
            .env("J2_ALLOW_PROC", "1")
            .env("J2_ALLOW_NET", "1")
            .stdin(Stdio::null())
            .output();
        match out {
            Ok(o) if o.status.success() => { passed += 1; println!("ok    {}", name); }
            Ok(o) => {
                failed.push(name.clone());
                let tail: String = String::from_utf8_lossy(&o.stderr).lines().rev().take(1).collect();
                println!("FAIL  {}  ({})", name, tail);
            }
            Err(e) => { failed.push(name.clone()); println!("FAIL  {}  ({})", name, e); }
        }
    }
    println!("----------------------------------------");
    println!("test: {} passed, {} failed (of {})", passed, failed.len(), files.len());
    if failed.is_empty() { ExitCode::SUCCESS } else { ExitCode::from(1) }
}

/// Reindent J source by bracket nesting depth. A deliberately conservative,
/// text-based formatter: it only normalizes leading indentation (4 spaces per
/// `{`/`[`/`(` level), preserving comments, blank lines, string contents, and
/// everything else verbatim: so it can never lose a comment or break code
/// (worst case: wrong indent). String- and comment-aware so brackets inside
/// `"..."`, `"""..."""`, or after `#` don't shift depth. (A full
/// spacing/alignment formatter needs comment-trivia in the AST: future work.)
fn format_source(src: &str) -> String {
    let mut out = String::new();
    let mut depth: i32 = 0;
    let mut in_triple = false;
    for raw in src.lines() {
        if in_triple {
            out.push_str(raw);
            out.push('\n');
            if raw.contains("\"\"\"") { in_triple = false; }
            continue;
        }
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            out.push('\n');
            continue;
        }
        let lead = trimmed.chars().take_while(|c| matches!(c, '}' | ']' | ')')).count() as i32;
        let ind = (depth - lead).max(0) as usize;
        out.push_str(&"    ".repeat(ind));
        out.push_str(trimmed);
        out.push('\n');
        depth += line_bracket_delta(trimmed, &mut in_triple);
        if depth < 0 { depth = 0; }
    }
    out
}

/// Net bracket depth change across one line, skipping brackets inside string
/// literals (`"..."` with `\"` escapes, and `"""`) and `#` comments. Sets
/// `in_triple` if a triple-quoted string is left open at end of line.
fn line_bracket_delta(line: &str, in_triple: &mut bool) -> i32 {
    let b = line.as_bytes();
    let mut i = 0usize;
    let mut delta = 0i32;
    let mut in_str = false;
    while i < b.len() {
        let c = b[i];
        if in_str {
            if c == b'\\' { i += 2; continue; }
            if c == b'"' { in_str = false; }
            i += 1;
            continue;
        }
        if c == b'"' && b.get(i + 1) == Some(&b'"') && b.get(i + 2) == Some(&b'"') {
            // triple-quoted string: find the closing """ on this line, else open.
            match line[i + 3..].find("\"\"\"") {
                Some(pos) => { i += 3 + pos + 3; }
                None => { *in_triple = true; return delta; }
            }
            continue;
        }
        match c {
            b'#' => return delta,
            b'"' => { in_str = true; i += 1; }
            b'{' | b'[' | b'(' => { delta += 1; i += 1; }
            b'}' | b']' | b')' => { delta -= 1; i += 1; }
            _ => { i += 1; }
        }
    }
    delta
}

/// A minimal stateful REPL. There is no standalone AST interpreter (J's
/// "dynamic" path is generated Rust), so each line is compiled: the build
/// cache makes unchanged sessions instant, but a new line pays one compile.
/// Definitions/bindings accumulate (persist) and re-run silently; bare
/// expressions are wrapped in `print(...)` and shown.
fn repl() -> ExitCode {
    use std::io::Write;
    eprintln!("J REPL: definitions/bindings persist; expressions are printed. `:quit` to exit.");
    eprintln!("(each line compiles to native; the build cache keeps re-runs fast.)");
    let mut session: Vec<String> = Vec::new();
    let stdin = std::io::stdin();
    let pid = std::process::id();
    let src_tmp = std::env::temp_dir().join(format!("j2_repl_{}.j2", pid));
    let bin_tmp = std::env::temp_dir().join(format!("j2_repl_bin_{}", pid));
    loop {
        eprint!("j> ");
        let _ = std::io::stderr().flush();
        let mut line = String::new();
        match stdin.read_line(&mut line) {
            Ok(0) => break,
            Ok(_) => {}
            Err(_) => break,
        }
        let line = line.trim_end().to_string();
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        if t == ":quit" || t == ":q" {
            break;
        }
        let is_def = is_repl_definition(t);
        let mut prog = session.join("\n");
        prog.push('\n');
        if is_def {
            prog.push_str(&line);
        } else if t.starts_with("print(") || t.starts_with("print ") {
            prog.push_str(&line);
        } else {
            prog.push_str(&format!("print({})", line));
        }
        if fs::write(&src_tmp, &prog).is_err() {
            eprintln!("repl: could not write session");
            continue;
        }
        match build_and_run(
            src_tmp.to_str().unwrap_or(""),
            bin_tmp.to_str().unwrap_or(""),
            &[],
        ) {
            Ok(_) => {
                if is_def {
                    session.push(line);
                }
            }
            Err(e) => eprintln!("{}", e),
        }
    }
    let _ = fs::remove_file(&src_tmp);
    let _ = fs::remove_file(&bin_tmp);
    ExitCode::SUCCESS
}

fn build(src_path: &str, out_path: &str, run_after: bool) -> Result<u8, String> {
    let src = fs::read_to_string(src_path).map_err(|e| format!("read {}: {}", src_path, e))?;
    // Resolve the complete import graph before looking in the binary cache.
    // An edited/deleted module, or the same entry text in another directory,
    // must never reuse a binary compiled from different source.
    let expanded = j2_compiler::imports::preprocess(&src, Some(Path::new(src_path)))
        .map_err(|e| e.render(&src, src_path))?;
    let runtime_path = locate_j_runtime()?;

    // Content-hash build cache: a program with identical source + compile mode
    // + unchanged deps (j2_runtime sources, forked backend-cc, this driver) reuses
    // its already-built binary, skipping transpile + cargo entirely - turning
    // a re-run from seconds into milliseconds. Disable with J2_NO_CACHE=1.
    // A matcher dump is a request to observe this compilation. Reusing a
    // cached binary would suppress it entirely. Match the wrapper's nonempty
    // environment-variable convention for enabling -Z par-dump.
    let par_dump = env::var_os("J2_PAR_DUMP").is_some_and(|v| !v.is_empty());
    let no_cache = env::var("J2_NO_CACHE").map(|v| v == "1").unwrap_or(false) || par_dump;
    let cache_key = build_cache_key(&expanded, Path::new(src_path), &runtime_path);
    let cache_dir = env::temp_dir().join("j2_bincache");
    let _ = fs::create_dir_all(&cache_dir);
    let cached = cache_dir.join(&cache_key);
    if !no_cache && cache_is_fresh(&cached, &runtime_path) {
        if fs::copy(&cached, out_path).is_ok() {
            if run_after {
                let status = Command::new(out_path)
                    .status()
                    .map_err(|e| format!("exec {}: {}", out_path, e))?;
                return Ok(status.code().unwrap_or(1) as u8);
            }
            return Ok(0);
        }
    }

    // The imports were already expanded for the key. Parse that exact snapshot
    // so a concurrently edited import cannot make the key and binary disagree.
    let tokens = j2_compiler::lexer::tokenize(&expanded)
        .map_err(|e| e.render(&expanded, src_path))?;
    let program = j2_compiler::parser::parse(&tokens)
        .map_err(|e| e.render(&expanded, src_path))?;
    let rust_src = j2_compiler::lower::lower_program(&program).rust_src;

    // Create a temp Cargo project that depends on j2_runtime. The package name
    // is unique per process so a shared (cached) target dir can hold many
    // concurrent builds without racing on the output binary.
    let pkg = format!("j2_c_{}", std::process::id());
    let tmp = std::env::temp_dir().join(format!("j2_build_{}", std::process::id()));
    let _ = fs::remove_dir_all(&tmp);
    fs::create_dir_all(tmp.join("src")).map_err(|e| format!("mkdir: {}", e))?;

    let cargo_toml = format!(
        r#"[package]
name = "{pkg}"
version = "0.1.0"
edition = "2021"

[[bin]]
name = "{pkg}"
path = "src/main.rs"

[dependencies]
j2_runtime = {{ path = "{}" }}

[profile.release]
opt-level = 3
"#,
        runtime_path.display()
    );
    fs::write(tmp.join("Cargo.toml"), cargo_toml).map_err(|e| format!("write Cargo.toml: {}", e))?;
    fs::write(tmp.join("src/main.rs"), rust_src).map_err(|e| format!("write main.rs: {}", e))?;
    // Pin the dependency graph for reproducible, supply-chain-safe builds: a
    // canonical Cargo.lock (baked into the driver) is copied into every
    // generated crate so j2_runtime's ~184 transitive deps resolve to the same
    // audited versions on every build instead of "latest compatible". cargo
    // only rewrites the per-build root entry; the locked deps are reused as-is.
    // Regenerate src/bin/j2/program.lock with `cargo generate-lockfile` after
    // changing j2_runtime's dependencies.
    fs::write(tmp.join("Cargo.lock"), include_str!("program.lock"))
        .map_err(|e| format!("write Cargo.lock: {}", e))?;

    // Self-contained, offline builds: when a vendored dependency directory is
    // present (bundled install), point cargo at it and build `--offline`, so a
    // generated program needs NO crates.io network and NO `~/.cargo` registry -
    // the user only ever had to install J. Absent (dev box), build online.
    let vendor = locate_j_vendor();
    let offline = vendor.is_some();
    if let Some(v) = &vendor {
        let cfg = format!(
            "[source.crates-io]\nreplace-with = \"vendored-sources\"\n\n[source.vendored-sources]\ndirectory = \"{}\"\n",
            v.display()
        );
        fs::create_dir_all(tmp.join(".cargo")).map_err(|e| format!("mkdir .cargo: {}", e))?;
        fs::write(tmp.join(".cargo").join("config.toml"), cfg)
            .map_err(|e| format!("write cargo config: {}", e))?;
    }

    // Auto-parallelism (the headline novelty) requires building the generated
    // backend with the J *fork* of backend-cc (which runs the MIR parallelizer and
    // links the forked std's parallel_runtime). This is the default; it's
    // disabled by `J2_PARALLEL=0`. We use a persistent, fork-keyed target dir
    // so j2_runtime + its deps compile once and are reused across runs. If the
    // forked build fails for any reason, we fall back to the system toolchain
    // so `j` never breaks.
    let parallel = env::var("J2_PARALLEL").map(|v| v != "0").unwrap_or(true);
    let forked = if parallel { locate_forked_backend() } else { None };

    // Parallelize ONLY the user's J crate, not its dependencies: the MIR
    // matchers run on every non-sysroot crate, and some (e.g. the
    // task-parallel `parallel_invoke` matcher on serde_json) miscompile
    // third-party code. We install a thin backend-cc *wrapper* that turns the
    // parallelizer ON (+ the `j2_parallelize` cfg that flips on the
    // `#[parallelize_associative_float]` opt-in) only when compiling the
    // generated J crate, and OFF for every dependency / build script.
    let wrapped = forked.as_ref().and_then(|r| {
        let cache = parallel_target_dir(r);
        let _ = fs::create_dir_all(&cache);
        write_backend_wrapper(&cache, r).ok().map(|w| (cache, w))
    });

    let cargo_bin = locate_bundled_cargo();
    let mut cargo_args: Vec<&str> = vec!["build", "--release", "--quiet"];
    if offline { cargo_args.push("--offline"); }
    let mut parallel_fell_back = false;
    let target_dir: PathBuf = if let Some((cache, wrapper)) = wrapped {
        // Capture cargo/backend-cc output rather than inheriting it: on success the
        // user sees nothing of the backend toolchain (J is meant to feel like its
        // own language); only their program's own output shows when it runs.
        let out = Command::new(&cargo_bin)
            .current_dir(&tmp)
            .args(&cargo_args)
            .env("RUSTC", &wrapper)
            .env("J2_TARGET_CRATE", &pkg)
            .env("CARGO_TARGET_DIR", &cache)
            .output()
            .map_err(|e| format!("invoke cargo (parallel): {}", e))?;
        if out.status.success() {
            if par_dump {
                eprint!("{}", String::from_utf8_lossy(&out.stderr));
            }
            cache
        } else {
            // Parallel build failed - fall back to the plain toolchain. Surface
            // the raw diagnostics only under J2_DEBUG (else they'd leak backend).
            if env::var("J2_DEBUG").is_ok() {
                eprint!("{}", String::from_utf8_lossy(&out.stderr));
                eprintln!("j: note: parallel build failed; retrying without it");
            }
            parallel_fell_back = true;
            cargo_build_system(&tmp, &runtime_path)?
        }
    } else {
        cargo_build_system(&tmp, &runtime_path)?
    };

    let built = target_dir.join("release").join(&pkg);
    let final_path = PathBuf::from(out_path);
    fs::copy(&built, &final_path).map_err(|e| format!("copy binary {}: {}", built.display(), e))?;
    // Populate the content-hash cache so the next identical run is instant.
    // NEVER cache a serial binary produced by the parallel-build fallback:
    // caching it would pin this source to serial forever even after the
    // parallel toolchain recovers - retry the parallel path on the next run.
    if !no_cache && !parallel_fell_back {
        // Publish a complete binary in one step. A simultaneous cache reader
        // must never copy a partially written executable.
        let pending = cache_dir.join(format!(".{cache_key}.{}", std::process::id()));
        if fs::copy(&built, &pending).is_ok() {
            let _ = fs::rename(&pending, &cached);
        }
        let _ = fs::remove_file(&pending);
    }
    if run_after {
        let status = Command::new(&final_path)
            .status()
            .map_err(|e| format!("exec {}: {}", final_path.display(), e))?;
        return Ok(status.code().unwrap_or(1) as u8);
    }
    Ok(0)
}

/// Build with the bundled/system toolchain, reusing dependencies between source
/// edits and separate generated projects. Return the actual target directory so
/// copying the result also works with an explicit CARGO_TARGET_DIR override.
fn cargo_build_system(tmp: &Path, runtime_path: &Path) -> Result<PathBuf, String> {
    let cargo_bin = locate_bundled_cargo();
    let caller_dir = env::current_dir().map_err(|e| format!("current directory: {}", e))?;
    let target_dir = system_target_dir(
        &cargo_bin, runtime_path, env::var_os("CARGO_TARGET_DIR").as_deref(), &caller_dir,
    );
    fs::create_dir_all(&target_dir).map_err(|e| format!("mkdir {}: {}", target_dir.display(), e))?;
    let mut args: Vec<&str> = vec!["build", "--release", "--quiet"];
    if locate_j_vendor().is_some() { args.push("--offline"); }
    // Capture (don't inherit) so no backend toolchain output reaches the user.
    let out = Command::new(&cargo_bin)
        .current_dir(tmp)
        .args(&args)
        .env("CARGO_TARGET_DIR", &target_dir)
        .output()
        .map_err(|e| format!("invoke cargo: {}", e))?;
    if !out.status.success() {
        // A failure here means the lowered program didn't compile - a J
        // internal error, not something the user wrote in backend. Show a clean
        // J-level message; the raw backend-cc/cargo diagnostics only under J2_DEBUG.
        if env::var("J2_DEBUG").is_ok() {
            eprint!("{}", String::from_utf8_lossy(&out.stderr));
        }
        return Err(
            "internal error: could not compile the program to native code \
             (re-run with J2_DEBUG=1 for details, or J2_NO_NATIVE=1 to use the interpreter)"
                .to_string(),
        );
    }
    Ok(target_dir)
}

/// An explicit target directory belongs to the caller; resolve relative paths
/// before switching Cargo's working directory to the generated project. The
/// default is independent of the program's path/PID, so dependency artifacts
/// survive edits. Cargo fingerprints still validate compiler, flags, and sources.
fn system_target_dir(
    cargo_bin: &std::ffi::OsStr,
    runtime_path: &Path,
    override_dir: Option<&std::ffi::OsStr>,
    caller_dir: &Path,
) -> PathBuf {
    if let Some(value) = override_dir.filter(|value| !value.is_empty()) {
        let path = PathBuf::from(value);
        return if path.is_absolute() { path } else { caller_dir.join(path) };
    }
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut h = DefaultHasher::new();
    cargo_bin.hash(&mut h);
    runtime_path.canonicalize().unwrap_or_else(|_| runtime_path.to_path_buf()).hash(&mut h);
    env::var_os("RUSTC").hash(&mut h);
    env::var_os("RUSTUP_TOOLCHAIN").hash(&mut h);
    env::temp_dir().join(format!("j2_serial_target_{:x}", h.finish()))
}

/// A persistent target dir for forked (parallel) builds, keyed by the fork's
/// path so swapping toolchains invalidates cleanly. Caching here amortizes
/// the one-time cost of compiling j2_runtime + its dependency tree.
fn parallel_target_dir(rustc: &Path) -> PathBuf {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut h = DefaultHasher::new();
    rustc.hash(&mut h);
    std::env::temp_dir().join(format!("j2_parallel_target_{:x}", h.finish()))
}

/// Key for the build cache: the fully expanded J source and entry location,
/// plus the compile-mode environment, runtime crate, and backend toolchain. Capability
/// flags (fs/proc/net) are NOT included: they only affect runtime behavior
/// (the binary reads them from the environment), not the binary itself.
fn build_cache_key(expanded: &str, entry_path: &Path, runtime_path: &Path) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut h = DefaultHasher::new();
    expanded.hash(&mut h);
    // Distinct driver builds can emit different code for the same source.
    // Keep their caches separate even when both are running concurrently.
    if let Ok(driver) = env::current_exe() {
        driver.hash(&mut h);
        if let Ok(metadata) = fs::metadata(driver) {
            metadata.len().hash(&mut h);
            if let Ok(modified) = metadata.modified() { modified.hash(&mut h); }
        }
    }
    entry_path.canonicalize().unwrap_or_else(|_| entry_path.to_path_buf()).hash(&mut h);
    // Hash effective code-generation modes, not spelling: unset and "0" mean
    // the same thing for NO_NATIVE, while unset and "1" both enable PARALLEL.
    // This lets an interpreter's compile fallback reuse a native build.
    for key in ["J2_PARALLEL", "J2_NO_NATIVE", "J2_NO_NESTED", "J2_TRUSTED"] {
        compile_flag_enabled(key, env::var(key).ok().as_deref()).hash(&mut h);
    }
    // Other toolchain settings can change generated code. J2_PATH is already
    // represented by the expanded import contents above.
    for key in [
        "J2_CARGO", "J2_BACKEND", "J2_VENDOR", "RUSTC", "RUSTUP_TOOLCHAIN", "RUSTC_WRAPPER",
        "RUSTC_WORKSPACE_WRAPPER", "RUSTFLAGS", "CARGO_ENCODED_RUSTFLAGS", "CARGO_BUILD_TARGET",
    ] {
        env::var_os(key).hash(&mut h);
    }
    runtime_path.display().to_string().hash(&mut h);
    if let Some(r) = locate_forked_backend() {
        r.display().to_string().hash(&mut h);
        // Content identity, not just path: a re-signed or updated toolchain
        // at the same location must invalidate previously cached binaries.
        if let Ok(m) = fs::metadata(&r) {
            m.len().hash(&mut h);
            if let Ok(t) = m.modified() {
                if let Ok(d) = t.duration_since(std::time::UNIX_EPOCH) {
                    d.as_secs().hash(&mut h);
                }
            }
        }
    }
    format!("j_{:x}", h.finish())
}

/// Match the actual frontend/build flag checks when constructing a cache key.
fn compile_flag_enabled(key: &str, value: Option<&str>) -> bool {
    if key == "J2_PARALLEL" { value != Some("0") } else { value == Some("1") }
}

fn file_mtime(p: &Path) -> Option<std::time::SystemTime> {
    fs::metadata(p).and_then(|m| m.modified()).ok()
}

/// Newest modification time of any file under `dir` (recursive), or None.
fn newest_mtime_in(dir: &Path) -> Option<std::time::SystemTime> {
    let mut newest: Option<std::time::SystemTime> = None;
    if let Ok(rd) = fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            let t = if p.is_dir() { newest_mtime_in(&p) } else { file_mtime(&p) };
            if let Some(t) = t {
                newest = Some(newest.map_or(t, |n| n.max(t)));
            }
        }
    }
    newest
}

/// A cached binary is reusable only if it exists and is newer than every
/// build input that isn't already folded into the cache key: the j2_runtime
/// sources, the forked backend-cc, and this driver binary. So changing the runtime
/// / fork / driver invalidates the cache automatically.
fn cache_is_fresh(cached: &Path, runtime_path: &Path) -> bool {
    let Some(cached_t) = file_mtime(cached) else { return false; };
    let mut dep_t = env::current_exe().ok().and_then(|e| file_mtime(&e));
    if let Some(rt) = newest_mtime_in(&runtime_path.join("src")) {
        dep_t = Some(dep_t.map_or(rt, |d| d.max(rt)));
    }
    for file in [runtime_path.join("Cargo.toml"), runtime_path.join("Cargo.lock")] {
        if let Some(t) = file_mtime(&file) {
            dep_t = Some(dep_t.map_or(t, |d| d.max(t)));
        }
    }
    if let Some(r) = locate_forked_backend() {
        if let Some(t) = file_mtime(&r) {
            dep_t = Some(dep_t.map_or(t, |d| d.max(t)));
        }
    }
    match dep_t {
        Some(d) => cached_t > d,
        None => true,
    }
}

/// Write a thin backend-cc wrapper that enables the auto-parallelizer only for
/// the J crate named by `$J2_TARGET_CRATE` (set per build), leaving every
/// dependency / build script un-parallelized. This is the safe way to get
/// per-crate codegen flags (RUSTFLAGS / `cargo backend-cc --` apply too broadly).
///
/// The wrapper is written at a STABLE path (in the shared cache dir, keyed
/// by the fork) with content independent of the per-run crate name, so cargo
/// fingerprints backend-cc identically across runs and reuses cached deps. The
/// target crate is selected at build time via the `J2_TARGET_CRATE` env var.
fn write_backend_wrapper(cache: &Path, rustc: &Path) -> Result<PathBuf, String> {
    let script = format!(
        r#"#!/bin/sh
# rustc wrapper: parallelize only the J crate named by $J2_TARGET_CRATE.
mine=0
prev=
for a in "$@"; do
  if [ "$prev" = "--crate-name" ] && [ -n "$J2_TARGET_CRATE" ] && [ "$a" = "$J2_TARGET_CRATE" ]; then mine=1; fi
  prev=$a
done
# `--cap-lints allow`: the generated Rust is machine-emitted, so its lints are
# noise to a J user. Capping them keeps the toolchain silent (J feels like its
# own language); genuine compile *errors* are unaffected.
if [ "$mine" = 1 ]; then
  exec "{rustc}" "$@" --cap-lints allow --cfg j2_parallelize -C parallelize=on ${{J2_PAR_DUMP:+-Z par-dump}}
else
  exec "{rustc}" "$@" --cap-lints allow -C parallelize=off
fi
"#,
        rustc = rustc.display(),
    );
    let path = cache.join("j2_backend_wrapper.sh");
    // Another compiler can execute this shared wrapper before either build
    // obtains Cargo's lock. Publish a complete executable inode so readers
    // never observe an empty/partial script or lose execute permissions.
    use std::sync::atomic::{AtomicUsize, Ordering};
    static NEXT_WRAPPER: AtomicUsize = AtomicUsize::new(0);
    let pending = cache.join(format!(
        ".j2_backend_wrapper.{}.{}", std::process::id(), NEXT_WRAPPER.fetch_add(1, Ordering::Relaxed)
    ));
    let published = (|| -> Result<(), String> {
        fs::write(&pending, script).map_err(|e| format!("write wrapper: {}", e))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = fs::metadata(&pending).map_err(|e| e.to_string())?.permissions();
            perms.set_mode(0o755);
            fs::set_permissions(&pending, perms).map_err(|e| e.to_string())?;
        }
        fs::rename(&pending, &path).map_err(|e| format!("publish wrapper: {}", e))
    })();
    let _ = fs::remove_file(&pending);
    published?;
    Ok(path)
}

/// Locate the J fork's stage1 rustc, which enables auto-parallelization.
/// Honors `J2_BACKEND`, else walks up from the binary / cwd for
/// `build/<triple>/stage1/bin/rustc`. Returns None if absent (→ system build).
fn locate_forked_backend() -> Option<PathBuf> {
    if let Ok(p) = env::var("J2_BACKEND") {
        let p = PathBuf::from(p);
        return if p.exists() { Some(p) } else { None };
    }
    let mut roots: Vec<PathBuf> = Vec::new();
    // Canonicalize so a PATH symlink (e.g. /usr/local/bin/j2 -> ~/.j2/bin/j2, how
    // the installer links it) resolves to the REAL bundle dir before we walk up
    // to find the toolchain / vendor / runtime beside the binary.
    if let Ok(exe) = env::current_exe().and_then(|p| p.canonicalize()) {
        let mut cur = exe.parent().map(|p| p.to_path_buf());
        while let Some(dir) = cur {
            roots.push(dir.clone());
            cur = dir.parent().map(|p| p.to_path_buf());
        }
    }
    if let Ok(cwd) = env::current_dir() {
        roots.push(cwd);
    }
    for root in roots {
        // Bundled install: `<root>/toolchain/bin/rustc` (clean layout that
        // hides the rust-fork structure; cargo + sysroot live beside it).
        let bundled = root.join("toolchain").join("bin").join("rustc");
        if bundled.exists() {
            return Some(bundled);
        }
        // Dev tree: `<root>/build/*/stage1/bin/rustc` and `<root>/rust/build/...`.
        for base in [root.join("build"), root.join("j2").join("build")] {
            if let Ok(entries) = fs::read_dir(&base) {
                for e in entries.flatten() {
                    let cand = e.path().join("stage1").join("bin").join("rustc");
                    if cand.exists() {
                        return Some(cand);
                    }
                }
            }
        }
    }
    None
}

/// Locate the cargo to drive generated-crate builds. A bundled J install ships
/// its own cargo next to the forked backend-cc, so a user needs NO system backend;
/// `J2_CARGO` overrides; on a dev box we fall back to the system `cargo`.
fn locate_bundled_cargo() -> std::ffi::OsString {
    if let Ok(p) = env::var("J2_CARGO") {
        if Path::new(&p).exists() {
            return p.into();
        }
    }
    if let Some(rustc) = locate_forked_backend() {
        if let Some(bin) = rustc.parent() {
            let c = bin.join("cargo");
            if c.exists() {
                return c.into_os_string();
            }
        }
    }
    "cargo".into()
}

/// Locate the vendored dependency directory (the ~173 crates J_runtime needs),
/// so generated programs build fully OFFLINE: no crates.io, no `~/.cargo`.
/// `J2_VENDOR` overrides; otherwise discovered relative to the binary/cwd. When
/// absent (dev box without a vendor dir) we build online as before.
fn locate_j_vendor() -> Option<PathBuf> {
    if let Ok(p) = env::var("J2_VENDOR") {
        let p = PathBuf::from(p);
        return if p.exists() { Some(p) } else { None };
    }
    let mut roots: Vec<PathBuf> = Vec::new();
    // Canonicalize so a PATH symlink (e.g. /usr/local/bin/j2 -> ~/.j2/bin/j2, how
    // the installer links it) resolves to the REAL bundle dir before we walk up
    // to find the toolchain / vendor / runtime beside the binary.
    if let Ok(exe) = env::current_exe().and_then(|p| p.canonicalize()) {
        let mut cur = exe.parent().map(|p| p.to_path_buf());
        while let Some(dir) = cur {
            roots.push(dir.clone());
            cur = dir.parent().map(|p| p.to_path_buf());
        }
    }
    if let Ok(cwd) = env::current_dir() {
        roots.push(cwd);
    }
    for root in roots {
        for cand in [root.join("j2-vendor"), root.join("j2").join("j2-vendor")] {
            if cand.is_dir() {
                return Some(cand);
            }
        }
    }
    None
}

fn build_and_run(src_path: &str, out_path: &str, extra_args: &[String]) -> Result<u8, String> {
    // Build without auto-running (so we can attach forwarded args).
    build(src_path, out_path, false)?;
    let final_path = PathBuf::from(out_path);
    let status = Command::new(&final_path)
        .args(extra_args)
        .status()
        .map_err(|e| format!("exec {}: {}", final_path.display(), e))?;
    Ok(status.code().unwrap_or(1) as u8)
}

/// Locate library/j2_runtime/ at a stable path relative to the j binary.
/// We try the source-tree layout first; users running outside the source
/// tree can set J2_RUNTIME_PATH explicitly.
fn locate_j_runtime() -> Result<PathBuf, String> {
    if let Ok(p) = env::var("J2_RUNTIME_PATH") {
        return Ok(PathBuf::from(p));
    }
    // Try walking up from the j binary's directory.
    // Canonicalize so a PATH symlink (e.g. /usr/local/bin/j2 -> ~/.j2/bin/j2, how
    // the installer links it) resolves to the REAL bundle dir before we walk up
    // to find the toolchain / vendor / runtime beside the binary.
    if let Ok(exe) = env::current_exe().and_then(|p| p.canonicalize()) {
        let mut cur = exe.parent().map(|p| p.to_path_buf());
        while let Some(dir) = cur {
            let candidate = dir.join("library/j2_runtime");
            if candidate.exists() { return Ok(candidate); }
            let candidate2 = dir.join("rust/library/j2_runtime");
            if candidate2.exists() { return Ok(candidate2); }
            cur = dir.parent().map(|p| p.to_path_buf());
        }
    }
    // Final fallback: relative to current working directory.
    let cwd = env::current_dir().map_err(|e| e.to_string())?;
    let candidate = cwd.join("rust/library/j2_runtime");
    if candidate.exists() { return Ok(candidate); }
    let candidate = cwd.join("library/j2_runtime");
    if candidate.exists() { return Ok(candidate); }
    Err("could not locate library/j2_runtime/; set J2_RUNTIME_PATH".into())
}

#[cfg(test)]
mod cache_tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let path = env::temp_dir().join(format!(
                "j2-cache-test-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        fn write(&self, file: &str, text: &str) -> PathBuf {
            let path = self.0.join(file);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, text).unwrap();
            path
        }

        fn key(&self, entry: &Path) -> String {
            let source = fs::read_to_string(entry).unwrap();
            let expanded = j2_compiler::imports::preprocess(&source, Some(entry)).unwrap();
            build_cache_key(&expanded, entry, Path::new("library/j2_runtime"))
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) { let _ = fs::remove_dir_all(&self.0); }
    }

    #[test]
    fn backend_wrapper_publication_preserves_open_and_concurrent_readers() {
        use std::io::Read;
        use std::sync::Barrier;

        let f = Fixture::new();
        let first = Path::new("/test/backend/first");
        let second = Path::new("/test/backend/second");
        let path = write_backend_wrapper(&f.0, first).unwrap();
        let first_script = fs::read_to_string(&path).unwrap();
        // A shell may have already opened the old script when another build
        // replaces it. That descriptor must continue to see the original file.
        let mut reader = fs::File::open(&path).unwrap();
        assert_eq!(write_backend_wrapper(&f.0, second).unwrap(), path);
        let second_script = fs::read_to_string(&path).unwrap();
        let mut open_script = String::new();
        reader.read_to_string(&mut open_script).unwrap();
        assert_eq!(open_script, first_script);
        assert_ne!(first_script, second_script);

        let ready = Barrier::new(3);
        std::thread::scope(|scope| {
            for backend in [first, second] {
                let cache = &f.0;
                let ready = &ready;
                scope.spawn(move || {
                    ready.wait();
                    for _ in 0..64 {
                        write_backend_wrapper(cache, backend).unwrap();
                    }
                });
            }
            ready.wait();
            for _ in 0..512 {
                let observed = fs::read_to_string(&path).unwrap();
                assert!(observed == first_script || observed == second_script,
                    "reader observed a partial or mixed wrapper");
            }
        });
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o755);
        }
        assert_eq!(fs::read_dir(&f.0).unwrap().count(), 1, "staging files should be removed");
    }

    #[test]
    fn compile_mode_cache_flags_normalize_equivalent_settings() {
        assert_eq!(compile_flag_enabled("J2_PARALLEL", None), compile_flag_enabled("J2_PARALLEL", Some("1")));
        assert!(!compile_flag_enabled("J2_PARALLEL", Some("0")));
        for key in ["J2_NO_NATIVE", "J2_NO_NESTED", "J2_TRUSTED"] {
            assert_eq!(compile_flag_enabled(key, None), compile_flag_enabled(key, Some("0")));
            assert!(!compile_flag_enabled(key, Some("false")));
            assert!(compile_flag_enabled(key, Some("1")));
        }
    }

    #[test]
    fn serial_dependency_cache_is_shared_across_program_locations() {
        let f = Fixture::new();
        let runtime = f.0.join("runtime");
        let first = system_target_dir("cargo".as_ref(), &runtime, None, &f.0.join("project-a"));
        let second = system_target_dir("cargo".as_ref(), &runtime, None, &f.0.join("project-b"));
        assert_eq!(first, second);
        assert_ne!(first, system_target_dir("other-cargo".as_ref(), &runtime, None, &f.0));
        assert_ne!(first, system_target_dir("cargo".as_ref(), &f.0.join("other-runtime"), None, &f.0));
    }

    #[test]
    fn serial_dependency_cache_honors_target_directory_override() {
        let f = Fixture::new();
        let runtime = f.0.join("runtime");
        let absolute = f.0.join("chosen-target");
        assert_eq!(
            system_target_dir("cargo".as_ref(), &runtime, Some(absolute.as_os_str()), &f.0),
            absolute,
        );
        assert_eq!(
            system_target_dir("cargo".as_ref(), &runtime, Some("relative-target".as_ref()), &f.0),
            f.0.join("relative-target"),
        );
    }

    #[test]
    fn cache_tracks_transitive_import_contents() {
        let f = Fixture::new();
        let entry = f.write("main.j2", "import \"module.j2\"\nprint(answer)\n");
        f.write("module.j2", "import \"nested.j2\"\n");
        f.write("nested.j2", "answer = 41\n");
        let before = f.key(&entry);
        assert_eq!(before, f.key(&entry), "unchanged imports should reuse the key");
        f.write("nested.j2", "answer = 42\n");
        assert_ne!(before, f.key(&entry), "an import edit must invalidate the cache");
    }

    #[test]
    fn identical_entries_resolve_imports_in_their_own_directories() {
        let f = Fixture::new();
        let source = "import \"value.j2\"\nprint(answer)\n";
        let a = f.write("a/main.j2", source);
        let b = f.write("b/main.j2", source);
        f.write("a/value.j2", "answer = 1\n");
        f.write("b/value.j2", "answer = 2\n");
        assert_ne!(f.key(&a), f.key(&b));
        // Even identical expansion keeps location-sensitive native escape
        // hatches and future source metadata separated.
        f.write("b/value.j2", "answer = 1\n");
        assert_ne!(f.key(&a), f.key(&b));
    }

    #[test]
    fn deleted_import_is_reported_before_any_cache_lookup() {
        let f = Fixture::new();
        let entry = f.write("main.j2", "import \"value.j2\"\nprint(answer)\n");
        let module = f.write("value.j2", "answer = 1\n");
        let _ = f.key(&entry);
        fs::remove_file(module).unwrap();
        let err = build(entry.to_str().unwrap(), f.0.join("out").to_str().unwrap(), false)
            .expect_err("a cache hit must not hide a missing import");
        assert!(err.contains("cannot read") && err.contains("value.j2"), "{err}");
        assert!(!f.0.join("out").exists());
    }
}
