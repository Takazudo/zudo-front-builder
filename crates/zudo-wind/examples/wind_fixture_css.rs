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
    spacing: BTreeMap<String, String>,
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
    #[serde(default = "default_source_file")]
    source_file: String,
    #[serde(default)]
    expected_candidates: Vec<String>,
    /// Authored global CSS file, relative to the fixture, assembled with the utilities.
    #[serde(default)]
    authored_css: Option<String>,
    #[serde(default)]
    utility_placement: PlacementChoice,
}

fn default_source_file() -> String {
    "index.html".to_owned()
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StrictManifest {
    case_ids: Vec<String>,
}

#[derive(Clone, Copy)]
enum InputMode {
    Legacy,
    Compiler,
    Extract,
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
    let mut mode = InputMode::Legacy;
    let mut manifest_path = None;
    if let Some(flag) = args.next() {
        if flag != "--mode" {
            return Err("expected --mode compiler|extract --manifest <path>".into());
        }
        mode = match args.next().as_deref() {
            Some(value) if value == "compiler" => InputMode::Compiler,
            Some(value) if value == "extract" => InputMode::Extract,
            _ => return Err("expected compiler or extract mode".into()),
        };
        if args.next().as_deref() != Some(std::ffi::OsStr::new("--manifest")) {
            return Err("strict mode requires --manifest".into());
        }
        manifest_path = Some(PathBuf::from(args.next().ok_or("missing manifest path")?));
        if args.next().is_some() {
            return Err("unexpected argument".into());
        }
    }

    let required = if let Some(path) = manifest_path {
        let manifest: StrictManifest = serde_json::from_slice(&fs::read(path)?)?;
        let set = manifest.case_ids.iter().collect::<BTreeSet<_>>();
        if set.len() != manifest.case_ids.len() || set.is_empty() {
            return Err("strict manifest has duplicate or no case IDs".into());
        }
        Some(manifest.case_ids.into_iter().collect::<BTreeSet<_>>())
    } else {
        None
    };

    let mut fixture_dirs = fs::read_dir(&fixture_root)?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<Result<Vec<_>, _>>()?;
    fixture_dirs.sort();

    let mut seen = BTreeSet::new();
    for fixture_dir in fixture_dirs {
        if !fixture_dir.is_dir() {
            continue;
        }
        let definition_path = fixture_dir.join("case.json");
        if !definition_path.is_file() {
            if required.is_some() {
                return Err(format!("missing case.json in {}", fixture_dir.display()).into());
            }
            continue;
        }
        let name = fixture_dir
            .file_name()
            .and_then(|s| s.to_str())
            .ok_or("invalid directory name")?
            .to_owned();
        if let Some(required) = &required {
            if !required.contains(&name) {
                return Err(format!("unlisted fixture {name}").into());
            }
        }
        seen.insert(name);
        generate_fixture(&fixture_dir, &output_root, mode)?;
    }
    if let Some(required) = required {
        if seen != required {
            return Err(format!(
                "missing fixtures: {:?}",
                required.difference(&seen).collect::<Vec<_>>()
            )
            .into());
        }
    }
    Ok(())
}

fn generate_fixture(
    fixture_dir: &Path,
    output_root: &Path,
    mode: InputMode,
) -> Result<(), Box<dyn Error>> {
    let directory_name = fixture_dir
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or("fixture directory name is not UTF-8")?;
    let definition_path = fixture_dir.join("case.json");
    let definition: CaseDefinition = serde_json::from_slice(&fs::read(&definition_path)?)?;
    let case_id = definition.case_id.clone();
    if matches!(mode, InputMode::Extract) && definition.expected_candidates.is_empty() {
        return Err("extract mode requires expectedCandidates".into());
    }
    if case_id.is_empty() {
        return Err(format!("{} has an empty caseId", definition_path.display()).into());
    }
    if !matches!(mode, InputMode::Legacy) && case_id != directory_name {
        return Err(
            format!("strict caseId {case_id} does not match directory {directory_name}").into(),
        );
    }
    let explicit_candidates = definition.explicit_candidates.clone();
    if matches!(mode, InputMode::Compiler) && explicit_candidates.is_empty() {
        return Err("compiler mode requires explicitCandidates".into());
    }
    if definition.source_file.contains("..") || Path::new(&definition.source_file).is_absolute() {
        return Err("sourceFile must be fixture-relative".into());
    }
    let source_id = format!("fixtures/{directory_name}/{}", definition.source_file);
    let source_path = fixture_dir.join(&definition.source_file);
    let (source, extraction) = match mode {
        InputMode::Compiler => (Vec::new(), None),
        _ => {
            let source = fs::read(&source_path)?;
            let kind = match source_path.extension().and_then(|s| s.to_str()) {
                Some("html") => SourceKind::Html,
                Some("tsx") => SourceKind::Tsx,
                Some("mdx") => SourceKind::Mdx,
                Some("ts") => SourceKind::Ts,
                Some("jsx") => SourceKind::Jsx,
                Some("js") => SourceKind::Js,
                Some("md") => SourceKind::Md,
                _ => return Err("unsupported sourceFile extension".into()),
            };
            (source.clone(), Some(extract_candidates(&source, kind)))
        }
    };
    let config = make_config(&definition);

    let mut candidates = extraction
        .iter()
        .flat_map(|extraction| extraction.candidates.iter())
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

    if matches!(mode, InputMode::Extract) && !explicit_candidates.is_empty() {
        return Err("extract mode forbids explicitCandidates".into());
    }
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
        &AuditInput::new(
            extraction
                .clone()
                .into_iter()
                .map(|extraction| AuditSource::new(source_id.clone(), extraction))
                .collect(),
        ),
        &config,
    );
    let explanations = explanation_map(&candidates, &config)?;
    let mut report = json!({
        "caseId": directory_name,
        "inputMode": match mode { InputMode::Legacy => "legacy", InputMode::Compiler => "compiler", InputMode::Extract => "extract" },
        "sourceBytes": source.len(),
        "extractedCandidates": extraction.as_ref().map(|e| e.candidates.iter().map(|c| c.text.as_str()).collect::<Vec<_>>()),
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
            spacing: definition.spacing.clone(),
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
            zudo_wind::DiagnosticCode::Zw014 => "ZW014",
            zudo_wind::DiagnosticCode::Zw015 => "ZW015",
            zudo_wind::DiagnosticCode::Zw016 => "ZW016",
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
