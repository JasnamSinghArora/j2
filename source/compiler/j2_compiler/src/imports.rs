// import preprocessor; inlines files, breaks cycles

use crate::error::J2Error;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

pub fn preprocess(src: &str, entry_path: Option<&Path>) -> Result<String, J2Error> {
    let base_dir = entry_path
        .and_then(|p| p.parent())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    let mut visited: HashSet<PathBuf> = HashSet::new();
    if let Some(p) = entry_path.and_then(|p| canonical(p)) {
        visited.insert(p);
    }
    let mut out = String::new();
    expand(src, &base_dir, &mut visited, &mut out, 0)?;
    Ok(out)
}

fn expand(
    src: &str,
    base: &Path,
    visited: &mut HashSet<PathBuf>,
    out: &mut String,
    depth: usize,
) -> Result<(), J2Error> {
    if depth > 32 {
        return Err(J2Error::syntax("import depth exceeds 32, likely a cycle", 0, 0));
    }
    for (lineno, line) in src.lines().enumerate() {
        let stripped = line.trim_start();
        if let Some(rest) = stripped.strip_prefix("import ") {
            let path_lit = rest.trim();
            // strip quotes, error on bad shape
            if path_lit.len() < 2
                || !path_lit.starts_with('"')
                || !path_lit.ends_with('"')
            {
                return Err(J2Error::syntax(
                    "import: expected `import \"path.j2\"`",
                    lineno + 1, 1,
                ));
            }
            let raw = &path_lit[1..path_lit.len() - 1];
            let target = resolve(raw, base);
            let canon = canonical(&target).unwrap_or(target.clone());
            if visited.contains(&canon) {
                // Already imported - emit comment + skip
                let _ = std::fmt::Write::write_fmt(out, format_args!("# import {:?} (skipped: already imported)\n", raw));
                continue;
            }
            visited.insert(canon.clone());
            let sub = std::fs::read_to_string(&target)
                .map_err(|e| J2Error::syntax(
                    format!("import: cannot read {:?}: {}", raw, e),
                    lineno + 1, 1,
                ))?;
            let _ = std::fmt::Write::write_fmt(out, format_args!("# >>> import {:?}\n", raw));
            let sub_base = canon.parent().map(PathBuf::from).unwrap_or_else(|| base.to_path_buf());
            expand(&sub, &sub_base, visited, out, depth + 1)?;
            let _ = std::fmt::Write::write_fmt(out, format_args!("# <<< end import {:?}\n", raw));
        } else {
            out.push_str(line);
            out.push('\n');
        }
    }
    Ok(())
}

/// resolve import via relative, lib/, J2_PATH search
fn resolve(raw: &str, base: &Path) -> PathBuf {
    let p = Path::new(raw);
    if p.is_absolute() {
        return p.to_path_buf();
    }
    let local = base.join(raw);
    if local.exists() {
        return local;
    }
    let lib = base.join("lib").join(raw);
    if lib.exists() {
        return lib;
    }
    if let Ok(jpath) = std::env::var("J2_PATH") {
        for dir in jpath.split(':').filter(|s| !s.is_empty()) {
            let cand = Path::new(dir).join(raw);
            if cand.exists() {
                return cand;
            }
        }
    }
    local
}

fn canonical(p: &Path) -> Option<PathBuf> {
    p.canonicalize().ok()
}
