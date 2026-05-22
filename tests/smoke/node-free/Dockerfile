# Smoke-test image for the node-free distribution path.
#
# No Node, no Rust, no pnpm — only curl, tar, sha256sum, ca-certificates.
# The image validates that zfb works as a self-contained binary: scaffold,
# build, and serve a node-free site without any JavaScript toolchain.
#
# Build args
# ----------
# ZFB_SMOKE_MODE   local    (default) — use the pre-built binary that the CI
#                            workflow stages at tests/smoke/node-free/zfb-bin
#                            before calling docker build.  Used in PR / merge
#                            CI where a fresh binary was just compiled.
#                  released — run install.sh to fetch the latest released
#                            binary from GitHub Releases over HTTPS.  Used in
#                            the post-release smoke job after release.yml
#                            completes.  The CI workflow stages a placeholder
#                            file at zfb-bin so COPY always succeeds.
#
# Docker build context: tests/smoke/node-free/
#   zfb-bin   real binary (local mode) or placeholder (released mode)
#   run.sh    assertion script
#
# Base image — must satisfy the binary's glibc requirement.  CI builds the zfb
# binary on ubuntu-latest (currently 24.04), which links against GLIBC 2.39.
# debian:bookworm-slim (Debian 12) ships GLIBC 2.36 and is therefore too old
# (zfb errors with "libc.so.6: version `GLIBC_2.39' not found").  trixie-slim
# (Debian 13, current stable as of 2025-06) ships GLIBC 2.41 and matches.
# If the release.yml builders are downgraded to ubuntu-22.04 for broader user
# glibc compatibility, this base can be rolled back to bookworm-slim.
FROM debian:trixie-slim

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

# Smoke assertion script: scaffold → build → assert → dev → curl.
COPY run.sh /smoke/run.sh
RUN chmod +x /smoke/run.sh

CMD ["/smoke/run.sh"]
