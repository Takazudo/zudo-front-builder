import { readdirSync, readFileSync, writeFileSync } from 'node:fs';
import { resolve, join, relative } from 'node:path';
import { createHash } from 'node:crypto';
import { gzipSync } from 'node:zlib';
const root = resolve(process.argv[2]);
const hash = bytes => createHash('sha256').update(bytes).digest('hex');
const walk = dir => readdirSync(dir, { withFileTypes: true }).flatMap(entry => {
  const path = join(dir, entry.name);
  if (entry.isDirectory()) return walk(path);
  if (!entry.isFile()) throw new Error(`unexpected non-file in production dist: ${path}`);
  return [path];
}).sort();
const files = walk(root).map(path => {
  const bytes = readFileSync(path);
  return { path: relative(root, path).replaceAll('\\', '/'), raw: bytes.length,
    gzip9: gzipSync(bytes, { level: 9, mtime: 0 }).length, sha256: hash(bytes) };
});
const js = files.filter(file => /\.(?:js|mjs|cjs)$/.test(file.path));
const entryChunks = js.filter(file => /^assets\/islands(?:-[^/]+)?\.js$/.test(file.path) && !/^assets\/islands-chunk-/.test(file.path));
if (!entryChunks.length) throw new Error('no production router entry retained; skipped test is not evidence');
const htmlFiles = files.filter(file => file.path.endsWith('.html'));
if (!htmlFiles.length) throw new Error('no production HTML retained');
const html = htmlFiles.map(file => readFileSync(join(root, file.path), 'utf8')).join('\n');
for (const chunk of entryChunks) {
  if (!html.includes('/' + chunk.path)) throw new Error(`HTML does not load entry ${chunk.path}`);
}
const sum = rows => ({ raw: rows.reduce((n, file) => n + file.raw, 0),
  gzip9: rows.reduce((n, file) => n + file.gzip9, 0) });
const summary = {
  node: process.version, zlib: process.versions.zlib, files, entryChunks,
  sharedChunks: js.filter(file => /^assets\/islands-chunk-/.test(file.path)),
  allJavaScript: sum(js),
  measurement: 'original production dist bytes; every .js/.mjs/.cjs file counted; no normalization or ceiling',
};
writeFileSync(resolve(root, '../inventory.json'), JSON.stringify(summary, null, 2) + '\n');
console.log(JSON.stringify({ entryChunks, sharedChunks: summary.sharedChunks, allJavaScript: summary.allJavaScript }));
