use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde_json::{json, Value};
use zudo_wind::{
    audit, compile, explain, extract_candidates, AuditInput, AuditSource, BreakpointConfig,
    CompileInput, DarkModeConfig, Origin, OriginCandidate, PositionKind, ResetMode, SourceKind,
    SourcePositionKind, TokenConfig, UtilityPlacement, WindConfig,
};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CaseDefinition {
    case_id: String,
    #[serde(default)]
    reset: ResetChoice,
    #[serde(default)]
    spacing_unit: Option<String>,
    #[serde(default)]
    default_transition_timing_function: Option<String>,
    #[serde(default)]
    colors: BTreeMap<String, String>,
    #[serde(default)]
    shadows: BTreeMap<String, String>,
    #[serde(default)]
    easings: BTreeMap<String, String>,
    #[serde(default)]
    breakpoints: BTreeMap<String, i64>,
    #[serde(default)]
    dark: Option<DarkChoice>,
    #[serde(default)]
    authored_classes: BTreeMap<String, bool>,
    #[serde(default)]
    explicit_candidates: Vec<String>,
    /// Authored global CSS file, relative to the fixture, assembled with the utilities.
    #[serde(default)]
    authored_css: Option<String>,
    #[serde(default)]
    utility_placement: PlacementChoice,
}

#[derive(Clone, Copy, Default, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum PlacementChoice {
    #[default]
    AfterAuthored,
    BeforeAuthored,
}

impl From<PlacementChoice> for UtilityPlacement {
    fn from(value: PlacementChoice) -> Self {
        match value {
            PlacementChoice::AfterAuthored => Self::AfterAuthored,
            PlacementChoice::BeforeAuthored => Self::BeforeAuthored,
        }
    }
}

#[derive(Clone, Copy, Default, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum ResetChoice {
    #[default]
    None,
    MinimalV1,
    OwnedV1,
}

impl From<ResetChoice> for ResetMode {
    fn from(value: ResetChoice) -> Self {
        match value {
            ResetChoice::None => Self::None,
            ResetChoice::MinimalV1 => Self::MinimalV1,
            ResetChoice::OwnedV1 => Self::OwnedV1,
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DarkChoice {
    attribute: String,
    value: String,
}

fn main() -> Result<(), Box<dyn Error>> {
    let mut args = env::args_os().skip(1);
    let fixture_root = PathBuf::from(
        args.next()
            .ok_or("usage: wind_fixture_css <fixture-root> <output-root>")?,
    );
    let output_root = PathBuf::from(
        args.next()
            .ok_or("usage: wind_fixture_css <fixture-root> <output-root>")?,
    );
    if args.next().is_some() {
        return Err("expected exactly two path arguments".into());
    }

    let mut fixture_dirs = fs::read_dir(&fixture_root)?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<Result<Vec<_>, _>>()?;
    fixture_dirs.sort();

    for fixture_dir in fixture_dirs {
        if !fixture_dir.is_dir() {
            continue;
        }
        let definition_path = fixture_dir.join("case.json");
        if !definition_path.is_file() {
            continue;
        }
        generate_fixture(&fixture_dir, &output_root)?;
    }
    Ok(())
}

fn generate_fixture(fixture_dir: &Path, output_root: &Path) -> Result<(), Box<dyn Error>> {
    let directory_name = fixture_dir
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or("fixture directory name is not UTF-8")?;
    let definition_path = fixture_dir.join("case.json");
    let definition: CaseDefinition = serde_json::from_slice(&fs::read(&definition_path)?)?;
    let case_id = definition.case_id.clone();
    if case_id.is_empty() {
        return Err(format!("{} has an empty caseId", definition_path.display()).into());
    }
    let explicit_candidates = definition.explicit_candidates.clone();
    let source_path = fixture_dir.join("index.html");
    let source = fs::read(&source_path)?;
    let source_id = format!("fixtures/{directory_name}/index.html");
    let extraction = extract_candidates(&source, SourceKind::Html);
    let config = make_config(&definition);

    let mut candidates = extraction
        .candidates
        .iter()
        .flat_map(|candidate| {
            candidate
                .occurrences
                .iter()
                .map(|occurrence| OriginCandidate {
                    text: candidate.text.clone(),
                    origin: Origin::Source {
                        source_id: source_id.clone(),
                        byte_offset: occurrence.byte_offset,
                        byte_length: occurrence.byte_length,
                        line: occurrence.line,
                        byte_column: occurrence.byte_column,
                        literal_byte_offset: occurrence.literal_byte_offset,
                        literal_byte_length: occurrence.literal_byte_length,
                        position_kind: match occurrence.position_kind {
                            PositionKind::Class => SourcePositionKind::Class,
                            PositionKind::Literal => SourcePositionKind::Literal,
                        },
                    },
                })
        })
        .collect::<Vec<_>>();

    let case_path = format!("{directory_name}/case.json");
    for (index, text) in explicit_candidates.into_iter().enumerate() {
        candidates.push(OriginCandidate {
            text,
            origin: Origin::Manifest {
                producer: "wind-fixture-css".to_owned(),
                path: case_path.clone(),
                index,
            },
        });
    }

    let result = compile(&CompileInput {
        candidates: candidates.clone(),
        config: config.clone(),
    });
    let output_dir = output_root.join(directory_name);
    fs::create_dir_all(&output_dir)?;
    let stylesheet = match &definition.authored_css {
        Some(path) => result.parts.with_authored(
            &fs::read_to_string(fixture_dir.join(path))?,
            config.utility_placement,
        ),
        None => result.stylesheet.clone(),
    };
    fs::write(output_dir.join("wind.css"), &stylesheet)?;

    let audit_report = audit(
        &AuditInput::new(vec![AuditSource::new(source_id, extraction)]),
        &config,
    );
    let explanations = explanation_map(&candidates, &config)?;
    let mut report = json!({
        "caseId": directory_name,
        "specCaseId": case_id,
        "diagnostics": result.diagnostics.iter().map(diagnostic_json).collect::<Vec<_>>(),
        "explanations": explanations,
        "audit": serde_json::from_str::<Value>(&zudo_wind::audit_json(&audit_report)?)?,
        "rules": result.rules.iter().map(|rule| rule.candidate.as_str()).collect::<Vec<_>>(),
        "ordinaryClasses": result.ordinary_classes.iter().map(|class| class.text.as_str()).collect::<Vec<_>>(),
        "authoredClasses": result.authored_classes.iter().map(|class| class.text.as_str()).collect::<Vec<_>>(),
        "hasErrors": result.has_errors(),
    });

    if matches!(case_id.as_str(), "W-A01" | "W-A09") {
        let mut reversed = candidates.clone();
        reversed.reverse();
        let reversed_result = compile(&CompileInput {
            candidates: reversed.clone(),
            config: config.clone(),
        });
        let reversed_explanations = explanation_map(&reversed, &config)?;
        let same_stylesheet = result.stylesheet.as_bytes() == reversed_result.stylesheet.as_bytes();
        let same_explanations = explanations == reversed_explanations;
        fs::write(
            output_dir.join("wind-order-2.css"),
            &reversed_result.stylesheet,
        )?;
        report["determinism"] = json!({
            "sameStylesheet": same_stylesheet,
            "sameExplanations": same_explanations,
            "reversedExplanations": reversed_explanations,
        });
    }

    let mut serialized = serde_json::to_vec_pretty(&report)?;
    serialized.push(b'\n');
    fs::write(output_dir.join("report.json"), serialized)?;

    Ok(())
}

fn make_config(definition: &CaseDefinition) -> WindConfig {
    let breakpoints = definition
        .breakpoints
        .iter()
        .map(|(name, min_width_px)| {
            (
                name.clone(),
                BreakpointConfig {
                    min_width_px: *min_width_px,
                },
            )
        })
        .collect();
    let dark = definition.dark.as_ref().map(|dark| DarkModeConfig {
        attribute: dark.attribute.clone(),
        value: dark.value.clone(),
    });

    WindConfig {
        reset: definition.reset.into(),
        default_transition_timing_function: definition.default_transition_timing_function.clone(),
        tokens: TokenConfig {
            spacing_unit: definition.spacing_unit.clone(),
            colors: definition.colors.clone(),
            shadows: definition.shadows.clone(),
            easings: definition.easings.clone(),
            ..TokenConfig::default()
        },
        breakpoints,
        dark,
        authored_classes: definition.authored_classes.clone(),
        utility_placement: definition.utility_placement.into(),
        ..WindConfig::default()
    }
}

fn explanation_map(
    candidates: &[OriginCandidate],
    config: &WindConfig,
) -> Result<BTreeMap<String, Value>, serde_json::Error> {
    let unique = candidates
        .iter()
        .map(|candidate| candidate.text.as_str())
        .collect::<BTreeSet<_>>();
    unique
        .into_iter()
        .map(|candidate| {
            Ok((
                candidate.to_owned(),
                serde_json::to_value(explain(candidate, config))?,
            ))
        })
        .collect()
}

fn diagnostic_json(diagnostic: &zudo_wind::Diagnostic) -> Value {
    json!({
        "severity": match diagnostic.severity {
            zudo_wind::Severity::Error => "error",
            zudo_wind::Severity::Warning => "warning",
            zudo_wind::Severity::AuditInfo => "auditInfo",
        },
        "code": match diagnostic.code {
            zudo_wind::DiagnosticCode::Zw001 => "ZW001",
            zudo_wind::DiagnosticCode::Zw002 => "ZW002",
            zudo_wind::DiagnosticCode::Zw003 => "ZW003",
            zudo_wind::DiagnosticCode::Zw004 => "ZW004",
            zudo_wind::DiagnosticCode::Zw005 => "ZW005",
            zudo_wind::DiagnosticCode::Zw006 => "ZW006",
            zudo_wind::DiagnosticCode::Zw007 => "ZW007",
            zudo_wind::DiagnosticCode::Zw008 => "ZW008",
            zudo_wind::DiagnosticCode::Zw009 => "ZW009",
            zudo_wind::DiagnosticCode::Zw010 => "ZW010",
            zudo_wind::DiagnosticCode::Zw011 => "ZW011",
            zudo_wind::DiagnosticCode::Zw012 => "ZW012",
            zudo_wind::DiagnosticCode::Zw013 => "ZW013",
        },
        "candidate": diagnostic.candidate,
        "origin": diagnostic.origin.as_deref().map(origin_json),
        "message": diagnostic.message,
        "suggestion": diagnostic.suggested_spelling,
        "rejectionId": diagnostic.rejection_id,
    })
}

fn origin_json(origin: &Origin) -> Value {
    match origin {
        Origin::Source {
            source_id,
            byte_offset,
            byte_length,
            line,
            byte_column,
            literal_byte_offset,
            literal_byte_length,
            position_kind,
        } => json!({
            "kind": "source",
            "sourceId": source_id,
            "byteOffset": byte_offset,
            "byteLength": byte_length,
            "line": line,
            "byteColumn": byte_column,
            "literalByteOffset": literal_byte_offset,
            "literalByteLength": literal_byte_length,
            "positionKind": match position_kind {
                SourcePositionKind::Class => "class",
                SourcePositionKind::Literal => "literal",
            },
        }),
        Origin::Safelist { owner, index } => json!({
            "kind": "safelist",
            "owner": owner,
            "index": index,
        }),
        Origin::Manifest {
            producer,
            path,
            index,
        } => json!({
            "kind": "manifest",
            "producer": producer,
            "path": path,
            "index": index,
        }),
        Origin::Config { key_path } => json!({
            "kind": "config",
            "keyPath": key_path,
        }),
        Origin::RoleClass { role_key } => json!({
            "kind": "roleClass",
            "roleKey": role_key,
        }),
        Origin::Stylesheet {
            path,
            byte_offset,
            byte_length,
        } => json!({
            "kind": "stylesheet",
            "path": path,
            "byteOffset": byte_offset,
            "byteLength": byte_length,
        }),
    }
}
