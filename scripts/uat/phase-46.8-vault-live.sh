#!/usr/bin/env bash
# Phase 46.8 — live UAT: vault-backed provider-key resolution + sealed-startup hard error.
#
# Proves the ONE thing automation can't: a real LLM completion through the live
# embedded server resolves the provider key FROM THE VAULT (key absent everywhere
# else), and a sealed/broken enabled vault makes the server FAIL STARTUP LOUDLY.
#
# Uses a THROWAWAY IRONHERMES_HOME — your real ~/.ironhermes is never touched.
# Run the numbered blocks yourself (the server runs in the foreground; you drive
# the completion in a browser). Don't `bash` the whole file top-to-bottom.
set -uo pipefail
cd "$(git -C "${0:A:h}" rev-parse --show-toplevel 2>/dev/null || echo /Users/you/code/ironhermes)"

# ─────────────────────────────────────────────────────────────────────────────
# SET THESE
PROVIDER="anthropic"                 # the provider whose key you'll move into the vault
                                     # (must match a provider in your config: anthropic|openai|openrouter|…)
TEST_HOME="/tmp/ih-vault-uat"        # throwaway home (NOT ~/.ironhermes)
BIN="target/debug/ironhermes"        # the CLI (already built with the feature)
# ─────────────────────────────────────────────────────────────────────────────

# ── 1. Preflight: port 8080 must be free (a stale orphan would answer instead) ──
lsof -iTCP:8080 -sTCP:LISTEN && { echo "!! Port 8080 in use — kill the holder(s) above first"; return 2>/dev/null || exit 1; } || echo "✓ port 8080 free"

# ── 2. Fresh test home; reuse YOUR model/provider config, then enable the vault ──
rm -rf "$TEST_HOME"; mkdir -p "$TEST_HOME"
cp ~/.ironhermes/config.yaml "$TEST_HOME/config.yaml"    # reuse your real model + provider setup
# CRITICAL: null out EVERY inline `api_key:` literal in the copied config. An inline key is
#   precedence #1 — it beats env AND the vault — so leaving even one in means the completion
#   resolves FROM CONFIG, not the vault: a silent false-positive (this exact trap bit the first
#   UAT). This rewrites `api_key: <value>` → `api_key: null` and never touches `api_key_env:`.
#   With env scrubbed (step 4/5) + empty .env, the vault becomes the ONLY key source.
sed -i '' -E 's/^([[:space:]]*)api_key:[[:space:]]*[^[:space:]].*$/\1api_key: null/' "$TEST_HOME/config.yaml"
_leftover=$(grep -nE '^[[:space:]]*api_key:[[:space:]]*\S' "$TEST_HOME/config.yaml" | grep -v 'api_key: null' || true)
[ -n "$_leftover" ] && echo "!! non-null inline api_key remains (will mask the vault): $_leftover" \
                    || echo "✓ inline api_key literals nulled — resolution will use vault/env only"
cat >> "$TEST_HOME/config.yaml" <<'YAML'

vault:
  enabled: true
  backend: rusty-vault
  rusty_vault:
    unseal_mode: keyfile
YAML
: > "$TEST_HOME/.env"                                    # empty .env → no key leaks in via dotenvy
echo "✓ test home ready: $TEST_HOME"

# ── 3. Init the vault and put your REAL key in it (masked prompt — never in argv/history) ──
# The CLI's `vault` subcommands need the rusty-vault backend COMPILED IN, so build
# the CLI with the feature first (without it, `vault set` hard-errors: backend not built).
cargo build -p ironhermes-cli --features rusty-vault
IRONHERMES_HOME="$TEST_HOME" "$BIN" vault init
IRONHERMES_HOME="$TEST_HOME" "$BIN" vault set "$PROVIDER"    # paste your real key at the masked prompt
IRONHERMES_HOME="$TEST_HOME" "$BIN" vault list              # expect: $PROVIDER
IRONHERMES_HOME="$TEST_HOME" "$BIN" doctor | grep -i vault  # expect: enabled / backend rusty-vault / unsealed

# ── 4. PRIMARY test — a real completion straight from the TERMINAL (no browser, no dx).
#      As of the NF-2 close-out, the CLI's `build_client` factory (used by `-e`/single,
#      `chat`, and `gateway`) applies the vault fallback, so `hermes -e` resolves the
#      provider key FROM THE VAULT. This is the simplest, most reliable proof — it avoids
#      the web-asset bundle AND the dx dev-proxy WebSocket path entirely.
#
#   Key vars scrubbed so resolution MUST fall through to the vault. `$BIN` was built with
#   `--features rusty-vault` in step 3 (required for the rusty-vault backend to open).
#   (unset the legacy key vars AND, if your provider uses a custom `api_key_env`, add `-u THAT_VAR`)
env -u ANTHROPIC_API_KEY -u OPENAI_API_KEY -u OPENROUTER_API_KEY \
    IRONHERMES_HOME="$TEST_HOME" \
    "$BIN" --provider "$PROVIDER" -e "Reply with exactly the word: vault-ok"
#   PASS (check 1): you get a real completion (e.g. "vault-ok") → the key was resolved
#     FROM THE VAULT (it exists nowhere else in the env).
#   FAIL: "no api key configured" / auth error.

# ── 5. SEALED-VAULT hard-error: break the vault, re-run the same command ──
mv "$TEST_HOME/vault.key" "$TEST_HOME/vault.key.bak"     # remove the unseal keyfile → vault can't open
env -u ANTHROPIC_API_KEY -u OPENAI_API_KEY -u OPENROUTER_API_KEY \
    IRONHERMES_HOME="$TEST_HOME" \
    "$BIN" --provider "$PROVIDER" -e "Reply with exactly the word: vault-ok"
#   PASS (check 2): the command FAILS LOUDLY — build_client's vault fallback hard-errors
#     naming `ironhermes vault init`/unlock (D-07) and never runs keyless.
#   FAIL: it completes anyway / "no api key configured" with no vault error.
mv "$TEST_HOME/vault.key.bak" "$TEST_HOME/vault.key"     # restore the keyfile

# ── 6. OPTIONAL — same thing through the WEB UI (if you want the browser experience) ──
#   The web server (AppState::init) is also vault-wired. It MUST be served via `dx` — a raw
#   `cargo run -p iron_hermes_ui` boots the server but serves a BLANK PAGE (no built wasm
#   bundle). And do NOT use `ironhermes gateway` for a web page — it's a Telegram bot.
#
#   IMPORTANT: invoke dx by ABSOLUTE PATH. `env … dx` does a raw PATH lookup that BYPASSES
#   your `alias dx=~/.cargo/bin/dx`, so it would hit the OTHER `dx` on your PATH
#   (/opt/homebrew/bin/dx, an npm `serve` wrapper) → "unknown option --package". The cargo
#   binary is the Dioxus one:
DX="$HOME/.cargo/bin/dx"
#   `dx serve` auto-enables the `server` feature; we only ADD `rusty-vault` (a
#   cfg(not(wasm32)) dep → no-op on the wasm client build, verified to compile clean).
env -u ANTHROPIC_API_KEY -u OPENAI_API_KEY -u OPENROUTER_API_KEY \
    IRONHERMES_HOME="$TEST_HOME" \
    "$DX" serve --package iron_hermes_ui --features rusty-vault
#   Open the URL dx prints (fullstack default http://localhost:8080) and send a message.
#   Same PASS/FAIL as check 1. If it won't STREAM but the server log shows a clean boot past
#   AppState::init (no "vault is not initialized" panic), that's the known dx-dev-proxy
#   WebSocket quirk, not a vault failure.

# ── 7. Cleanup ──
rm -rf "$TEST_HOME"
