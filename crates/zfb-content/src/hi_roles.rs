//! Fixed 18-role semantic highlight taxonomy and the syntect-scope→role
//! classifier for the code-highlight token epic (#1528).
//!
//! syntect grammars emit dozens of fine-grained TextMate scopes
//! (`keyword.operator.assignment.js`, `entity.other.attribute-name.class.css`,
//! …). Emitting one CSS custom property per raw scope would explode the token
//! surface and couple our themes to grammar internals. Instead every token is
//! collapsed into one of exactly 18 [`HiRole`]s — the design-token vocabulary
//! the class-emission sub maps to `--zfb-hi-<suffix>` properties. The reduced
//! set is FIXED by issue #1529; the prefix→role mappings may be refined but
//! roles may not be added or removed.

use std::sync::LazyLock;

use syntect::parsing::Scope;

/// Declare the fixed semantic-role taxonomy once, then derive every public
/// Rust representation from it. The fixed-size arrays intentionally enforce
/// the 18-role contract at compile time.
macro_rules! define_hi_roles {
    ($( $variant:ident => ($short:literal, $full:literal) ),+ $(,)?) => {
        /// One of the 18 semantic roles a highlighted token can carry.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub enum HiRole {
            $( $variant, )+
        }

        impl HiRole {
            /// Every role, in taxonomy order (matches the #1529 table).
            pub const ALL: [HiRole; 18] = [
                $( HiRole::$variant, )+
            ];

            /// Full role names in taxonomy order, for const consumers such as
            /// `zfb::config::CODE_HIGHLIGHT_ROLES`.
            pub const FULL_NAMES: [&str; 18] = [
                $( $full, )+
            ];

            /// The class suffix for this role: the emitted class is
            /// `<prefix>-<suffix>` and the CSS custom property is
            /// `--zfb-hi-<suffix>`. Must stay unique and must never be
            /// `"line"` — the code-enrichment wrapper already uses a `line`
            /// class, so a collision would fuse token and line styling.
            pub fn short_name(self) -> &'static str {
                match self {
                    $( HiRole::$variant => $short, )+
                }
            }

            /// The full role name — the user-facing `codeHighlight.roleClasses`
            /// key (e.g. `"keyword"`) and the config-validation name in
            /// `CODE_HIGHLIGHT_ROLES` (`crates/zfb/src/config.rs`). The
            /// class-mode emitter resolves `roleClasses` overrides by THIS
            /// name; the default class uses [`Self::short_name`] (`hi-kw`).
            /// The two must not be conflated — user config keys by full name,
            /// so looking overrides up by `short_name` silently drops them.
            pub fn full_name(self) -> &'static str {
                match self {
                    $( HiRole::$variant => $full, )+
                }
            }
        }
    };
}

define_hi_roles! {
    Escape => ("esc", "escape"),
    Operator => ("op", "operator"),
    Comment => ("com", "comment"),
    String => ("str", "string"),
    Number => ("num", "number"),
    Constant => ("const", "constant"),
    Keyword => ("kw", "keyword"),
    Function => ("fn", "function"),
    Type => ("ty", "type"),
    Namespace => ("ns", "namespace"),
    Property => ("prop", "property"),
    Variable => ("var", "variable"),
    Tag => ("tag", "tag"),
    Attribute => ("attr", "attribute"),
    Punctuation => ("punct", "punctuation"),
    Inserted => ("ins", "inserted"),
    Deleted => ("del", "deleted"),
    Heading => ("hd", "heading"),
}

/// Scope-prefix → role rules, kept as plain strings so the mapping is
/// auditable at a glance against the #1529 table. Order is irrelevant:
/// [`classify`] always picks the *longest* matching prefix at each scope, so
/// e.g. `keyword.operator` (op) wins over `keyword` (kw),
/// `constant.character.escape` (esc) over `constant` (const),
/// `entity.other.attribute-name.class` (prop) over `entity.other.attribute-name`
/// (attr), and `support.type.property-name` (prop) over `support.type` (ty).
const RULES: &[(&str, HiRole)] = &[
    ("constant.character.escape", HiRole::Escape),
    ("keyword.operator", HiRole::Operator),
    ("comment", HiRole::Comment),
    ("string", HiRole::String),
    ("constant.numeric", HiRole::Number),
    ("constant", HiRole::Constant),
    ("keyword", HiRole::Keyword),
    ("storage", HiRole::Keyword),
    ("entity.name.function", HiRole::Function),
    ("support.function", HiRole::Function),
    ("variable.function", HiRole::Function),
    ("entity.name.type", HiRole::Type),
    ("entity.name.class", HiRole::Type),
    ("entity.name.enum", HiRole::Type),
    ("entity.name.interface", HiRole::Type),
    ("entity.name.trait", HiRole::Type),
    ("entity.name.struct", HiRole::Type),
    ("entity.name.union", HiRole::Type),
    ("support.type.property-name", HiRole::Property),
    ("support.type", HiRole::Type),
    ("support.class", HiRole::Type),
    ("entity.name.namespace", HiRole::Namespace),
    ("entity.name.module", HiRole::Namespace),
    ("variable.other.member", HiRole::Property),
    ("variable.other.property", HiRole::Property),
    ("entity.other.attribute-name.class", HiRole::Property),
    ("entity.other.attribute-name.id", HiRole::Property),
    ("entity.other.attribute-name", HiRole::Attribute),
    ("entity.name.label", HiRole::Variable),
    ("variable", HiRole::Variable),
    ("entity.name.tag", HiRole::Tag),
    ("markup.inserted", HiRole::Inserted),
    ("markup.deleted", HiRole::Deleted),
    ("markup.heading", HiRole::Heading),
    ("punctuation", HiRole::Punctuation),
];

/// [`RULES`] compiled to interned [`Scope`]s once, so [`classify`] matches with
/// syntect's bitwise [`Scope::is_prefix_of`] (atom-boundary correct, no
/// per-token string allocation or global-repo locking).
static COMPILED_RULES: LazyLock<Vec<(Scope, HiRole)>> = LazyLock::new(|| {
    RULES
        .iter()
        .map(|(prefix, role)| {
            (
                Scope::new(prefix).expect("hi_roles rule prefix is a valid scope"),
                *role,
            )
        })
        .collect()
});

/// Delimiter scopes skipped during the walk: `punctuation.definition.*` (string
/// quotes, comment markers, interpolation braces) and `punctuation.section.*`
/// (block braces/brackets). They classify by their *enclosing* construct, not
/// as punctuation — a string's `"` becomes str via the surrounding
/// `string.*` scope, a comment's `//` becomes com. Bare
/// `punctuation.separator/terminator/accessor` is NOT skipped and falls to the
/// `punctuation` rule → punct.
static SKIP_PREFIXES: LazyLock<[Scope; 2]> = LazyLock::new(|| {
    [
        Scope::new("punctuation.definition").expect("valid skip prefix"),
        Scope::new("punctuation.section").expect("valid skip prefix"),
    ]
});

/// Classify a token's scope stack into a [`HiRole`], or `None` when no rule
/// matches anywhere (the caller emits bare escaped text inheriting the base
/// `--zfb-hi-fg`).
///
/// `scopes` is a syntect scope stack ordered outermost-first (index 0 is the
/// grammar root, e.g. `source.rust`), exactly as [`syntect::parsing::ScopeStack::as_slice`]
/// returns it. The walk runs innermost→outermost; at each scope, delimiter
/// scopes are skipped, then the longest matching prefix rule wins; the first
/// scope that yields a role terminates the walk. Innermost-wins is what makes a
/// template-literal interpolation variable classify as var rather than str.
pub fn classify(scopes: &[Scope]) -> Option<HiRole> {
    for &scope in scopes.iter().rev() {
        if SKIP_PREFIXES.iter().any(|p| p.is_prefix_of(scope)) {
            continue;
        }
        let mut best_len = 0u32;
        let mut best_role: Option<HiRole> = None;
        for &(prefix, role) in COMPILED_RULES.iter() {
            if prefix.is_prefix_of(scope) && prefix.len() > best_len {
                best_len = prefix.len();
                best_role = Some(role);
            }
        }
        if best_role.is_some() {
            return best_role;
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use syntect::easy::ScopeRegionIterator;
    use syntect::parsing::{ParseState, ScopeStack, SyntaxSet};
    use syntect::util::LinesWithEndings;

    /// Map a #1529 corpus language token to its bundled syntect syntax name.
    /// TypeScript is NOT in syntect's bundled `SyntaxSet`, so `ts` (like `js`)
    /// resolves to the JavaScript grammar — the same fallback the docs-site
    /// highlighter uses (`syntect_highlight::resolve_alias`).
    fn syntax_name_for(lang: &str) -> &'static str {
        match lang {
            "rust" => "Rust",
            "ts" | "js" => "JavaScript",
            "css" => "CSS",
            "html" => "HTML",
            "json" => "JSON",
            "bash" => "Bourne Again Shell (bash)",
            "python" => "Python",
            "md" => "Markdown",
            "diff" => "Diff",
            other => panic!("unmapped corpus language: {other}"),
        }
    }

    /// Tokenize `code` under the grammar for `lang`, returning `(text, role)`
    /// for every non-empty token. Both `ParseState` and `ScopeStack` persist
    /// across lines, so multi-line constructs (block comments, template/raw
    /// strings) carry their scope state into line 2+.
    fn tokenize(lang: &str, code: &str) -> Vec<(String, Option<HiRole>)> {
        let ss = SyntaxSet::load_defaults_newlines();
        let name = syntax_name_for(lang);
        let syntax = ss
            .find_syntax_by_name(name)
            .unwrap_or_else(|| panic!("bundled SyntaxSet missing syntax {name:?}"));
        let mut state = ParseState::new(syntax);
        let mut stack = ScopeStack::new();
        let mut out = Vec::new();
        for line in LinesWithEndings::from(code) {
            let ops = state.parse_line(line, &ss).expect("parse_line");
            for (tok, op) in ScopeRegionIterator::new(&ops, line) {
                stack.apply(op).expect("apply scope op");
                if tok.is_empty() {
                    continue;
                }
                out.push((tok.to_string(), classify(stack.as_slice())));
            }
        }
        out
    }

    /// Role of the first token whose trimmed text equals `needle`.
    fn role_of(toks: &[(String, Option<HiRole>)], needle: &str) -> Option<HiRole> {
        toks.iter()
            .find(|(t, _)| t.trim() == needle)
            .unwrap_or_else(|| {
                let seen: Vec<&str> = toks.iter().map(|(t, _)| t.trim()).collect();
                panic!("no token {needle:?} among {seen:?}")
            })
            .1
    }

    #[test]
    fn taxonomy_has_exactly_18_roles_with_unique_nonempty_suffixes() {
        assert_eq!(HiRole::ALL.len(), 18, "the taxonomy is fixed at 18 roles");

        let suffixes: Vec<&str> = HiRole::ALL.iter().map(|r| r.short_name()).collect();
        let unique: HashSet<&str> = suffixes.iter().copied().collect();
        assert_eq!(unique.len(), 18, "short names must be unique: {suffixes:?}");

        for s in &suffixes {
            assert!(!s.is_empty(), "short name must be non-empty");
            assert_ne!(
                *s, "line",
                "short name must not collide with `line` wrapper class"
            );
        }
    }

    #[test]
    fn taxonomy_role_names_match_fixed_public_mapping_and_order() {
        let expected = [
            ("esc", "escape"),
            ("op", "operator"),
            ("com", "comment"),
            ("str", "string"),
            ("num", "number"),
            ("const", "constant"),
            ("kw", "keyword"),
            ("fn", "function"),
            ("ty", "type"),
            ("ns", "namespace"),
            ("prop", "property"),
            ("var", "variable"),
            ("tag", "tag"),
            ("attr", "attribute"),
            ("punct", "punctuation"),
            ("ins", "inserted"),
            ("del", "deleted"),
            ("hd", "heading"),
        ];

        assert_eq!(
            HiRole::ALL.map(|role| (role.short_name(), role.full_name())),
            expected,
            "enum accessors must retain the canonical mapping and order"
        );
        assert_eq!(
            HiRole::FULL_NAMES,
            expected.map(|(_, full_name)| full_name),
            "const full-name array must retain the canonical taxonomy order"
        );
    }

    #[test]
    fn rust_keywords_strings_comments_numbers_escapes_and_types() {
        let toks = tokenize(
            "rust",
            "fn main() {\n    let x = 42; // c\n    let s = \"he\\nlo\";\n}\n",
        );
        assert_eq!(role_of(&toks, "let"), Some(HiRole::Keyword));
        assert_eq!(role_of(&toks, "="), Some(HiRole::Operator));
        assert_eq!(role_of(&toks, "42"), Some(HiRole::Number));
        assert_eq!(role_of(&toks, "//"), Some(HiRole::Comment)); // `//` marker → com (skip rule)
        assert_eq!(role_of(&toks, "\""), Some(HiRole::String)); // quote delimiter → str (skip rule)
        assert_eq!(role_of(&toks, "\\n"), Some(HiRole::Escape));
        assert_eq!(role_of(&toks, ";"), Some(HiRole::Punctuation));
    }

    #[test]
    fn rust_type_name_classifies_as_type() {
        let toks = tokenize("rust", "struct Foo {}\n");
        assert_eq!(role_of(&toks, "struct"), Some(HiRole::Keyword));
        assert_eq!(role_of(&toks, "Foo"), Some(HiRole::Type));
    }

    #[test]
    fn rust_multiline_block_comment_line2_stays_comment() {
        let toks = tokenize("rust", "/* b1\n b2 */\n");
        // Line 2 (`b2`) is only a comment because ScopeStack state persisted
        // across the newline — the classic cross-line coverage check.
        assert_eq!(role_of(&toks, "b2"), Some(HiRole::Comment));
    }

    #[test]
    fn rust_multiline_string_line2_stays_string() {
        let toks = tokenize("rust", "let y = \"m1\nm2\";\n");
        assert_eq!(role_of(&toks, "m2"), Some(HiRole::String));
    }

    #[test]
    fn javascript_operator_beats_keyword_and_interpolation_var_beats_string() {
        // `js` and `ts` share the JavaScript grammar in the bundled set.
        for lang in ["js", "ts"] {
            let toks = tokenize(
                lang,
                "const g = `hi ${name}`;\nlet z = 1 + 2;\nclass Bar {}\n",
            );
            assert_eq!(role_of(&toks, "const"), Some(HiRole::Keyword));
            // `+` is scoped keyword.operator.arithmetic → op wins over keyword.
            assert_eq!(role_of(&toks, "+"), Some(HiRole::Operator));
            assert_eq!(role_of(&toks, "1"), Some(HiRole::Number));
            // Inside `${ }` the innermost variable scope wins over the enclosing
            // template string → var, not str.
            assert_eq!(role_of(&toks, "name"), Some(HiRole::Variable));
            assert_eq!(role_of(&toks, "hi"), Some(HiRole::String));
            assert_eq!(role_of(&toks, "Bar"), Some(HiRole::Type));
        }
    }

    #[test]
    fn css_property_and_class_and_id_selectors_classify_as_property() {
        let toks = tokenize("css", ".foo { color: red; }\n#bar { top: 0; }\n");
        // `.class` and `#id` selector names → prop (longest-prefix beats attr).
        assert_eq!(role_of(&toks, "foo"), Some(HiRole::Property));
        assert_eq!(role_of(&toks, "bar"), Some(HiRole::Property));
        // property name → prop (support.type.property-name beats support.type → ty).
        assert_eq!(role_of(&toks, "color"), Some(HiRole::Property));
        assert_eq!(role_of(&toks, "0"), Some(HiRole::Number));
        assert_eq!(role_of(&toks, ":"), Some(HiRole::Punctuation));
    }

    #[test]
    fn html_tag_and_attribute_names() {
        let toks = tokenize("html", "<a href=\"y\">hi</a>\n");
        assert_eq!(role_of(&toks, "a"), Some(HiRole::Tag));
        // A generic attribute → attr (a `class`/`id` attribute would be prop).
        assert_eq!(role_of(&toks, "href"), Some(HiRole::Attribute));
    }

    #[test]
    fn json_strings_and_numbers() {
        let toks = tokenize("json", "{ \"key\": 123 }\n");
        assert_eq!(role_of(&toks, "\""), Some(HiRole::String));
        assert_eq!(role_of(&toks, "key"), Some(HiRole::String));
        assert_eq!(role_of(&toks, "123"), Some(HiRole::Number));
    }

    #[test]
    fn bash_hash_comment_marker_and_string() {
        let toks = tokenize("bash", "# c\necho \"hi\"\n");
        // `#` line-comment marker → com via the skip rule.
        assert_eq!(role_of(&toks, "#"), Some(HiRole::Comment));
        assert_eq!(role_of(&toks, "hi"), Some(HiRole::String));
    }

    #[test]
    fn python_keywords_function_number_and_comment() {
        let toks = tokenize("python", "def f():\n    return 1  # c\n");
        assert_eq!(role_of(&toks, "def"), Some(HiRole::Keyword));
        assert_eq!(role_of(&toks, "return"), Some(HiRole::Keyword));
        assert_eq!(role_of(&toks, "f"), Some(HiRole::Function));
        assert_eq!(role_of(&toks, "1"), Some(HiRole::Number));
        assert_eq!(role_of(&toks, "#"), Some(HiRole::Comment));
    }

    #[test]
    fn markdown_heading_fence_classifies_as_heading() {
        let toks = tokenize("md", "# Heading\n");
        assert_eq!(role_of(&toks, "#"), Some(HiRole::Heading));
        assert_eq!(role_of(&toks, "Heading"), Some(HiRole::Heading));
    }

    #[test]
    fn diff_plus_and_minus_lines_classify_as_inserted_and_deleted() {
        // Header lines (`---`/`+++`/`@@`) are omitted so the only `-`/`+`
        // tokens are the line markers, keeping `role_of` unambiguous. Each
        // marker is scoped punctuation.definition.{deleted,inserted} (skipped)
        // over markup.{deleted,inserted} → del/ins via the skip rule.
        let toks = tokenize("diff", "-removed\n+added\n");
        assert_eq!(role_of(&toks, "-"), Some(HiRole::Deleted));
        assert_eq!(role_of(&toks, "removed"), Some(HiRole::Deleted));
        assert_eq!(role_of(&toks, "+"), Some(HiRole::Inserted));
        assert_eq!(role_of(&toks, "added"), Some(HiRole::Inserted));
    }

    #[test]
    fn rust_module_name_classifies_as_namespace() {
        let toks = tokenize("rust", "mod foo {}\n");
        // `foo` is scoped entity.name.module.rust → ns.
        assert_eq!(role_of(&toks, "foo"), Some(HiRole::Namespace));
    }

    #[test]
    fn language_constant_classifies_as_constant() {
        // `constant.language.*` matches only the bare `constant` rule → const
        // (there is no `constant.language` rule, and `constant.numeric` does
        // not prefix it).
        let js = tokenize("js", "let a = null;\n");
        assert_eq!(role_of(&js, "null"), Some(HiRole::Constant));
        let py = tokenize("python", "x = None\n");
        assert_eq!(role_of(&py, "None"), Some(HiRole::Constant));
    }

    #[test]
    fn empty_scope_stack_yields_no_role() {
        assert_eq!(classify(&[]), None);
    }
}
