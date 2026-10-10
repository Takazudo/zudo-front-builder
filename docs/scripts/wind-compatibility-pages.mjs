import { createHash } from "node:crypto";
import { readFileSync, readdirSync } from "node:fs";
import { join } from "node:path";
import { COMPATIBILITY_BUCKETS } from "../src/config/navigation-groups.mjs";
import { WIND_SOURCE_SHA } from "../../scripts/wind-compatibility/inventory.mjs";

const hash = (value) => createHash("sha256").update(JSON.stringify(value)).digest("hex");
const yamlString = (value) => {
  const text = String(value);
  const safePlain =
    /^[\p{L}\p{N}][\p{L}\p{N} .,;!?()/、。・-]*$/u.test(text) &&
    !/^(?:true|false|null|yes|no|on|off|~|[0-9]+(?:\.[0-9]+)?)$/i.test(text);
  return safePlain ? text : JSON.stringify(text);
};
// Complete reviewed upstream-plus-Wind rows, including unmapped registrations.
export const REVIEWED_INVENTORY_DIGEST =
  "fa8966e857fb2b12f2f1382621106d050a0f05cbd29f47d56976eb9670aac80f";
const code = (value) => {
  const content = String(value);
  const ticks = "`".repeat(
    1 + Math.max(0, ...[...content.matchAll(/`+/g)].map(([run]) => run.length)),
  );
  return `${ticks}${content.startsWith("`") || content.endsWith("`") ? ` ${content} ` : content}${ticks}`;
};
const cell = (value) =>
  String(value ?? "—")
    .replaceAll("|", "\\|")
    .replace(/\s+/g, " ");
const prose = (value) =>
  cell(value)
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll("{", "&#123;")
    .replaceAll("}", "&#125;");
const bucket = (name) => (/^[a-z]/i.test(name) ? name[0].toLowerCase() : "symbols");
const labels = {
  en: {
    title: "Tailwind class compatibility",
    description: "Source-reviewed class and utility-root inventory for zudo-wind.",
    intro:
      "Find an exact class or bounded utility root using site search or the alphabetical sections below. A mapped registration is a source inspection, not a browser-tested compatibility claim. No Tailwind palette or spacing scale is supplied by Wind.",
    reference: "Pinned reference",
    source: "Reviewed Wind source",
    published: "Published-release support",
    notPublished: "Not established by this source inventory",
    verification: "Inventory evidence",
    profile: "Compatibility profile",
    rows: "Upstream registrations",
    entries: "Wind catalog entries",
    browse: "Browse class registrations",
    class: "Upstream class or root",
    wind: "Wind mapping",
    status: "Status",
    config: "Configuration",
    behavior: "Behavior and difference",
    alternative: "Alternative or issue",
    evidence: "Verification status",
    environment: "Environment / report",
    notes:
      "The source tag is observed; its link to the npm artifact is unverified. Browser comparison and accepted-reference state are separate. Physical `mx-auto` differs from logical-axis conventions in writing modes. See the migration guide for reset, cascade, and variant differences.",
  },
  ja: {
    title: "Tailwind クラス互換性",
    description: "zudo-wind のソースを確認したクラスとユーティリティ基部の一覧です。",
    intro:
      "完全なクラス名または有限の基部をサイト内検索か下のアルファベット順のセクションから探せます。対応する登録はソース上の確認であり、ブラウザーで検証した互換性を意味しません。Wind は Tailwind の色や余白スケールを暗黙には提供しません。",
    reference: "固定した参照",
    source: "確認した Wind ソース",
    published: "公開版での対応",
    notPublished: "このソース一覧からは未確定",
    verification: "一覧の根拠",
    profile: "互換性プロファイル",
    rows: "上流の登録",
    entries: "Wind カタログ項目",
    browse: "クラスの登録を探す",
    class: "上流のクラスまたは基部",
    wind: "Wind への対応",
    status: "状態",
    config: "必要な設定",
    behavior: "動作と相違",
    alternative: "代替または課題",
    evidence: "検証状態",
    environment: "環境 / レポート",
    notes:
      "ソースタグは観測値です。npm アーティファクトとの対応は未検証です。ブラウザー比較と受け入れ済み参照は別です。`mx-auto` の物理軸は書字方向による論理軸の慣例と異なります。リセット、カスケード、バリアントの相違は移行ガイドを参照してください。",
  },
};

const japaneseAlternative = new Map([
  [
    "Author the retained native form in CSS and reserve its complete class with wind.authoredClasses.",
    "残す形式は CSS に記述し、完全なクラス名を wind.authoredClasses に登録してください。",
  ],
  [
    "Author table-layout: auto in CSS and reserve the complete class with wind.authoredClasses.",
    "table-layout: auto を CSS に記述し、完全なクラス名を wind.authoredClasses に登録してください。",
  ],
  [
    "Author table-layout: fixed in CSS and reserve the complete class with wind.authoredClasses.",
    "table-layout: fixed を CSS に記述し、完全なクラス名を wind.authoredClasses に登録してください。",
  ],
  ["Author order in CSS for retained forms.", "残す形式の order は CSS に記述してください。"],
  [
    "Author flex-basis in CSS for retained forms, optionally using configured --zw-spacing-* or --zw-size-* variables.",
    "残す形式の flex-basis は CSS に記述してください。設定済みの --zw-spacing-* または --zw-size-* 変数も使えます。",
  ],
  ["Author fill in CSS for retained forms.", "残す形式の fill は CSS に記述してください。"],
  [
    "Author stroke or stroke-width in CSS for retained forms.",
    "残す形式の stroke または stroke-width は CSS に記述してください。",
  ],
]);
const japaneseStatus = {
  "candidate-mapping-untested": "候補対応・未検証",
  "unmapped-upstream-registration": "上流登録・Wind 対応なし",
  "reviewed-difference": "相違を確認",
  "excluded-deferred": "対象外・延期",
  "native-css-review-candidate": "CSS での代替を確認中",
  "requires-review": "要確認",
  "partial-native-adoption-unverified": "一部実装・未検証",
  "variant-unmapped": "バリアント対応なし",
};
function japaneseSubset(value, field) {
  const capability = value.includes("negative ASCII integer")
    ? "負の ASCII 整数 0..2147483647 のみ"
    : value.includes("positive integer fractions")
      ? "auto、full、px、0、min、max、fit、content と、分子・分母が 1..1000000 の正の整数の分数"
      : value.includes("fill-current")
        ? "正確な fill-current と fill-none"
        : value.includes("stroke-current")
          ? "正確な stroke-current と stroke-none"
          : "first、last、none と ASCII 整数 0..2147483647";
  const exclusions = value.includes("negative ASCII integer")
    ? "負のキーワード、任意値、カスタムプロパティ、名前付きトークン、整数以外の接尾辞"
    : value.includes("positive integer fractions")
      ? "0 以外の数値スケール、名前付きトークン、任意値、無効な分数"
      : value.includes("fill-current")
        ? "名前付き色、transparent、inherit、不透明度修飾子、任意値、URL"
        : value.includes("stroke-current")
          ? "名前付き色、transparent、inherit、不透明度修飾子、任意値、URL、stroke-width"
          : "任意値、カスタムプロパティ、名前付きトークン、整数以外の接尾辞";
  return `Wind は ${capability}に対応します。対象外: ${exclusions}。${field === "config" ? "トークン設定でこのネイティブ範囲は拡大しません。" : "ソースとカタログ上の対応だけではブラウザーの一致は証明されません。"}`;
}
function localizedNote(value, locale, field) {
  if (!value || locale === "en") return value;
  if (field === "alternative") {
    const translated = japaneseAlternative.get(value);
    if (!translated) throw Error(`Missing Japanese alternative: ${value}`);
    return translated;
  }
  if (value === "dark enabled") return "dark バリアントの有効化が必要です。";
  if (value === "breakpoint name configured") return "ブレークポイント名の設定が必要です。";
  if (
    value === "State must be in Wind is_state allowlist; compound nesting and ordering unverified"
  )
    return "状態は Wind の is_state 許可リストに含まれる必要があります。複合ネストと順序は未検証です。";
  if (value === "No configuration established for this row.")
    return "この行の設定要件は未確定です。";
  if (value === "Pattern domain requires case-specific source review; no default token guarantee.")
    return "パターンの範囲には個別のソース確認が必要です。既定トークンの保証はありません。";
  const theme = value.match(
    /^Upstream theme keys: (.*); Wind named tokens use var\(--zw-\.\.\.\) in zw-tokens; numeric spacingUnit is optional where relevant\.$/,
  );
  if (theme)
    return `上流のテーマキー: ${theme[1]}。Wind の名前付きトークンは zw-tokens 内の var(--zw-...) を使います。該当する場合、数値 spacingUnit は任意です。`;
  if (value.startsWith("Wind supported subset:") || value.startsWith("Wind supports "))
    return japaneseSubset(value, field);
  const axes = value.match(
    /^Tailwind (.*) uses logical axes; Wind (.*) uses physical axes\. Review horizontal and vertical writing modes\.$/,
  );
  if (axes)
    return `Tailwind の ${axes[1]} は論理軸、Wind の ${axes[2]} は物理軸を使います。横書きと縦書きを確認してください。`;
  throw Error(`Missing Japanese ${field} translation: ${value}`);
}

// Fingerprint of the reviewed Wind source closure, independent of shallow CI Git history.
// The zfb/zudo-react h-import alias changes extraction entry matching without
// changing the pinned utility compatibility catalog.
// #318 refreshes only Cargo.lock for the optional Rolldown prototype: Wind
// source/catalog are unchanged; all 117 Wind library tests pass at this closure.
export const REVIEWED_WIND_SOURCE_DIGEST =
  "68fc7286f0d2f394582bc8beeeb7bbc9900df82f1f184a612c88b5d159fafa2a";
export function assertCurrentWindSource(root, pin, expectedDigest = REVIEWED_WIND_SOURCE_DIGEST) {
  if (pin !== WIND_SOURCE_SHA) throw Error(`Unreviewed Wind support pin ${pin}`);
  const paths = [];
  const walk = (directory) => {
    for (const entry of readdirSync(join(root, directory), { withFileTypes: true })) {
      const path = `${directory}/${entry.name}`;
      if (entry.isDirectory()) walk(path);
      else if (entry.isFile()) paths.push(path);
      else throw Error(`Unsupported Wind source path ${path}`);
    }
  };
  walk("crates/zudo-wind");
  paths.push("Cargo.toml", "Cargo.lock");
  paths.sort();
  const digest = createHash("sha256");
  for (const path of paths) {
    digest
      .update(path)
      .update("\0")
      .update(readFileSync(join(root, path)))
      .update("\0");
  }
  if (paths.length !== 100 || digest.digest("hex") !== expectedDigest)
    throw Error(`Wind source differs from reviewed support pin ${pin}`);
}

export function validateCompatibilityInventory(inventory, catalog, profile) {
  if (inventory?.schemaVersion !== 1 || inventory.kind !== "wind-compatibility-inventory")
    throw Error("Unsupported compatibility inventory");
  if (
    inventory.wind?.gitSha !== WIND_SOURCE_SHA ||
    inventory.catalogDigest !== hash(catalog) ||
    inventory.profileDigest !== hash(profile) ||
    inventory.wind.specVersion !== catalog.specVersion ||
    inventory.wind.specRevision !== catalog.specRevision ||
    inventory.wind.catalogEntryCount !== catalog.entries.length ||
    inventory.profile?.id !== profile.profileId ||
    inventory.profile?.version !== profile.profileVersion ||
    inventory.profile?.revision !== profile.profileRevision
  )
    throw Error("Stale compatibility inventory source/catalog/profile pin");
  if (
    inventory.rows.length !== 1288 ||
    inventory.windEntries.length !== catalog.entries.length ||
    new Set(inventory.rows.map((row) => row.id)).size !== inventory.rows.length ||
    new Set(inventory.windEntries.map((row) => row.catalogId)).size !== catalog.entries.length
  )
    throw Error("Compatibility inventory row accounting failed");
  const ids = new Set(catalog.entries.map((entry) => entry.id));
  for (const row of inventory.rows) {
    if (row.windMapping?.catalogId && !ids.has(row.windMapping.catalogId))
      throw Error(`Unknown mapping ${row.id}`);
    if (
      row.windMapping?.catalogIds &&
      (!Array.isArray(row.windMapping.catalogIds) ||
        !row.windMapping.catalogIds.length ||
        row.windMapping.catalogIds.some((id) => !ids.has(id)))
    )
      throw Error(`Unknown bounded mapping ${row.id}`);
    if (
      row.evidence?.windGitSha !== WIND_SOURCE_SHA ||
      row.evidence.status !== "source-inspected" ||
      row.evidence.independentlyDifferentialTested !== null ||
      row.evidence.browserEnvironment !== null ||
      row.evidence.reportId !== null
    )
      throw Error(`Unsupported verification claim ${row.id}`);
  }
  for (const entry of inventory.windEntries)
    if (
      !ids.has(entry.catalogId) ||
      entry.evidence?.windGitSha !== WIND_SOURCE_SHA ||
      entry.evidence.status !== "source-inspected"
    )
      throw Error(`Unsupported Wind entry ${entry.id}`);
  if (hash(inventory) !== REVIEWED_INVENTORY_DIGEST)
    throw Error("Unreviewed compatibility inventory content");
  return inventory;
}

export function renderCompatibilityPages(inventory, catalog, profile, locale, families) {
  const data = validateCompatibilityInventory(inventory, catalog, profile);
  const t = labels[locale];
  if (!t) throw Error(`Unsupported locale ${locale}`);
  const familyByEntry = new Map(
    families.flatMap((family) => family.entries.map((id) => [id, family.id])),
  );
  const grouped = new Map();
  for (const row of data.rows) {
    const key = bucket(row.upstream.name ?? row.upstream.representation);
    if (!grouped.has(key)) grouped.set(key, []);
    grouped.get(key).push(row);
  }
  const pages = new Map();
  const sortedBuckets = [...grouped.keys()].sort((a, b) =>
    a === "symbols" ? -1 : b === "symbols" ? 1 : a.localeCompare(b),
  );
  if (JSON.stringify(sortedBuckets) !== JSON.stringify(COMPATIBILITY_BUCKETS))
    throw Error("Compatibility bucket membership changed");
  const reference = `${data.upstreamPin.package}@${data.upstreamPin.version} (${data.upstreamPin.tag}, ${data.upstreamPin.observedTagCommit})`;
  const common = [
    `${t.reference}: ${code(reference)}.`,
    `${t.source}: ${code(data.wind.gitSha)}; spec ${code(`${data.wind.specVersion}.${data.wind.specRevision}`)}.`,
    `${t.profile}: ${code(`${data.profile.id} ${data.profile.version}.${data.profile.revision}`)}.`,
    `${t.published}: ${t.notPublished}. ${t.verification}: ${code("source-inspected")}. ${locale === "ja" ? "この表にブラウザー検証の結果は含まれません。現在の受け入れ済み参照は" : "This table does not carry browser verification results. See the current accepted reference in"} [${locale === "ja" ? "正式な状態記録" : "the canonical status record"}](https://github.com/Takazudo/zudo-front-builder/blob/main/tests/wind-compatibility/reference/accepted.json)${locale === "ja" ? "を確認してください。" : "."}`,
  ];
  const profileExamples = [...new Set(data.profileCases.map((item) => item.upstreamCandidate))];
  const missingExamples = ["line-clamp-2", "ring-2", "container"];
  for (const candidate of missingExamples) {
    const name = candidate.replace(/-\d+$/, "");
    if (!data.rows.some((row) => row.upstream.name === name && row.windMapping === null))
      throw Error(`Missing upstream example ${candidate} has no source row`);
  }
  pages.set(
    "index.mdx",
    [
      "---",
      `title: ${yamlString(t.title)}`,
      `description: ${yamlString(t.description)}`,
      "sidebar_position: 19",
      "hide_toc: true",
      "generated: true",
      "---",
      "",
      t.intro,
      "",
      ...common,
      "",
      `${t.rows}: ${code(data.rows.length)}. ${t.entries}: ${code(data.windEntries.length)}.`,
      "",
      `## ${locale === "ja" ? "検索例" : "Class lookup examples"}`,
      "",
      `${locale === "ja" ? "プロファイル内の候補" : "Profile candidates"}: ${profileExamples.map(code).join(", ")}.`,
      "",
      `${locale === "ja" ? "未対応の基部の例" : "Examples with unmapped roots"}: ${missingExamples.map(code).join(", ")}. ${locale === "ja" ? "これは上流の基部登録の例で、任意の接尾辞が動作するという主張ではありません。" : "These illustrate upstream root registrations; they do not claim that every suffix works."}`,
      "",
      `${t.notes} [${locale === "ja" ? "移行ガイド" : "Migration guide"}](../coming-from-tailwind.mdx).`,
      "",
    ].join("\n"),
  );
  for (const key of sortedBuckets) {
    const rows = grouped.get(key);
    const lines = [
      `<h2 id="compat-${key}">${key.toUpperCase()}</h2>`,
      "",
      `| ${[t.class, t.wind, t.status, t.config, t.behavior, t.alternative, t.evidence, t.environment].join(" | ")} |`,
      `| ${Array(8).fill("---").join(" | ")} |`,
    ];
    for (const row of rows) {
      const family = familyByEntry.get(
        row.windMapping?.catalogId ?? row.windMapping?.catalogIds?.[0],
      );
      const mapping = row.windMapping
        ? `${code(row.windMapping.wind ?? "—")}${family ? ` ([${family}](../utilities/${family}.mdx))` : ""}`
        : "—";
      const issue = row.trackingIssue
        ? `[#${row.trackingIssue.split("/").at(-1)}](${row.trackingIssue})`
        : null;
      const alt = [localizedNote(row.alternative, locale, "alternative"), issue]
        .filter(Boolean)
        .join("; ");
      const status =
        locale === "ja"
          ? `${japaneseStatus[row.disposition]} (${code(row.disposition)})`
          : code(row.disposition);
      lines.push(
        `| ${[code(row.upstream.representation ?? row.upstream.name), mapping, status, prose(localizedNote(row.configurationRequirement, locale, "config")), prose(localizedNote(row.semanticDifference, locale, "behavior")), prose(alt), code(row.evidence.status), row.evidence.browserEnvironment ?? "—"].map(cell).join(" | ")} |`,
      );
    }
    pages.set("index.mdx", pages.get("index.mdx") + "\n" + lines.concat("").join("\n"));
  }
  return pages;
}

export function loadCompatibilityInputs(root) {
  const read = (path) => JSON.parse(readFileSync(join(root, path), "utf8"));
  const inventory = read("tests/wind-compatibility/inventory.v1.json");
  assertCurrentWindSource(root, inventory.wind.gitSha);
  return { inventory, profile: read("tests/wind-compatibility/profile.json") };
}
