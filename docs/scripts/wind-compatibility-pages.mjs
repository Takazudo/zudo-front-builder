import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { WIND_SOURCE_SHA } from "../../scripts/wind-compatibility/inventory.mjs";

const hash = (value) => createHash("sha256").update(JSON.stringify(value)).digest("hex");
const code = (value) => `\`${String(value).replaceAll("`", "\\`")}\``;
const cell = (value) => String(value ?? "—").replaceAll("|", "\\|").replace(/\s+/g, " ");
const bucket = (name) => /^[a-z]/i.test(name) ? name[0].toLowerCase() : "symbols";
const labels = {
  en: {
    title: "Tailwind class compatibility", description: "Source-reviewed class and utility-root inventory for zudo-wind.",
    intro: "Find an exact class or bounded utility root using site search or the alphabetical pages below. A mapped registration is a source inspection, not a browser-tested compatibility claim. No Tailwind palette or spacing scale is supplied by Wind.",
    reference: "Pinned reference", source: "Reviewed Wind source", published: "Published-release support", notPublished: "Not established by this source inventory", verification: "Verification", profile: "Compatibility profile", rows: "Upstream registrations", entries: "Wind catalog entries", browse: "Browse class registrations", class: "Upstream class or root", wind: "Wind mapping", status: "Status", config: "Configuration", behavior: "Behavior and difference", alternative: "Alternative or issue", evidence: "Verification status", environment: "Environment / report", notes: "The source tag is observed; its link to the npm artifact is unverified. Browser comparison and accepted-reference state are separate. Physical `mx-auto` differs from logical-axis conventions in writing modes. See the migration guide for reset, cascade, and variant differences.",
  },
  ja: {
    title: "Tailwind クラス互換性", description: "zudo-wind のソースを確認したクラスとユーティリティ基部の一覧です。",
    intro: "完全なクラス名または有限の基部をサイト内検索か下のアルファベット順ページから探せます。対応する登録はソース上の確認であり、ブラウザーで検証した互換性を意味しません。Wind は Tailwind の色や余白スケールを暗黙には提供しません。",
    reference: "固定した参照", source: "確認した Wind ソース", published: "公開版での対応", notPublished: "このソース一覧からは未確定", verification: "検証", profile: "互換性プロファイル", rows: "上流の登録", entries: "Wind カタログ項目", browse: "クラスの登録を探す", class: "上流のクラスまたは基部", wind: "Wind への対応", status: "状態", config: "必要な設定", behavior: "動作と相違", alternative: "代替または課題", evidence: "検証状態", environment: "環境 / レポート", notes: "ソースタグは観測値です。npm アーティファクトとの対応は未検証です。ブラウザー比較と受け入れ済み参照は別です。`mx-auto` の物理軸は書字方向による論理軸の慣例と異なります。リセット、カスケード、バリアントの相違は移行ガイドを参照してください。",
  },
};

export function validateCompatibilityInventory(inventory, catalog, profile) {
  if (inventory?.schemaVersion !== 1 || inventory.kind !== "wind-compatibility-inventory") throw Error("Unsupported compatibility inventory");
  if (inventory.wind?.gitSha !== WIND_SOURCE_SHA || inventory.catalogDigest !== hash(catalog) || inventory.profileDigest !== hash(profile) || inventory.wind.specVersion !== catalog.specVersion || inventory.wind.specRevision !== catalog.specRevision || inventory.wind.catalogEntryCount !== catalog.entries.length || inventory.profile?.id !== profile.profileId || inventory.profile?.version !== profile.profileVersion || inventory.profile?.revision !== profile.profileRevision) throw Error("Stale compatibility inventory source/catalog/profile pin");
  if (inventory.rows.length !== 1288 || inventory.windEntries.length !== catalog.entries.length || new Set(inventory.rows.map((row) => row.id)).size !== inventory.rows.length || new Set(inventory.windEntries.map((row) => row.catalogId)).size !== catalog.entries.length) throw Error("Compatibility inventory row accounting failed");
  const ids = new Set(catalog.entries.map((entry) => entry.id));
  for (const row of inventory.rows) {
    if (row.windMapping?.catalogId && !ids.has(row.windMapping.catalogId)) throw Error(`Unknown mapping ${row.id}`);
    if (row.evidence?.windGitSha !== WIND_SOURCE_SHA || row.evidence.status !== "source-inspected" || row.evidence.independentlyDifferentialTested !== null || row.evidence.browserEnvironment !== null || row.evidence.reportId !== null) throw Error(`Unsupported verification claim ${row.id}`);
  }
  for (const entry of inventory.windEntries) if (!ids.has(entry.catalogId) || entry.evidence?.windGitSha !== WIND_SOURCE_SHA || entry.evidence.status !== "source-inspected") throw Error(`Unsupported Wind entry ${entry.id}`);
  return inventory;
}

export function renderCompatibilityPages(inventory, catalog, profile, locale, families) {
  const data = validateCompatibilityInventory(inventory, catalog, profile);
  const t = labels[locale];
  if (!t) throw Error(`Unsupported locale ${locale}`);
  const familyByEntry = new Map(families.flatMap((family) => family.entries.map((id) => [id, family.id])));
  const grouped = new Map();
  for (const row of data.rows) {
    const key = bucket(row.upstream.name ?? row.upstream.representation);
    if (!grouped.has(key)) grouped.set(key, []);
    grouped.get(key).push(row);
  }
  const pages = new Map();
  const sortedBuckets = [...grouped.keys()].sort((a,b) => a === "symbols" ? -1 : b === "symbols" ? 1 : a.localeCompare(b));
  const reference = `${data.upstreamPin.package}@${data.upstreamPin.version} (${data.upstreamPin.tag}, ${data.upstreamPin.observedTagCommit})`;
  const common = [`${t.reference}: ${code(reference)}.`, `${t.source}: ${code(data.wind.gitSha)}; spec ${code(`${data.wind.specVersion}.${data.wind.specRevision}`)}.`, `${t.profile}: ${code(`${data.profile.id} ${data.profile.version}.${data.profile.revision}`)}.`, `${t.published}: ${t.notPublished}. ${t.verification}: ${code("source-inspected")}; browser ${code("not recorded")}; accepted reference ${code("null")}.`];
  const profileExamples = [...new Set(data.profileCases.map((item) => item.upstreamCandidate))];
  const missingExamples = ["line-clamp-2", "ring-2", "container"];
  for (const candidate of missingExamples) {
    const name = candidate.replace(/-\d+$/, "");
    if (!data.rows.some((row) => row.upstream.name === name && row.windMapping === null)) throw Error(`Missing upstream example ${candidate} has no source row`);
  }
  pages.set("index.mdx", ["---",`title: ${JSON.stringify(t.title)}`,`description: ${JSON.stringify(t.description)}`,"sidebar_position: 19","generated: true","---","",t.intro,"",...common,"",`${t.rows}: ${code(data.rows.length)}. ${t.entries}: ${code(data.windEntries.length)}.`,"",`## ${locale === "ja" ? "検索例" : "Class lookup examples"}`,"",`${locale === "ja" ? "プロファイル内の候補" : "Profile candidates"}: ${profileExamples.map(code).join(", ")}.`,"",`${locale === "ja" ? "未対応の基部の例" : "Examples with unmapped roots"}: ${missingExamples.map(code).join(", ")}. ${locale === "ja" ? "これは上流の基部登録の例で、任意の接尾辞が動作するという主張ではありません。" : "These illustrate upstream root registrations; they do not claim that every suffix works."}`,"",`## ${t.browse}`,"",...sortedBuckets.map((key) => `- [${key.toUpperCase()}](${key}.mdx) (${grouped.get(key).length})`),"",`${t.notes} [${locale === "ja" ? "移行ガイド" : "Migration guide"}](../coming-from-tailwind.mdx).`,""].join("\n"));
  for (const key of sortedBuckets) {
    const rows = grouped.get(key);
    const lines = ["---",`title: ${JSON.stringify(`${t.title}: ${key.toUpperCase()}`)}`,`description: ${JSON.stringify(t.description)}`,`sidebar_position: ${20 + sortedBuckets.indexOf(key)}`,"generated: true","---","",t.intro,"",`[${locale === "ja" ? "互換性一覧" : "Compatibility index"}](index.mdx) · [${locale === "ja" ? "移行ガイド" : "Migration guide"}](../coming-from-tailwind.mdx)`,"",...common,"",`| ${[t.class,t.wind,t.status,t.config,t.behavior,t.alternative,t.evidence,t.environment].join(" | ")} |`,`| ${Array(8).fill("---").join(" | ")} |`];
    for (const row of rows) {
      const family = familyByEntry.get(row.windMapping?.catalogId);
      const mapping = row.windMapping ? `${code(row.windMapping.wind ?? "—")}${family ? ` ([${family}](../utilities/${family}.mdx))` : ""}` : "—";
      const alt = [row.alternative,row.trackingIssue].filter(Boolean).join("; ");
      lines.push(`| ${[code(row.upstream.name ?? row.upstream.representation),mapping,code(row.disposition),row.configurationRequirement,row.semanticDifference,alt,code(row.evidence.status),row.evidence.browserEnvironment ?? "—"].map(cell).join(" | ")} |`);
    }
    pages.set(`${key}.mdx`,lines.concat("").join("\n"));
  }
  return pages;
}

export function loadCompatibilityInputs(root) {
  const read = (path) => JSON.parse(readFileSync(join(root,path),"utf8"));
  return { inventory: read("tests/wind-compatibility/inventory.v1.json"), profile: read("tests/wind-compatibility/profile.json") };
}
