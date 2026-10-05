import { readFile, writeFile } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "../..");
const sourcePath = resolve(root, "packages/zfb/src/zudo-react/vocabulary.ts");
const renderPath = resolve(root, "packages/zfb/src/zudo-react/render-html.ts");
const hydratePath = resolve(root, "packages/zfb/src/zudo-react/hydrate.ts");
const references = [
  {
    path: resolve(root, "docs/src/content/docs/zudo-react/vocabulary.mdx"),
    locale: "en",
  },
  {
    path: resolve(root, "docs/src/content/docs-ja/zudo-react/vocabulary.mdx"),
    locale: "ja",
  },
];

function fail(message) {
  throw new Error(`renderer vocabulary generator: ${message}`);
}

function exactOne(match, label) {
  if (match.length !== 1) fail(`expected one ${label} source declaration, found ${match.length}`);
  return match[0];
}

function stripComments(source) {
  let output = "";
  let state = "code";
  let quote = "";
  for (let index = 0; index < source.length; index++) {
    const character = source[index];
    const next = source[index + 1];
    if (state === "line-comment") {
      if (character === "\n") {
        output += character;
        state = "code";
      } else output += " ";
      continue;
    }
    if (state === "block-comment") {
      if (character === "*" && next === "/") {
        output += "  ";
        index++;
        state = "code";
      } else output += character === "\n" ? "\n" : " ";
      continue;
    }
    if (state === "quoted") {
      output += character;
      if (character === "\\" && next !== undefined) {
        output += next;
        index++;
      } else if (character === quote) state = "code";
      continue;
    }
    if ((character === "'" || character === '"' || character === "`") && state === "code") {
      output += character;
      quote = character;
      state = "quoted";
      continue;
    }
    if (character === "/" && next === "/") {
      output += "  ";
      index++;
      state = "line-comment";
    } else if (character === "/" && next === "*") {
      output += "  ";
      index++;
      state = "block-comment";
    } else output += character;
  }
  return output;
}

function parseWordSet(source, name) {
  const pattern = new RegExp(
    `^(?:export\\s+)?const\\s+${name}\\s*=\\s*words\\(\\s*"([^"\\\\]*)"\\s*,?\\s*\\);`,
    "gm",
  );
  const matches = [...source.matchAll(pattern)];
  const value = exactOne(matches, `words set ${name}`)[1];
  const words = value.split(" ");
  if (words.some((word) => !/^[A-Za-z0-9:._-]+$/.test(word)))
    fail(`${name} contains an unsupported word literal`);
  if (new Set(words).size !== words.length) fail(`${name} contains duplicate names`);
  return words;
}

function compact(source) {
  return stripComments(source).replace(/\s+/g, "");
}

function requireFragments(source, label, fragments) {
  const normalized = compact(source);
  for (const fragment of fragments) {
    if (!normalized.includes(compact(fragment)))
      fail(`${label} policy changed; review the reference generator`);
  }
}

function parseAliases(source) {
  const blocks = [
    ...source.matchAll(
      /^const propAliases: Readonly<Record<string, PropAlias>> = \{\n([\s\S]*?)\n\};/gm,
    ),
  ];
  const body = exactOne(blocks, "private propAliases object")[1];
  const aliases = [];
  const pattern =
    /^  ([A-Za-z][A-Za-z0-9]*): \{ spelling: "([A-Za-z0-9:._-]+)"(?:, namespace: "(html|svg)")? \},$/;
  for (const line of body.split("\n")) {
    if (!line.trim()) continue;
    const match = line.match(pattern);
    if (!match) fail(`unsupported propAliases entry shape: ${line.trim()}`);
    aliases.push({ name: match[1], spelling: match[2], namespace: match[3] });
  }
  if (aliases.length === 0) fail("propAliases must contain at least one entry");
  if (new Set(aliases.map(({ name }) => name)).size !== aliases.length)
    fail("propAliases contains duplicate spellings");
  return aliases;
}

function validateVocabularySemantics(source) {
  requireFragments(source, "attribute value", [
    "if (value == null) return undefined;",
    'if (custom) return typeof value === "string" ? undefined : "requires a string";',
    'if (name === "style") return undefined;',
    'if (booleanAttrs.has(name)) return typeof value === "boolean" ? undefined : "requires a boolean";',
    'if (overloadedBooleanAttrs.has(name)) return typeof value === "string" || typeof value === "boolean" ? undefined : "requires a string or boolean";',
    'if (enumeratedBooleanAttrs.has(name)) return typeof value === "string" || typeof value === "boolean" ? undefined : "requires a string or boolean";',
    'if (numericAttrs.has(name)) return typeof value === "string" || (typeof value === "number" && Number.isFinite(value)) ? undefined : "requires a string or finite number";',
    'if (stringAttrs.has(name)) return typeof value === "string" ? undefined : "requires a string";',
    'if (/^on[a-z]/.test(name)) return typeof value === "string" ? undefined : "requires a string";',
    'typeof value === "boolean" && /^(data-|aria-)/.test(name)',
  ]);
  requireFragments(source, "alias suggestion", [
    "if (/^on[A-Z]/.test(name) || legacyDialect.has(name)) return true;",
    "if (custom) return false;",
    "alias !== undefined && (!alias.namespace || alias.namespace === namespace) && allowedAliasTarget(alias.spelling, namespace)",
    "if (/^on[A-Z]/.test(name)) return eventSuggestion(name);",
    "if (custom && !legacyDialect.has(name)) return undefined;",
    "return allowedAliasTarget(alias.spelling, namespace) ? alias.spelling : undefined;",
    'event === "DoubleClick" ? "dblclick" : event.toLowerCase()',
  ]);
  requireFragments(source, "namespace alias target", [
    'if (spelling === "rawHtml") return namespace === "html";',
    'commonAttrs.has(spelling) || (namespace === "svg" ? svgAttrs.has(spelling) : htmlAttrs.has(spelling))',
  ]);
  requireFragments(source, "form runtime props", [
    'export const formProps = words("modelValue modelChecked defaultValue defaultChecked");',
  ]);
}

export function validateRendererPolicies(render, hydrate) {
  const tagFragments = [
    "const custom = /^[a-z][a-z0-9]*(?:-[a-z0-9]+)+$/.test(tag);",
    'const elementNamespace = tag === "svg" || namespace === "svg" ? "svg" : "html";',
    'const childNamespace = tag === "foreignObject" ? "html" : elementNamespace;',
    'elementNamespace === "svg" ? !svgTags.has(tag) : !custom && !htmlTags.has(tag)',
  ];
  requireFragments(render, "server tag/namespace", tagFragments);
  requireFragments(hydrate, "client tag/namespace", [
    "const custom = /^[a-z][a-z0-9]*(?:-[a-z0-9]+)+$/.test(tag);",
    'const ownNamespace = tag === "svg" || namespace === SVG ? SVG : HTML;',
    'const childNamespace = tag === "foreignObject" ? HTML : ownNamespace;',
    "ownNamespace === SVG ? !svgTags.has(tag) : !custom && !htmlTags.has(tag)",
  ]);
  const attributeFragments = [
    "/^[A-Za-z_:][A-Za-z0-9_:.-]*$/.test(name)",
    "!custom && !commonAttrs.has(name)",
    'namespace === "svg" ? svgAttrs.has(name) : htmlAttrs.has(name)',
    "!/^data-[\\w.-]+$/.test(name)",
    "!/^aria-[\\w.-]+$/.test(name)",
    "!/^on[a-z]+$/.test(name)",
    'name.startsWith("on:")',
    "/^on:[A-Za-z][A-Za-z0-9_-]*(?::capture)?$/.test(name)",
    'const reserved = words("key ref children rawHtml")',
    'if (raw !== undefined && hasChildren) fail("ZR_RAW_HTML", context, `${tag} has children and rawHtml`);',
    'raw !== undefined && (elementNamespace === "svg"',
  ];
  requireFragments(render, "server attribute/listener/reserved prop", attributeFragments);
  requireFragments(hydrate, "client attribute/listener", [
    "/^[A-Za-z_:][A-Za-z0-9_:.-]*$/.test(name)",
    "!custom && !commonAttrs.has(name)",
    "ownNamespace === SVG ? svgAttrs.has(name) : htmlAttrs.has(name)",
    "!/^data-[\\w.-]+$/.test(name)",
    "!/^aria-[\\w.-]+$/.test(name)",
    "!/^on[a-z]+$/.test(name)",
    'name.startsWith("on:")',
  ]);
}

export function extractVocabulary(source) {
  const code = stripComments(source);
  validateVocabularySemantics(code);
  const sets = {
    htmlTags: parseWordSet(code, "htmlTags"),
    svgTags: parseWordSet(code, "svgTags"),
    commonAttrs: parseWordSet(code, "commonAttrs"),
    htmlAttrs: parseWordSet(code, "htmlAttrs"),
    svgAttrs: parseWordSet(code, "svgAttrs"),
    booleanAttrs: parseWordSet(code, "booleanAttrs"),
    enumeratedBooleanAttrs: parseWordSet(code, "enumeratedBooleanAttrs"),
    overloadedBooleanAttrs: parseWordSet(code, "overloadedBooleanAttrs"),
    numericAttrs: parseWordSet(code, "numericAttrs"),
    stringAttrs: parseWordSet(code, "stringAttrs"),
    legacyDialect: parseWordSet(code, "legacyDialect"),
  };
  const aliases = parseAliases(code);
  for (const alias of aliases) {
    if (aliasTargetNamespaces(alias, sets).length === 0)
      fail(`${alias.name} points to an unsupported runtime spelling ${alias.spelling}`);
  }
  return { ...sets, aliases };
}

function aliasTargetNamespaces(alias, vocabulary) {
  return ["html", "svg"].filter((namespace) => {
    if (alias.namespace && alias.namespace !== namespace) return false;
    if (alias.spelling === "rawHtml") return namespace === "html";
    return (
      vocabulary.commonAttrs.includes(alias.spelling) ||
      (namespace === "svg"
        ? vocabulary.svgAttrs.includes(alias.spelling)
        : vocabulary.htmlAttrs.includes(alias.spelling))
    );
  });
}

function sorted(values) {
  return [...values].sort((left, right) => (left < right ? -1 : left > right ? 1 : 0));
}

function nameCell(values, perLine = 8) {
  const names = sorted(values).map((name) => `\`${name}\``);
  const lines = [];
  for (let index = 0; index < names.length; index += perLine)
    lines.push(names.slice(index, index + perLine).join(", "));
  return lines.join("<br />");
}

function tagTable(values, locale, columns = 4) {
  const names = [...values];
  const tagLabel = locale === "en" ? "Tag" : "タグ";
  const header = `| ${Array.from({ length: columns }, () => tagLabel).join(" | ")} |`;
  const divider = `| ${Array.from({ length: columns }, () => "---").join(" | ")} |`;
  const rows = [];
  for (let index = 0; index < names.length; index += columns) {
    rows.push(
      `| ${Array.from({ length: columns }, (_, column) => names[index + column] ?? "")
        .map((name) => (name ? `\`${name}\`` : ""))
        .join(" | ")} |`,
    );
  }
  return [header, divider, ...rows].join("\n");
}

function attributeRows(vocabulary, locale) {
  const labels =
    locale === "en"
      ? ["Common on HTML and SVG", "HTML namespace", "SVG namespace"]
      : ["HTML と SVG の共通", "HTML 名前空間", "SVG 名前空間"];
  const categories = [
    [labels[0], vocabulary.commonAttrs],
    [labels[1], vocabulary.htmlAttrs],
    [labels[2], vocabulary.svgAttrs],
  ];
  return categories
    .map(([label, names]) => `| ${label} | ${names.length} | ${nameCell(names)} |`)
    .join("\n");
}

function valueRows(vocabulary, locale) {
  const japanese = locale === "ja";
  const rows = [
    [
      japanese ? "真偽値（有無）" : "Boolean presence",
      vocabulary.booleanAttrs,
      japanese
        ? "真偽値のみです。true は値なしの属性として出力し、false は省略します。"
        : "Boolean only: true emits a bare attribute; false is omitted.",
    ],
    [
      japanese ? "列挙型の真偽値" : "Enumerated boolean",
      vocabulary.enumeratedBooleanAttrs,
      japanese
        ? "文字列または真偽値です。真偽値は `true` または `false` の文字列として出力します。"
        : "String or boolean; booleans serialize as the strings `true` or `false`.",
    ],
    [
      japanese ? "オーバーロード真偽値" : "Overloaded boolean",
      vocabulary.overloadedBooleanAttrs,
      japanese
        ? "文字列または真偽値です。true は空の値を出力し、false は省略します。"
        : "String or boolean; true emits an empty value, false is omitted.",
    ],
    [
      japanese ? "数値" : "Numeric",
      vocabulary.numericAttrs,
      japanese ? "文字列または有限数です。" : "String or finite number.",
    ],
    [
      japanese ? "文字列のみ" : "String only",
      vocabulary.stringAttrs,
      japanese
        ? "文字列のみです。数値と真偽値は拒否されます。"
        : "A string; numbers and booleans are rejected.",
    ],
  ];
  const headers = japanese
    ? ["分類", "件数", "名前", "値の扱い"]
    : ["Category", "Count", "Names", "Value handling"];
  return [
    `| ${headers.join(" | ")} |`,
    "| --- | ---: | --- | --- |",
    ...rows.map(
      ([label, names, handling]) =>
        `| ${label} | ${names.length} | ${nameCell(names, 6)} | ${handling} |`,
    ),
  ].join("\n");
}

function aliasRows(vocabulary, locale) {
  return vocabulary.aliases
    .map((alias) => {
      const namespaces = aliasTargetNamespaces(alias, vocabulary);
      const customBehavior = vocabulary.legacyDialect.includes(alias.name)
        ? locale === "en"
          ? "Also rejected on custom elements"
          : "カスタム要素でも拒否"
        : locale === "en"
          ? "Ordinary string attribute"
          : "通常の文字列属性";
      return `| \`${alias.name}\` | \`${alias.spelling}\` | ${namespaces.map((namespace) => namespace.toUpperCase()).join(", ") || "—"} | ${customBehavior} |`;
    })
    .join("\n");
}

function pageEnglish(v) {
  return `---
title: Supported HTML and SVG vocabulary
description: The current tag names, attribute names, value categories, and rejected React spellings for zudo-react.
sidebar_position: 15
---

## What the renderer accepts

The renderer uses the finite names below for standard HTML and SVG elements. Attribute acceptance is **per name across the namespace, not per element**: for example, \`loading\` is accepted on \`iframe\`, \`img\`, \`video\`, and \`div\`. The HTML and SVG attribute sets are separate; common attributes and the listed open-ended families apply in both namespaces. These counts and tables come from the current \`vocabulary.ts\` source, including \`rb\`.

The \`svg\` tag starts the SVG namespace. A \`foreignObject\` switches its children to HTML; a nested \`svg\` switches back to SVG. An unsupported tag in the active namespace reports \`ZR_TAG\`.

## Element names

### HTML tags (${v.htmlTags.length})

${tagTable(v.htmlTags, "en")}

An HTML custom element name matching \`^[a-z][a-z0-9]*(?:-[a-z0-9]+)+$\` (for example, \`x-card\`) is also accepted. Custom element attributes follow their separate string-only rule below. Custom elements also support \`on:<event>\` listeners and reserved runtime props.

### SVG tags (${v.svgTags.length})

${tagTable(v.svgTags, "en")}

## Attribute names

The lists below are flat name sets, not per-tag HTML tables. A name in the HTML set is accepted on any supported HTML tag; a name in the SVG set is accepted on any supported SVG tag. Some names occur in more than one set.

| Name group | Count | Names |
| --- | ---: | --- |
${attributeRows(v, "en")}

These families are accepted on standard HTML and SVG elements:

| Family | Accepted names | Value rule |
| --- | --- | --- |
| Data attributes | \`data-[\\w.-]+\` | Strings and finite numbers; booleans serialize as \`"true"\` or \`"false"\`. |
| ARIA attributes | \`aria-[\\w.-]+\` | Strings and finite numbers; booleans serialize as \`"true"\` or \`"false"\`. |
| Inline event attributes | \`on[a-z]+\` (for example, \`onclick\`, \`onload\`) | Strings, including reactive string values; null and undefined omit the attribute. The renderer escapes and updates the browser's inline-handler attribute. The browser may execute it when the event fires, so use trusted strings. Function values are rejected. |
| Native event listeners | \`on:<event>\` or \`on:<event>:capture\` | A function prop with a case-sensitive event name matching \`[A-Za-z][A-Za-z0-9_-]*\`; it installs a native listener and emits no HTML attribute. |

The attribute name must match \`^[A-Za-z_:][A-Za-z0-9_:.-]*$\`. Null and undefined values are omitted. For standard attributes not listed in a special value category, strings and finite numbers are accepted.

## Attribute value categories

The following value rules apply to the names shown whenever those names are accepted in the active namespace. The sets can overlap with the common, HTML, and SVG name groups.

${valueRows(v, "en")}

\`style\` is common to both namespaces but has a dedicated handler: CSS strings pass through, while plain object styles are validated. It is not a string-only standard attribute. Custom element attributes take precedence over these standard categories and must be strings; reactive string values are supported, and null and undefined are omitted.

## Rejected React spellings and suggestions (${v.aliases.length})

These are **diagnostic suggestions**, not accepted aliases: the authored React spelling fails and is neither rewritten nor emitted. The suggestion is available only when its target spelling is valid in the current namespace. \`dangerouslySetInnerHTML\` suggests the special renderer prop \`rawHtml\`, which is not an HTML attribute. \`on[A-Z]\` event props are handled separately: for example, \`onClick\` suggests \`on:click\`, and \`onDoubleClick\` suggests \`on:dblclick\`.

| Rejected spelling | Suggested spelling | Namespace where ZR_PROP_DIALECT suggests the target | Custom element behavior |
| --- | --- | --- | --- |
${aliasRows(v, "en")}

Ordinary aliases are not checked specially on custom elements, which accept otherwise valid attribute names as strings. The legacy spellings listed here remain rejected on custom elements, as do React-style \`on[A-Z]\` props. A legacy spelling is also rejected in other namespaces, but has no suggestion there when its target is invalid. Other mappings produce \`ZR_PROP_DIALECT\` only in the listed namespace; in other namespaces they fail with \`ZR_ATTRIBUTE\` as unsupported attribute names.

## Runtime props are not attributes

- \`key\` belongs to the description, and \`children\` supplies child content; neither is emitted as an attribute.
- \`ref\` is a client runtime object prop. Native listeners use \`on:<event>\` as described above.
- \`modelValue\`, \`modelChecked\`, \`defaultValue\`, and \`defaultChecked\` are form runtime props, not HTML attributes. See the [forms guide](./forms.mdx).
- \`rawHtml\` supplies trusted renderer-owned markup, is mutually exclusive with children, and is unsupported in SVG. See [Components and JSX](./components-and-jsx.mdx#trusted-raw-html).
- \`style\` uses a dedicated handler: CSS strings pass through, while plain object styles are validated.

## Read a vocabulary error

An unsupported attribute name or value reports \`ZR_ATTRIBUTE\`; an unsupported tag reports \`ZR_TAG\`. For static rendering, the message includes the tag or attribute, element path, and phase:

~~~text
ZR_TAG: madeuptag at root in static render
ZR_ATTRIBUTE: input.enterkeyhint at root in static render
~~~

See [server rendering](./server-rendering.mdx#read-a-render-failure) for the full diagnostic format.
`;
}

function pageJapanese(v) {
  return `---
title: 対応する HTML と SVG の語彙
description: zudo-react が受け付ける現在のタグ名、属性名、値の分類、React 形式の診断候補です。
sidebar_position: 15
---

## レンダラーが受け付ける名前

標準の HTML 要素と SVG 要素では、以下の有限な名前を使います。属性の受け入れ判定は**要素ごとではなく、名前ごとに名前空間内で行います**。たとえば \`loading\` は \`iframe\`、\`img\`、\`video\`、\`div\` のすべてで使えます。HTML と SVG の属性セットは別で、共通属性と後述のパターン属性は両方の名前空間で使えます。表の件数と名前は、\`rb\` を含む現在の \`vocabulary.ts\` から生成しています。

\`svg\` タグから SVG 名前空間が始まります。\`foreignObject\` の子は HTML 名前空間になり、その内側にある \`svg\` で SVG に戻ります。現在の名前空間で未対応のタグは \`ZR_TAG\` を報告します。

## 要素名

### HTML タグ（${v.htmlTags.length}）

${tagTable(v.htmlTags, "ja")}

\`^[a-z][a-z0-9]*(?:-[a-z0-9]+)+$\` に一致する HTML カスタム要素名（例: \`x-card\`）も使えます。カスタム要素の属性には、後述の文字列のみの規則を適用します。カスタム要素は \`on:<event>\` リスナーと予約済みランタイム prop にも対応します。

### SVG タグ（${v.svgTags.length}）

${tagTable(v.svgTags, "ja")}

## 属性名

以下はタグごとの HTML 属性表ではなく、名前の集合です。HTML の名前は対応するすべての HTML タグで、SVG の名前は対応するすべての SVG タグで使えます。複数の集合に含まれる名前もあります。

| 名前の集合 | 件数 | 名前 |
| --- | ---: | --- |
${attributeRows(v, "ja")}

次の名前パターンは標準の HTML 要素と SVG 要素で使えます。

| 種類 | 受け付ける名前 | 値の規則 |
| --- | --- | --- |
| data 属性 | \`data-[\\w.-]+\` | 文字列と有限数。真偽値は \`"true"\` または \`"false"\` に変換します。 |
| ARIA 属性 | \`aria-[\\w.-]+\` | 文字列と有限数。真偽値は \`"true"\` または \`"false"\` に変換します。 |
| インラインイベント属性 | \`on[a-z]+\`（例: \`onclick\`、\`onload\`） | リアクティブな文字列を含む文字列値です。null と undefined では属性を省略します。レンダラーはブラウザーのインラインハンドラー属性としてエスケープして出力し、更新します。イベント時にブラウザーが実行することがあるため、信頼できる文字列だけを使います。関数値は拒否されます。 |
| ネイティブイベントリスナー | \`on:<event>\` または \`on:<event>:capture\` | \`[A-Za-z][A-Za-z0-9_-]*\` に一致する大文字小文字を区別するイベント名の関数 prop です。ネイティブリスナーを登録し、HTML 属性は出力しません。 |

属性名は \`^[A-Za-z_:][A-Za-z0-9_:.-]*$\` に一致する必要があります。null と undefined の値は省略します。値の特別な分類にない標準属性では、文字列と有限数を使えます。

## 属性値の分類

以下の値の規則は、対応する名前空間でその名前が使える場合に適用します。名前は共通、HTML、SVG の各集合と重なることがあります。

${valueRows(v, "ja")}

\`style\` は両方の名前空間で使える共通 prop ですが、専用のハンドラーで処理します。CSS 文字列はそのまま通し、プレーンオブジェクト形式は検証します。標準要素では文字列のみの属性ではありません。カスタム要素では標準属性の分類よりカスタム要素の規則が優先され、属性値は文字列のみです。リアクティブな文字列値も使え、null と undefined は省略します。

## 拒否される React 形式の綴りと候補（${v.aliases.length}）

以下は**診断に表示する候補**であり、受け入れられる別名ではありません。React 形式の綴りはエラーになり、書き換えもそのまま出力もされません。候補を表示するのは、現在の名前空間で対応する綴りが有効な場合だけです。\`dangerouslySetInnerHTML\` の候補 \`rawHtml\` は専用のレンダラー prop であり、HTML 属性ではありません。\`on[A-Z]\` 形式のイベント prop は別に扱います。たとえば \`onClick\` には \`on:click\`、\`onDoubleClick\` には \`on:dblclick\` が候補として表示されます。

| 拒否される綴り | 提示される綴り | \`ZR_PROP_DIALECT\` の候補が出る名前空間 | カスタム要素での扱い |
| --- | --- | --- | --- |
${aliasRows(v, "ja")}

通常の別名はカスタム要素では特別扱いされません。カスタム要素は構文に合う任意の属性名を文字列値で受け付けます。この表の旧式の綴りと \`on[A-Z]\` 形式はカスタム要素でも拒否されます。旧式の綴りは他の名前空間でも拒否されますが、候補の綴りが無効なら候補は表示されません。その他の対応表記は、表に記載した名前空間でだけ \`ZR_PROP_DIALECT\` を報告し、それ以外では未対応属性名として \`ZR_ATTRIBUTE\` を報告します。

## 属性ではないランタイム prop

- \`key\` は記述に属し、\`children\` は子の内容を指定します。どちらも属性として出力されません。
- \`ref\` はクライアントランタイム用のオブジェクト prop です。ネイティブリスナーには前述の \`on:<event>\` を使います。
- \`modelValue\`、\`modelChecked\`、\`defaultValue\`、\`defaultChecked\` は HTML 属性ではなく、フォーム用のランタイム prop です。[フォームガイド](./forms.mdx)を参照してください。
- \`rawHtml\` は信頼済みのマークアップをレンダラーに渡します。children とは併用できず、SVG では使えません。[コンポーネントと JSX](./components-and-jsx.mdx)を参照してください。
- \`style\` は専用のハンドラーで処理します。CSS 文字列はそのまま通し、プレーンオブジェクト形式は検証します。

## 語彙エラーを読む

未対応の属性名または値は \`ZR_ATTRIBUTE\`、未対応のタグは \`ZR_TAG\` を報告します。静的レンダリングでは、メッセージにタグまたは属性、要素パス、処理段階が含まれます。

~~~text
ZR_TAG: madeuptag at root in static render
ZR_ATTRIBUTE: input.enterkeyhint at root in static render
~~~

詳しい診断形式は[サーバーレンダリング](./server-rendering.mdx)を参照してください。
`;
}

export function generateReferencePages(source) {
  const vocabulary = extractVocabulary(source);
  const pages = new Map([
    [references[0].path, pageEnglish(vocabulary)],
    [references[1].path, pageJapanese(vocabulary)],
  ]);
  for (const [path, page] of pages) validateMarkdownTables(page, path);
  return pages;
}

function validateMarkdownTables(page, path) {
  let expectedColumns;
  for (const line of page.split("\n")) {
    const trimmed = line.trim();
    if (!trimmed.startsWith("|") || !trimmed.endsWith("|")) {
      expectedColumns = undefined;
      continue;
    }
    const columns = trimmed.slice(1, -1).split("|").length;
    if (expectedColumns === undefined) expectedColumns = columns;
    else if (columns !== expectedColumns)
      fail(
        `${path.slice(root.length + 1)} has a Markdown table row with ${columns} cells; expected ${expectedColumns}`,
      );
  }
}

async function main() {
  const [source, render, hydrate] = await Promise.all([
    readFile(sourcePath, "utf8"),
    readFile(renderPath, "utf8"),
    readFile(hydratePath, "utf8"),
  ]);
  validateRendererPolicies(render, hydrate);
  const generated = generateReferencePages(source);
  const check = process.argv.includes("--check");
  let drift = false;
  for (const [path, expected] of generated) {
    const actual = await readFile(path, "utf8").catch((error) => {
      if (error.code === "ENOENT") return "";
      throw error;
    });
    if (actual === expected) continue;
    drift = true;
    if (!check) {
      await writeFile(path, expected);
      process.stdout.write(`updated ${path.slice(root.length + 1)}\n`);
    } else {
      process.stderr.write(`stale generated renderer vocabulary: ${path.slice(root.length + 1)}\n`);
    }
  }
  if (check && drift) process.exitCode = 1;
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  main().catch((error) => {
    process.stderr.write(`${error.message}\n`);
    process.exitCode = 1;
  });
}
