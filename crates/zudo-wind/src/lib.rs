//! Core syntax and data model for the versioned zudo-wind language.

mod candidate;
mod compile;
mod diagnostic;
mod tokenizer;
mod variant;

pub use candidate::{parse_candidate, Candidate, UtilityPart};
pub use compile::{CompileInput, CompileResult, OriginCandidate};
pub use diagnostic::{Diagnostic, DiagnosticCode, Origin, Severity, SourcePositionKind};
pub use tokenizer::{structural_split, StructuralSplit};
pub use variant::{Variant, VariantChain, VariantKind, VariantVocabulary};

pub const SPEC_VERSION: u32 = 1;
