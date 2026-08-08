#!/usr/bin/env bash
# IronHermes — vault migration hardening wrapper.
#
# Moves EVERY plaintext provider API key on this host into the encrypted
# RustyVault store, then verifies the vault is actually load-bearing. Wraps the
# built-in `ironhermes vault migrate` (which imports from .env ONLY) and closes
# the two gaps it deliberately leaves:
#   - inline `api_key:` literals in config.yaml (precedence #1 — they silently
#     MASK the vault: resolution never falls through to it while they exist)
#   - the plaintext 0600 backups left on disk after migration
#
# Usage:
#   vault-migrate.sh                 # interactive (confirms before each change)
#   vault-migrate.sh --yes           # no prompts (CI / provisioning)
#   vault-migrate.sh --dry-run       # report what would change; change nothing
#   vault-migrate.sh --purge-backups # delete plaintext backups after verify passes
#
# Requirements:
#   - an `ironhermes` binary built with `--features rusty-vault`
#     (release/CI/Docker builds do NOT include it — build from source)
#   - IRONHERMES_HOME (default ~/.ironhermes) with config.yaml
#
# Effects (in order):
#   1. Preflight: locate binary + home, verify config.yaml exists
#   2. Back up config.yaml (0600, timestamped) before any edit
#   3. Ensure the `vault:` block exists in config.yaml (append if missing)
#   4. `ironhermes vault init` (skipped if the keyfile already exists)
#   5. `ironhermes vault migrate` (built-in .env import + scrub)
#   6. Move inline providers.<name>.api_key literals into the vault, then
#      null each one — only after its vault write is confirmed
#   7. Verify: `vault list` + `doctor` vault checks + residual plaintext scan
#   8. Backups: list them (or delete with --purge-backups after verify passes)
#
# Secret hygiene: key VALUES are never echoed, logged, or passed as argv —
# they move via pipe into `ironhermes vault set` (masked/piped prompt, D-03).
set -euo pipefail

# ── flags ────────────────────────────────────────────────────────────────────
YES=false
DRY_RUN=false
PURGE_BACKUPS=false
for arg in "$@"; do
  case "$arg" in
    --yes) YES=true ;;
    --dry-run) DRY_RUN=true ;;
    --purge-backups) PURGE_BACKUPS=true ;;
    -h|--help) sed -n '2,36p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) echo "unknown flag: $arg (see --help)" >&2; exit 2 ;;
  esac
done

# ── helpers ──────────────────────────────────────────────────────────────────
info()  { printf '\033[36m▸\033[0m %s\n' "$*"; }
ok()    { printf '\033[32m✓\033[0m %s\n' "$*"; }
warn()  { printf '\033[33m!\033[0m %s\n' "$*"; }
fail()  { printf '\033[31m✗\033[0m %s\n' "$*" >&2; exit 1; }

confirm() { # confirm "<question>" — true if --yes or user answers y
  $YES && return 0
  $DRY_RUN && return 1
  local reply
  read -r -p "$1 [y/N] " reply
  [[ "$reply" == "y" || "$reply" == "Y" ]]
}

# BSD (macOS) vs GNU sed in-place
sedi() { if sed --version >/dev/null 2>&1; then sed -i "$@"; else sed -i '' "$@"; fi; }

# ── 1. preflight ─────────────────────────────────────────────────────────────
IH_HOME="${IRONHERMES_HOME:-$HOME/.ironhermes}"
CONFIG="$IH_HOME/config.yaml"
ENV_FILE="$IH_HOME/.env"

BIN="${IRONHERMES_BIN:-}"
if [ -z "$BIN" ]; then
  for cand in "$HOME/.local/bin/ironhermes" "$(command -v ironhermes 2>/dev/null || true)" \
              "target/release/ironhermes" "target/debug/ironhermes"; do
    [ -n "$cand" ] && [ -x "$cand" ] && BIN="$cand" && break
  done
fi
[ -n "$BIN" ] && [ -x "$BIN" ] || fail "no ironhermes binary found — set IRONHERMES_BIN or install one"
[ -f "$CONFIG" ] || fail "no config.yaml at $CONFIG — run \`ironhermes setup\` first"

info "home:   $IH_HOME"
info "binary: $BIN"
$DRY_RUN && warn "DRY RUN — no changes will be made"

# ── 2+3. config.yaml vault block (backup before any edit) ───────────────────
TS="$(date -u +%Y%m%dT%H%M%SZ)"
CONFIG_BACKUP="$CONFIG.pre-vault-$TS.bak"

if grep -qE '^vault:' "$CONFIG"; then
  # Block exists — require it to be usable rather than editing nested YAML.
  if awk '/^vault:/{f=1;next} f&&/^[^ ]/{f=0} f' "$CONFIG" | grep -qE 'backend:[[:space:]]*rusty-vault'; then
    ok "config.yaml already has a vault: block with backend: rusty-vault"
  else
    fail "config.yaml has a vault: block but backend is not rusty-vault — edit it manually (see docs/CONFIGURATION.md#vault-vault), then re-run"
  fi
  if ! awk '/^vault:/{f=1;next} f&&/^[^ ]/{f=0} f' "$CONFIG" | grep -qE 'enabled:[[:space:]]*true'; then
    fail "vault: block present but enabled is not true — set vault.enabled: true, then re-run"
  fi
else
  info "config.yaml has no vault: block — will append one (enabled, rusty-vault, keyfile unseal)"
  if ! $DRY_RUN && confirm "Append the vault: block to $CONFIG?"; then
    cp -p "$CONFIG" "$CONFIG_BACKUP" && chmod 600 "$CONFIG_BACKUP"
    ok "config backup: $CONFIG_BACKUP"
    cat >> "$CONFIG" <<'YAML'

vault:
  enabled: true
  backend: rusty-vault
  rusty_vault:
    unseal_mode: keyfile
YAML
    ok "vault: block appended"
  elif $DRY_RUN; then
    info "(dry-run) would back up config.yaml and append the vault: block"
  else
    fail "cannot continue without the vault: block"
  fi
fi

# ── 4. vault init (idempotent: skip when the keyfile already exists) ─────────
# Default data_dir is $IRONHERMES_HOME/vault; the keyfile is its sibling
# <data_dir>.key. Honor an explicit rusty_vault.data_dir override if present.
DATA_DIR="$(awk '/^vault:/{f=1;next} f&&/^[^ ]/{f=0} f&&/data_dir:/{sub(/.*data_dir:[[:space:]]*/,"");gsub(/["'"'"']/,"");print;exit}' "$CONFIG")"
[ -n "$DATA_DIR" ] || DATA_DIR="$IH_HOME/vault"
KEYFILE="${DATA_DIR%.}"; KEYFILE="${KEYFILE%/}.key"

if [ -f "$KEYFILE" ]; then
  ok "vault already initialized (keyfile present: $KEYFILE)"
elif $DRY_RUN; then
  info "(dry-run) would run: ironhermes vault init  (data dir: $DATA_DIR)"
else
  info "initializing vault at $DATA_DIR"
  IRONHERMES_HOME="$IH_HOME" "$BIN" vault init \
    || fail "vault init failed — if the error names the rusty-vault feature, rebuild: cargo build --release --features rusty-vault -p ironhermes-cli"
fi

# ── 5. built-in migrate: .env → vault (backup + scrub handled by the CLI) ────
if $DRY_RUN; then
  if [ -f "$ENV_FILE" ]; then
    # Names only — same match set the CLI uses: 3 built-ins + configured api_key_env names.
    ENV_VARS="$(printf '%s\n' OPENROUTER_API_KEY ANTHROPIC_API_KEY OPENAI_API_KEY; \
                grep -E '^\s*api_key_env:' "$CONFIG" | sed -E 's/.*api_key_env:\s*//; s/["'"'"']//g; s/\s*(#.*)?$//')"
    FOUND="$(grep -oE '^[A-Z0-9_]+' "$ENV_FILE" | grep -Fxf <(printf '%s\n' $ENV_VARS) || true)"
    if [ -n "$FOUND" ]; then
      info "(dry-run) vault migrate would import from .env: $(echo "$FOUND" | tr '\n' ' ')"
    else
      info "(dry-run) vault migrate: no provider keys found in .env"
    fi
  else
    info "(dry-run) vault migrate: no .env file — nothing to import"
  fi
else
  info "running built-in: ironhermes vault migrate (.env import)"
  IRONHERMES_HOME="$IH_HOME" "$BIN" vault migrate
fi

# ── 6. inline providers.<name>.api_key literals → vault, then null ───────────
# These are precedence #1 and MASK the vault; the built-in migrate never touches
# them (D-13 scope fence). Safe-by-ordering: null a line only after its vault
# write is confirmed by `vault list`.
INLINE="$(awk '
  /^providers:[[:space:]]*$/ { inblock=1; next }
  inblock && /^[^[:space:]]/ { inblock=0 }
  inblock && /^  [A-Za-z0-9_-]+:[[:space:]]*$/ {
    prov=$0; sub(/^  /,"",prov); sub(/:.*$/,"",prov); next
  }
  inblock && /^    api_key:[[:space:]]*[^[:space:]]/ {
    val=$0; sub(/^    api_key:[[:space:]]*/,"",val); gsub(/[[:space:]]+$/,"",val)
    if (val != "null" && val != "~" && val != "\"\"" && val != "'"''"'")
      printf "%d\t%s\n", NR, prov
  }' "$CONFIG")"

if [ -z "$INLINE" ]; then
  ok "no inline provider api_key literals in config.yaml"
else
  info "inline provider api_key literals found (values not shown):"
  echo "$INLINE" | while IFS=$'\t' read -r ln prov; do echo "    line $ln: providers.$prov.api_key"; done
  if $DRY_RUN; then
    info "(dry-run) would move each into the vault and null the config line"
  elif confirm "Move these into the vault and null the config lines?"; then
    [ -f "$CONFIG_BACKUP" ] || { cp -p "$CONFIG" "$CONFIG_BACKUP" && chmod 600 "$CONFIG_BACKUP" && ok "config backup: $CONFIG_BACKUP"; }
    while IFS=$'\t' read -r ln prov; do
      # Extract the value at that exact line; strip surrounding quotes. Never echoed.
      val="$(sed -n "${ln}p" "$CONFIG" | sed -E 's/^[[:space:]]*api_key:[[:space:]]*//; s/^["'"'"']//; s/["'"'"'][[:space:]]*$//; s/[[:space:]]+$//')"
      [ -n "$val" ] || { warn "line $ln (providers.$prov): empty after parse — skipped"; continue; }
      # stdout+stderr captured (the piped masked-prompt label prints to stderr);
      # surfaced only on failure so errors are never hidden.
      set_out="$(printf '%s\n' "$val" | IRONHERMES_HOME="$IH_HOME" "$BIN" vault set "$prov" 2>&1)" \
        || { unset val; echo "$set_out" >&2; fail "vault set $prov failed — config line left untouched"; }
      unset val set_out
      IRONHERMES_HOME="$IH_HOME" "$BIN" vault list | grep -qx "$prov" \
        || fail "vault list does not show '$prov' after set — config line left untouched"
      sedi "${ln}s/^\\([[:space:]]*api_key:\\).*/\\1 null/" "$CONFIG"
      ok "providers.$prov.api_key → vault (config line ${ln} nulled)"
    done <<< "$INLINE"
  else
    warn "skipped — inline keys REMAIN precedence #1; the vault will not be consulted for them"
  fi
fi

# Any api_key literal still non-null after the pass above (e.g. legacy
# model.api_key, role overrides) — report only; those live at deprecated
# precedence spots and migrating them needs operator judgment.
OTHER="$(grep -nE '^[[:space:]]*api_key:[[:space:]]*[^[:space:]]' "$CONFIG" | grep -vE 'api_key:[[:space:]]*(null|~)[[:space:]]*$' || true)"
if [ -n "$OTHER" ]; then
  warn "api_key literals still remain in config.yaml (values not shown):"
  echo "$OTHER" | sed -E 's/^([0-9]+):.*/    line \1: api_key: <value hidden>/'
  warn "review these manually — they resolve before the vault and stay readable on disk"
fi

# ── 7. verify ────────────────────────────────────────────────────────────────
$DRY_RUN && { info "(dry-run) done — no changes made"; exit 0; }

info "verifying…"
echo "  vault keys (names only):"
IRONHERMES_HOME="$IH_HOME" "$BIN" vault list | sed 's/^/    /'
echo "  doctor vault checks:"
IRONHERMES_HOME="$IH_HOME" "$BIN" doctor 2>/dev/null | grep -i vault | sed 's/^/    /' || warn "doctor produced no vault lines"

VERIFY_OK=true
if [ -f "$ENV_FILE" ]; then
  ENV_VARS="$(printf '%s\n' OPENROUTER_API_KEY ANTHROPIC_API_KEY OPENAI_API_KEY; \
              grep -E '^\s*api_key_env:' "$CONFIG" | sed -E 's/.*api_key_env:\s*//; s/["'"'"']//g; s/\s*(#.*)?$//')"
  RESIDUAL="$(grep -oE '^[A-Z0-9_]+' "$ENV_FILE" | grep -Fxf <(printf '%s\n' $ENV_VARS) || true)"
  if [ -n "$RESIDUAL" ]; then
    warn "provider keys still present in .env (migrate may have partially failed): $(echo "$RESIDUAL" | tr '\n' ' ')"
    VERIFY_OK=false
  fi
fi
if awk '/^providers:[[:space:]]*$/{f=1;next} f&&/^[^[:space:]]/{f=0} f' "$CONFIG" \
   | grep -qE '^    api_key:[[:space:]]*[^[:space:]]' \
   && awk '/^providers:[[:space:]]*$/{f=1;next} f&&/^[^[:space:]]/{f=0} f' "$CONFIG" \
   | grep -E '^    api_key:[[:space:]]*[^[:space:]]' | grep -qvE '(null|~)[[:space:]]*$'; then
  warn "non-null inline provider api_key literals remain in config.yaml"
  VERIFY_OK=false
fi
$VERIFY_OK && ok "verify passed — no plaintext provider keys left in .env or providers.* config"

# ── 8. plaintext backups ─────────────────────────────────────────────────────
shopt -s nullglob
BACKUPS=("$IH_HOME"/.env.pre-vault-*.bak "$CONFIG".pre-vault-*.bak)
shopt -u nullglob
if [ ${#BACKUPS[@]} -gt 0 ]; then
  if $PURGE_BACKUPS && $VERIFY_OK; then
    rm -f -- "${BACKUPS[@]}"
    ok "purged ${#BACKUPS[@]} plaintext backup file(s)"
  elif $PURGE_BACKUPS; then
    warn "verify FAILED — backups kept despite --purge-backups (you may still need them)"
  else
    warn "plaintext backups remain on disk (0600, but same-user readable):"
    printf '    %s\n' "${BACKUPS[@]}"
    warn "delete them once you've confirmed everything works:  rm ${BACKUPS[*]}"
  fi
fi

# ── done ─────────────────────────────────────────────────────────────────────
ok "vault migration complete"
cat <<'NOTE'

  Next steps:
   - Restart long-running services so they resolve keys from the vault
     (gateway: launchctl kickstart / systemctl --user restart; web UI server).
   - EVERY binary that should read the vault must be built with the feature:
       cargo build --release --features rusty-vault -p ironhermes-cli
       cargo build --release --features server,rusty-vault -p iron_hermes_ui
   - Back up the vault data dir AND its 0600 keyfile together; without the
     keyfile the vault cannot be unsealed.
NOTE
