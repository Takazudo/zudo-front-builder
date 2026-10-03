//! Pure, vocabulary-free extraction of complete source candidates.
mod markdown;
mod markup;
mod script;

use crate::structural_split;
use std::collections::BTreeMap;

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
    pub text: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ExtractionResult {
    pub candidates: Vec<ExtractedCandidate>,
    pub notes: Vec<ExtractionNote>,
}

pub fn extract_candidates(bytes: &[u8], kind: SourceKind) -> ExtractionResult {
    let Ok(source) = std::str::from_utf8(bytes) else {
        return ExtractionResult {
            notes: vec![ExtractionNote {
                kind: NoteKind::InvalidUtf8,
                byte_offset: 0,
                text: "source is not UTF-8".into(),
            }],
            ..Default::default()
        };
    };
    let mut collector = Collector {
        source,
        found: BTreeMap::new(),
        notes: Vec::new(),
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
    found: BTreeMap<String, Vec<Occurrence>>,
    notes: Vec<ExtractionNote>,
}

impl Collector<'_> {
    pub(super) fn note(&mut self, kind: NoteKind, at: usize, text: impl Into<String>) {
        self.notes.push(ExtractionNote {
            kind,
            byte_offset: at,
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
                        let mapped_start = source_map.map_or(start, |map| map[start]);
                        let mapped_end = source_map.map_or(i, |map| map[i]);
                        let offset = at + mapped_start;
                        let prefix = &self.source[..offset.min(self.source.len())];
                        let line = prefix.bytes().filter(|&b| b == b'\n').count() + 1;
                        let byte_column = prefix.rsplit('\n').next().map_or(1, |s| s.len() + 1);
                        let occurrence = Occurrence {
                            byte_offset: offset,
                            byte_length: mapped_end - mapped_start,
                            line,
                            byte_column,
                            literal_byte_offset: span.0,
                            literal_byte_length: span.1,
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
    pub(super) fn occurrence_counts(&self) -> BTreeMap<String, usize> {
        self.found
            .iter()
            .map(|(key, values)| (key.clone(), values.len()))
            .collect()
    }

    pub(super) fn remap_new_occurrences(
        &mut self,
        counts: &BTreeMap<String, usize>,
        base: usize,
        source_map: &[usize],
    ) {
        for (name, occurrences) in &mut self.found {
            for occurrence in occurrences.iter_mut().skip(*counts.get(name).unwrap_or(&0)) {
                let start = occurrence
                    .byte_offset
                    .saturating_sub(base)
                    .min(source_map.len() - 1);
                let end = (start + occurrence.byte_length).min(source_map.len() - 1);
                let literal_start = occurrence
                    .literal_byte_offset
                    .saturating_sub(base)
                    .min(source_map.len() - 1);
                let literal_end =
                    (literal_start + occurrence.literal_byte_length).min(source_map.len() - 1);
                occurrence.byte_offset = base + source_map[start];
                occurrence.byte_length = source_map[end] - source_map[start];
                occurrence.literal_byte_offset = base + source_map[literal_start];
                occurrence.literal_byte_length =
                    source_map[literal_end] - source_map[literal_start];
                let prefix = &self.source[..occurrence.byte_offset];
                occurrence.line = prefix.bytes().filter(|&b| b == b'\n').count() + 1;
                occurrence.byte_column = prefix.rsplit('\n').next().map_or(1, |s| s.len() + 1);
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
                let classes: Vec<(usize, usize)> = occurrences
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
