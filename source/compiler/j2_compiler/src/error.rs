// J's error hierarchy at compile time

use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum J2ErrorKind {
    SyntaxError,
    TypeError,
    ValueError,
    ConversionError,
    ZeroDivisionError,
    OverflowError,
    IndexError,
    KeyError,
    NameError,
    MutabilityError,
    FlowError,
    InfiniteFlowError,
    RecursionError,
    RuntimeError,
}

impl J2ErrorKind {
    pub fn name(&self) -> &'static str {
        match self {
            Self::SyntaxError => "SyntaxError",
            Self::TypeError => "TypeError",
            Self::ValueError => "ValueError",
            Self::ConversionError => "ConversionError",
            Self::ZeroDivisionError => "ZeroDivisionError",
            Self::OverflowError => "OverflowError",
            Self::IndexError => "IndexError",
            Self::KeyError => "KeyError",
            Self::NameError => "NameError",
            Self::MutabilityError => "MutabilityError",
            Self::FlowError => "FlowError",
            Self::InfiniteFlowError => "InfiniteFlowError",
            Self::RecursionError => "RecursionError",
            Self::RuntimeError => "RuntimeError",
        }
    }
}

#[derive(Debug, Clone)]
pub struct J2Error {
    pub kind: J2ErrorKind,
    pub msg: String,
    pub line: usize,
    pub col: usize,
}

impl J2Error {
    pub fn syntax<S: Into<String>>(msg: S, line: usize, col: usize) -> Self {
        Self { kind: J2ErrorKind::SyntaxError, msg: msg.into(), line, col }
    }
    pub fn name<S: Into<String>>(msg: S) -> Self {
        Self { kind: J2ErrorKind::NameError, msg: msg.into(), line: 0, col: 0 }
    }
    pub fn type_err<S: Into<String>>(msg: S) -> Self {
        Self { kind: J2ErrorKind::TypeError, msg: msg.into(), line: 0, col: 0 }
    }
    pub fn value<S: Into<String>>(msg: S) -> Self {
        Self { kind: J2ErrorKind::ValueError, msg: msg.into(), line: 0, col: 0 }
    }
    /// backfill line/col from sub-scanners lacking position
    pub fn or_loc(mut self, line: usize, col: usize) -> Self {
        if self.line == 0 { self.line = line; }
        if self.col == 0 { self.col = col; }
        self
    }
    /// render diagnostic with locator, source line, caret
    pub fn render(&self, src: &str, path: &str) -> String {
        let mut out = format!("error[{}]: {}\n", self.kind.name(), self.msg);
        if self.line == 0 {
            return out;
        }
        out.push_str(&format!("  --> {}:{}:{}\n", path, self.line, self.col));
        if let Some(text) = src.lines().nth(self.line - 1) {
            let num = self.line.to_string();
            let pad = " ".repeat(num.len());
            out.push_str(&format!("{} |\n", pad));
            out.push_str(&format!("{} | {}\n", num, text));
            let caret = " ".repeat(self.col.saturating_sub(1));
            out.push_str(&format!("{} | {}^\n", pad, caret));
        }
        out
    }
}

impl fmt::Display for J2Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.line > 0 {
            write!(f, "{}: {} (line {}, col {})", self.kind.name(), self.msg, self.line, self.col)
        } else {
            write!(f, "{}: {}", self.kind.name(), self.msg)
        }
    }
}

impl std::error::Error for J2Error {}
