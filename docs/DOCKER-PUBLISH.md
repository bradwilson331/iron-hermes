# Docker Publish Workflow

`.github/workflows/docker-publish.yml` builds the IronHermes container image,
gates it, signs it and publishes it to GitHub Container Registry.

## What it publishes

- **Images:** `ghcr.io/<owner>/<repo>` (on the public GitHub mirror,
  `ghcr.io/bradwilson331/iron-hermes`) and the opt-in `-dev-tools` variant
  (`...:latest-dev-tools`, `...:<version>-dev-tools`) that adds the git and gh
  CLIs, Node 22 and a pinned wrangler on top of the default image.
- **Platform:** `linux/amd64` only.
- **Tags:** only `v*` tag pushes get tags: `latest`, the release tag itself
  (`v1.2.3`) and its semver form (`1.2.3`). Tags are applied as the **last**
  step, after every gate below has passed. Images are first pushed by digest
  only, so a failed gate leaves no `latest` or version tag behind.
- **Build:** the root `Dockerfile` (multi-stage: `rust:1.96-trixie` builder ->
  `debian:trixie-slim` runtime, both pinned to a multi-arch index digest; the
  entrypoint drops to uid 10000, `IRONHERMES_HOME=/opt/data` volume, port 8080).
  The default image carries Chromium for the browser tools (54.11 D-15 was
  reversed); the `-dev-tools` variant adds Node and wrangler on top of it.
  GitHub Actions layer cache (`type=gha`) is used for both read and write.
- **Labels:** `org.opencontainers.image.*` from `docker/metadata-action`, with
  `licenses` forced to `MIT AND AGPL-3.0-only` (the image ships the AGPL
  `ironhermes-tirith` sidecar; the repo licence alone would say MIT).

## Release gates and signing

After both images are pushed (by digest, untagged), the workflow runs these
steps in order. Each one must pass before the next starts:

1. **Dockle** (`scripts/container-lint.sh --image <ref>@<digest>`) on both
   digests. Ignores live in `.dockleignore`, each with a justification.
2. **Hardened-container smoke** (`scripts/tests/container_hardened_smoke.sh`) on
   both digests, with `IH_SMOKE_RT=docker` so a real rootful
   `docker compose up` runs on the Linux runner. The step fails unless the log
   has `INFO runtime=docker` and `PASS compose_up`, so a skipped compose check
   cannot slip through.
3. **cosign sign** (keyless, by digest) for both images.
4. **cosign verify** of both digests against this workflow's identity.
5. **Promote tags** (`docker buildx imagetools create`) onto the signed digests.

Steps 3-5 run only for `v*` tag pushes. A manual `workflow_dispatch` is a
gates-only dry run: it never signs and never tags. An unsigned digest therefore
means a failed gate or a dry run; do not deploy it. Hadolint,
the digest-pin check and the other static checks run earlier, in CI's
`scripts/ci-gates.sh` (Gate 11).

## Verify at pull

Signatures are keyless (Sigstore), so there is no key to distribute. Verify the
digest you are about to run, for both tags:

```bash
for tag in latest latest-dev-tools; do
  cosign verify "ghcr.io/bradwilson331/iron-hermes:${tag}" \
    --certificate-identity-regexp '^https://github\.com/bradwilson331/iron-hermes/\.github/workflows/docker-publish\.yml@refs/tags/v.*$' \
    --certificate-oidc-issuer https://token.actions.githubusercontent.com
done
```

Prefer pinning the verified digest (`...@sha256:<digest>`) in your compose file
over a moving tag. Sliplane pulls cannot run cosign, so verify the digest on a
workstation first, then point Sliplane at that exact `@sha256:` reference.

## Triggers

| Trigger | When |
|---------|------|
| `push` on tags `v*` | Cutting a release tag (e.g. `v0.9.0`) |
| `workflow_dispatch` | Manual gates-only dry run from the Actions tab: builds and gates, never signs or tags |

Ordinary branch pushes do **not** trigger a build.

## Auth

Logs in to GHCR with the workflow's own `GITHUB_TOKEN`
(`permissions: packages: write`). No personal access token or repo secret is
required. `id-token: write` is granted for keyless cosign signing; the OIDC
token is used only by the cosign steps, which run only on `v*` tag refs; the
workflow itself runs only on `v*` tags and manual dispatch.

## Remote caveats

- This repository's GitHub remote (`origin-github`) is the **scrubbed showcase
  mirror** produced by `build-showcase.sh` — raw `develop` is never pushed
  there. Release tags intended to publish an image must be cut on the showcase
  branch and pushed to GitHub, where Actions runs.
- The private Gitea remotes (`origin`, `origin2`) do not run this workflow
  unless Gitea Actions is enabled; pushing branches there is inert with respect
  to image publishing.

## Cutting a release

```bash
./build-showcase.sh                 # produce/update the scrubbed showcase branch
git tag v<version> <showcase-sha>
git push origin-github v<version>   # triggers the build, gates, signing and GHCR push
```

Then verify (above) and pull the image with:

```bash
docker compose up -d   # docker-compose.yml is the hardened default run
```
