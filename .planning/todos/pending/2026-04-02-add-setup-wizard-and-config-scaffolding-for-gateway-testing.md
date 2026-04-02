---
created: 2026-04-02T14:08:44.764Z
title: Add setup wizard and config scaffolding for gateway testing
area: tooling
files:
  - crates/ironhermes-cli/src/main.rs
  - /Users/twilson/code/hermes-agent/setup-hermes.sh
---

## Problem

Phase 2 (Telegram Gateway) is code-complete but the human smoke test requires manual config setup: TELEGRAM_BOT_TOKEN env var, OPENROUTER_API_KEY, and a `~/.ironhermes/config.yaml` with gateway.platforms.telegram fields (enabled, token, whitelist with user ID). There's no automated way to scaffold this config or guide the user through setup.

The original hermes-agent has a `setup-hermes.sh` script that handles: dependency installation, environment file creation from template, PATH setup, skills syncing, and an interactive setup wizard (`hermes setup`). IronHermes needs an equivalent that covers:

1. **Config file scaffolding** — create `~/.ironhermes/config.yaml` with sensible defaults and placeholder fields for API keys and gateway settings
2. **Setup wizard** — interactive CLI command (`ironhermes setup`) that prompts for API keys, Telegram bot token, user ID for whitelist
3. **Environment validation** — `ironhermes doctor` already exists but needs to check gateway-specific requirements (bot token set, whitelist non-empty, LLM API key valid)
4. **.env template** — provide a `.env.example` with all supported env vars documented

Without this, every new developer (and the Phase 2 smoke test) requires manually reading docs and hand-crafting config files.

## Solution

Reference hermes-agent's `setup-hermes.sh` for the UX pattern. For IronHermes (Rust):

- Add `Setup` subcommand to CLI that interactively prompts for required config values
- Generate `~/.ironhermes/config.yaml` from a template with user-provided values
- Extend `Doctor` subcommand to validate gateway prerequisites
- Create `.env.example` in repo root documenting all env vars
- Consider a `setup-ironhermes.sh` shell script for first-time clone experience (cargo build + config scaffold)
