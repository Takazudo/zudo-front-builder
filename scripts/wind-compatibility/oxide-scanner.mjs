import { createHash } from "node:crypto";
import { createRequire } from "node:module";
import { mkdir, readFile, writeFile } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import { gunzipSync } from "node:zlib";
import { outsideCheckout, sha256, tarPackageIdentity } from "./reference.mjs";

const version = "4.3.2";
const packages = {
  wrapper: {
    name: "@tailwindcss/oxide",
    url: "https://registry.npmjs.org/@tailwindcss/oxide/-/oxide-4.3.2.tgz",
    sri: "sha512-z8ZgnzX8gdNoWLBLqBPoh/sjnxkwvf9ZuWjnO0l0yIzbLa5/9S+eC5QxGZKRobVHIC3/1BoMWjHblqWjcgFgag==",
  },
  "darwin-arm64": {
    name: "@tailwindcss/oxide-darwin-arm64",
    url: "https://registry.npmjs.org/@tailwindcss/oxide-darwin-arm64/-/oxide-darwin-arm64-4.3.2.tgz",
    sri: "sha512-GZypeUY/IDJW3877KeM+O67vbXr3MBnbtEL4aYhNErv/JWZhye2vGSWWG9tB6iiqR2MqRNkY8IOUy4NdSZV26w==",
  },
  "linux-x64": {
    name: "@tailwindcss/oxide-linux-x64-gnu",
    url: "https://registry.npmjs.org/@tailwindcss/oxide-linux-x64-gnu/-/oxide-linux-x64-gnu-4.3.2.tgz",
    sri: "sha512-kqCZpSKOBEJO4mz7OqWoofBZeXTAwaVGPj0ErAj7CojmhKpWVWVOnrt9dE8odoIraZq4oj3ausM37kXi+Tow8w==",
  },
};

function members(gz) {
  const tar = gunzipSync(gz),
    result = new Map();
  for (let at = 0; at + 512 <= tar.length;) {
    const name = tar.toString("utf8", at, at + 100).replace(/\0.*$/, "");
    if (!name) break;
    const size = parseInt(
      tar
        .toString("ascii", at + 124, at + 136)
        .replace(/\0.*$/, "")
        .trim(),
      8,
    );
    if (!Number.isFinite(size)) throw Error("Malformed oxide tarball");
    if (name.startsWith("package/") && !name.endsWith("/"))
      result.set(name.slice(8), tar.subarray(at + 512, at + 512 + size));
    at += 512 + Math.ceil(size / 512) * 512;
  }
  return result;
}

async function verifiedPackage(info, cache, fetcher) {
  const base = await outsideCheckout(
    resolve(cache, info.name.replaceAll("/", "-") + "-" + version),
  );
  const archive = await outsideCheckout(resolve(base, "archive.tgz"));
  let bytes;
  try {
    bytes = await readFile(archive);
  } catch (error) {
    if (error.code !== "ENOENT") throw error;
    const response = await fetcher(info.url);
    if (!response.ok) throw Error(`Scanner acquisition HTTP ${response.status}`);
    bytes = Buffer.from(await response.arrayBuffer());
  }
  const actualSri = "sha512-" + createHash("sha512").update(bytes).digest("base64");
  if (
    actualSri !== info.sri ||
    tarPackageIdentity(bytes).name !== info.name ||
    tarPackageIdentity(bytes).version !== version
  )
    throw Error(`Scanner artifact identity mismatch: ${info.name}`);
  await mkdir(base, { recursive: true });
  await writeFile(archive, bytes);
  return { base, members: members(bytes), archiveSha256: sha256(bytes), integrity: info.sri };
}

export async function loadIndependentScanner(cache, fetcher = fetch) {
  const platform = `${process.platform}-${process.arch}`;
  if (!packages[platform]) throw Error(`Unpinned Tailwind scanner platform ${platform}`);
  const wrapper = await verifiedPackage(packages.wrapper, cache, fetcher);
  const native = await verifiedPackage(packages[platform], cache, fetcher);
  const index = wrapper.members.get("index.js");
  const binaryEntry = [...native.members].find(([name]) => name.endsWith(".node"));
  if (!index || !binaryEntry) throw Error("Pinned scanner JS or native binary missing");
  const indexPath = await outsideCheckout(resolve(wrapper.base, "index.js"));
  const binaryPath = await outsideCheckout(resolve(native.base, binaryEntry[0]));
  await mkdir(dirname(binaryPath), { recursive: true });
  for (const [path, expected] of [
    [indexPath, index],
    [binaryPath, binaryEntry[1]],
  ]) {
    try {
      if (sha256(await readFile(path)) !== sha256(expected))
        throw Error(`Scanner module changed: ${path}`);
    } catch (error) {
      if (error.code !== "ENOENT") throw error;
      await writeFile(path, expected);
    }
  }
  const prior = process.env.NAPI_RS_NATIVE_LIBRARY_PATH;
  process.env.NAPI_RS_NATIVE_LIBRARY_PATH = binaryPath;
  let Scanner;
  try {
    ({ Scanner } = createRequire(import.meta.url)(indexPath));
  } finally {
    if (prior === undefined) delete process.env.NAPI_RS_NATIVE_LIBRARY_PATH;
    else process.env.NAPI_RS_NATIVE_LIBRARY_PATH = prior;
  }
  if (typeof Scanner !== "function") throw Error("Pinned scanner API absent");
  return {
    version,
    Scanner,
    identity: {
      wrapperIntegrity: wrapper.integrity,
      wrapperSha256: wrapper.archiveSha256,
      wrapperModuleSha256: sha256(index),
      nativePackage: packages[platform].name,
      nativeIntegrity: native.integrity,
      nativeSha256: native.archiveSha256,
      nativeModuleSha256: sha256(binaryEntry[1]),
      platform,
    },
  };
}

export function scanOriginal(Scanner, source, extension) {
  if (!["html", "tsx", "mdx"].includes(extension))
    throw Error(`Unsupported scan extension ${extension}`);
  return [
    ...new Set(new Scanner({ sources: [] }).scanFiles([{ content: source, extension }])),
  ].sort();
}
