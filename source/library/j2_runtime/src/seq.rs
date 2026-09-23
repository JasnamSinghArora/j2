// eager seq; mutable bindings share one cell

use crate::value::J2Value;

#[derive(Debug, Default, Clone)]
pub struct J2Seq {
    pub items: Vec<J2Value>,
}
