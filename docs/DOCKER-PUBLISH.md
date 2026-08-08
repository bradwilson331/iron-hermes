# Docker Publish Workflow

`.github/workflows/docker-publish.yml` builds the IronHermes container image and
publishes it to GitHub Container Registry.

## What it publishes

- **Image:** `ghcr.io/<owner>/<repo>` — on the public GitHub mirror this resolves
  to `ghcr.io/bradwilson331/iron-hermes`.
- **Platform:** `linux/amd64` only.
- **Tags:** every run pushes `latest`; tag-triggered runs additionally push the
  git tag itself (`v1.2.3`) and its semver form (`1.2.3`).
- **Build:** the root `Dockerfile` (multi-stage: `rust:latest` builder →
  `debian:bookworm-slim` runtime, non-root UID 10000, `IRONHERMES_HOME=/opt/data`
  volume, port 8080). GitHub Actions layer cache (`type=gha`) is used for both
  read and write.

## Triggers

| Trigger | When |
|---------|------|
| `push` on tags `v*` | Cutting a release tag (e.g. `v0.9.0`) |
| `workflow_dispatch` | Manual run from the Actions tab |

Ordinary branch pushes do **not** trigger a build.

## Auth

Logs in to GHCR with the workflow's own `GITHUB_TOKEN`
(`permissions: packages: write`). No personal access token or repo secret is
required.

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
git push origin-github v<version>   # triggers the build + GHCR push
```

Then pull the image with:

```bash
docker pull ghcr.io/bradwilson331/iron-hermes:latest
docker run -v ironhermes-data:/opt/data ghcr.io/bradwilson331/iron-hermes:latest
```
