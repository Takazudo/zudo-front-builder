# Smoke-test image for the node-free distribution path — TypeScript config variant.
#
# No Node, no Rust, no pnpm — only curl, tar, sha256sum, ca-certificates.
# This image validates that zfb.config.ts is evaluated via the in-process V8
# evaluator (Sub 3 #417) without any Node runtime on PATH.
#
# Build args
# ----------
# ZFB_SMOKE_MODE   local    (default) — use the pre-built binary that the CI
#                            workflow stages at tests/smoke/node-free-ts/zfb-bin
#                            before calling docker build.  Used in PR / merge
#                            CI where a fresh binary was just compiled.
#                  released — run install.sh to fetch the latest released
#                            binary from GitHub Releases over HTTPS.  Used in
#                            the post-release smoke job after release.yml
#                            completes.  The CI workflow stages a placeholder
#                            file at zfb-bin so COPY always succeeds.
#
# Docker build context: tests/smoke/node-free-ts/
#   zfb-bin   real binary (local mode) or placeholder (released mode)
#   run.sh    assertion script
#
# Released binary targets GLIBC 2.35 (ubuntu-22.04 baseline). Smoke runs against
# bookworm (Debian 12, GLIBC 2.36) to lock that floor in.
FROM debian:bookworm-slim

ARG ZFB_SMOKE_MODE=local

RUN apt-get update -qq && \
    apt-get install -y --no-install-recommends \
        ca-certificates \
        curl \
        tar \
    && rm -rf /var/lib/apt/lists/*

# Ensure ~/.local/bin is on PATH (install.sh places the binary there by default).
ENV PATH="/root/.local/bin:${PATH}"

# Stage the binary file from the build context.  In local mode this is the
# real compiled binary; in released mode the workflow provides a placeholder
# so that COPY succeeds, and install.sh then replaces it below.
COPY zfb-bin /root/.local/bin/zfb

RUN if [ "$ZFB_SMOKE_MODE" = "released" ]; then \
        # released mode: fetch and install from the public GitHub Releases URL.
        # This validates the exact curl-pipe path that end users run.
        curl -fsSL https://raw.githubusercontent.com/Takazudo/zudo-front-builder/main/install.sh | sh; \
    fi

RUN chmod +x /root/.local/bin/zfb

# Smoke assertion script: scaffold → swap config to TS → build → assert → dev → curl.
COPY run.sh /smoke/run.sh
RUN chmod +x /smoke/run.sh

CMD ["/smoke/run.sh"]
