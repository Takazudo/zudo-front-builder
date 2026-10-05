//! #3524: complete class expressions and same-module constants reach class
//! positions; unrelated strings and partial fragments stay low-confidence.

use std::collections::BTreeMap;

use zudo_wind::{
    audit, extract_candidates, extract_candidates_with_options, AuditInput, ExtractionOptions,
    ExtractionResult, NoteKind, PositionKind, SourceKind, WindConfig,
};

fn extract(source: &str) -> ExtractionResult {
    extract_candidates(source.as_bytes(), SourceKind::Tsx)
}

fn kinds(result: &ExtractionResult, text: &str) -> Vec<PositionKind> {
    result
        .candidates
        .iter()
        .find(|candidate| candidate.text == text)
        .map(|candidate| {
            candidate
                .occurrences
                .iter()
                .map(|occurrence| occurrence.position_kind)
                .collect()
        })
        .unwrap_or_default()
}

fn assert_class(result: &ExtractionResult, text: &str) {
    let kinds = kinds(result, text);
    assert!(
        !kinds.is_empty() && kinds.iter().all(|kind| *kind == PositionKind::Class),
        "{text}: {kinds:?}"
    );
}

fn assert_literal(result: &ExtractionResult, text: &str) {
    let kinds = kinds(result, text);
    assert!(
        !kinds.is_empty() && kinds.iter().all(|kind| *kind == PositionKind::Literal),
        "{text}: {kinds:?}"
    );
}

#[test]
fn conditional_branches_inside_a_class_template_are_class_positions() {
    let result = extract(
        r#"export const B = ({ isActive }) => (
  <button class={`flex border ${isActive ? "border-accent ring-2 ring-accent" : "border-muted"}`} />
);"#,
    );
    for text in [
        "flex",
        "border",
        "border-accent",
        "ring-2",
        "ring-accent",
        "border-muted",
    ] {
        assert_class(&result, text);
    }
}

#[test]
fn direct_conditionals_logical_fallbacks_and_helpers_are_class_positions() {
    let result = extract(
        r#"const A = ({ on, tone, size }) => (
  <div
    className={on ? "p-2" : "p-4"}
    class={cn("flex", on && "gap-2", tone ?? "text-ink", [size === "lg" ? "w-full" : "w-1/2"], { "rounded-md": on })}
  />
);"#,
    );
    for text in [
        "p-2",
        "p-4",
        "flex",
        "gap-2",
        "text-ink",
        "w-full",
        "w-1/2",
        "rounded-md",
    ] {
        assert_class(&result, text);
    }
    // A comparison operand is not a class value.
    assert_literal(&result, "lg");
}

#[test]
fn gallery_nav_and_tooltip_constants_reach_their_class_attributes() {
    let result = extract(
        r#"const focusClass = "focus-visible:outline-2";
const linkClass = "px-3 text-ink aria-[current=page]:bg-soft";
const navLinkClass = `py-2 ${linkClass} ${focusClass}`;
const tipClass = `opacity-0 left-1/2 [.group:hover_&]:opacity-100 ${"x"}`;
const alias = tipClass;
export const A = () => (
  <a class={navLinkClass}>
    <span class={alias}>t</span>
  </a>
);"#,
    );
    for text in [
        "focus-visible:outline-2",
        "px-3",
        "text-ink",
        "aria-[current=page]:bg-soft",
        "py-2",
        "opacity-0",
        "left-1/2",
        "[.group:hover_&]:opacity-100",
        "x",
    ] {
        assert_class(&result, text);
    }
}

#[test]
fn typed_and_exported_constants_are_traced() {
    let result = extract(
        r#"export const shell: string = "grid gap-4";
const A = () => <main class={shell} />;"#,
    );
    assert_class(&result, "grid");
    assert_class(&result, "gap-4");
}

#[test]
fn unrelated_strings_stay_low_confidence() {
    let result = extract(
        r#"import styles from "./ring-2.module.css";
import { sharedClass } from "./shared";
const unused = "ring-2 animate-spin";
let mutable = "scale-50";
const reassigned = "p-8";
const url = "https://example.com/p-3";
describe("renders flex ring-4", () => {
  it("handles animate-ping", () => {});
});
const A = ({ n }) => (
  <div class={sharedClass}>
    <p class={mutable} />
    <p class={styles.ring} />
    <p class={pick("m-2")} />
    <p class={"ring-" + n} />
    <p class={`ring-${n}`} />
  </div>
);"#,
    );
    for text in [
        "ring-2",
        "animate-spin",
        "scale-50",
        "ring-4",
        "animate-ping",
        "m-2",
    ] {
        assert_literal(&result, text);
    }
    assert!(!result.candidates.iter().any(|c| c.text == "ring-"));
    assert!(result
        .notes
        .iter()
        .any(|note| note.kind == NoteKind::DynamicConstruction && note.text == "ring-"));
}

#[test]
fn reassigned_and_shadowed_constants_are_not_traced() {
    let result = extract(
        r#"const tone = "p-8";
function f() { const tone = "p-9"; return tone; }
let swapped = "m-8";
swapped = "m-9";
const A = () => <div class={tone}><b class={swapped} /></div>;"#,
    );
    for text in ["p-8", "p-9", "m-8", "m-9"] {
        assert_literal(&result, text);
    }
}

#[test]
fn constant_cycles_terminate() {
    let result = extract(
        r#"const a = b;
const b = flag ? a : "gap-2";
const A = () => <div class={a} />;"#,
    );
    assert_class(&result, "gap-2");
}

#[test]
fn proven_complete_strings_error_like_direct_classes_through_audit() {
    let result = extract(
        r#"const ringClass = "ring-2";
const A = ({ on }) => <div class={`flex ${on ? "ring-2" : ""}`}><i class={ringClass} /></div>;"#,
    );
    let mut config = WindConfig::default();
    config.tokens.colors = BTreeMap::new();
    let report = audit(&AuditInput::single("card.tsx", result), &config);
    let ring: Vec<_> = report
        .diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.candidate.as_deref() == Some("ring-2"))
        .collect();
    assert_eq!(ring.len(), 2, "{ring:?}");
    assert!(ring
        .iter()
        .all(|diagnostic| diagnostic.code == "ZW004" && diagnostic.severity == "error"));
}

#[test]
fn parameters_and_destructured_bindings_shadow_constants() {
    let result = extract(
        r#"const tone = "table-row";
const size = "p-6";
const width = "w-4";
export function A({ tone }) {
  return <td className={tone} />;
}
const B = (size) => <p class={size} />;
const C = () => {
  const { width } = props;
  return <i class={width} />;
};"#,
    );
    for text in ["table-row", "p-6", "w-4"] {
        assert_literal(&result, text);
    }
}

#[test]
fn compared_groups_are_conditions_not_class_values() {
    let result = extract(
        r#"const A = ({ v }) => (
  <div class={(v || "order-first") === "a" ? "flex" : "grid"} />
);"#,
    );
    assert_literal(&result, "order-first");
    assert_class(&result, "flex");
    assert_class(&result, "grid");
}

#[test]
fn configured_helper_traces_multiline_templates_nested_values_and_direct_attributes() {
    let source = r#"const viaCtl = ctl(`
  text-xl
  bg-nope-1
`);
const other = mystery("text-hidden");
const cvaValue = cva("text-cva");
const twValue = twMerge("text-tw");
const unicode = élément("text-wide");
export const Page = ({ on }) => <>
<p class={viaCtl} />
<div class={ctl([
  "p-2",
  [on && "m-1"],
  { "rounded-md": on }
])} /><span class={unicode} />
</>;"#;
    let default = extract(source);
    assert_literal(&default, "text-xl");
    assert_literal(&default, "bg-nope-1");
    assert_literal(&default, "p-2");
    let mut options = ExtractionOptions::default();
    options.class_helpers.insert("ctl".into());
    options.class_helpers.insert("élément".into());
    let result = extract_candidates_with_options(source.as_bytes(), SourceKind::Tsx, &options);
    for text in [
        "text-xl",
        "bg-nope-1",
        "p-2",
        "m-1",
        "rounded-md",
        "text-wide",
    ] {
        assert_class(&result, text);
    }
    assert_literal(&result, "text-hidden");
    assert_literal(&result, "text-cva");
    assert_literal(&result, "text-tw");
}

#[test]
fn candidate_index_uses_helper_options_for_file_extraction() {
    use zudo_wind::{CandidateIndex, SourceId};
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("view.tsx");
    std::fs::write(
        &path,
        "export const Page = () => <div class={ctl(`text-xl`)} />;",
    )
    .unwrap();
    let file = zudo_wind::ExpandedFile {
        id: SourceId::new("root", std::path::Path::new("view.tsx")).unwrap(),
        path,
    };
    let mut options = ExtractionOptions::default();
    options.class_helpers.insert("ctl".into());
    let mut index = CandidateIndex::with_options(options);
    let extraction = index.index_file(&file).unwrap();
    assert_class(&extraction, "text-xl");
    assert!(index.live_set().contains("text-xl"));
}

#[test]
fn owned_factory_props_trace_values_and_preserve_authored_spans() {
    let source = r#"import { h as make, type h as TypeH } from "@takazudo/zfb/zudo-react";
const saved = "ring-2";
const view = make("div", {
  class: active ? `flex ${saved}` : "gap-2",
  'className': cn("p-3", { "rounded-md": active }),
  "class": "text-ink",
});"#;
    let result = extract_candidates(source.as_bytes(), SourceKind::Ts);
    for value in ["ring-2", "flex", "gap-2", "p-3", "rounded-md", "text-ink"] {
        assert_class(&result, value);
        let candidate = result
            .candidates
            .iter()
            .find(|candidate| candidate.text == value)
            .unwrap();
        assert!(
            candidate.occurrences.iter().any(|occurrence| {
                let span = &source
                    [occurrence.byte_offset..occurrence.byte_offset + occurrence.byte_length];
                span == value && occurrence.position_kind == PositionKind::Class
            }),
            "{value} has no authored class span"
        );
    }
    let mut config = WindConfig::default();
    config.tokens.colors = BTreeMap::new();
    let report = audit(&AuditInput::single("view.ts", result), &config);
    assert!(report
        .diagnostics
        .iter()
        .any(
            |diagnostic| diagnostic.candidate.as_deref() == Some("ring-2")
                && diagnostic.code == "ZW004"
                && diagnostic.severity == "error"
        ));
}

#[test]
fn unrelated_factory_data_types_and_shadowed_imports_stay_literals() {
    let source = r#"import { h as make } from "@takazudo/zfb/zudo-react";
import { h as foreign } from "preact";
import type { h as TypeH } from "@takazudo/zfb/zudo-react";
const className = "p-1";
type Shape = { className: "p-2" };
const data = { class: "p-3" };
const label = "class";
foreign("div", { class: "p-4" });
h("div", { class: "p-5" });
TypeH("div", { class: "p-6" });
make("div", { other: "p-7" }, { class: "p-8" });
make("div", { "other": "p-9" });
const choose = label === "class" ? "m-1" : "m-2";
function nested(make) { return make("div", { class: "p-10" }); }
make("div", { class: "p-11" });"#;
    let result = extract_candidates(source.as_bytes(), SourceKind::Ts);
    for value in [
        "p-1", "p-2", "p-3", "p-4", "p-5", "p-6", "p-8", "p-9", "m-1", "m-2", "p-10", "p-11",
    ] {
        assert_literal(&result, value);
    }

    let unshadowed = r#"import { h as make } from '@takazudo/zfb/zudo-react';
import { h as foreign } from 'preact';
import type { h as TypeH } from '@takazudo/zfb/zudo-react';
const data = { class: 'p-3' };
type Shape = { className: 'p-2' };
foreign('div', { class: 'p-4' });
h('div', { class: 'p-5' });
TypeH('div', { class: 'p-6' });
make('div', { other: 'p-7' }, { class: 'p-8' });
make('div', { other: 'p-9' });
switch (kind) { case 'class': value = 'm-3'; break; }
const pick = kind ? 'm-4' : 'm-5';
make('div', { class: 'p-12' });"#;
    let result = extract_candidates(unshadowed.as_bytes(), SourceKind::Ts);
    for value in [
        "p-2", "p-3", "p-4", "p-5", "p-6", "p-8", "p-9", "m-3", "m-4", "m-5",
    ] {
        assert_literal(&result, value);
    }
    assert_class(&result, "p-12");
}
