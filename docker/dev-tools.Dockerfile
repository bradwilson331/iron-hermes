# syntax=docker/dockerfile:1
# check=skip=InvalidDefaultArgInFrom
#
# dev-tools variant (D-22): an OPT-IN toolchain layer for the Telegram
# site-deploy flow. It adds git, gh, Node 22 + npm/npx and a pinned wrangler
# on top of ANY ironhermes base image. The base image stays lean: the
# repo-root Dockerfile is not touched and the default build still ships no
# git, gh, node, npm or wrangler.
#
# Build (BOTH build args are required; neither has a default, so a missing one
# fails the build loudly):
#
#   podman build --build-arg WRANGLER_VERSION=4.140.0 -f docker/dev-tools.Dockerfile \
#     --build-arg BASE_IMAGE=<ironhermes image> \
#     -t <tag>-dev-tools .
#
# The build context needs no repo files. No USER, ENTRYPOINT or ENV is set
# here: the base image's entrypoint, gosu privilege drop and IRONHERMES_HOME
# are inherited unchanged.
#
# approved-wrangler: 4.140.0
# (pinned at the package-legitimacy checkpoint: published 2026-09-25,
#  engines.node >=22, repo github.com/cloudflare/workers-sdk, no postinstall)

# Node source. The digest is the multi-arch INDEX digest (linux/amd64 and
# linux/arm64 both listed), so the same pin serves the amd64 CI publish and
# arm64 local builds. Node 22 is copied from the official image because Debian
# trixie ships Node 20 and wrangler >= 4.100 needs Node >= 22; both are
# Debian trixie, so glibc matches.
ARG NODE_IMAGE=docker.io/library/node:22-trixie-slim@sha256:154ba2f4d6fec323d28e4f4bb86bba4677f1223391a1979cf521304e03a98dfa
# No default on purpose: a missing BASE_IMAGE must fail the build. The only
# honest default would be a digest, and the base digest is produced by the same
# CI run that builds this file (docker-publish.yml passes it), so a static one
# would go stale and silently layer on an old base. The `check=skip` directive
# on line 2 silences BuildKit's InvalidDefaultArgInFrom warning for this
# deliberate choice; an unset BASE_IMAGE still fails the build.
ARG BASE_IMAGE

FROM ${NODE_IMAGE} AS node_source

FROM ${BASE_IMAGE}

# Runs as root, like the base image's apt layer.
COPY --from=node_source /usr/local/bin/node /usr/local/bin/node
COPY --from=node_source /usr/local/lib/node_modules /usr/local/lib/node_modules
RUN ln -s ../lib/node_modules/npm/bin/npm-cli.js /usr/local/bin/npm && \
    ln -s ../lib/node_modules/npm/bin/npx-cli.js /usr/local/bin/npx

RUN apt-get update && \
    apt-get install -y --no-install-recommends git gh ca-certificates && \
    rm -rf /var/lib/apt/lists/*

# No default on purpose: pinned at the legitimacy checkpoint, never `latest`.
ARG WRANGLER_VERSION
RUN test -n "$WRANGLER_VERSION" && \
    npm install -g "wrangler@${WRANGLER_VERSION}" && \
    npm cache clean --force

# Build-time probe: a broken toolchain fails the build.
RUN git --version && gh --version && node --version && npm --version && wrangler --version
RUN case "$(node --version)" in v22.*) ;; *) exit 1 ;; esac
