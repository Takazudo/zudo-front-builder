// Reserved-boundary guard for rawHtml payloads, shared by the server renderer
// and the client runtime so both accept exactly the same payloads.
//
// Rule: a payload is rejected when it can create a reserved boundary, decided
// by an HTML-tokenizer pass (WHATWG tokenization, data state), not a text regex:
//  - a start or end tag carrying an attribute whose ASCII-lowercased name is
//    `data-zfb-island` or `data-zfb-island-skip-ssr` (any value form, or none);
//  - a comment, including a bogus `<!…>`, `<?…>` or `</…>` comment, whose data
//    contains `zr:1:` (hydration matches `zr:1:` / `/zr:1:` comment prefixes);
//  - a payload that ends inside a tag, comment or `</`, which would absorb the
//    renderer's closing boundary marker.
// Marker-like text in character data or quoted attribute values is inert and
// renders as written. The tokenizer models the data state only, so payloads it
// cannot model take the textual rule, which rejects any `data-zfb-island` or
// `zr:1:` occurrence: `noscript`/`xmp`/`noembed`/`noframes`/`plaintext` hosts,
// and payloads naming a RAWTEXT/RCDATA/script/PLAINTEXT element (its end tag may
// also close a raw-text ancestor) or a CDATA section (foreign content).
// `script`/`style` hosts keep the documented marker and closing-tag rules.
// Script additionally checks whether HTML tokenization consumes its renderer
// closing tag; style is RAWTEXT and has no script escape states.

const reservedAttributes = new Set(["data-zfb-island", "data-zfb-island-skip-ssr"]);
const rawTextHosts = new Set(["noscript", "xmp", "noembed", "noframes", "plaintext"]);
const unmodeled =
  /<\/?(?:script|style|textarea|title|xmp|iframe|noembed|noframes|noscript|plaintext)|<!\[cdata\[/i;
const space = /[\t\n\f\r ]/;
const nameEnd = /[\t\n\f\r />=]/;
const unquotedEnd = /[\t\n\f\r >]/;

export function rawHtmlReserved(payload: string, tag: string): boolean {
  if (tag === "script" || tag === "style")
    return (
      /\x3c!--\/?zr:1:|data-zfb-island(?:-skip-ssr)?\s*=/.test(payload) ||
      (tag === "script" && scriptConsumesClosingTag(payload))
    );
  if (rawTextHosts.has(tag) || unmodeled.test(payload))
    return /data-zfb-island|zr:1:/i.test(payload);
  let i = 0;
  for (;;) {
    const lt = payload.indexOf("<", i);
    if (lt < 0) return false;
    const c = payload[lt + 1];
    let start: number;
    if (c === undefined) return false;
    if (/[a-z]/i.test(c) || (c === "/" && /[a-z]/i.test(payload[lt + 2] ?? ""))) {
      i = tagEnd(payload, c === "/" ? lt + 2 : lt + 1);
      if (i < 0) return true;
      continue;
    }
    if (c === "!" && payload.startsWith("--", lt + 2)) {
      start = lt + 4;
      if (payload[start] === ">" || payload.startsWith("->", start)) {
        i = start;
        continue;
      }
      const dashes = payload.indexOf("-->", start);
      const bang = payload.indexOf("--!>", start);
      const end = dashes < 0 || (bang >= 0 && bang < dashes) ? bang : dashes;
      if (end < 0 || payload.slice(start, end).includes("zr:1:")) return true;
      i = end;
      continue;
    }
    if (c === "/") {
      // `</` followed by the closing marker's `<` would open a bogus comment.
      if (lt + 2 >= payload.length) return true;
      if (payload[lt + 2] === ">") {
        i = lt + 3;
        continue;
      }
      start = lt + 2;
    } else if (c === "!" || c === "?") start = lt + 1;
    else {
      i = lt + 1;
      continue;
    }
    const end = payload.indexOf(">", start);
    if (end < 0 || payload.slice(start, end).includes("zr:1:")) return true;
    i = end;
  }
}

// Walk the WHATWG script-data escape states with the exact suffix emitted by
// the renderer. A `</script>` in double-escaped text only exits that state;
// it does not close the element. Incomplete names and `<!--` tails must be
// evaluated with the suffix attached, rather than classified by payload alone.
function scriptConsumesClosingTag(payload: string): boolean {
  // The HTML input stream normalizes CR and CRLF to LF before tokenization.
  const source = `${payload.replace(/\r\n?/g, "\n")}</script>`;
  type State =
    | "data"
    | "less"
    | "endOpen"
    | "endName"
    | "escapeStart"
    | "escapeStartDash"
    | "escaped"
    | "escapedDash"
    | "escapedDashDash"
    | "escapedLess"
    | "doubleStart"
    | "double"
    | "doubleDash"
    | "doubleDashDash"
    | "doubleLess"
    | "doubleEnd";
  let state: State = "data";
  let endFallback: "data" | "escaped" = "data";
  let buffer = "";
  for (let i = 0; i < source.length; i++) {
    const c = source[i]!;
    const alpha = /[a-zA-Z]/.test(c);
    const delimiter = c === "/" || c === ">" || /[\t\n\f ]/.test(c);
    switch (state) {
      case "data":
        if (c === "<") state = "less";
        break;
      case "less":
        if (c === "/") {
          endFallback = "data";
          state = "endOpen";
        } else if (c === "!") state = "escapeStart";
        else {
          state = "data";
          i--;
        }
        break;
      case "escapeStart":
        if (c === "-") state = "escapeStartDash";
        else {
          state = "data";
          i--;
        }
        break;
      case "escapeStartDash":
        if (c === "-") state = "escapedDashDash";
        else {
          state = "data";
          i--;
        }
        break;
      case "escaped":
      case "escapedDash":
      case "escapedDashDash":
        if (c === "<") state = "escapedLess";
        else if (state === "escapedDashDash" && c === ">") state = "data";
        else if (c === "-") state = state === "escaped" ? "escapedDash" : "escapedDashDash";
        else state = "escaped";
        break;
      case "escapedLess":
        if (c === "/") {
          endFallback = "escaped";
          state = "endOpen";
        } else if (alpha) {
          buffer = "";
          state = "doubleStart";
          i--;
        } else {
          state = "escaped";
          i--;
        }
        break;
      case "endOpen":
        if (alpha) {
          buffer = "";
          state = "endName";
          i--;
        } else {
          state = endFallback;
          i--;
        }
        break;
      case "endName":
        if (alpha) buffer = buffer.length < 7 ? buffer + c.toLowerCase() : "!";
        else if (delimiter && buffer === "script" && c === ">") return false;
        else {
          state = endFallback;
          i--;
        }
        break;
      case "doubleStart":
      case "doubleEnd":
        if (alpha) buffer = buffer.length < 7 ? buffer + c.toLowerCase() : "!";
        else if (delimiter)
          state =
            buffer === "script"
              ? state === "doubleStart"
                ? "double"
                : "escaped"
              : state === "doubleStart"
                ? "escaped"
                : "double";
        else {
          state = state === "doubleStart" ? "escaped" : "double";
          i--;
        }
        break;
      case "double":
      case "doubleDash":
      case "doubleDashDash":
        if (c === "<") state = "doubleLess";
        else if (state === "doubleDashDash" && c === ">") state = "data";
        else if (c === "-") state = state === "double" ? "doubleDash" : "doubleDashDash";
        else state = "double";
        break;
      case "doubleLess":
        if (c === "/") {
          buffer = "";
          state = "doubleEnd";
        } else {
          state = "double";
          i--;
        }
        break;
    }
  }
  return true;
}

// Returns the index just past the tag's `>`, or -1 when the tag is unterminated
// or carries a reserved attribute.
function tagEnd(s: string, j: number): number {
  const n = s.length;
  while (j < n && !space.test(s[j]!) && s[j] !== "/" && s[j] !== ">") j++;
  for (;;) {
    while (j < n && (space.test(s[j]!) || s[j] === "/")) j++;
    if (j >= n) return -1;
    if (s[j] === ">") return j + 1;
    const start = j++;
    while (j < n && !nameEnd.test(s[j]!)) j++;
    if (reservedAttributes.has(s.slice(start, j).toLowerCase())) return -1;
    while (j < n && space.test(s[j]!)) j++;
    if (s[j] !== "=") continue;
    j++;
    while (j < n && space.test(s[j]!)) j++;
    const quote = s[j];
    if (quote === '"' || quote === "'") {
      const close = s.indexOf(quote, j + 1);
      if (close < 0) return -1;
      j = close + 1;
    } else while (j < n && !unquotedEnd.test(s[j]!)) j++;
  }
}
