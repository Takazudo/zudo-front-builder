#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Severity {
    Error,
    Warning,
    AuditInfo,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DiagnosticCode {
    Zw001,
    Zw002,
    Zw003,
    Zw004,
    Zw005,
    Zw006,
    Zw007,
    Zw008,
    Zw009,
    Zw010,
    Zw011,
    Zw012,
    Zw013,
    /// A migration-vocabulary foreign utility: reported, never generated.
    Zw014,
    /// A host token entry replaced an effective preset token value.
    Zw015,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SourcePositionKind {
    Class,
    Literal,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Origin {
    Source {
        source_id: String,
        byte_offset: usize,
        byte_length: usize,
        line: usize,
        byte_column: usize,
        literal_byte_offset: usize,
        literal_byte_length: usize,
        position_kind: SourcePositionKind,
    },
    Safelist {
        owner: String,
        index: usize,
    },
    Manifest {
        producer: String,
        path: String,
        index: usize,
    },
    Config {
        key_path: String,
    },
    RoleClass {
        role_key: String,
    },
    Stylesheet {
        path: String,
        byte_offset: usize,
        byte_length: usize,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Diagnostic {
    pub severity: Severity,
    pub code: DiagnosticCode,
    pub candidate: Option<String>,
    pub origin: Option<Box<Origin>>,
    pub message: String,
    pub suggested_spelling: Option<String>,
    pub rejection_id: Option<&'static str>,
}

impl Diagnostic {
    pub(crate) fn parse(
        text: &str,
        code: DiagnosticCode,
        rejection_id: &'static str,
        message: &str,
    ) -> Self {
        Self {
            severity: Severity::Error,
            code,
            candidate: Some(text.to_owned()),
            origin: None,
            message: message.to_owned(),
            suggested_spelling: None,
            rejection_id: Some(rejection_id),
        }
    }
}
