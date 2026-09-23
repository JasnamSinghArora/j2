// J lexer; token set per spec 2.7/2.8

use crate::error::J2Error;

#[derive(Debug, Clone, PartialEq)]
pub enum Tok {
    // Literals
    IntLit(i64),
    FloatLit(f64),
    TextLit(String),
    // Boolean / null
    True,
    False,
    Null,
    // built-in constants, reserved names per spec
    Pi,
    E,
    Tau,
    Inf,
    Nan,
    MaxVal,
    MinVal,
    // Keywords
    Do,
    Else,
    For,
    Func,
    Give,
    Global,
    If,
    In,
    Loop,
    Not,
    Or,
    And,
    Repeat,
    Skip,
    Stop,
    Try,
    Until,
    Assert,
    Native,
    Class,
    Extends,
    /// raw source captured inside `backend` block
    NativeBlockSource(String),
    ColonColon,
    // Identifier
    Ident(String),
    Underscore,
    // Arithmetic
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    StarStar,
    // Comparison
    EqEq,
    BangEq,
    Lt,
    Gt,
    LtEq,
    GtEq,
    // Assignment
    Eq,
    ColonEq,
    PlusEq,
    MinusEq,
    StarEq,
    SlashEq,
    PercentEq,
    // Increment / decrement
    PlusPlus,
    MinusMinus,
    // Range
    DotDot,
    // Pipe
    PipePipe, // >>
    // Filter
    Question,
    // Bitwise
    Amp,        // &
    Pipe,  // bitwise OR; `>>` is the pipe op
    Caret,      // ^
    Tilde,      // ~
    Shl,        // <<
    // (Shr / >> is shared with PipePipe
    LParen,
    RParen,
    LBracket,
    RBracket,
    LBrace,
    RBrace,
    Comma,
    Colon,
    Dot,
    Arrow,      // -> (return-type annotation in typed function signatures)
    // Logical line end
    Newline,
    // End of input
    Eof,
}

#[derive(Debug, Clone)]
pub struct Token {
    pub kind: Tok,
    pub line: usize,
    pub col: usize,
}

pub fn tokenize(src: &str) -> Result<Vec<Token>, J2Error> {
    let mut tokens = Vec::new();
    let chars: Vec<char> = src.chars().collect();
    let mut i = 0;
    let mut line = 1;
    let mut col = 1;
    let mut paren_depth = 0i32;
    let mut bracket_depth = 0i32;
    let mut brace_depth = 0i32;

    while i < chars.len() {
        let c = chars[i];
        let start_line = line;
        let start_col = col;

        // skip inline whitespace; newlines tokens outside brackets
        if c == ' ' || c == '\t' {
            i += 1;
            col += 1;
            continue;
        }
        if c == '\r' {
            // CR LF or bare CR
            i += 1;
            if i < chars.len() && chars[i] == '\n' {
                i += 1;
            }
            if paren_depth == 0 && bracket_depth == 0 && brace_depth == 0 {
                push_newline(&mut tokens, line, col);
            }
            line += 1;
            col = 1;
            continue;
        }
        if c == '\n' {
            i += 1;
            if paren_depth == 0 && bracket_depth == 0 && brace_depth == 0 {
                push_newline(&mut tokens, line, col);
            }
            line += 1;
            col = 1;
            continue;
        }

        // Comments: # to end of line
        if c == '#' {
            while i < chars.len() && chars[i] != '\n' && chars[i] != '\r' {
                i += 1;
            }
            continue;
        }

        // Identifiers + keywords + reserved constants
        if c == '_' || c.is_ascii_alphabetic() {
            let mut s = String::new();
            while i < chars.len()
                && (chars[i] == '_' || chars[i].is_ascii_alphanumeric())
            {
                s.push(chars[i]);
                i += 1;
                col += 1;
            }
            let tk = classify_ident(&s);
            let is_native = matches!(tk, Tok::Native);
            tokens.push(Token { kind: tk, line: start_line, col: start_col });
            // Raw-passthrough for `backend { ... }`
            if is_native {
                // Skip whitespace.
                while i < chars.len() && (chars[i] == ' ' || chars[i] == '\t') {
                    i += 1; col += 1;
                }
                if i < chars.len() && chars[i] == '{' {
                    let block_line = line;
                    let block_col = col;
                    i += 1; col += 1; // consume the opening `{`
                    let mut depth: i32 = 1;
                    let mut raw = String::new();
                    while i < chars.len() && depth > 0 {
                        let ch = chars[i];
                        // skip string literals so braces don't miscount
                        if ch == '"' {
                            raw.push(ch); i += 1; col += 1;
                            while i < chars.len() {
                                let c2 = chars[i];
                                raw.push(c2); i += 1; col += 1;
                                if c2 == '\\' && i < chars.len() {
                                    raw.push(chars[i]); i += 1; col += 1;
                                } else if c2 == '"' { break; }
                            }
                            continue;
                        }
                        if ch == '\'' {
                            // char literal or lifetime; best-effort scan
                            raw.push(ch); i += 1; col += 1;
                            // peek to decide
                            if i + 1 < chars.len() && (chars[i] == '\\' || chars[i+1] == '\'') {
                                while i < chars.len() {
                                    let c2 = chars[i];
                                    raw.push(c2); i += 1; col += 1;
                                    if c2 == '\\' && i < chars.len() {
                                        raw.push(chars[i]); i += 1; col += 1;
                                    } else if c2 == '\'' { break; }
                                }
                            }
                            continue;
                        }
                        if ch == '/' && i + 1 < chars.len() && chars[i+1] == '/' {
                            // line comment - copy until newline
                            while i < chars.len() && chars[i] != '\n' {
                                raw.push(chars[i]); i += 1; col += 1;
                            }
                            continue;
                        }
                        if ch == '/' && i + 1 < chars.len() && chars[i+1] == '*' {
                            // block comment - copy until matching */
                            raw.push(ch); raw.push('*'); i += 2; col += 2;
                            let mut bd: i32 = 1;
                            while i < chars.len() && bd > 0 {
                                let c2 = chars[i];
                                raw.push(c2);
                                if c2 == '\n' { line += 1; col = 1; } else { col += 1; }
                                i += 1;
                                if c2 == '/' && i < chars.len() && chars[i] == '*' { bd += 1; raw.push('*'); i += 1; col += 1; }
                                if c2 == '*' && i < chars.len() && chars[i] == '/' { bd -= 1; raw.push('/'); i += 1; col += 1; }
                            }
                            continue;
                        }
                        if ch == '{' { depth += 1; }
                        if ch == '}' { depth -= 1; if depth == 0 { i += 1; col += 1; break; } }
                        raw.push(ch);
                        if ch == '\n' { line += 1; col = 1; } else { col += 1; }
                        i += 1;
                    }
                    tokens.push(Token { kind: Tok::NativeBlockSource(raw), line: block_line, col: block_col });
                }
            }
            continue;
        }

        // Numeric literals (integer or float). Spec section 2.4.
        if c.is_ascii_digit() || (c == '.' && i + 1 < chars.len() && chars[i + 1].is_ascii_digit()) {
            let (tk, consumed) = lex_number(&chars, i).map_err(|e| e.or_loc(start_line, start_col))?;
            tokens.push(Token { kind: tk, line: start_line, col: start_col });
            col += consumed;
            i += consumed;
            continue;
        }

        // text literal, escapes; triple-quoted multi-line
        if c == '"' {
            if i + 2 < chars.len() && chars[i + 1] == '"' && chars[i + 2] == '"' {
                let (s, consumed, nl) = lex_triple_text(&chars, i).map_err(|e| e.or_loc(start_line, start_col))?;
                tokens.push(Token { kind: Tok::TextLit(s), line: start_line, col: start_col });
                i += consumed;
                line += nl;
                col = 1;
                continue;
            } else {
                let (s, consumed) = lex_text(&chars, i, start_line).map_err(|e| e.or_loc(start_line, start_col))?;
                tokens.push(Token { kind: Tok::TextLit(s), line: start_line, col: start_col });
                i += consumed;
                col += consumed;
                continue;
            }
        }

        // Two-/three-character operators come first to avoid e.g
        let two = if i + 1 < chars.len() { Some((chars[i], chars[i + 1])) } else { None };
        if let Some((a, b)) = two {
            let tk = match (a, b) {
                ('*', '*') => Some(Tok::StarStar),
                ('=', '=') => Some(Tok::EqEq),
                ('!', '=') => Some(Tok::BangEq),
                ('<', '=') => Some(Tok::LtEq),
                ('>', '=') => Some(Tok::GtEq),
                (':', '=') => Some(Tok::ColonEq),
                ('+', '=') => Some(Tok::PlusEq),
                ('-', '=') => Some(Tok::MinusEq),
                ('*', '=') => Some(Tok::StarEq),
                ('/', '=') => Some(Tok::SlashEq),
                ('%', '=') => Some(Tok::PercentEq),
                ('+', '+') => Some(Tok::PlusPlus),
                ('-', '-') => Some(Tok::MinusMinus),
                ('-', '>') => Some(Tok::Arrow),
                ('.', '.') => Some(Tok::DotDot),
                ('>', '>') => Some(Tok::PipePipe),
                ('<', '<') => Some(Tok::Shl),
                (':', ':') => Some(Tok::ColonColon),
                _ => None,
            };
            if let Some(tk) = tk {
                tokens.push(Token { kind: tk, line: start_line, col: start_col });
                i += 2;
                col += 2;
                continue;
            }
        }

        // Single-char operators / delimiters
        let single = match c {
            '+' => Some(Tok::Plus),
            '-' => Some(Tok::Minus),
            '*' => Some(Tok::Star),
            '/' => Some(Tok::Slash),
            '%' => Some(Tok::Percent),
            '=' => Some(Tok::Eq),
            '<' => Some(Tok::Lt),
            '>' => Some(Tok::Gt),
            '&' => Some(Tok::Amp),
            '|' => Some(Tok::Pipe),
            '^' => Some(Tok::Caret),
            '~' => Some(Tok::Tilde),
            '(' => {
                paren_depth += 1;
                Some(Tok::LParen)
            }
            ')' => {
                paren_depth = (paren_depth - 1).max(0);
                Some(Tok::RParen)
            }
            '[' => {
                bracket_depth += 1;
                Some(Tok::LBracket)
            }
            ']' => {
                bracket_depth = (bracket_depth - 1).max(0);
                Some(Tok::RBracket)
            }
            '{' => {
                brace_depth += 1;
                Some(Tok::LBrace)
            }
            '}' => {
                brace_depth = (brace_depth - 1).max(0);
                Some(Tok::RBrace)
            }
            ',' => Some(Tok::Comma),
            ':' => Some(Tok::Colon),
            '.' => Some(Tok::Dot),
            '?' => Some(Tok::Question),
            _ => None,
        };
        if let Some(tk) = single {
            tokens.push(Token { kind: tk, line: start_line, col: start_col });
            i += 1;
            col += 1;
            continue;
        }

        return Err(J2Error::syntax(
            format!("unexpected character '{}' (U+{:04X})", c, c as u32),
            start_line,
            start_col,
        ));
    }

    tokens.push(Token { kind: Tok::Eof, line, col });
    Ok(tokens)
}

fn push_newline(tokens: &mut Vec<Token>, line: usize, col: usize) {
    // Collapse consecutive newlines and avoid leading newline
    if matches!(tokens.last().map(|t| &t.kind), Some(Tok::Newline) | None) {
        return;
    }
    tokens.push(Token { kind: Tok::Newline, line, col });
}

fn classify_ident(s: &str) -> Tok {
    match s {
        "do" => Tok::Do,
        "else" => Tok::Else,
        "for" => Tok::For,
        "func" => Tok::Func,
        "give" => Tok::Give,
        "global" => Tok::Global,
        "if" => Tok::If,
        "in" => Tok::In,
        "loop" => Tok::Loop,
        "not" => Tok::Not,
        "or" => Tok::Or,
        "and" => Tok::And,
        "repeat" => Tok::Repeat,
        "skip" => Tok::Skip,
        "stop" => Tok::Stop,
        "try" => Tok::Try,
        "until" => Tok::Until,
        "assert" => Tok::Assert,
        "native" => Tok::Native,
        "class" => Tok::Class,
        "extends" => Tok::Extends,
        "true" => Tok::True,
        "false" => Tok::False,
        "null" => Tok::Null,
        "PI" => Tok::Pi,
        "E" => Tok::E,
        "TAU" => Tok::Tau,
        "INF" => Tok::Inf,
        "NAN" => Tok::Nan,
        "MAX_VAL" => Tok::MaxVal,
        "MIN_VAL" => Tok::MinVal,
        "_" => Tok::Underscore,
        _ => Tok::Ident(s.to_string()),
    }
}

fn lex_number(chars: &[char], start: usize) -> Result<(Tok, usize), J2Error> {
    // numbers per spec 2.4, underscores allowed
    let mut i = start;
    let mut buf = String::new();
    let mut is_float = false;

    // integer part, may be empty (.001)
    while i < chars.len() && (chars[i].is_ascii_digit() || chars[i] == '_') {
        if chars[i] != '_' {
            buf.push(chars[i]);
        } else if i == start || !chars[i - 1].is_ascii_digit() {
            return Err(J2Error::syntax(
                "underscore must be between digits",
                0,
                0,
            ));
        }
        i += 1;
    }

    // Fractional part
    if i < chars.len() && chars[i] == '.' {
        // fractional dot, not `..` range
        if i + 1 < chars.len() && chars[i + 1] == '.' {
            // It's `..` - stop the number here
        } else {
            is_float = true;
            buf.push('.');
            i += 1;
            while i < chars.len() && (chars[i].is_ascii_digit() || chars[i] == '_') {
                if chars[i] != '_' {
                    buf.push(chars[i]);
                }
                i += 1;
            }
        }
    }

    // Exponent
    if i < chars.len() && (chars[i] == 'e' || chars[i] == 'E') {
        is_float = true;
        buf.push('e');
        i += 1;
        if i < chars.len() && (chars[i] == '+' || chars[i] == '-') {
            buf.push(chars[i]);
            i += 1;
        }
        let exp_start = i;
        while i < chars.len() && (chars[i].is_ascii_digit() || chars[i] == '_') {
            if chars[i] != '_' {
                buf.push(chars[i]);
            }
            i += 1;
        }
        if i == exp_start {
            return Err(J2Error::syntax(
                "exponent has no digits",
                0,
                0,
            ));
        }
    }

    let consumed = i - start;
    if is_float {
        let v = buf.parse::<f64>().map_err(|e| J2Error::syntax(e.to_string(), 0, 0))?;
        Ok((Tok::FloatLit(v), consumed))
    } else {
        let v = buf.parse::<i64>().map_err(|e| J2Error::syntax(e.to_string(), 0, 0))?;
        Ok((Tok::IntLit(v), consumed))
    }
}

fn lex_text(chars: &[char], start: usize, line: usize) -> Result<(String, usize), J2Error> {
    // Single-line text literal: "...". Escapes per section 2.5.1.
    let mut i = start + 1; // skip opening "
    let mut s = String::new();
    while i < chars.len() {
        let c = chars[i];
        if c == '"' {
            return Ok((s, i - start + 1));
        }
        if c == '\n' || c == '\r' {
            return Err(J2Error::syntax("unterminated text literal", line, 0));
        }
        if c == '\\' {
            i += 1;
            if i >= chars.len() {
                return Err(J2Error::syntax("trailing backslash", line, 0));
            }
            let esc = match chars[i] {
                '\\' => '\\',
                '"' => '"',
                'n' => '\n',
                't' => '\t',
                'r' => '\r',
                other => {
                    return Err(J2Error::syntax(
                        format!("unknown escape \\{}", other),
                        line,
                        0,
                    ))
                }
            };
            s.push(esc);
            i += 1;
        } else {
            s.push(c);
            i += 1;
        }
    }
    Err(J2Error::syntax("unterminated text literal", line, 0))
}

fn lex_triple_text(chars: &[char], start: usize) -> Result<(String, usize, usize), J2Error> {
    // Triple-quoted multi-line text. section 2.5.2.
    let mut i = start + 3; // skip """
    let mut s = String::new();
    let mut newlines = 0;
    while i + 2 < chars.len() {
        if chars[i] == '"' && chars[i + 1] == '"' && chars[i + 2] == '"' {
            return Ok((s, i + 3 - start, newlines));
        }
        if chars[i] == '\n' {
            newlines += 1;
        }
        s.push(chars[i]);
        i += 1;
    }
    Err(J2Error::syntax("unterminated triple-quoted text", 0, 0))
}
