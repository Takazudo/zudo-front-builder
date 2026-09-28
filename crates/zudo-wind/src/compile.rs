use crate::{Diagnostic, Origin, WindConfig};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OriginCandidate {
    pub text: String,
    pub origin: Origin,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CompileInput {
    pub candidates: Vec<OriginCandidate>,
    pub config: WindConfig,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CompileResult {
    pub stylesheet: String,
    pub diagnostics: Vec<Diagnostic>,
}
