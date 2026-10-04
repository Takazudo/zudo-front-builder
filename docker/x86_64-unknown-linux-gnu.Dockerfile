# Build image for the x86_64-unknown-linux-gnu release binary (issue #3623).
#
# Used by cross-rs via Cross.toml ([target.x86_64-unknown-linux-gnu.dockerfile]).
# cross mounts the host's rustup toolchain, cargo registry, and the workspace
# into this container, so the image only has to provide the C/C++ link step:
#   - gcc      → `cc`, the default linker driver for the x86_64 target
#   - gcc-c++  → libstdc++ devel; rusty_v8's build.rs emits
#                `cargo:rustc-link-lib=dylib=stdc++` on Linux
#
# Why Amazon Linux 2023: its glibc is 2.34, so a binary linked here can
# reference at most GLIBC_2.34 and therefore runs on Amazon Linux 2023 (the
# default AWS Amplify Hosting / CodeBuild image), RHEL 9 / Rocky 9 / Alma 9,
# Ubuntu 22.04+, and Debian 12+. The previous ubuntu-22.04 runner build
# required GLIBC_2.35, which excluded every glibc-2.34 distro (#3584).
#
# Why not lower: the published arm64 binary (built on an Ubuntu 20.04 cross
# image) measures GLIBC_2.30 as its highest required symbol, so the embedded
# V8 prebuilt rules out manylinux_2_28 (glibc 2.28) outright, and every
# glibc 2.30–2.33 distro base (Ubuntu 20.04, Debian 11) is past end of life.
# AL2023 is supported until 2028 and keeps glibc at 2.34 for its lifetime.
#
# Pinned by digest for reproducibility (the `2023` tag is rolling). To roll
# forward:
#   docker pull amazonlinux:2023
#   docker inspect --format='{{index .RepoDigests 0}}' amazonlinux:2023
# then confirm `docker run --rm amazonlinux:2023 ldd --version` still reports
# 2.34 and update the digest below. The GLIBC floor assertion in release.yml
# and node-free-smoke.yml fails the build if the ceiling ever moves.
#
# rusty_v8's build.rs downloads the prebuilt static archive with deno, then
# python3, then curl; this image has neither deno nor python3, so the curl
# fallback (curl-minimal + ca-certificates ship in the base image) is the one
# that runs. zfb's own build.rs stages esbuild through reqwest/rustls and
# needs nothing from the image.
FROM amazonlinux:2023@sha256:5b29412077a463b4a3a8fbc99a8cdf4b929f38a3ecc8dac10328d8f36b0099b8

RUN dnf install -y -q gcc gcc-c++ && dnf clean all && rm -rf /var/cache/dnf
