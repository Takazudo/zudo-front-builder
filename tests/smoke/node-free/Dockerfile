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
#                  released — run install.sh (fetched at ZFB_INSTALL_SOURCE_SHA)
#                            to fetch the released binary from GitHub Releases
#                            over HTTPS.  Used in
#                            the post-release smoke job after release.yml
#                            completes.  The CI workflow stages a placeholder
#                            file at zfb-bin so COPY always succeeds.
#
# Docker build context: tests/smoke/node-free/
#   zfb-bin   real binary (local mode) or placeholder (released mode)
#   run.sh    assertion script
#
# Released binary targets GLIBC 2.34 (linked inside an Amazon Linux 2023 cross
# container, #3623). Smoke runs against bookworm (Debian 12, GLIBC 2.36) for
# the Debian/Ubuntu path; the floor itself is proven by the amazonlinux:2023
# smoke job in node-free-smoke.yml. Forward compatibility to newer GLIBC is a
# property of glibc itself — no separate trixie smoke needed.
FROM debian:bookworm-slim

ARG ZFB_SMOKE_MODE=local

# ZFB_VERSION pins the release tag install.sh resolves in released mode. CI passes
# the just-released tag (workflow_run.head_branch) so the smoke installs that exact
# release's assets. Required because every release so far is a pre-release, and
# install.sh's default path (GET /releases/latest) excludes pre-releases → 404.
# Empty in local mode, where the released branch below is skipped.
ARG ZFB_VERSION=

# ZFB_INSTALL_SOURCE_SHA is the full 40-hex release source commit the workflow
# checked out (#4784). Released mode fetches install.sh at exactly that commit,
# never a branch or tag, and prints one zfb-installer-bytes/v1 record of the
# bytes it then executes. scripts/verify-release-delivery.mjs reads this file
# at the release source to decide that the record is required.
ARG ZFB_INSTALL_SOURCE_SHA=

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

RUN set -eu; \
    if [ "$ZFB_SMOKE_MODE" = "released" ]; then \
        # released mode: fetch and install from the public GitHub URL. This
        # validates the curl-then-sh path that end users run. ZFB_VERSION pins
        # the tag so install.sh downloads that exact release's assets.
        case "$ZFB_INSTALL_SOURCE_SHA" in \
            *[!0-9a-f]* | "") echo "ZFB_INSTALL_SOURCE_SHA must be the 40-hex release source commit (got '${ZFB_INSTALL_SOURCE_SHA}')" >&2; exit 1 ;; \
        esac; \
        if [ "${#ZFB_INSTALL_SOURCE_SHA}" -ne 40 ]; then \
            echo "ZFB_INSTALL_SOURCE_SHA must be the 40-hex release source commit (got '${ZFB_INSTALL_SOURCE_SHA}')" >&2; exit 1; \
        fi; \
        install_script="$(mktemp)"; \
        curl -fsSL \
            --retry 5 \
            --retry-delay 2 \
            --retry-connrefused \
            --retry-all-errors \
            -o "$install_script" \
            "https://raw.githubusercontent.com/Takazudo/zudo-front-builder/${ZFB_INSTALL_SOURCE_SHA}/install.sh"; \
        install_digest="$(sha256sum "$install_script" | cut -d ' ' -f 1)"; \
        echo "zfb-installer-bytes/v1 source=${ZFB_INSTALL_SOURCE_SHA} path=install.sh sha256=${install_digest}"; \
        ZFB_VERSION="$ZFB_VERSION" sh "$install_script"; \
        rm -f "$install_script"; \
    fi

RUN chmod +x /root/.local/bin/zfb

# Smoke assertion script: scaffold → build → assert → dev → curl.
COPY run.sh /smoke/run.sh
RUN chmod +x /smoke/run.sh

CMD ["/smoke/run.sh"]
