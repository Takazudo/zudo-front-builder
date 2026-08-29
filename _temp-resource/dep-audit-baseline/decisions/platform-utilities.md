# Platform/process utility dependency decisions

Audit of issue #2751 against the checked-out code and resolved dependency metadata. All three terminal verdicts are KEEP; no dependency is removed.

## `local-ip-address = "0.6"`

**Terminal verdict: KEEP.**

Evidence:

- `crates/zfb/src/output.rs:203-209` enables the network URL section only for an unspecified bind host. `collect_network_urls` at `crates/zfb/src/output.rs:215-233` calls `local_ip_address::list_afinet_netifas()`, consumes the returned interface vector, and formats one URL for every non-loopback `IpAddr::V4`. This is all-interface enumeration, not primary-address selection.
- `output.rs` has no UDP-connect/local-address path. A UDP-connect trick selects one socket's primary egress address, so it cannot preserve the current one-URL-per-interface behavior for machines with multiple LAN/VPN interfaces.
- The resolved `local-ip-address` implementation has platform-specific interface enumeration (Unix `getifaddrs`, Windows adapter enumeration), and the repository ships `packages/zfb-win32-x64-msvc`. However, CI's Windows leg in `.github/workflows/release.yml` builds and packages the binary only; Rust test/nextest runs are on the Linux health lane and macOS exam lane. No Windows Rust coverage exists to validate a replacement's OS-specific behavior.

The strict contrary gate is not met: there is no replacement that both preserves all-interface enumeration and is actually exercised by Windows Rust coverage.

## `wait-timeout = "0.2"`

**Terminal verdict: KEEP.**

Evidence:

- `crates/zfb-build/src/adapter.rs:41` imports `wait_timeout::ChildExt`; `crates/zfb-build/src/adapter.rs:329-345` calls `child.wait_timeout(TIMEOUT)`, then kills and waits for the child when the timeout returns `None`, producing a bounded failure instead of hanging a build.
- `cargo tree -p zfb-build --edges normal` resolves `wait-timeout v0.2.1` directly under `zfb-build`; its only target-gated support dependency on this Unix host is `libc`, with no extra application-level transitive dependency. The crate exposes both Unix and Windows implementations.
- Replacing this with a polling loop would change the process-wait primitive and is an immediate abandon trigger under the issue's process-supervision gate; it is not an equivalent safe simplification.

The strict contrary gate is not met. KEEP the maintained cross-platform bounded wait.

## `npm-run-all2 = "^7.0.2"`

**Terminal verdict: KEEP.**

Evidence:

- `docs/package.json:10` defines `dev` as `run-p dev:zfb dev:history`; `docs/package.json:13` defines `dev:network` as `run-p dev:zfb:network dev:history`. Both commands supervise two long-running processes: zfb and the history server.
- This is process supervision, not merely command sequencing. A replacement must forward Ctrl-C/SIGINT (and termination signals), propagate child exit status, and clean up both children when one exits or the user stops the dev loop. The issue's signal-supervision abandon rule applies immediately to an unproven replacement.

The strict contrary gate is not met: no replacement has been demonstrated to preserve signal forwarding, exit-code propagation, and child cleanup. KEEP.

## Checks

- Inspected the three call sites, package scripts, manifests, lockfile, resolved crate platform implementations, and CI workflow invocations.
- Ran targeted `cargo tree` dependency inspection and `git diff --check`; no build or full test suite is warranted because this change only records dependency rationale and removes no code.
