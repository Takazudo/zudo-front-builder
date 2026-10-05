//! Pure, vocabulary-free extraction of complete source candidates.
mod markdown;
mod markup;
mod script;

use crate::structural_split;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SourceKind {
    Tsx,
    Ts,
    Jsx,
    Js,
    Mjs,
    Mdx,
    Md,
    Html,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum PositionKind {
    Class,
    Literal,
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct Occurrence {
    pub byte_offset: usize,
    pub byte_length: usize,
    pub line: usize,
    pub byte_column: usize,
    pub literal_byte_offset: usize,
    pub literal_byte_length: usize,
    pub position_kind: PositionKind,
    pub adjacent_interpolation: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExtractedCandidate {
    pub text: String,
    pub occurrences: Vec<Occurrence>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum NoteKind {
    DynamicConstruction,
    MalformedClassCandidate,
    InvalidUtf8,
    UnterminatedLiteral,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExtractionNote {
    pub kind: NoteKind,
    pub byte_offset: usize,
    /// One-based line and byte column of `byte_offset` in the original source.
    pub line: usize,
    pub byte_column: usize,
    pub text: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ExtractionResult {
    pub candidates: Vec<ExtractedCandidate>,
    pub notes: Vec<ExtractionNote>,
}

/// Names with class-list argument semantics. Additional names are explicit opt-ins.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExtractionOptions {
    pub class_helpers: BTreeSet<String>,
}

impl Default for ExtractionOptions {
    fn default() -> Self {
        Self {
            class_helpers: ["clsx", "cn", "cx", "classNames", "classnames"]
                .map(str::to_owned)
                .into(),
        }
    }
}

pub fn extract_candidates(bytes: &[u8], kind: SourceKind) -> ExtractionResult {
    extract_candidates_with_options(bytes, kind, &ExtractionOptions::default())
}

pub fn extract_candidates_with_options(
    bytes: &[u8],
    kind: SourceKind,
    options: &ExtractionOptions,
) -> ExtractionResult {
    let Ok(source) = std::str::from_utf8(bytes) else {
        return ExtractionResult {
            notes: vec![ExtractionNote {
                kind: NoteKind::InvalidUtf8,
                byte_offset: 0,
                line: 1,
                byte_column: 1,
                text: "source is not UTF-8".into(),
            }],
            ..Default::default()
        };
    };
    let mut collector = Collector {
        source,
        newline_offsets: source
            .bytes()
            .enumerate()
            .filter_map(|(at, byte)| (byte == b'\n').then_some(at))
            .collect(),
        found: BTreeMap::new(),
        notes: Vec::new(),
        frames: Vec::new(),
        options,
    };
    match kind {
        SourceKind::Tsx | SourceKind::Ts | SourceKind::Jsx | SourceKind::Js | SourceKind::Mjs => {
            script::scan(source, 0, &mut collector)
        }
        SourceKind::Html => markup::scan(source, 0, &mut collector, true),
        SourceKind::Md | SourceKind::Mdx => {
            markdown::scan(source, &mut collector, kind == SourceKind::Mdx)
        }
    }
    collector.finish()
}

pub(super) struct Collector<'a> {
    source: &'a str,
    options: &'a ExtractionOptions,
    /// Byte offsets of newlines in the original source, sorted for position lookups.
    newline_offsets: Vec<usize>,
    found: BTreeMap<String, Vec<Occurrence>>,
    notes: Vec<ExtractionNote>,
    /// Decoded texts being scanned in place of their raw spelling, innermost
    /// last: each maps a byte offset of its decoded text to `base` plus a
    /// byte offset into the enclosing coordinate space.
    frames: Vec<Frame>,
}

struct Frame {
    base: usize,
    map: Vec<usize>,
}

impl Collector<'_> {
    /// Scans `decoded` (whose `map` has one entry per decoded byte plus the
    /// end) as if it sat at `base`, reporting every position in raw-source
    /// coordinates.
    pub(super) fn within_decoded(
        &mut self,
        base: usize,
        map: Vec<usize>,
        scan: impl FnOnce(&mut Self),
    ) {
        self.frames.push(Frame { base, map });
        scan(self);
        self.frames.pop();
    }

    /// The raw-source offset of an offset in the current scan coordinates.
    fn resolve(&self, mut at: usize) -> usize {
        for frame in self.frames.iter().rev() {
            at = frame.base + frame.map[at.min(frame.map.len() - 1)];
        }
        at
    }

    /// One-based line and byte column of a raw-source offset. Every mapped
    /// offset lands on a char boundary; an offset that does not is reported
    /// at the start of its character rather than panicking.
    fn line_column(&self, at: usize) -> (usize, usize, usize) {
        let mut at = at.min(self.source.len());
        while !self.source.is_char_boundary(at) {
            at -= 1;
        }
        let preceding_lines = self
            .newline_offsets
            .partition_point(|&newline| newline < at);
        let line_start = if preceding_lines == 0 {
            0
        } else {
            self.newline_offsets[preceding_lines - 1] + 1
        };
        (at, preceding_lines + 1, at - line_start + 1)
    }

    pub(super) fn note(&mut self, kind: NoteKind, at: usize, text: impl Into<String>) {
        let (byte_offset, line, byte_column) = self.line_column(self.resolve(at));
        self.notes.push(ExtractionNote {
            kind,
            byte_offset,
            line,
            byte_column,
            text: text.into(),
        });
    }
    #[allow(clippy::too_many_arguments)]
    pub(super) fn tokens(
        &mut self,
        raw: &str,
        at: usize,
        span: (usize, usize),
        position_kind: PositionKind,
        adjacent_right: bool,
        adjacent_left: bool,
        source_map: Option<&[usize]>,
    ) {
        let mut start = 0;
        let mut stack = Vec::new();
        let mut quote = None;
        let mut escape = false;
        for (i, ch) in raw.char_indices().chain(std::iter::once((raw.len(), ' '))) {
            if i < raw.len() {
                if escape {
                    escape = false;
                    continue;
                }
                if ch == '\\' {
                    escape = true;
                    continue;
                }
                if let Some(q) = quote {
                    if q == ch {
                        quote = None;
                    }
                    continue;
                }
                if matches!(ch, '\'' | '"') && !stack.is_empty() {
                    quote = Some(ch);
                    continue;
                }
                if matches!(ch, '[' | '(') {
                    stack.push(ch);
                    continue;
                }
                if matches!(ch, ']' | ')') {
                    stack.pop();
                    continue;
                }
            }
            if i == raw.len() || (ch.is_whitespace() && stack.is_empty()) {
                if start < i {
                    let token = &raw[start..i];
                    let left = adjacent_left && start == 0;
                    let right = adjacent_right && i == raw.len();
                    let complete = structural_split(token).is_ok()
                        && !token.chars().any(|c| c.is_control())
                        && !token.ends_with('-')
                        && token.chars().all(|c| {
                            c.is_ascii_alphanumeric()
                                || "-_:/.![]()\\'\",%#@+*=".contains(c)
                                || !c.is_ascii()
                        });
                    if left || (right && !complete) {
                        self.note(
                            NoteKind::DynamicConstruction,
                            at + source_map.map_or(start, |map| map[start]),
                            token,
                        );
                    } else if complete || position_kind == PositionKind::Class {
                        if !complete {
                            self.note(
                                NoteKind::MalformedClassCandidate,
                                at + source_map.map_or(start, |map| map[start]),
                                token,
                            );
                        }
                        let mapped_start =
                            self.resolve(at + source_map.map_or(start, |map| map[start]));
                        let mapped_end = self.resolve(at + source_map.map_or(i, |map| map[i]));
                        let (byte_offset, line, byte_column) = self.line_column(mapped_start);
                        let literal_start = self.resolve(span.0);
                        let literal_end = self.resolve(span.0 + span.1);
                        let occurrence = Occurrence {
                            byte_offset,
                            byte_length: mapped_end.saturating_sub(byte_offset),
                            line,
                            byte_column,
                            literal_byte_offset: literal_start,
                            literal_byte_length: literal_end.saturating_sub(literal_start),
                            position_kind,
                            adjacent_interpolation: left || right,
                        };
                        self.found
                            .entry(token.to_owned())
                            .or_default()
                            .push(occurrence);
                    }
                }
                start = i + ch.len_utf8();
            }
        }
    }
    fn finish(mut self) -> ExtractionResult {
        let candidates = self
            .found
            .into_iter()
            .map(|(text, mut occurrences)| {
                // A class-context pass re-records some literal spans; the
                // class position supersedes the low-confidence one.
                let classes: BTreeSet<(usize, usize)> = occurrences
                    .iter()
                    .filter(|occurrence| occurrence.position_kind == PositionKind::Class)
                    .map(|occurrence| (occurrence.byte_offset, occurrence.byte_length))
                    .collect();
                occurrences.retain(|occurrence| {
                    occurrence.position_kind == PositionKind::Class
                        || !classes.contains(&(occurrence.byte_offset, occurrence.byte_length))
                });
                occurrences.sort();
                occurrences.dedup();
                ExtractedCandidate { text, occurrences }
            })
            .collect();
        self.notes
            .sort_by(|a, b| a.byte_offset.cmp(&b.byte_offset).then(a.text.cmp(&b.text)));
        self.notes.dedup();
        ExtractionResult {
            candidates,
            notes: self.notes,
        }
    }
}
