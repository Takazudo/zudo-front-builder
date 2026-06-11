//! Minimal source-map v3 decoder used to map a stack frame from the
//! bundled SSR worker back to the original `.tsx` location.
//!
//! Why a hand-rolled decoder rather than the `sourcemap` crate: the
//! we want to keep this crate's runtime-side dep surface tight. Decoding
//! the `mappings` field is ~80 lines of VLQ + state machine — small
//! enough to own.
//!
//! Scope: just enough to power [`decode_position`] —
//! `(generated_line, generated_col) → Option<DecodedFrame>`. The
//! decoder exposes the minimum the framed-error renderer
//! ([`crate::diagnostics`] in the `zfb` crate) needs:
//!
//! - The original source path the bundled position came from.
//! - The original 1-based line and column.
//! - The original file's content when the map embeds `sourcesContent`,
//!   so the renderer can frame a snippet without going back to disk.
//!
//! Higher-level concerns (multi-file mapping, name lookups, index
//! source maps) are intentionally NOT supported. We reach for the
//! `sourcemap` crate when the use case grows past stack-frame
//! attribution.

use serde_json::Value;

/// A position decoded from a source map.
///
/// Lines and columns are 1-based to match what editors and humans use.
/// The internal `mappings` grammar is 0-based and the conversion
/// happens in [`decode_position`] before the value is exposed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedFrame {
    /// Original source file (typically a path, exactly as it appears in
    /// the map's `sources` array).
    pub file: String,
    /// 1-based line in the original file.
    pub line: usize,
    /// 1-based column in the original file.
    pub col: usize,
    /// Original source text, when the map embeds `sourcesContent`.
    pub source_content: Option<String>,
}

/// Decode a generated `(line, col)` (both 1-based) against the supplied
/// source-map JSON and return the matching original frame, or `None` if
/// the map does not cover the requested position.
///
/// The decoder picks the **closest preceding** mapping on the requested
/// line — same convention as Chrome's stack-trace mapping. When no
/// mapping exists on or before the line, returns `None`.
pub fn decode_position(map_json: &str, line_1: usize, col_1: usize) -> Option<DecodedFrame> {
    let map: Value = serde_json::from_str(map_json).ok()?;
    let sources: Vec<String> = map
        .get("sources")?
        .as_array()?
        .iter()
        .map(|v| v.as_str().unwrap_or("").to_string())
        .collect();
    let sources_content: Option<Vec<Option<String>>> = map.get("sourcesContent").and_then(|v| {
        v.as_array().map(|arr| {
            arr.iter()
                .map(|item| item.as_str().map(|s| s.to_string()))
                .collect()
        })
    });
    let mappings = map.get("mappings")?.as_str()?;

    // VLQ state. Source-map v3 stores deltas — every line resets the
    // generated column delta but NOT the source/line/col/name indexes.
    let mut state = State::default();
    let mut best_for_line: Option<MappingPoint> = None;
    let want_line0 = line_1.saturating_sub(1);
    let want_col0: i64 = col_1.saturating_sub(1) as i64;

    for (line_idx, line) in mappings.split(';').enumerate() {
        // Each line resets generated column.
        state.gen_col = 0;
        // Track best mapping on the requested line.
        if line_idx < want_line0 {
            // Replay this line so deltas advance, but don't record.
            replay_line(line, &mut state);
            continue;
        }
        if line_idx > want_line0 {
            break;
        }

        // We're on the requested generated line.
        for segment in line.split(',') {
            if segment.is_empty() {
                continue;
            }
            let mut bytes = segment.as_bytes();
            let gen_col_delta = decode_vlq(&mut bytes)?;
            state.gen_col = state.gen_col.checked_add(gen_col_delta)?;
            if !bytes.is_empty() {
                let src_delta = decode_vlq(&mut bytes)?;
                let line_delta = decode_vlq(&mut bytes)?;
                let col_delta = decode_vlq(&mut bytes)?;
                state.src_idx = state.src_idx.checked_add(src_delta)?;
                state.src_line = state.src_line.checked_add(line_delta)?;
                state.src_col = state.src_col.checked_add(col_delta)?;
                if !bytes.is_empty() {
                    // name index — consume but ignore.
                    let _name_delta = decode_vlq(&mut bytes)?;
                }
            }
            // Closest preceding mapping wins.
            if state.gen_col <= want_col0 {
                let candidate = MappingPoint {
                    src_idx: state.src_idx,
                    src_line: state.src_line,
                    src_col: state.src_col,
                };
                best_for_line = Some(candidate);
            } else {
                // Further segments on this line are past the requested
                // column — break early since segments are sorted.
                break;
            }
        }
        break;
    }

    let best = best_for_line?;
    if best.src_idx >= sources.len() as i64 || best.src_idx < 0 {
        return None;
    }
    // `src_line` and `src_col` are signed accumulators of VLQ deltas;
    // a malformed map can leave them negative even when `src_idx` is
    // valid. Reject those instead of silently wrapping to garbage usize.
    if best.src_line < 0 || best.src_col < 0 {
        return None;
    }
    let src_idx = best.src_idx as usize;
    let file = sources.get(src_idx)?.clone();
    let source_content = sources_content
        .as_ref()
        .and_then(|sc| sc.get(src_idx).cloned().flatten());

    Some(DecodedFrame {
        file,
        line: (best.src_line as usize) + 1,
        col: (best.src_col as usize) + 1,
        source_content,
    })
}

/// State carried across the source-map decoding state machine. All
/// fields are signed because VLQ deltas are signed; the public
/// position is always non-negative.
#[derive(Default)]
struct State {
    gen_col: i64,
    src_idx: i64,
    src_line: i64,
    src_col: i64,
}

#[derive(Clone, Copy)]
struct MappingPoint {
    src_idx: i64,
    src_line: i64,
    src_col: i64,
}

/// Replay a `mappings` line for its side effects only — used to skip
/// ahead to the requested generated line without recording anything.
fn replay_line(line: &str, state: &mut State) {
    for segment in line.split(',') {
        if segment.is_empty() {
            continue;
        }
        let mut bytes = segment.as_bytes();
        let Some(delta) = decode_vlq(&mut bytes) else {
            return;
        };
        state.gen_col = state.gen_col.saturating_add(delta);
        if !bytes.is_empty() {
            let Some(d1) = decode_vlq(&mut bytes) else {
                return;
            };
            let Some(d2) = decode_vlq(&mut bytes) else {
                return;
            };
            let Some(d3) = decode_vlq(&mut bytes) else {
                return;
            };
            state.src_idx = state.src_idx.saturating_add(d1);
            state.src_line = state.src_line.saturating_add(d2);
            state.src_col = state.src_col.saturating_add(d3);
            if !bytes.is_empty() {
                let _ = decode_vlq(&mut bytes);
            }
        }
    }
}

/// Decode a single VLQ value at the head of `bytes`, advancing the
/// slice past the consumed characters. Returns `None` on malformed
/// input.
fn decode_vlq(bytes: &mut &[u8]) -> Option<i64> {
    const SHIFT_LIMIT: u32 = 63;
    let mut result: i64 = 0;
    let mut shift: u32 = 0;
    loop {
        if bytes.is_empty() {
            return None;
        }
        let byte = bytes[0];
        *bytes = &bytes[1..];
        let digit = b64_decode(byte)?;
        let cont = (digit & 32) != 0;
        let value = (digit & 31) as i64;
        if shift > SHIFT_LIMIT {
            return None;
        }
        result |= value << shift;
        shift += 5;
        if !cont {
            break;
        }
    }
    let negative = (result & 1) != 0;
    let magnitude = result >> 1;
    Some(if negative { -magnitude } else { magnitude })
}

/// Decode a single base64-VLQ digit. Returns `None` for invalid input.
fn b64_decode(c: u8) -> Option<u8> {
    match c {
        b'A'..=b'Z' => Some(c - b'A'),
        b'a'..=b'z' => Some(c - b'a' + 26),
        b'0'..=b'9' => Some(c - b'0' + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Source-map v3 fixture: bundle line 1 col 0 maps to source 0
    /// line 0 col 0, line 1 col 6 maps to source 0 line 0 col 4.
    /// `AAAA` is the canonical "no delta" segment.
    fn fixture_simple() -> &'static str {
        // Bundle:
        //   var x = 1;
        //   throw new Error("boom");
        // Source `src/page.tsx`:
        //   const x = 1;
        //   throw new Error("boom");
        //
        // Mappings: line 1 starts at gen col 0 → src line 0 col 0;
        //           line 2 starts at gen col 0 → src line 1 col 0.
        // VLQ for (0,0,0,0) is "AAAA"; (0,0,+1,0) is "AACA".
        // ";" separates lines.
        r#"{
            "version": 3,
            "sources": ["src/page.tsx"],
            "sourcesContent": ["const x = 1;\nthrow new Error(\"boom\");\n"],
            "mappings": "AAAA;AACA"
        }"#
    }

    #[test]
    fn decode_position_returns_first_segment_on_requested_line() {
        let frame = decode_position(fixture_simple(), 2, 1).expect("decode");
        assert_eq!(frame.file, "src/page.tsx");
        assert_eq!(frame.line, 2);
        assert_eq!(frame.col, 1);
        assert!(frame.source_content.is_some());
    }

    #[test]
    fn decode_position_first_line_decodes() {
        let frame = decode_position(fixture_simple(), 1, 1).expect("decode");
        assert_eq!(frame.line, 1);
        assert_eq!(frame.col, 1);
    }

    #[test]
    fn decode_position_returns_none_when_line_uncovered() {
        // Line 99 is past the end of the mappings.
        assert!(decode_position(fixture_simple(), 99, 1).is_none());
    }

    #[test]
    fn decode_position_returns_none_for_invalid_json() {
        assert!(decode_position("not json", 1, 1).is_none());
    }

    #[test]
    fn decode_position_handles_multiple_segments_on_a_line() {
        // Two segments on line 1: gen col 0 → src col 0, gen col 4 → src col 6.
        // Deltas: ("A","A","A","A"), then ("I","A","A","M") where I=4 (gen col delta),
        // M=6 (src col delta), 0 src/line deltas.
        let map = r#"{
            "version": 3,
            "sources": ["src/page.tsx"],
            "mappings": "AAAA,IAAM"
        }"#;
        let mid = decode_position(map, 1, 5).expect("decode");
        assert_eq!(mid.col, 7);
        let early = decode_position(map, 1, 1).expect("decode");
        assert_eq!(early.col, 1);
    }
}
