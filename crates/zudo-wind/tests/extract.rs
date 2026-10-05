use zudo_wind::{
    audit, extract_candidates, extract_candidates_with_options, AuditInput, ExtractionOptions,
    NoteKind, PositionKind, SourceKind, WindConfig,
};

#[test]
fn configured_ignored_attributes_skip_preview_literals_and_embedded_markup() {
    let mut options = ExtractionOptions::default();
    options.ignore_attributes.extend(
        [
            "html",
            "css",
            "title",
            "displayJs",
            "data-preview",
            "preview:html",
            "étiquette",
            "e\u{301}tiquette",
        ]
        .map(str::to_owned),
    );
    for (source, kind) in [
        (
            r#"<Preview html={`<div class="hidden bg-red-500">x</div>`} css={`.x { color: red; }`} title="suppressed" /><div class="flex missing-class" />"#,
            SourceKind::Mdx,
        ),
        (
            r#"export const View = () => <><Preview html={`<div class="hidden bg-red-500">x</div>`} css={`.x { color: red; }`} title="suppressed" /><div className="flex missing-class" /></>;"#,
            SourceKind::Tsx,
        ),
        (
            r#"<Preview html="<div class='hidden bg-red-500'>x</div>" css=".x { color: red; }" title="suppressed"></Preview><div class="flex missing-class"></div>"#,
            SourceKind::Html,
        ),
    ] {
        let result = extract_candidates_with_options(source.as_bytes(), kind, &options);
        let found: Vec<_> = result.candidates.iter().map(|c| c.text.as_str()).collect();
        assert!(
            found.contains(&"flex") && found.contains(&"missing-class"),
            "{kind:?}: {found:?}"
        );
        assert!(
            !found.contains(&"hidden")
                && !found.contains(&"bg-red-500")
                && !found.contains(&"suppressed"),
            "{kind:?}: {found:?}"
        );
        assert!(result
            .candidates
            .iter()
            .find(|c| c.text == "missing-class")
            .unwrap()
            .occurrences
            .iter()
            .any(|o| o.position_kind == PositionKind::Class));
    }
    let ordinary = extract_candidates_with_options(
        b"const html = 'flex'; const title = 'grid'; widget.html = 'block'; { let html; html = 'p-2'; }",
        SourceKind::Ts,
        &options,
    );
    assert!(ordinary.candidates.iter().any(|c| c.text == "flex"));
    assert!(ordinary.candidates.iter().any(|c| c.text == "grid"));
    assert!(ordinary.candidates.iter().any(|c| c.text == "block"));
    assert!(ordinary.candidates.iter().any(|c| c.text == "p-2"));
    let named = extract_candidates_with_options(
        br#"const node = <Preview data-preview="hidden" preview:html="block" displayJs="grid" />;"#,
        SourceKind::Tsx,
        &options,
    );
    assert!(named.candidates.is_empty(), "{named:?}");
    let unicode = extract_candidates_with_options(
        "<Preview étiquette=\"hidden\" />".as_bytes(),
        SourceKind::Tsx,
        &options,
    );
    assert!(unicode.candidates.is_empty(), "{unicode:?}");
    let decomposed = extract_candidates_with_options(
        "<Preview e\u{301}tiquette=\"hidden\" />".as_bytes(),
        SourceKind::Tsx,
        &options,
    );
    assert!(decomposed.candidates.is_empty(), "{decomposed:?}");
}

#[test]
fn ignored_jsx_expression_excludes_nested_literals_and_class_helpers() {
    let mut options = ExtractionOptions::default();
    options.ignore_attributes.insert("html".into());
    options.class_helpers.insert("ctl".into());
    let source = r#"const s = '<Preview';
let html; html = 'p-2';
let html2; html2 = 'm-2';
const View = () => <>
  <Preview title=">" html={renderPreview('<div class="hidden">')} />
  <Preview html={condition ? '<div class="hidden">' : '<div class="grid">'} />
  <Preview html={renderPreview('<div class="bg-red-500">')} />
  <Preview html={`<i class="bg-blue-500">${ctl('text-xl')}</i>`} />
  <Preview html={ctl('border-2')} />
  <Preview onClick={() => { let html; html = 'p-4'; }} />
  <Outer child={<Preview html={renderPreview('<div class="bg-green-500">')} />} />
  <div className="flex missing-real" />
</>;"#;
    let result = extract_candidates_with_options(source.as_bytes(), SourceKind::Tsx, &options);
    let found: Vec<_> = result.candidates.iter().map(|c| c.text.as_str()).collect();
    for kept in ["p-2", "m-2", "p-4", "flex", "missing-real"] {
        assert!(found.contains(&kept), "{kept}: {found:?}");
    }
    for ignored in [
        "hidden",
        "grid",
        "bg-red-500",
        "bg-blue-500",
        "text-xl",
        "border-2",
        "bg-green-500",
    ] {
        assert!(!found.contains(&ignored), "{ignored}: {found:?}");
    }
}

fn names(bytes: &[u8], kind: SourceKind) -> Vec<String> {
    extract_candidates(bytes, kind)
        .candidates
        .into_iter()
        .map(|c| c.text)
        .collect()
}

#[test]
fn eight_source_kinds_and_sorted_determinism() {
    let cases: &[(&[u8], SourceKind, &[&str])] = &[
        (
            include_bytes!("fixtures/extract/basic.ts"),
            SourceKind::Ts,
            &["p-2", "hover:bg-panel"],
        ),
        (
            include_bytes!("fixtures/extract/basic.js"),
            SourceKind::Js,
            &["m-2", "flex"],
        ),
        (
            include_bytes!("fixtures/extract/basic.jsx"),
            SourceKind::Jsx,
            &["grid", "gap-2"],
        ),
        (
            include_bytes!("fixtures/extract/basic.mjs"),
            SourceKind::Mjs,
            &["px-2", "py-3", "py-4"],
        ),
        (
            include_bytes!("fixtures/extract/basic.html"),
            SourceKind::Html,
            &["p-2", "hover:bg-panel", "text-sm"],
        ),
        (
            include_bytes!("fixtures/extract/hello.mdx"),
            SourceKind::Mdx,
            &[],
        ),
        (
            include_bytes!("fixtures/extract/styling.md"),
            SourceKind::Md,
            &[],
        ),
        (
            include_bytes!("fixtures/extract/option-row.tsx"),
            SourceKind::Tsx,
            &["last:border-b-0", "pb-vsp-2xs", "[overflow-wrap:anywhere]"],
        ),
    ];
    for &(input, kind, expected) in cases {
        let one = extract_candidates(input, kind);
        assert_eq!(one, extract_candidates(input, kind));
        let found = names(input, kind);
        assert!(found.windows(2).all(|w| w[0] < w[1]));
        for name in expected {
            assert!(
                found.iter().any(|s| s == name),
                "missing {name:?} in {kind:?}"
            );
        }
    }
}

#[test]
fn literal_map_and_template_class_positions() {
    let result = extract_candidates(
        include_bytes!("fixtures/extract/callout.tsx"),
        SourceKind::Tsx,
    );
    for name in [
        "border-sky-500",
        "bg-emerald-50",
        "dark:bg-violet-950/40",
        "text-rose-700",
    ] {
        let entry = result.candidates.iter().find(|c| c.text == name).unwrap();
        assert!(entry
            .occurrences
            .iter()
            .all(|o| o.position_kind == PositionKind::Literal));
    }
    for name in ["my-6", "rounded-md", "flex", "items-center"] {
        let entry = result.candidates.iter().find(|c| c.text == name).unwrap();
        assert!(entry
            .occurrences
            .iter()
            .any(|o| o.position_kind == PositionKind::Class));
    }
}

#[test]
fn interpolation_boundary_and_dynamic_fragment() {
    let result = extract_candidates(
        include_bytes!("fixtures/extract/option-row.tsx"),
        SourceKind::Tsx,
    );
    let item = result
        .candidates
        .iter()
        .find(|c| c.text == "last:border-b-0")
        .unwrap();
    assert!(item.occurrences.iter().any(|o| o.adjacent_interpolation));
    let dynamic = extract_candidates(b"const x = `bg-${color}`;", SourceKind::Ts);
    assert!(!dynamic.candidates.iter().any(|c| c.text == "bg-"));
    assert_eq!(
        dynamic
            .notes
            .iter()
            .filter(|n| n.kind == NoteKind::DynamicConstruction)
            .count(),
        1
    );
}

#[test]
fn classname_declarations_with_concatenation_are_dynamic() {
    for declaration in ["const", "let", "var"] {
        let source = format!(
            "{declaration} className = \"bg-\" + \"assembled\";\n\
             const classes = {{ tone: \"bg-sky-500\" }};\n\
             const node = <div className=\"p-2\" />;"
        );
        let result = extract_candidates(source.as_bytes(), SourceKind::Tsx);
        assert!(!result.candidates.iter().any(|c| c.text == "bg-assembled"));
        assert!(!result.candidates.iter().any(|c| c.text == "bg-"));
        assert!(!result.candidates.iter().any(|c| c.text == "assembled"));
        assert!(result
            .notes
            .iter()
            .any(|n| { n.kind == NoteKind::DynamicConstruction && n.text == "bg-" }));
        let literal = result
            .candidates
            .iter()
            .find(|c| c.text == "bg-sky-500")
            .unwrap();
        assert!(literal
            .occurrences
            .iter()
            .all(|o| o.position_kind == PositionKind::Literal));
        let jsx = result.candidates.iter().find(|c| c.text == "p-2").unwrap();
        assert!(jsx
            .occurrences
            .iter()
            .any(|o| o.position_kind == PositionKind::Class));

        let report = audit(
            &AuditInput::single("declaration.tsx", result),
            &WindConfig::default(),
        );
        assert!(report
            .dynamic_constructions
            .iter()
            .any(|d| { d.source_id == "declaration.tsx" && d.text == "bg-" }));
    }
}

#[test]
fn markdown_fences_are_ignored() {
    let names = names(
        include_bytes!("fixtures/extract/styling.md"),
        SourceKind::Md,
    );
    assert!(!names.iter().any(|n| n == "text-blue-600"));
}

#[test]
fn non_utf8_and_unterminated_are_safe() {
    assert_eq!(
        extract_candidates(b"\xff", SourceKind::Ts).notes[0].kind,
        NoteKind::InvalidUtf8
    );
    let _ = extract_candidates(b"const x = 'unfinished", SourceKind::Ts);
    let layout = extract_candidates(
        include_bytes!("fixtures/extract/layout.tsx"),
        SourceKind::Tsx,
    );
    assert!(layout.candidates.iter().any(|c| c
        .occurrences
        .iter()
        .any(|o| o.position_kind == PositionKind::Literal)));
    assert!(layout.candidates.iter().any(|c| c
        .occurrences
        .iter()
        .any(|o| o.position_kind == PositionKind::Class)));
}

#[test]
fn offsets_comments_entities_and_embedded_markup() {
    let source = b"<!-- <div class='hidden'> --><div class=\"p-2&amp;m-1&#32;flex\"></div>";
    let result = extract_candidates(source, SourceKind::Html);
    assert!(!result.candidates.iter().any(|c| c.text == "hidden"));
    let flex = result.candidates.iter().find(|c| c.text == "flex").unwrap();
    let at = source.windows(4).position(|w| w == b"flex").unwrap();
    assert_eq!(flex.occurrences[0].byte_offset, at);
    let source = b"const html = '<div class=\\\"px-2\\\">'; // 'hidden'";
    let result = extract_candidates(source, SourceKind::Ts);
    let entry = result.candidates.iter().find(|c| c.text == "px-2").unwrap();
    assert!(entry
        .occurrences
        .iter()
        .any(|o| o.position_kind == PositionKind::Class));
    let at = source.windows(4).position(|w| w == b"px-2").unwrap();
    assert!(entry
        .occurrences
        .iter()
        .any(|o| o.position_kind == PositionKind::Class && o.byte_offset == at));
    assert!(!result.candidates.iter().any(|c| c.text == "hidden"));
}

#[test]
fn malformed_class_is_preserved_and_unicode_is_safe() {
    let source = "é<div class='hover::block w-[calc(1px]'>";
    let result = extract_candidates(source.as_bytes(), SourceKind::Html);
    assert!(result.candidates.iter().any(|c| c.text == "hover::block"));
    assert!(result.candidates.iter().any(|c| c.text == "w-[calc(1px]"));
    assert!(result
        .notes
        .iter()
        .any(|n| n.kind == NoteKind::MalformedClassCandidate));
}

#[test]
fn markdown_inline_code_is_ignored_but_jsx_template_is_read() {
    let source =
        b"`<div class=\"hidden\">`\n<Component className={`flex ${state ? \"p-2\" : \"m-2\"}`} />";
    let result = extract_candidates(source, SourceKind::Mdx);
    let names: Vec<_> = result.candidates.iter().map(|c| c.text.as_str()).collect();
    assert!(!names.contains(&"hidden"));
    for item in ["flex", "p-2", "m-2"] {
        assert!(names.contains(&item));
    }
}

#[test]
fn multi_backtick_inline_code_is_ignored() {
    let source = b"``<div class='hidden'>`` <div class='flex'>";
    let names = names(source, SourceKind::Md);
    assert!(names.contains(&"flex".to_owned()));
    assert!(!names.contains(&"hidden".to_owned()));
}

#[test]
fn page_and_toggle_fixtures_keep_arbitrary_and_variant_candidates() {
    let page = names(
        include_bytes!("fixtures/extract/index.tsx"),
        SourceKind::Tsx,
    );
    for item in ["tracking-[0.08em]", "first:pt-0", "group-hover:text-accent"] {
        assert!(page.contains(&item.to_owned()));
    }
    let toggle = names(
        include_bytes!("fixtures/extract/theme-toggle.tsx"),
        SourceKind::Tsx,
    );
    for item in ["size-8", "dark:hover:border-neutral-700"] {
        assert!(toggle.contains(&item.to_owned()));
    }
}

#[test]
fn module_specifiers_urls_and_non_class_attributes_are_not_candidates() {
    let source = br#"import "../styles/global.css";
import { useState } from "preact/hooks";
import path from "node:path";
export { thing } from "./types.js";
const lazy = await import("virtual:widgets");
const legacy = require("./legacy.cjs");
const svg = "http://www.w3.org/2000/svg";
const name = "flex";
export const Page = () => (
  <html>
    <head>
      <meta name="viewport" content="width=device-width, initial-scale=1" />
      <link rel="stylesheet" href="/assets/app.css" />
    </head>
    <body className="grid gap-2">
      <a href="/">Back to checks</a>
      <img src="./logo.svg" alt="block" />
      <p className={cx("p-4", active && "text-accent")}>x</p>
    </body>
  </html>
);
"#;
    let found = names(source, SourceKind::Tsx);
    for noise in [
        "../styles/global.css",
        "preact/hooks",
        "node:path",
        "./types.js",
        "virtual:widgets",
        "./legacy.cjs",
        "http://www.w3.org/2000/svg",
        "viewport",
        "width=device-width,",
        "initial-scale=1",
        "stylesheet",
        "/assets/app.css",
        "/",
        "./logo.svg",
    ] {
        assert!(!found.contains(&noise.to_owned()), "{noise}: {found:?}");
    }
    for kept in ["grid", "gap-2", "p-4", "text-accent", "flex", "block"] {
        assert!(found.contains(&kept.to_owned()), "{kept}: {found:?}");
    }
}

#[test]
fn literal_punctuation_is_filtered_only_outside_balanced_arbitrary_forms() {
    let source = r#"const noise = "class=\"swatch\" system-ui, /* */ @media + * = 100% #fff";
const kept = "text-[#fff] w-[calc(100%+2px)] content-['a,b+#@%'] bg-[url('data:image/svg+xml,#fff')] クリア";
const broken = "w-[calc(100%+2px]";
const node = <div className="system-ui, @media w-[calc(100%+2px]" />;"#;
    let result = extract_candidates(source.as_bytes(), SourceKind::Tsx);
    let found: Vec<_> = result.candidates.iter().map(|c| c.text.as_str()).collect();
    for noise in [
        "class=\"swatch\"",
        "system-ui,",
        "/*",
        "*/",
        "@media",
        "+",
        "*",
        "=",
        "100%",
        "#fff",
    ] {
        assert!(
            !result
                .candidates
                .iter()
                .any(|candidate| candidate.text == noise
                    && candidate
                        .occurrences
                        .iter()
                        .any(|o| o.position_kind == PositionKind::Literal)),
            "literal {noise:?}: {found:?}"
        );
    }
    for kept in [
        "text-[#fff]",
        "w-[calc(100%+2px)]",
        "content-['a,b+#@%']",
        "bg-[url('data:image/svg+xml,#fff')]",
        "クリア",
    ] {
        assert!(found.contains(&kept), "missing {kept:?}: {found:?}");
    }
    assert!(result
        .candidates
        .iter()
        .find(|c| c.text == "w-[calc(100%+2px]")
        .is_some_and(|c| c
            .occurrences
            .iter()
            .all(|o| o.position_kind == PositionKind::Class)));
    for class_error in ["system-ui,", "@media"] {
        assert!(result
            .candidates
            .iter()
            .find(|c| c.text == class_error)
            .is_some_and(|c| c
                .occurrences
                .iter()
                .any(|o| o.position_kind == PositionKind::Class)));
    }
}

#[test]
fn embedded_markup_templates_use_class_attributes_without_raw_tokens() {
    for (source, kind) in [
        (
            r#"const html = '<div class=\"px-2\" style=\"color: red\">x</div>'"#,
            SourceKind::Ts,
        ),
        (
            r#"<Preview html={`<div class="px-2" style="color: red">x</div>`} />"#,
            SourceKind::Mdx,
        ),
        (
            r#"<Preview html={`<div className="px-2">x</div>`} />"#,
            SourceKind::Tsx,
        ),
    ] {
        let result = extract_candidates(source.as_bytes(), kind);
        let class = result
            .candidates
            .iter()
            .find(|c| c.text == "px-2")
            .expect("embedded class");
        assert_eq!(class.occurrences.len(), 1, "{kind:?}: {result:?}");
        assert_eq!(class.occurrences[0].position_kind, PositionKind::Class);
        assert!(
            !result
                .candidates
                .iter()
                .any(|c| c.text == "class=\"px-2\"" || c.text == "style=\"color:"),
            "{kind:?}: {result:?}"
        );
    }
    let html = extract_candidates(br#"<div class="px-2"></div>"#, SourceKind::Html);
    assert!(html
        .candidates
        .iter()
        .find(|c| c.text == "px-2")
        .is_some_and(|c| c.occurrences[0].position_kind == PositionKind::Class));

    let actual_class = extract_candidates(
        br#"const node = <div className={'<i class="bad,'} />;"#,
        SourceKind::Tsx,
    );
    assert!(
        actual_class
            .candidates
            .iter()
            .any(|c| c.text == "class=\"bad,"
                && c.occurrences
                    .iter()
                    .any(|o| o.position_kind == PositionKind::Class)),
        "{actual_class:?}"
    );
}

#[test]
fn concatenated_url_literals_stay_skipped() {
    let source = r#"const cdn = "https://cdn.example.test/app" + suffix;
const image = `data:image/svg+xml,${svg}`;
const className = "text-[#fff]";"#;
    let result = extract_candidates(source.as_bytes(), SourceKind::Ts);
    assert!(
        !result
            .candidates
            .iter()
            .any(|c| c.text.starts_with("https://") || c.text.starts_with("data:")),
        "{result:?}"
    );
    assert!(result.candidates.iter().any(|c| c.text == "text-[#fff]"));
}

#[test]
fn style_element_css_and_hidden_inputs_are_not_candidates() {
    let source = br#"const critical = `.x { display: grid }`;
export const Layout = () => (
  <>
    <style>{`.card { display: flex }`}</style>
    <style rawHtml={`.box { position: absolute }`} />
    <input type="hidden" name="token" className="sr-only" />
  </>
);
"#;
    let found = names(source, SourceKind::Tsx);
    for noise in ["flex", "absolute", "hidden", "token"] {
        assert!(!found.contains(&noise.to_owned()), "{noise}: {found:?}");
    }
    assert!(found.contains(&"sr-only".to_owned()), "{found:?}");
    assert!(
        found.contains(&"grid".to_owned()),
        "an unattributed template stays conservative: {found:?}"
    );
}

#[test]
fn unknown_template_text_stays_conservatively_scanned() {
    let found = names(
        b"const css = `.a { display: flex }`;\nconst key = `user-${id}-hidden`;",
        SourceKind::Ts,
    );
    assert!(
        found.contains(&"flex;".to_owned()) || found.contains(&"flex".to_owned()),
        "{found:?}"
    );
}
