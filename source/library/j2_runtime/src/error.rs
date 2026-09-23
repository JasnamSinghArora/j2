// J runtime error hierarchy

use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum J2ErrKind {
    BaseError,
    SystemExit,
    KeyboardInterrupt,
    Error,
    ArithmeticError,
    ZeroDivisionError,
    OverflowError,
    TypeError,
    ValueError,
    ConversionError,
    LookupError,
    IndexError,
    KeyError,
    NameError,
    MutabilityError,
    FlowError,
    InfiniteFlowError,
    RuntimeError,
    RecursionError,
    SyntaxError,
    /// Internal control-flow signal for `give`
    GiveSignal,
}

impl J2ErrKind {
    pub fn name(&self) -> &'static str {
        match self {
            Self::BaseError => "BaseError",
            Self::SystemExit => "SystemExit",
            Self::KeyboardInterrupt => "KeyboardInterrupt",
            Self::Error => "Error",
            Self::ArithmeticError => "ArithmeticError",
            Self::ZeroDivisionError => "ZeroDivisionError",
            Self::OverflowError => "OverflowError",
            Self::TypeError => "TypeError",
            Self::ValueError => "ValueError",
            Self::ConversionError => "ConversionError",
            Self::LookupError => "LookupError",
            Self::IndexError => "IndexError",
            Self::KeyError => "KeyError",
            Self::NameError => "NameError",
            Self::MutabilityError => "MutabilityError",
            Self::FlowError => "FlowError",
            Self::InfiniteFlowError => "InfiniteFlowError",
            Self::RuntimeError => "RuntimeError",
            Self::RecursionError => "RecursionError",
            Self::SyntaxError => "SyntaxError",
            Self::GiveSignal => "GiveSignal",
        }
    }
    /// Is `self` a subclass of `other`
    pub fn is_subclass_of(&self, other: J2ErrKind) -> bool {
        if *self == other { return true; }
        let parent = match self {
            Self::SystemExit | Self::KeyboardInterrupt | Self::Error => Some(Self::BaseError),
            Self::ArithmeticError | Self::TypeError | Self::ValueError
            | Self::LookupError | Self::NameError | Self::FlowError
            | Self::RuntimeError | Self::SyntaxError => Some(Self::Error),
            Self::ZeroDivisionError | Self::OverflowError => Some(Self::ArithmeticError),
            Self::ConversionError => Some(Self::ValueError),
            Self::IndexError | Self::KeyError => Some(Self::LookupError),
            Self::MutabilityError => Some(Self::NameError),
            Self::InfiniteFlowError => Some(Self::FlowError),
            Self::RecursionError => Some(Self::RuntimeError),
            // control-flow signal; outside tree, no handler catches
            Self::GiveSignal => None,
            Self::BaseError => None,
        };
        match parent {
            Some(p) => p.is_subclass_of(other),
            None => false,
        }
    }
}

#[derive(Debug, Clone)]
pub struct J2Err {
    pub kind: J2ErrKind,
    pub msg: String,
}

impl J2Err {
    pub fn new(kind: J2ErrKind, msg: impl Into<String>) -> Self {
        Self { kind, msg: msg.into() }
    }
    pub fn type_err(m: impl Into<String>) -> Self { Self::new(J2ErrKind::TypeError, m) }
    pub fn value(m: impl Into<String>) -> Self { Self::new(J2ErrKind::ValueError, m) }
    pub fn name_err(m: impl Into<String>) -> Self { Self::new(J2ErrKind::NameError, m) }
    pub fn mutability(m: impl Into<String>) -> Self { Self::new(J2ErrKind::MutabilityError, m) }
    pub fn index(m: impl Into<String>) -> Self { Self::new(J2ErrKind::IndexError, m) }
    pub fn key(m: impl Into<String>) -> Self { Self::new(J2ErrKind::KeyError, m) }
    pub fn zero_div(m: impl Into<String>) -> Self { Self::new(J2ErrKind::ZeroDivisionError, m) }
    pub fn overflow(m: impl Into<String>) -> Self { Self::new(J2ErrKind::OverflowError, m) }
    pub fn conversion(m: impl Into<String>) -> Self { Self::new(J2ErrKind::ConversionError, m) }
    pub fn infinite_flow(m: impl Into<String>) -> Self { Self::new(J2ErrKind::InfiniteFlowError, m) }
    pub fn recursion(m: impl Into<String>) -> Self { Self::new(J2ErrKind::RecursionError, m) }
    pub fn runtime(m: impl Into<String>) -> Self { Self::new(J2ErrKind::RuntimeError, m) }
    /// Internal: the `give` early-return control-flow signal.
    pub fn give_signal() -> Self { Self::new(J2ErrKind::GiveSignal, "give") }
    /// Is this the internal `give` signal
    pub fn is_give_signal(&self) -> bool { self.kind == J2ErrKind::GiveSignal }
}

impl fmt::Display for J2Err {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.kind.name(), self.msg)
    }
}

impl std::error::Error for J2Err {}

pub type J2Result<T> = Result<T, J2Err>;
