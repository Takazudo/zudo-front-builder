//! Structured CLI output helpers (logging, progress, error formatting).
//!
//! This module is a small toolkit used by the `zfb` command implementations
//! (`dev`, `build`, `preview`, `new`). It centralises:
//!
//! * Coloured status logging (`info`, `success`, `warn`, `error`, `ready`).
//! * A thin progress-bar wrapper around [`indicatif::ProgressBar`]
//!   ([`BuildProgress`]).
//! * User-friendly multi-line error rendering for `anyhow::Error`
//!   ([`format_error`]).
//!
//! All colour output respects the `NO_COLOR` convention. Internally, the
//! formatting helpers delegate to [`owo_colors`] with the `supports-colors`
//! feature enabled, so when the relevant stream is not a TTY, or `NO_COLOR`
//! is set, no ANSI escape codes are emitted.
//!
//! The helpers print to `stdout` for informational output (`info`,
//! `success`, `ready`) and to `stderr` for `warn` / `error`, matching
//! conventional Unix CLI behaviour.

use std::net::IpAddr;
use std::time::Duration;

use indicatif::{ProgressBar, ProgressStyle};
use owo_colors::{OwoColorize, Stream};

// ---------------------------------------------------------------------------
// Status logging
// ---------------------------------------------------------------------------

/// Format an informational status line ("info <msg>") with a dim cyan label.
///
/// Pure formatter used internally by [`info`] and exercised by the unit tests.
fn fmt_info(msg: &str) -> String {
    let label = "info".if_supports_color(Stream::Stdout, |t| t.cyan().to_string());
    format!("{} {}", label, msg)
}

/// Format a success status line with a green checkmark prefix.
fn fmt_success(msg: &str) -> String {
    let mark = "✓".if_supports_color(Stream::Stdout, |t| t.green().to_string());
    format!("{} {}", mark, msg)
}

/// Format a warning status line with a yellow `⚠` prefix.
fn fmt_warn(msg: &str) -> String {
    let mark = "⚠".if_supports_color(Stream::Stderr, |t| t.yellow().to_string());
    format!("{} {}", mark, msg)
}

/// Format an error status line with a red `✗` prefix.
fn fmt_error(msg: &str) -> String {
    let mark = "✗".if_supports_color(Stream::Stderr, |t| t.red().to_string());
    format!("{} {}", mark, msg)
}

/// Format a "ready" line that highlights a server URL.
fn fmt_ready(url: &str) -> String {
    let arrow = "→".if_supports_color(Stream::Stdout, |t| t.green().to_string());
    let url_styled = url.if_supports_color(Stream::Stdout, |t| t.bold().to_string());
    format!("{} ready on {}", arrow, url_styled)
}

/// Format the loud "watcher may be dead" warning for a `TimedOut`
/// watcher-liveness self-check verdict (issue #1718, epic #1716).
///
/// This is FSEvents-motivated (a stalled macOS FSEvents stream silently
/// killing hot-reload is the reported symptom, issue #1687), but the
/// self-check itself is backend-agnostic — it runs the identical handshake
/// against inotify on Linux — so the wording names the hot-reload
/// consequence plus the common real-world causes rather than blaming
/// FSEvents specifically. `pub(crate)` (not private, unlike the other
/// `fmt_*` helpers above) because `commands::dev` reuses this exact text
/// as the decision output of its own pure verdict-to-warning mapping — see
/// `commands::dev::watcher_liveness_warning_for`.
pub(crate) fn fmt_watcher_liveness_timed_out() -> String {
    "hot-reload looks dead on this machine: the dev server's own watcher \
     self-check did not observe a filesystem change within its deadline, \
     so editing pages/, content/, components/, styles/, or src/ may not \
     trigger a rebuild until you restart `zfb dev`.\n  Common causes: a \
     stalled fseventsd (macOS — try `sudo killall fseventsd`), a \
     Dropbox/OneDrive/iCloud-synced project directory intercepting file \
     events, or antivirus/EDR software hooking filesystem calls."
        .to_string()
}

/// Print an informational status message to `stdout`.
///
/// Output: `info <msg>` with the `info` label rendered in cyan when colours
/// are supported. Empty messages are rendered as just the prefix.
pub fn info(msg: impl AsRef<str>) {
    println!("{}", fmt_info(msg.as_ref()));
}

/// Print a success message to `stdout`, prefixed with a green `✓`.
pub fn success(msg: impl AsRef<str>) {
    println!("{}", fmt_success(msg.as_ref()));
}

/// Print a warning message to `stderr`, prefixed with a yellow `⚠`.
pub fn warn(msg: impl AsRef<str>) {
    eprintln!("{}", fmt_warn(msg.as_ref()));
}

/// Print an error message to `stderr`, prefixed with a red `✗`.
///
/// For full multi-line error chains, prefer [`format_error`] and pipe the
/// result through this helper or `eprintln!`.
pub fn error(msg: impl AsRef<str>) {
    eprintln!("{}", fmt_error(msg.as_ref()));
}

/// Print a "ready" notification announcing a server URL, e.g. for `zfb dev`.
///
/// Output style: `→ ready on <url>` with the arrow in green and the URL in
/// bold when colours are supported.
///
/// This helper is intentionally kept for callers that already know the exact
/// final URL (e.g. `zfb preview`, which hardcodes `localhost` and has no
/// `--host` flag — preview is intentionally untouched by the unspecified-host
/// banner logic because preview cannot bind to unspecified hosts today; the
/// helper below (`ready_with_interfaces`) is structured so a future preview
/// `--host` flag can adopt it with no change to this function).
// Kept for `zfb preview` and future `--host` adoption; currently only called from tests.
#[allow(dead_code)]
pub fn ready(url: &str) {
    println!("{}", fmt_ready(url));
}

/// Format a multi-line "ready" banner for the unspecified-host case.
///
/// Pure formatter: accepts a precomputed `network_urls` slice so the rendering
/// logic can be unit-tested without any OS-level interface enumeration.
///
/// Output shape (following Vite's convention):
/// ```text
/// → ready
///   Local:    http://localhost:PORT/
///   Network:  http://192.168.x.x:PORT/
///             http://10.x.x.x:PORT/
/// ```
fn fmt_ready_multi(scheme: &str, port: u16, network_urls: &[String]) -> String {
    let arrow = "→".if_supports_color(Stream::Stdout, |t| t.green().to_string());
    let local_url = format!("{}://localhost:{}/", scheme, port);
    let local_styled = local_url.if_supports_color(Stream::Stdout, |t| t.bold().to_string());
    let mut out = format!("{} ready\n", arrow);
    out.push_str(&format!("  Local:    {}\n", local_styled));
    if network_urls.is_empty() {
        out.push_str("  Network:  (none)\n");
    } else {
        for (i, url) in network_urls.iter().enumerate() {
            let url_styled = url.if_supports_color(Stream::Stdout, |t| t.bold().to_string());
            if i == 0 {
                out.push_str(&format!("  Network:  {}\n", url_styled));
            } else {
                out.push_str(&format!("            {}\n", url_styled));
            }
        }
    }
    out
}

/// Return `true` when `host` represents an unspecified (bind-all) address.
///
/// Handles `0.0.0.0`, `::`, and the URL-bracketed form `[::]`.
fn host_is_unspecified(host: &str) -> bool {
    // Strip URL brackets from IPv6 addresses (e.g. `[::]` → `::`).
    let stripped = host.strip_prefix('[').and_then(|s| s.strip_suffix(']'));
    let addr_str = stripped.unwrap_or(host);
    addr_str
        .parse::<IpAddr>()
        .map(|ip| ip.is_unspecified())
        .unwrap_or(false)
}

/// Print a "ready" banner for `zfb dev`, choosing single-line or multi-line
/// output depending on whether the bound host is an unspecified address.
///
/// - When `host` is `localhost`, `127.0.0.1`, or any other specific address,
///   this falls through to the same `→ ready on <url>` single-line shape as
///   `ready()`, so existing UX is unchanged.
/// - When `host` is `0.0.0.0` or `::` (bind-all), non-loopback IPv4 interfaces
///   are enumerated from the OS and printed as `Network:` entries alongside a
///   `Local: http://localhost:PORT/` entry (following Vite's convention).
pub fn ready_with_interfaces(scheme: &str, host: &str, port: u16) {
    if host_is_unspecified(host) {
        let network_urls = collect_network_urls(scheme, port);
        print!("{}", fmt_ready_multi(scheme, port, &network_urls));
    } else {
        println!("{}", fmt_ready(&format!("{}://{}:{}", scheme, host, port)));
    }
}

/// Enumerate non-loopback IPv4 interfaces and build a URL for each.
fn collect_network_urls(scheme: &str, port: u16) -> Vec<String> {
    match local_ip_address::list_afinet_netifas() {
        Ok(interfaces) => interfaces
            .into_iter()
            .filter_map(|(_name, ip)| {
                // IPv4-only for now — IPv6 URL form requires bracketing
                // (`http://[fe80::1]:PORT/`) which adds surface area; IPv4
                // covers the common LAN/VPN use-case Vite targets.
                if let IpAddr::V4(v4) = ip {
                    if !v4.is_loopback() {
                        return Some(format!("{}://{}:{}/", scheme, v4, port));
                    }
                }
                None
            })
            .collect(),
        Err(_) => Vec::new(),
    }
}

// ---------------------------------------------------------------------------
// Progress
// ---------------------------------------------------------------------------

/// Progress-bar wrapper used by long-running multi-step commands such as
/// `zfb build`.
///
/// Wraps [`indicatif::ProgressBar`] with a project-standard template and
/// emits a green summary line on completion.
// Intended for `zfb build` progress reporting; not yet wired up in the build command.
#[allow(dead_code)]
pub struct BuildProgress {
    bar: ProgressBar,
}

#[allow(dead_code)]
impl BuildProgress {
    /// Create a new progress bar for `total` units of work.
    ///
    /// The bar uses a stable template (`[bar] pos/len msg`) so that callers
    /// just need to call [`BuildProgress::inc`] for each completed unit.
    pub fn new(total: u64) -> Self {
        let bar = ProgressBar::new(total);
        // The template is intentionally kept simple; if it fails to parse
        // (which shouldn't happen given the literal string), we fall back
        // to indicatif's default style.
        if let Ok(style) =
            ProgressStyle::with_template("{spinner:.cyan} [{bar:30.green/dim}] {pos}/{len} {msg}")
        {
            bar.set_style(style.progress_chars("=> "));
        }
        Self { bar }
    }

    /// Advance the bar by one step and update the trailing message.
    pub fn inc(&self, label: &str) {
        self.bar.set_message(label.to_string());
        self.bar.inc(1);
    }

    /// Finish the bar and print a one-line summary of the form
    /// `✓ N pages built in X.XXs`.
    pub fn finish_and_summary(self, count: u64, elapsed: Duration) {
        self.bar.finish_and_clear();
        println!("{}", fmt_summary(count, elapsed));
    }
}

/// Render the build summary line. Extracted so it can be unit-tested without
/// touching `stdout`.
// Private helper for BuildProgress::finish_and_summary; only reached via tests until build uses it.
#[allow(dead_code)]
fn fmt_summary(count: u64, elapsed: Duration) -> String {
    let mark = "✓".if_supports_color(Stream::Stdout, |t| t.green().to_string());
    format!(
        "{} {} pages built in {:.2}s",
        mark,
        count,
        elapsed.as_secs_f64()
    )
}

// ---------------------------------------------------------------------------
// Error formatting
// ---------------------------------------------------------------------------

/// Render an [`anyhow::Error`] as a multi-line, user-friendly block.
///
/// Output shape:
///
/// ```text
/// error: <top-level message>
///   caused by: <next>
///   caused by: <next>
///   at <file>:<line>          // only when a location hint is found
/// ```
///
/// The location hint is a best-effort heuristic: any chain entry whose text
/// contains a `path:line` token (e.g. `config.toml:12`) contributes the
/// first such token as a final `at` line. This mirrors the Epic 6 quality
/// bar of "tell users where to look" without imposing a specific error type.
pub fn format_error(err: &anyhow::Error) -> String {
    // First chance: when the chain has a [`FramedError`] anywhere, emit
    // the framed snippet block instead of the legacy chain-only shape.
    // This is what surfaces fully-qualified diagnostics for the four
    // error classes covered by `crate::diagnostics`.
    if let Some(framed) = err
        .chain()
        .filter_map(|c| c.downcast_ref::<crate::diagnostics::FramedError>())
        .next()
    {
        // Render the framed snippet, but also include any anyhow
        // `.context(...)` messages from the rest of the chain so the
        // caller's "while doing X" breadcrumbs aren't dropped. Dedupe
        // by exact string equality so we don't repeat the framed
        // header / its contained line.
        let framed_text = crate::diagnostics::render_framed(&framed.0);
        let mut seen = std::collections::HashSet::new();
        for line in framed_text.lines() {
            seen.insert(line.to_string());
        }
        let mut out = framed_text;
        let mut extra: Vec<String> = Vec::new();
        for cause in err.chain() {
            // Skip the FramedError itself — its content is already
            // rendered above.
            if cause
                .downcast_ref::<crate::diagnostics::FramedError>()
                .is_some()
            {
                continue;
            }
            let msg = format!("{cause}");
            if seen.insert(msg.clone()) {
                extra.push(msg);
            }
        }
        if !extra.is_empty() {
            if !out.ends_with('\n') {
                out.push('\n');
            }
            for msg in extra {
                out.push_str("  caused by: ");
                out.push_str(&msg);
                out.push('\n');
            }
        }
        return out;
    }

    let mut chain = err.chain();
    let head = match chain.next() {
        Some(e) => e,
        // anyhow guarantees at least one error in the chain, but be defensive.
        None => return String::from("error: <empty error chain>\n"),
    };
    let mut out = format!("error: {}\n", head);
    for cause in chain {
        out.push_str(&format!("  caused by: {}\n", cause));
    }
    if let Some(hint) = location_hint(err) {
        out.push_str(&format!("  at {}\n", hint));
    }
    out
}

/// Walk the error chain searching for the first `path:line` looking token.
///
/// Heuristic: split each chain entry's `Display` form on whitespace and
/// surrounding punctuation, then accept a token only when:
///
/// 1. It contains exactly one trailing `:<digits>` segment.
/// 2. The left-hand side looks like a file path — i.e. it contains a `/`
///    or `\` separator, **or** it ends with a file-extension-like component
///    (a `.` followed by 1–8 ASCII alphanumeric characters with at least
///    one letter, e.g. `.toml`, `.rs`).
/// 3. The token does **not** contain `://` (filters out URLs such as
///    `http://localhost:8080`).
///
/// This intentionally rejects host-port pairs (`localhost:8080`),
/// IP-port pairs (`127.0.0.1:8080`), and IPv6 forms (`[::1]:8080`) so that
/// run-time error messages mentioning addresses are not mis-rendered as
/// source locations.
fn location_hint(err: &anyhow::Error) -> Option<String> {
    for cause in err.chain() {
        let text = cause.to_string();
        for raw in text.split(|c: char| c.is_whitespace() || matches!(c, '(' | ')' | ',' | ';')) {
            let token = raw.trim_matches(|c: char| matches!(c, '"' | '\'' | '`' | '.'));
            if token.contains("://") {
                continue;
            }
            let Some((path, line)) = token.rsplit_once(':') else {
                continue;
            };
            if path.is_empty() {
                continue;
            }
            if line.parse::<u32>().is_err() {
                continue;
            }
            if !looks_like_file_path(path) {
                continue;
            }
            return Some(format!("{}:{}", path, line));
        }
    }
    None
}

/// Decide whether `path` plausibly refers to a file rather than a network
/// host or numeric address. See [`location_hint`] for the full heuristic.
fn looks_like_file_path(path: &str) -> bool {
    if path.contains('/') || path.contains('\\') {
        return true;
    }
    // Last `.`-separated segment must look like a file extension:
    // 1–8 ASCII alphanumerics with at least one letter.
    let Some(ext) = path.rsplit('.').next() else {
        return false;
    };
    if ext.is_empty() || ext == path {
        // No `.` in the token — not a filename-with-extension.
        return false;
    }
    if ext.len() > 8 {
        return false;
    }
    ext.chars().all(|c| c.is_ascii_alphanumeric()) && ext.chars().any(|c| c.is_ascii_alphabetic())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// `owo_colors::set_override` / `unset_override` are process-wide globals, so
/// tests that toggle them race under the parallel test harness (zfb#963).
/// Every override-touching test (here and in `crate::diagnostics`) takes this
/// lock for its whole body before the first toggle.
#[cfg(test)]
pub(crate) mod color_override_lock {
    use std::sync::{Mutex, MutexGuard, PoisonError};

    static LOCK: Mutex<()> = Mutex::new(());

    pub(crate) fn lock() -> MutexGuard<'static, ()> {
        // A test that panicked while holding the lock poisons it; the global
        // override state is still consistent enough for the next test (each
        // test sets it explicitly first), so recover instead of cascading.
        LOCK.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::{anyhow, Context};

    /// Strip ANSI escape sequences (CSI ... m) from a string for assertions
    /// that should be colour-agnostic.
    fn strip_ansi(s: &str) -> String {
        let mut out = String::with_capacity(s.len());
        let mut chars = s.chars().peekable();
        while let Some(c) = chars.next() {
            if c == '\u{1b}' && chars.peek() == Some(&'[') {
                chars.next(); // consume '['
                for inner in chars.by_ref() {
                    if inner == 'm' {
                        break;
                    }
                }
            } else {
                out.push(c);
            }
        }
        out
    }

    #[test]
    fn format_error_renders_caused_by_chain() {
        let err: anyhow::Error = Err::<(), _>(anyhow!("disk read failure"))
            .context("loading frontmatter")
            .context("rendering page")
            .unwrap_err();

        let rendered = format_error(&err);
        assert!(
            rendered.starts_with("error: rendering page\n"),
            "got: {rendered}"
        );
        assert!(
            rendered.contains("  caused by: loading frontmatter\n"),
            "got: {rendered}"
        );
        assert!(
            rendered.contains("  caused by: disk read failure\n"),
            "got: {rendered}"
        );
    }

    #[test]
    fn format_error_surfaces_path_line_hint() {
        let err: anyhow::Error = Err::<(), _>(anyhow!("invalid value at config.toml:12"))
            .context("parsing zfb.toml")
            .unwrap_err();

        let rendered = format_error(&err);
        assert!(
            rendered.contains("  at config.toml:12\n"),
            "expected location hint in:\n{rendered}"
        );
    }

    #[test]
    fn format_error_does_not_treat_urls_as_locations() {
        let err: anyhow::Error = Err::<(), _>(anyhow!("connection refused: http://localhost:8080"))
            .context("starting dev server")
            .unwrap_err();

        let rendered = format_error(&err);
        assert!(
            !rendered.contains("  at "),
            "URL should not be surfaced as a location:\n{rendered}"
        );
    }

    #[test]
    fn format_error_does_not_treat_host_port_pairs_as_locations() {
        // localhost:8080 (no path separator, no extension), 127.0.0.1:8080
        // (numeric "extension"), and [::1]:8080 (IPv6 + port) must all be
        // ignored so we do not invent a phantom file location.
        for snippet in [
            "bind failed on localhost:8080",
            "address 127.0.0.1:8080 already in use",
            "ipv6 [::1]:8080 unreachable",
        ] {
            let err: anyhow::Error = Err::<(), _>(anyhow!("{snippet}"))
                .context("dev server")
                .unwrap_err();
            let rendered = format_error(&err);
            assert!(
                !rendered.contains("  at "),
                "snippet {snippet:?} produced a location line:\n{rendered}"
            );
        }
    }

    #[test]
    fn format_error_uses_framed_renderer_when_chain_carries_framed_error() {
        let _color_lock = crate::output::color_override_lock::lock();
        owo_colors::set_override(false);
        let diag = crate::diagnostics::Diagnostic::with_source(
            "posts/intro.md",
            2,
            8,
            "kaboom",
            "line 1\nline 2 has the bug\nline 3\n",
        );
        let err: anyhow::Error =
            anyhow::Error::new(crate::diagnostics::FramedError(diag)).context("rendering page");

        let rendered = format_error(&err);
        // Framed renderer used → starts with the framed header.
        assert!(rendered.starts_with("error: kaboom\n"), "got:\n{rendered}");
        assert!(
            rendered.contains(" --> posts/intro.md:2:8\n"),
            "got:\n{rendered}"
        );
        // anyhow `.context(...)` messages are also surfaced — they
        // were silently dropped before. The framed body does not
        // already contain "rendering page", so it must show up under
        // the standard `caused by:` shape.
        assert!(
            rendered.contains("caused by: rendering page"),
            "expected context line preserved, got:\n{rendered}",
        );
    }

    #[test]
    fn format_error_omits_hint_when_no_location() {
        let err: anyhow::Error = Err::<(), _>(anyhow!("plain failure"))
            .context("higher-level context")
            .unwrap_err();

        let rendered = format_error(&err);
        assert!(
            !rendered.contains("  at "),
            "did not expect a location line in:\n{rendered}"
        );
    }

    #[test]
    fn status_helpers_do_not_panic_on_empty_strings() {
        // The public helpers print to stdout/stderr; we only need to confirm
        // they execute without panicking on empty input.
        info("");
        success("");
        warn("");
        error("");
        ready("");
    }

    #[test]
    fn fmt_watcher_liveness_timed_out_names_hot_reload_and_common_causes() {
        let text = fmt_watcher_liveness_timed_out();
        let lower = text.to_lowercase();
        assert!(text.contains("hot-reload"), "got: {text}");
        assert!(text.contains("restart `zfb dev`"), "got: {text}");
        assert!(lower.contains("fseventsd"), "got: {text}");
        assert!(lower.contains("dropbox"), "got: {text}");
        assert!(
            lower.contains("antivirus") || lower.contains("edr"),
            "got: {text}"
        );
    }

    #[test]
    fn build_progress_zero_total_summary_does_not_panic() {
        let p = BuildProgress::new(0);
        p.finish_and_summary(0, Duration::ZERO);
    }

    #[test]
    fn fmt_summary_renders_expected_text() {
        // Colour codes vary depending on environment, so strip them before
        // asserting on the underlying text shape.
        let line = strip_ansi(&fmt_summary(0, Duration::ZERO));
        assert_eq!(line, "✓ 0 pages built in 0.00s");

        let line = strip_ansi(&fmt_summary(7, Duration::from_millis(1234)));
        assert_eq!(line, "✓ 7 pages built in 1.23s");
    }

    // -----------------------------------------------------------------------
    // Tests for the multi-line banner (issue #499 / #487)
    // -----------------------------------------------------------------------

    #[test]
    fn host_is_unspecified_recognises_all_ipv4() {
        assert!(host_is_unspecified("0.0.0.0"));
    }

    #[test]
    fn host_is_unspecified_recognises_ipv6_bare() {
        assert!(host_is_unspecified("::"));
    }

    #[test]
    fn host_is_unspecified_recognises_ipv6_bracketed() {
        assert!(host_is_unspecified("[::]"));
    }

    #[test]
    fn host_is_unspecified_returns_false_for_specific_hosts() {
        for h in ["localhost", "127.0.0.1", "192.168.1.1", "::1"] {
            assert!(
                !host_is_unspecified(h),
                "expected false for {:?}, got true",
                h
            );
        }
    }

    /// fmt_ready_multi with a mock network_urls slice — no real network state.
    #[test]
    fn fmt_ready_multi_renders_expected_shape() {
        let _color_lock = crate::output::color_override_lock::lock();
        owo_colors::set_override(false);
        let network_urls = vec![
            "http://192.168.1.10:3000/".to_string(),
            "http://10.0.0.5:3000/".to_string(),
        ];
        let rendered = fmt_ready_multi("http", 3000, &network_urls);
        let plain = strip_ansi(&rendered);

        assert!(plain.contains("→ ready\n"), "header missing: {plain:?}");
        assert!(
            plain.contains("  Local:    http://localhost:3000/\n"),
            "local URL missing: {plain:?}"
        );
        assert!(
            plain.contains("  Network:  http://192.168.1.10:3000/\n"),
            "first network URL missing: {plain:?}"
        );
        assert!(
            plain.contains("            http://10.0.0.5:3000/\n"),
            "second network URL (continuation indent) missing: {plain:?}"
        );

        owo_colors::unset_override();
    }

    /// When network_urls is empty, the banner should still render gracefully.
    #[test]
    fn fmt_ready_multi_handles_no_network_interfaces() {
        let _color_lock = crate::output::color_override_lock::lock();
        owo_colors::set_override(false);
        let rendered = fmt_ready_multi("http", 8080, &[]);
        let plain = strip_ansi(&rendered);

        assert!(plain.contains("→ ready\n"), "header missing: {plain:?}");
        assert!(
            plain.contains("  Local:    http://localhost:8080/\n"),
            "local URL missing: {plain:?}"
        );
        assert!(
            plain.contains("  Network:  (none)\n"),
            "none placeholder missing: {plain:?}"
        );

        owo_colors::unset_override();
    }

    /// NO_COLOR suppression: owo-colors override=false means no ANSI in output.
    #[test]
    fn fmt_ready_multi_no_color_suppresses_ansi() {
        let _color_lock = crate::output::color_override_lock::lock();
        owo_colors::set_override(false);
        let network_urls = vec!["http://192.168.1.1:3000/".to_string()];
        let rendered = fmt_ready_multi("http", 3000, &network_urls);
        assert!(
            !rendered.contains('\u{1b}'),
            "expected no ANSI when colour override is off: {rendered:?}"
        );
        owo_colors::unset_override();
    }

    /// With colour override on, ANSI escapes must be present.
    #[test]
    fn fmt_ready_multi_with_color_emits_ansi() {
        let _color_lock = crate::output::color_override_lock::lock();
        owo_colors::set_override(true);
        let network_urls = vec!["http://192.168.1.1:3000/".to_string()];
        let rendered = fmt_ready_multi("http", 3000, &network_urls);
        assert!(
            rendered.contains('\u{1b}'),
            "expected ANSI when colour override is on: {rendered:?}"
        );
        owo_colors::unset_override();
    }

    #[test]
    fn color_override_toggle_controls_ansi_output() {
        // owo-colors exposes a process-wide override. We toggle it explicitly
        // to verify the formatting helpers honour it (which is the same code
        // path consulted when NO_COLOR is set in the environment).
        let _color_lock = crate::output::color_override_lock::lock();
        owo_colors::set_override(true);
        let coloured = fmt_info("hello");
        assert!(
            coloured.contains('\u{1b}'),
            "expected ANSI escape when colour override is enabled, got: {coloured:?}"
        );

        owo_colors::set_override(false);
        let plain = fmt_info("hello");
        assert!(
            !plain.contains('\u{1b}'),
            "expected no ANSI escape when colour override is disabled, got: {plain:?}"
        );
        assert_eq!(plain, "info hello");

        // Restore default behaviour for any subsequent tests.
        owo_colors::unset_override();
    }
}
