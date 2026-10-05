//! Core syntax and data model for the versioned zudo-wind language.

mod audit;
mod breakpoints;
mod candidate;
pub mod catalog;
mod compile;
mod config;
mod decimal;
mod diagnostic;
mod emit;
mod escape;
mod explain;
mod index;
mod layers;
mod manifest;
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

pub use audit::{
    audit, audit_json, audit_with_token_overrides, render_audit, AuditConflict, AuditInput,
    AuditNote, AuditOutcome, AuditReport, AuditSource, DeadClass, DynamicConstruction,
    InterpolatedCandidate, UnrecognizedClass,
};
pub use breakpoints::{BreakpointConfig, RankedBreakpoint, ValidatedBreakpoints};
pub use candidate::{parse_candidate, Candidate, UtilityPart};
pub use catalog::{
    Catalog, CatalogEntry, CatalogError, Declaration, Example, Resolution, ResolvedRule,
    SelectorShape, ValueGrammar, ValueKind,
};
pub use compile::{
    compile, compile_validated, CompileInput, CompileResult, GeneratedProvenance, OriginCandidate,
    ProvenanceKind, RuleKind, RuleMetadata, StylesheetParts, UtilityPlacement,
};
pub use config::{DarkModeConfig, ResetMode, ValidatedWindConfig, WindConfig};
pub use decimal::{Decimal, DecimalDimension, DecimalError};
pub use diagnostic::{Diagnostic, DiagnosticCode, Origin, Severity, SourcePositionKind};
pub use escape::escape_class_name;
pub use explain::{
    explain, explain_disabled, explain_with_generation,
    explain_with_generation_and_token_overrides, explanation_json, render_explanation,
    DeclarationView, DiagnosticView, Explanation, ExplanationOutcome, HostTokenOverride,
    OriginView, ParsedCandidate, SortTuple, TokenResolution,
};
pub use index::CandidateIndex;
pub use layers::LAYER_ORDER;
pub use manifest::{classify_manifest_candidates, ExcludedCandidate, ManifestClassification};
pub use order::SortKey;
pub use reset::reset_css;
pub use selector::Specificity;
pub use source_plan::{
    compile_exclusion_pattern, ExclusionMatcher, PositiveRoot, SourceExclusion, SourceId,
    SourcePlan,
};
pub use token_vars::{emit_token_variables, TokenVariable};
pub use tokenizer::{structural_split, StructuralSplit};
pub use tokens::{FontSizeToken, TokenCategory, TokenConfig, TokenOverride, ValidatedTokens};
pub use value_check::ValueStatus;
pub use variant::{Variant, VariantChain, VariantKind, VariantVocabulary};
pub use walk::{
    expand_changed_path, expand_file_set, is_candidate_source, ExpandedFile, FileSet,
    WalkDiagnostic, PACKAGE_ROOT_TRAVERSES,
};

pub const SPEC_VERSION: u32 = 1;

pub const SPEC_REVISION: u32 = 6;

pub mod extract;
pub use extract::{
    extract_candidates, extract_candidates_with_options, ExtractedCandidate, ExtractionNote,
    ExtractionOptions, ExtractionResult, NoteKind, Occurrence, PositionKind, SourceKind,
};
