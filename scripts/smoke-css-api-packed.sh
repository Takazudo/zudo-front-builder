#!/usr/bin/env bash
set -euo pipefail
# Match the release/install runner umask; keep the installed 0755 assertion strict.
umask 022
# Level 3: freshly built SDK + native carrier, installed outside the workspace.
# No registry package or workspace symlink may satisfy the compiler dependency.
: "${ZFB_LINUX_X64_CSS_ADDON:?built addon required}"
: "${ZFB_LINUX_X64_BINARY:?built CLI required for parity checks}"
repo="$PWD"
room="$(mktemp -d)"
echo "CSS API clean room: $room"
cp "$ZFB_LINUX_X64_CSS_ADDON" packages/zfb-linux-x64-gnu/zfb-css.node
chmod 0644 packages/zfb-linux-x64-gnu/zfb-css.node
cp "$ZFB_LINUX_X64_BINARY" packages/zfb-linux-x64-gnu/zfb
chmod 755 packages/zfb-linux-x64-gnu/zfb
(cd packages/zfb-linux-x64-gnu && npm pack --pack-destination "$room" >/dev/null)
(cd packages/zfb-slugify && pnpm pack --pack-destination "$room" >/dev/null)
(cd packages/zfb && pnpm pack --pack-destination "$room" >/dev/null)
node --input-type=module - "$room" <<'JS'
import fs from 'node:fs';
const room = process.argv[2];
const names = ['zfb', 'zfb-slugify', 'zfb-linux-x64-gnu'];
const dependencies = Object.fromEntries(names.map(n => [`@takazudo/${n}`, `file:${room}/${fs.readdirSync(room).find(f => f.startsWith(`takazudo-${n}-`) && /^\d/.test(f.slice(`takazudo-${n}-`.length)))}`]));
fs.writeFileSync(`${room}/package.json`, JSON.stringify({private:true,type:'module',dependencies}));
fs.writeFileSync(`${room}/pnpm-workspace.yaml`, `minimumReleaseAge: 0\noverrides:\n${Object.entries(dependencies).map(([n,p])=>`  '${n}': '${p}'`).join('\n')}\n`);
JS
(cd "$room" && pnpm install --ignore-scripts --no-frozen-lockfile)
cp tests/css-api/consumer.mjs "$room/consumer.mjs"
cp tests/css-api/consumer.ts "$room/consumer.ts"
# Typechecking is against installed declarations, with only Node types supplied
# from the tooling dependency tree. No paths/baseUrl/workspace SDK aliases.
"$repo/packages/zfb/node_modules/.bin/tsc" --noEmit --strict --skipLibCheck --module nodenext --target es2022 --types node --typeRoots "$repo/packages/zfb/node_modules/@types" "$room/consumer.ts"
# Artifact downloads lose executable mode; parity uses the identical staged CLI.
ZFB_CSS_API_ROOM="$room" ZFB_CSS_API_REPO="$repo" ZFB_CSS_API_CLI="$repo/packages/zfb-linux-x64-gnu/zfb" node "$room/consumer.mjs"
cp tests/css-api/readonly.mjs "$room/readonly.mjs"
mkdir "$room/readonly-project"
cp crates/zfb/tests/fixtures/css-determinism/entry.css "$room/readonly-project/entry.css"
mkdir "$room/readonly-project/src"
cp crates/zfb/tests/fixtures/css-determinism/src/index.html "$room/readonly-project/src/index.html"
printf '%s\n' 'throw new Error("must not evaluate config")' > "$room/readonly-project/zfb.config.ts"
chmod -R a-w "$room/readonly-project"
node --input-type=module - "$room/readonly-project" <<'JS'
import fs from 'node:fs';
import assert from 'node:assert/strict';
assert.throws(() => fs.writeFileSync(`${process.argv[2]}/probe`, 'denied'), {code:'EACCES'});
JS
strace -f -o "$room/native.trace" -e trace=process,file node "$room/readonly.mjs" "$room/readonly-project"
node tests/css-api/trace.mjs "$room/native.trace"
if [[ "${ZFB_CSS_API_BROWSER:-0}" == 1 ]]; then
  node tests/css-api/browser.mjs "$room"
fi
echo "CSS_API_ROOM=$room"
