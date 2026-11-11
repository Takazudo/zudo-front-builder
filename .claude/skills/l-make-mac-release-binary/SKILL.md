---
description: "Owner-Mac phase of a zfb release: build and qualify BOTH Darwin targets (aarch64 native, x86_64 Rosetta-translated) from the exact release source with scripts/build-darwin-local.sh, and upload the ten Darwin inputs (archives, CSS addons, checksums, qualification receipts) to the existing DRAFT GH Release without clobbering. Follows RELEASING.md's Phase M. Mac only; never publishes the Release. Triggers on \"make mac binary\", \"build mac release binary\", \"upload mac binary\", \"darwin release inputs\"."
user-invocable: true
argument-description: "Required: the tag (e.g. v6.1.0). Optional: the source SHA (defaults to the draft Release's target commit)."
---

# /l-make-mac-release-binary

**Follow [`RELEASING.md`](../../../RELEASING.md) "Phase M: Darwin inputs on the owner's Mac"
exactly.** This skill is only its Claude Code entry point. The contract (assets, receipt schema,
admission rules) is [`scripts/darwin-artifact-contract.md`](../../../scripts/darwin-artifact-contract.md).

## Steps

1. Require `uname -s` = `Darwin` and `sysctl -n hw.optional.arm64` = `1`; otherwise stop — a non-Mac
   host can only consume uploaded inputs.
2. Resolve the source: `gh release view <tag> --json isDraft,targetCommitish`. The Release must be a
   **draft**; its `targetCommitish` is the source SHA unless one was given (they must agree).
3. Check out that SHA detached in a clean checkout, create the **local** tag `<tag>` at it (never
   push it), and run, through the heavy guard when present:

   ```sh
   ZFB_X64_NODE=/absolute/path/to/x64/node CARGO_BUILD_JOBS=1 \
     scripts/build-darwin-local.sh --source <sha> --upload <tag>
   ```

   Run with `--check-only` first if unsure about the binding. Never pass or emulate `--clobber`;
   on a "DIFFERENT bytes" refusal, stop and report — the owner decides.
4. Report the uploaded asset names, each receipt's `host.arch_label` (`native-arm64`,
   `rosetta-translated` — never call x86_64 "native Intel"), `qualification.status`, every check's
   first result, and `node scripts/release/release-status.mjs --tag <tag>`'s recommendation.

Do not publish the Release and do not add the provenance-downgrade marker unless the owner
explicitly does so; publishing continues from `RELEASING.md` A11 in any environment.
