// j2 driver; lowers, cargo-builds, then runs

use std::env;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};

mod interp;

fn main() -> ExitCode {
    let mut args: Vec<String> = env::args().collect();
    // Capability flags to env vars, deny-by-default
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
    // Check args[1] only; program args pass through
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
            // fmt prints formatted source; -w rewrites file
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
            // Interpreter-first with hidden native-compile fallback
            let extra: Vec<String> = args.iter().skip(2).cloned().collect();
            run_interp_or_compile(file, &extra)
        }
        other => {
            eprintln!("j: unknown subcommand {:?}", other);
            ExitCode::from(2)
        }
    }
}

/// Print the `j` usage summary
fn print_usage() {
    println!("j2 {} (the J2 programming language)", env!("CARGO_PKG_VERSION"));
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

/// Heuristic for REPL definitions vs expressions
fn is_repl_definition(t: &str) -> bool {
    if t.starts_with("func ") || t.starts_with("global ") {
        return true;
    }
    // `NAME = ...` or `NAME := ...`
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

/// The default run path
fn run_interp_or_compile(file: &str, extra: &[String]) -> ExitCode {
    let force_compile =
        env::var("J2_NO_NATIVE").is_ok() || env::var("J2_FORCE_NATIVE").is_ok();
    if !force_compile && extra.is_empty() {
        if let Ok(src) = fs::read_to_string(file) {
            let path = Path::new(file);
            // Import, lex, parse; interpret only on success
            if let Ok(expanded) = j2_compiler::imports::preprocess(&src, Some(path)) {
                if let Ok(toks) = j2_compiler::lexer::tokenize(&expanded) {
                    if let Ok(prog) = j2_compiler::parser::parse(&toks) {
                        // Fully supported only; partial run duplicates effects
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
    // Fallback/forced full compile with rich diagnostics
    let out_name = format!("/tmp/j_{}", file.replace('/', "_").trim_end_matches(".j2"));
    match build_and_run(file, &out_name, extra) {
        Ok(code) => ExitCode::from(code),
        Err(e) => { eprintln!("{}", e); ExitCode::from(1) }
    }
}

/// Run all `*.j2` tests by re-invoking self
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

/// Reindent J source by bracket nesting depth
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

/// Bracket depth delta, skipping strings and comments
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
            // Triple-quoted string; find closer or stay open
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

/// A minimal stateful REPL
fn repl() -> ExitCode {
    use std::io::Write;
    eprintln!("J2 REPL. Definitions persist, expressions are printed. `:quit` to exit.");
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
    let runtime_path = locate_j_runtime()?;

    // Content-hash cache skips rebuild; J2_NO_CACHE=1 disables
    let no_cache = env::var("J2_NO_CACHE").map(|v| v == "1").unwrap_or(false);
    let cache_key = build_cache_key(&src, &runtime_path);
    let cache_dir = env::temp_dir().join("j2_bincache");
    let _ = fs::create_dir_all(&cache_dir);
    let cached = cache_dir.join(&cache_key);
    if !no_cache && cache_is_fresh(&cached, &runtime_path) {
        if fs::copy(&cached, out_path).is_ok() {
            if run_after {
                let status = Command::new(out_path)
                    .status()
                    .map_err(|e| format!("exec {}: {}", out_path, e))?;
                return Ok(status.code().unwrap_or(0) as u8);
            }
            return Ok(0);
        }
    }

    let rust_src = j2_compiler::compile_to_rust_with_path(&src, Path::new(src_path))
        .map_err(|e| e.render(&src, src_path))?;

    // Temp crate; per-process name avoids target races
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
    // Baked Cargo.lock pins deps; regen via generate-lockfile
    fs::write(tmp.join("Cargo.lock"), include_str!("program.lock"))
        .map_err(|e| format!("write Cargo.lock: {}", e))?;

    // Vendored deps present, build offline; else online
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

    // Fork auto-parallelizes by default; system toolchain fallback
    let parallel = env::var("J2_PARALLEL").map(|v| v != "0").unwrap_or(true);
    let forked = if parallel { locate_forked_backend() } else { None };

    // Wrapper parallelizes only J crate; deps miscompile
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
        // Capture cargo/backend-cc output rather than inheriting it
        let out = Command::new(&cargo_bin)
            .current_dir(&tmp)
            .args(&cargo_args)
            .env("RUSTC", &wrapper)
            .env("J2_TARGET_CRATE", &pkg)
            .env("CARGO_TARGET_DIR", &cache)
            .output()
            .map_err(|e| format!("invoke cargo (parallel): {}", e))?;
        if out.status.success() {
            cache
        } else {
            // Parallel build failed; raw errors need J2_DEBUG
            if env::var("J2_DEBUG").is_ok() {
                eprint!("{}", String::from_utf8_lossy(&out.stderr));
                eprintln!("j: note: parallel build failed; retrying without it");
            }
            parallel_fell_back = true;
            cargo_build_system(&tmp)?;
            tmp.join("target")
        }
    } else {
        cargo_build_system(&tmp)?;
        tmp.join("target")
    };

    let built = target_dir.join("release").join(&pkg);
    let final_path = PathBuf::from(out_path);
    fs::copy(&built, &final_path).map_err(|e| format!("copy binary {}: {}", built.display(), e))?;
    // Never cache fallback binary, pins serial forever
    if !no_cache && !parallel_fell_back {
        let _ = fs::copy(&built, &cached);
    }
    if run_after {
        let status = Command::new(&final_path)
            .status()
            .map_err(|e| format!("exec {}: {}", final_path.display(), e))?;
        return Ok(status.code().unwrap_or(0) as u8);
    }
    Ok(0)
}

/// Build temp project with bundled/system toolchain
fn cargo_build_system(tmp: &Path) -> Result<(), String> {
    let cargo_bin = locate_bundled_cargo();
    let mut args: Vec<&str> = vec!["build", "--release", "--quiet"];
    if locate_j_vendor().is_some() { args.push("--offline"); }
    // Capture output so toolchain noise stays hidden
    let out = Command::new(&cargo_bin)
        .current_dir(tmp)
        .args(&args)
        .output()
        .map_err(|e| format!("invoke cargo: {}", e))?;
    if !out.status.success() {
        // J internal error; raw diagnostics need J2_DEBUG
        if env::var("J2_DEBUG").is_ok() {
            eprint!("{}", String::from_utf8_lossy(&out.stderr));
        }
        return Err(
            "internal error: could not compile the program to native code \
             (re-run with J2_DEBUG=1 for details, or J2_NO_NATIVE=1 to use the interpreter)"
                .to_string(),
        );
    }
    Ok(())
}

/// A persistent target dir for forked
fn parallel_target_dir(rustc: &Path) -> PathBuf {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut h = DefaultHasher::new();
    rustc.hash(&mut h);
    std::env::temp_dir().join(format!("j2_parallel_target_{:x}", h.finish()))
}

/// Key for the build cache
fn build_cache_key(src: &str, runtime_path: &Path) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut h = DefaultHasher::new();
    src.hash(&mut h);
    env::var("J2_PARALLEL").unwrap_or_default().hash(&mut h);
    env::var("J2_NO_NATIVE").unwrap_or_default().hash(&mut h);
    env::var("J2_TRUSTED").unwrap_or_default().hash(&mut h);
    runtime_path.display().to_string().hash(&mut h);
    if let Some(r) = locate_forked_backend() {
        r.display().to_string().hash(&mut h);
        // Content identity, not just path
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

fn file_mtime(p: &Path) -> Option<std::time::SystemTime> {
    fs::metadata(p).and_then(|m| m.modified()).ok()
}

/// Newest mtime under `dir`, recursive
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

/// Cached binary must postdate runtime, fork, driver
fn cache_is_fresh(cached: &Path, runtime_path: &Path) -> bool {
    let Some(cached_t) = file_mtime(cached) else { return false; };
    let mut dep_t = env::current_exe().ok().and_then(|e| file_mtime(&e));
    if let Some(rt) = newest_mtime_in(&runtime_path.join("src")) {
        dep_t = Some(dep_t.map_or(rt, |d| d.max(rt)));
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

/// Write stable-path wrapper; parallelize only J2_TARGET_CRATE
fn write_backend_wrapper(cache: &Path, rustc: &Path) -> Result<PathBuf, String> {
    let script = format!(
        r#"#!/bin/sh
# wrapper: parallelize only $J2_TARGET_CRATE
mine=0
prev=
for a in "$@"; do
  if [ "$prev" = "--crate-name" ] && [ -n "$J2_TARGET_CRATE" ] && [ "$a" = "$J2_TARGET_CRATE" ]; then mine=1; fi
  prev=$a
done
# generated code, cap lints, keep errors
if [ "$mine" = 1 ]; then
  exec "{rustc}" "$@" --cap-lints allow --cfg j2_parallelize -C parallelize=on ${{J2_PAR_DUMP:+-Z par-dump}}
else
  exec "{rustc}" "$@" --cap-lints allow -C parallelize=off
fi
"#,
        rustc = rustc.display(),
    );
    let path = cache.join("j2_backend_wrapper.sh");
    fs::write(&path, script).map_err(|e| format!("write wrapper: {}", e))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&path).map_err(|e| e.to_string())?.permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&path, perms).map_err(|e| e.to_string())?;
    }
    Ok(path)
}

/// Locate forked stage1 compiler, None if absent
fn locate_forked_backend() -> Option<PathBuf> {
    if let Ok(p) = env::var("J2_BACKEND") {
        let p = PathBuf::from(p);
        return if p.exists() { Some(p) } else { None };
    }
    let mut roots: Vec<PathBuf> = Vec::new();
    // Canonicalize so a PATH symlink
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
        // Bundled install path, hides fork structure
        let bundled = root.join("toolchain").join("bin").join("rustc");
        if bundled.exists() {
            return Some(bundled);
        }
        // dev tree, stage1 build dirs
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

/// Locate the cargo to drive generated-crate builds
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

/// Locate the vendored dependency directory
fn locate_j_vendor() -> Option<PathBuf> {
    if let Ok(p) = env::var("J2_VENDOR") {
        let p = PathBuf::from(p);
        return if p.exists() { Some(p) } else { None };
    }
    let mut roots: Vec<PathBuf> = Vec::new();
    // Canonicalize so a PATH symlink
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
    // Build only, so forwarded args attach
    build(src_path, out_path, false)?;
    let final_path = PathBuf::from(out_path);
    let status = Command::new(&final_path)
        .args(extra_args)
        .status()
        .map_err(|e| format!("exec {}: {}", final_path.display(), e))?;
    Ok(status.code().unwrap_or(0) as u8)
}

/// Locate j2_runtime; J2_RUNTIME_PATH overrides source-tree lookup
fn locate_j_runtime() -> Result<PathBuf, String> {
    if let Ok(p) = env::var("J2_RUNTIME_PATH") {
        return Ok(PathBuf::from(p));
    }
    // Canonicalize exe so installer symlink resolves bundle
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
    Err("could not locate library/j2_runtime/, set J2_RUNTIME_PATH".into())
}
