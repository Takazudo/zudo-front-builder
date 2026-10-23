//! Esbuild never lets a chunk import an entry, and zfb renames entries after
//! bundling (`islands.js` -> `islands-<hash>.js`) while writing chunks
//! verbatim, so a split build must not keep shared code in an entry chunk:
//! a chunk importing `./islands.js` 404s in production (#4444/#4448).
//! The bundles below are the oracle; nothing here predicts Rolldown's chunking.
use super::prepared::Request;
use oxc::{
    allocator::Allocator,
    ast::ast::{
        ExportAllDeclaration, ExportFromDeclaration, Expression, ImportDeclaration,
        ImportExpression,
    },
    ast_visit::{walk, Visit},
    parser::Parser,
    span::SourceType,
};
use std::{collections::BTreeMap, fs, process::Command};

const ENTRY: &str = "islands.js";

#[test]
fn split_builds_do_not_fold_shared_code_into_an_already_loaded_entry() {
    let mut cmd = Command::new("unused-prepared-esbuild");
    cmd.current_dir(std::env::temp_dir()).args([
        "entry.js",
        "--bundle",
        "--format=esm",
        "--splitting",
        "--outdir=out",
    ]);
    let request = Request::decode(&cmd).unwrap();
    let optimization = request
        .options
        .experimental
        .and_then(|experimental| experimental.chunk_optimization)
        .expect("split builds configure chunk optimization");
    assert!(!optimization.is_avoid_redundant_chunk_loads_enabled());
    assert!(!optimization.is_merge_common_chunks_enabled());
}

/// Literal module specifiers of every static import, side-effect import,
/// re-export and dynamic `import()` in one emitted file.
#[derive(Default)]
struct Specifiers(Vec<String>);

impl<'a> Visit<'a> for Specifiers {
    fn visit_import_declaration(&mut self, decl: &ImportDeclaration<'a>) {
        self.0.push(decl.source.value.as_str().to_owned());
    }

    fn visit_export_from_declaration(&mut self, decl: &ExportFromDeclaration<'a>) {
        self.0.push(decl.source.value.as_str().to_owned());
    }

    fn visit_export_all_declaration(&mut self, decl: &ExportAllDeclaration<'a>) {
        self.0.push(decl.source.value.as_str().to_owned());
    }

    fn visit_import_expression(&mut self, import: &ImportExpression<'a>) {
        match import.source.without_parentheses() {
            Expression::StringLiteral(literal) => self.0.push(literal.value.as_str().to_owned()),
            Expression::TemplateLiteral(template) if template.expressions.is_empty() => self
                .0
                .push(template.quasis[0].value.raw.as_str().to_owned()),
            _ => {}
        }
        walk::walk_import_expression(self, import);
    }
}

fn specifiers(name: &str, source: &str) -> Vec<String> {
    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, source, SourceType::mjs()).parse();
    assert!(
        !parsed.fatal_error && !parsed.diagnostics.has_errors(),
        "emitted {name} must parse as an ES module:\n{source}"
    );
    let mut found = Specifiers::default();
    found.visit_program(&parsed.program);
    found.0
}

/// Bundles `entry.js` from `files` exactly as zfb's shared islands pass asks
/// (`--splitting`, stable entry name, self-hashed flat chunks) and returns
/// every emitted JS file by name.
fn bundle_split(name: &str, files: &[(&str, &str)]) -> BTreeMap<String, String> {
    let dir = std::env::temp_dir().join(format!("zfb-native-split-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir(&dir).unwrap();
    let dir = fs::canonicalize(dir).unwrap();
    for (path, source) in files {
        fs::write(dir.join(path), source).unwrap();
    }
    let mut cmd = Command::new("unused-prepared-esbuild");
    cmd.current_dir(&dir).args([
        "entry.js",
        "--bundle",
        "--format=esm",
        "--platform=browser",
        "--splitting=true",
        "--outdir=out",
        "--entry-names=islands",
        "--chunk-names=islands-chunk-[hash]",
        "--metafile=out/meta.json",
    ]);
    let result = super::run_prepared(&cmd);
    let mut emitted = BTreeMap::new();
    if let Ok(entries) = fs::read_dir(dir.join("out")) {
        for entry in entries {
            let path = entry.unwrap().path();
            if path.extension().is_some_and(|ext| ext == "js") {
                let file = path.file_name().unwrap().to_string_lossy().into_owned();
                emitted.insert(file, fs::read_to_string(&path).unwrap());
            }
        }
    }
    fs::remove_dir_all(&dir).unwrap();
    result.unwrap();
    emitted
}

fn render(emitted: &BTreeMap<String, String>) -> String {
    emitted
        .iter()
        .map(|(file, source)| format!("--- {file} ---\n{source}"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// No emitted chunk imports the entry, every relative specifier names an
/// emitted file, and the graph really split (a chunk exists, specifiers were
/// checked). Returns the number of relative specifiers checked.
fn assert_no_chunk_imports_the_entry(emitted: &BTreeMap<String, String>) -> usize {
    assert!(
        emitted.contains_key(ENTRY),
        "entry {ENTRY} missing:\n{}",
        render(emitted)
    );
    let chunks: Vec<_> = emitted.keys().filter(|file| *file != ENTRY).collect();
    assert!(
        !chunks.is_empty(),
        "the lazy import() must produce at least one chunk:\n{}",
        render(emitted)
    );
    for chunk in &chunks {
        assert!(
            chunk.starts_with("islands-chunk-"),
            "chunk {chunk} must use the chunk-name template:\n{}",
            render(emitted)
        );
    }
    let mut checked = 0;
    for (file, source) in emitted {
        for specifier in specifiers(file, source) {
            let Some(target) = specifier.strip_prefix("./") else {
                panic!(
                    "{file} imports non-flat specifier {specifier:?}:\n{}",
                    render(emitted)
                );
            };
            if file != ENTRY {
                assert_ne!(
                    target,
                    ENTRY,
                    "chunk {file} imports the entry, which production renames:\n{}",
                    render(emitted)
                );
            }
            assert!(
                emitted.contains_key(target),
                "{file} imports {specifier:?}, which was not emitted:\n{}",
                render(emitted)
            );
            checked += 1;
        }
    }
    assert!(
        checked > 0,
        "no relative specifier was checked:\n{}",
        render(emitted)
    );
    checked
}

#[test]
fn lazy_chunk_sharing_an_eager_module_does_not_import_the_entry() {
    let emitted = bundle_split(
        "shared-esm",
        &[
            (
                "entry.js",
                "import { sharedState, claim } from \"./shared.js\";\n\
                 claim(\"eager\");\n\
                 globalThis.openLazy = () => import(\"./lazy.js\").then((m) => m.describe());\n\
                 globalThis.eagerOwner = sharedState.owner;\n",
            ),
            (
                "shared.js",
                "export const sharedState = { owner: \"unclaimed\", claims: 0 };\n\
                 export function claim(owner) { sharedState.owner = owner; sharedState.claims += 1; }\n",
            ),
            (
                "lazy.js",
                "import { sharedState } from \"./shared.js\";\n\
                 export function describe() { return `lazy saw ${sharedState.owner}/${sharedState.claims}`; }\n",
            ),
        ],
    );
    assert_no_chunk_imports_the_entry(&emitted);
}

#[test]
fn lazy_chunk_sharing_a_cjs_interop_module_does_not_import_the_entry() {
    // Both sides need Rolldown's CJS interop runtime helpers as well as the
    // shared module itself.
    let emitted = bundle_split(
        "shared-cjs",
        &[
            (
                "entry.js",
                "import shared from \"./shared.cjs\";\n\
                 shared.claim(\"eager\");\n\
                 globalThis.openLazy = () => import(\"./lazy.js\").then((m) => m.describe());\n",
            ),
            (
                "shared.cjs",
                "const state = { owner: \"unclaimed\" };\n\
                 module.exports = { state, claim(owner) { state.owner = owner; } };\n",
            ),
            (
                "lazy.js",
                "import shared from \"./shared.cjs\";\n\
                 import legacy from \"./legacy.cjs\";\n\
                 export function describe() { return `lazy saw ${shared.state.owner} ${legacy.tag}`; }\n",
            ),
            ("legacy.cjs", "exports.tag = \"legacy\";\n"),
        ],
    );
    assert_no_chunk_imports_the_entry(&emitted);
}

#[test]
fn lazy_import_of_a_module_the_entry_imports_eagerly_does_not_import_the_entry() {
    let emitted = bundle_split(
        "dynamic-of-eager",
        &[
            (
                "entry.js",
                "import { eagerValue } from \"./eager.js\";\n\
                 globalThis.eagerValue = eagerValue;\n\
                 globalThis.openLazy = () => import(\"./lazy.js\").then((m) => m.load());\n",
            ),
            (
                "eager.js",
                "export const eagerValue = { owner: \"eager\" };\n\
                 export function touch() { return eagerValue.owner; }\n",
            ),
            (
                "lazy.js",
                "export function load() { return import(\"./eager.js\").then((m) => m.touch()); }\n",
            ),
        ],
    );
    assert_no_chunk_imports_the_entry(&emitted);
}
