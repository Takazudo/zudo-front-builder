# Mac recipe: neutral receipt N, Darwin stage attempt 3 and apply (#4314)

The owner answered the [decision packet](SECURITY-4314-DECISION-PACKET.md) on 2026-10-20:

- **D1 yes:** the owner attests N.
- **D2 yes:** the Linux `actualGzip` assertion form is accepted.
- **D3 no:** the operator string is not published.

This recipe covers only the Mac side of the [reconciliation plan](https://github.com/Takazudo/zudo-front-builder/issues/4314#issuecomment-6277405812). It does not build, measure, push, merge or touch the original evidence.

Run the blocks in order, in one fresh Bash shell, and stop at the first failed check. Every value printed for return is a digest, a count, a placeholder or an exit code. **Do not paste any local path, user name, host name or the original operator string into a public comment.** Hand files back through the private project channel.

## 0. Inputs and preconditions

```bash
set -euo pipefail
# Site values (private; never published).
SOURCE='/absolute/path/to/the clean bda633b checkout used for collection-01'
TOOLS='/absolute/path/to/the clean 7e79458 reviewed-tools checkout'
ZFB_NODE='/absolute/path/to/node-24.14.0/bin/node'
ORIG_RECEIPT='/absolute/path/to/collection-01/receipt.json'
BUNDLE='/absolute/path/to/collection-01/bundle'
ATTEMPT2='/absolute/path/to/the stage-attempt-02 directory'
NEW='/absolute/permanent/path/to/a NEW attempt-03 directory'   # must not exist
APPLY='/absolute/path/to/a NEW worktree directory'             # must not exist

SOURCE_SHA=bda633b3589e201737b02cf2060f7b24d41a45a7
TOOL_SHA=7e79458304878d51bafd29e24ec25e93b84893ca
ORIG_SHA=cfae5404e8e6a0f61c045eb024369cf28321669c9160bb3a3532108d0131e50c
BUNDLE_SHA=136395016503fb8062e8a902cfc5e11191461325952e0216f09f6ffe0a03c83e
SCRIPT_COMMIT=0cc504331ef92bdad81fdafa79227d6c21d409ab
SCRIPT_SHA=167686d3d9205e63d3a392861b0fa4b231927bed7401c847aec3f0bdc3ff05d9

test ! -e "$NEW"; test ! -e "$APPLY"
test "$(git -C "$SOURCE" --no-replace-objects rev-parse HEAD)" = "$SOURCE_SHA"
test "$(git -C "$TOOLS" --no-replace-objects rev-parse HEAD)" = "$TOOL_SHA"
test -z "$(git -C "$SOURCE" status --porcelain --untracked-files=normal)"
test -z "$(git -C "$TOOLS" status --porcelain --untracked-files=normal)"
test "$("$ZFB_NODE" --version)" = v24.14.0
test "$(shasum -a 256 "$ORIG_RECEIPT" | awk '{print $1}')" = "$ORIG_SHA"
mkdir -p "$NEW"
# Snapshot attempt 2 so its untouched state can be proven at the end.
(cd "$ATTEMPT2" && find . -type f -print0 | sort -z | xargs -0 shasum -a 256) > "$NEW/attempt2.before.sha256"
```

## 1. Make N

The script comes from a reviewed commit and is checked by digest. It is not run from the tools checkout, which stays at `7e79458`.

```bash
git -C "$TOOLS" fetch --quiet origin "$SCRIPT_COMMIT"
git -C "$TOOLS" show "$SCRIPT_COMMIT:scripts/neutralize-local-receipt.mjs" > "$NEW/neutralize-local-receipt.mjs"
test "$(shasum -a 256 "$NEW/neutralize-local-receipt.mjs" | awk '{print $1}')" = "$SCRIPT_SHA"
test -z "$(git -C "$TOOLS" status --porcelain --untracked-files=normal)"
```

Write the private replacement map `$NEW/map.json`.

- **`prefixes`:** one entry for **every** absolute prefix that occurs in the original receipt's `commands` and `bundle.location`. Map each one to a placeholder from this fixed list:
  - `<RETAIN>`, `<SOURCE>`, `<TOOLS>`, `<TARGET>`, `<HEAVY_GUARD>`, `<ZFB_NODE>`, `<PNPM_BIN>`
  - `<HOME>`, only as a last resort
- **`operator`** (D3 no): exactly `"zfb owner, local Darwin arm64 collection"`.
- **`forbidden`:** the local user name, the short and long host names, and any other identifying fragment you know of.

The map holds local paths, so it stays private.

```bash
"$ZFB_NODE" "$NEW/neutralize-local-receipt.mjs" \
  --tools "$TOOLS" --receipt "$ORIG_RECEIPT" --expected-sha "$ORIG_SHA" \
  --source-sha "$SOURCE_SHA" --bundle "$BUNDLE" --map "$NEW/map.json" \
  --out "$NEW/receipt-neutral.json" > "$NEW/neutral-report.json" 2> "$NEW/neutralize.log"
N_SHA="$(shasum -a 256 "$NEW/receipt-neutral.json" | awk '{print $1}')"
test "$(shasum -a 256 "$ORIG_RECEIPT" | awk '{print $1}')" = "$ORIG_SHA"
cat "$NEW/neutral-report.json"   # digests, placeholder counts and booleans only
```

The script refuses, and writes nothing, in any of these cases:

- either digest differs
- the schema fails
- a fact other than `commands`, `bundle.location` or `operator` changes
- any listed prefix, `/Users/`, `/private/`, `/Volumes/`, `/home/`, `/root/`, the home directory, user name, host name, a `forbidden` entry, or the original operator text remains

A refusal is a finding. Fix only the map, never the original, and rerun into a new `--out`.

**Owner attestation (D1), on #4314, by the owner.** Post N's digest with this text, and nothing local:

> I attest the receipt with SHA-256 `<N_SHA>` as a new local-native-darwin receipt for the retained bundle `136395016503fb8062e8a902cfc5e11191461325952e0216f09f6ffe0a03c83e`, collected 2026-10-19 at source `bda633b3589e201737b02cf2060f7b24d41a45a7`. It is derived from the private original `cfae5404…` by the placeholder map recorded in the derived report, and is not that original.

## 2. Stage attempt 3

These are the same stager arguments as attempt 2. Only the receipt and the new stage directory differ.

```bash
cd "$TOOLS"
if "$ZFB_NODE" "$TOOLS/scripts/rebaseline-island-size-darwin.mjs" \
  --source-root "$SOURCE" --tool-sha "$TOOL_SHA" --source-sha "$SOURCE_SHA" \
  --artifact "$BUNDLE" --local-receipt "$NEW/receipt-neutral.json" --local-receipt-sha "$N_SHA" \
  --reason cargo-producer-lock --stage "$NEW/stage" \
  --expected-old-cargo-sha 230f0acb80e141c66dc5cfe50d028a840103ea8fcce5ff88250b7b0668e1b138 \
  --expected-new-cargo-sha 563194257102ac98a57ba6561fc0084c35518aaf83e21f68106d07e0c604f5bd \
  --old-cargo-ref 21d6807e442bbed61e578c165a91d64c4a7dc141:Cargo.lock \
  --added-packages rustls@0.23.46 \
  --reviewed-cargo-transition research/v3-island-size/cargo-transitions/rustls-security-4314.json \
  --previous-contract-ref "$SOURCE_SHA" > "$NEW/stage.log" 2>&1; then s=0; else s=$?; fi
printf '%s\n' "$s" > "$NEW/stage.exit-code"; test "$s" = 0
test -z "$(git -C "$SOURCE" status --porcelain --untracked-files=normal)"
test -z "$(git -C "$TOOLS" status --porcelain --untracked-files=normal)"
```

**Leak scan of the publishable staged files.** The scan must print nothing. The manifest is not scanned, because it keeps absolute evidence paths and stays private.

```bash
S="$NEW/stage"
FILES=("$S/research/v3-island-size/README.md" "$S/research/v3-island-size/decision.json" "$S/scripts/__tests__/island-size-budget.test.mjs")
for frag in /Users/ /private/ /Volumes/ /home/ "$HOME" "$(id -un)" "$(hostname -s)" "$(hostname)" "$SOURCE" "$TOOLS" "$NEW" "$BUNDLE"; do
  grep -lF -- "$frag" "${FILES[@]}" && echo "LEAK: a fragment was found (do not print it)"
done
# Also run the same loop once with the original operator text as the fragment.
```

**Consistency with attempt 2.** Only the receipt and the text naming its digest may differ. Every ceiling, allowance and lock line must be identical.

```bash
{ diff "$ATTEMPT2/stage/research/v3-island-size/decision.json" "$S/research/v3-island-size/decision.json" || true; } > "$NEW/decision.a2-a3.diff"
grep -cE '^[<>]' "$NEW/decision.a2-a3.diff" || true   # changed lines
grep -E '^[<>]' "$NEW/decision.a2-a3.diff" | grep -ciE 'ceiling|allowance|cargoLock|cargoTransition|actualGzip' || true   # expect 0
(cd "$S" && shasum -a 256 stage-manifest.json research/v3-island-size/README.md research/v3-island-size/decision.json scripts/__tests__/island-size-budget.test.mjs)
```

**Return through the private channel, and stop here for L1's independent review:**

- `neutral-report.json`, `N_SHA`
- `stage.log`, `stage.exit-code`
- the four digests above and the two diff counts
- the three staged files (scan clean)
- a copy of `stage-manifest.json` with its absolute paths replaced by the same placeholders, labelled *derived*, with its original digest beside it

## 3. Apply attempt 3, after L1's review says go

The apply re-verifies the receipt, the bundle, the clean tools checkout, and a clean checkout at `bda633b`. A new worktree leaves the original `SOURCE` checkout untouched.

```bash
git -C "$SOURCE" worktree add --detach "$APPLY" "$SOURCE_SHA"
cd "$APPLY"
test "$(git rev-parse HEAD)" = "$SOURCE_SHA"; test -z "$(git status --porcelain --untracked-files=normal)"
if "$ZFB_NODE" "$TOOLS/scripts/rebaseline-island-size-darwin.mjs" --apply "$NEW/stage" > "$NEW/apply.log" 2>&1; then a=0; else a=$?; fi
printf '%s\n' "$a" > "$NEW/apply.exit-code"; test "$a" = 0
git status --porcelain --untracked-files=normal   # expect exactly the three destinations, " M"
shasum -a 256 research/v3-island-size/README.md research/v3-island-size/decision.json scripts/__tests__/island-size-budget.test.mjs
(cd "$ATTEMPT2" && find . -type f -print0 | sort -z | xargs -0 shasum -a 256) | diff - "$NEW/attempt2.before.sha256" && echo "attempt 2 untouched"
test "$(shasum -a 256 "$ORIG_RECEIPT" | awk '{print $1}')" = "$ORIG_SHA"
```

**Return:**

- `apply.log`, `apply.exit-code`
- the `git status` lines
- the three digests, which must equal attempt 3's `staged` values
- "attempt 2 untouched"

**No commit, no push.** Keep `$APPLY` until L1 confirms the bytes are integrated.

## Stop conditions (keep everything, report, do not retry in place)

- any precondition, digest or schema check fails
- the neutralizer refuses
- the stager or apply exits non-zero
- the leak scan prints anything
- the attempt-2 diff touches a ceiling, allowance, lock or transition line
- `git status` after the apply shows anything other than the three destinations
- attempt 2 or the original receipt changed
