//! Core syntax and data model for the versioned zudo-wind language.

mod breakpoints;
mod candidate;
pub mod catalog;
mod compile;
mod config;
mod decimal;
mod diagnostic;
mod emit;
mod escape;
mod index;
mod layers;
mod order;
mod reset;
mod selector;
mod source_plan;
mod token_vars;
mod tokenizer;
mod tokens;
mod value_check;
mod variant;
mod walk;

pub use breakpoints::{BreakpointConfig, RankedBreakpoint, ValidatedBreakpoints};
pub use candidate::{parse_candidate, Candidate, UtilityPart};
pub use catalog::{
    Catalog, CatalogEntry, CatalogError, Declaration, Example, Resolution, ResolvedRule,
    SelectorShape, ValueGrammar, ValueKind,
};
pub use compile::{
    compile, compile_validated, CompileInput, CompileResult, GeneratedProvenance, OriginCandidate,
    ProvenanceKind, RuleKind, RuleMetadata, StylesheetParts,
};
pub use config::{DarkModeConfig, ResetMode, ValidatedWindConfig, WindConfig};
pub use decimal::{Decimal, DecimalDimension, DecimalError};
pub use diagnostic::{Diagnostic, DiagnosticCode, Origin, Severity, SourcePositionKind};
pub use escape::escape_class_name;
pub use index::CandidateIndex;
pub use layers::LAYER_ORDER;
pub use order::SortKey;
pub use reset::reset_css;
pub use selector::Specificity;
pub use source_plan::{PositiveRoot, SourceId, SourcePlan};
pub use token_vars::{emit_token_variables, TokenVariable};
pub use tokenizer::{structural_split, StructuralSplit};
pub use tokens::{FontSizeToken, TokenCategory, TokenConfig, ValidatedTokens};
pub use value_check::ValueStatus;
pub use variant::{Variant, VariantChain, VariantKind, VariantVocabulary};
pub use walk::{expand_file_set, ExpandedFile, FileSet, WalkDiagnostic};

pub const SPEC_VERSION: u32 = 1;

pub const SPEC_REVISION: u32 = 2;

pub mod extract;
pub use extract::{
    extract_candidates, ExtractedCandidate, ExtractionNote, ExtractionResult, NoteKind, Occurrence,
    PositionKind, SourceKind,
};
