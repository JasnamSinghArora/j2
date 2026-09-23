// J2 frontend pipeline from source to backend

pub mod ast;
pub mod error;
pub mod lexer;
pub mod lower;
pub mod parser;
pub mod imports;
pub mod typeck;

pub use error::J2Error;

/// compile J source; imports unresolved (no path)
pub fn compile_to_rust(src: &str) -> Result<String, J2Error> {
    let expanded = imports::preprocess(src, None)?;
    let toks = lexer::tokenize(&expanded)?;
    let prog = parser::parse(&toks)?;
    Ok(lower::lower_program(&prog).rust_src)
}

/// same, resolving imports relative to `entry_path`
pub fn compile_to_rust_with_path(src: &str, entry_path: &std::path::Path) -> Result<String, J2Error> {
    let expanded = imports::preprocess(src, Some(entry_path))?;
    let toks = lexer::tokenize(&expanded)?;
    let prog = parser::parse(&toks)?;
    Ok(lower::lower_program(&prog).rust_src)
}
