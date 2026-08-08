#!/usr/bin/env bash
# Phase 36.17.2.2 — Telegram media delivery live UAT fixture setup.
# Creates the /tmp/uat-* files referenced in telegram_media_uat.md Prerequisites.
#
# Usage:
#   bash crates/ironhermes-gateway/tests/uat-setup-36.17.2.2.sh           # create
#   bash crates/ironhermes-gateway/tests/uat-setup-36.17.2.2.sh --clean   # remove
#   bash crates/ironhermes-gateway/tests/uat-setup-36.17.2.2.sh --verify  # check only
#
# All fixtures land at /tmp/uat-* which is the canonical allowed-root the
# MediaSender impl accepts by default (no IRONHERMES_HOME required).

set -euo pipefail

PHOTO=/tmp/uat-photo.png
VOICE=/tmp/uat-voice.ogg
MUSIC=/tmp/uat-music.mp3
DOC=/tmp/uat-doc.pdf
OVERSIZE=/tmp/uat-oversize.png

FIXTURES=("$PHOTO" "$VOICE" "$MUSIC" "$DOC" "$OVERSIZE")

log() { printf '[uat-setup] %s\n' "$*"; }
warn() { printf '[uat-setup] WARN: %s\n' "$*" >&2; }

verify() {
  local missing=0
  for f in "${FIXTURES[@]}"; do
    if [ -f "$f" ]; then
      printf '  %s  %s\n' "OK " "$(ls -lh "$f" | awk '{print $5"\t"$NF}')"
    else
      printf '  %s  %s\n' "MISS" "$f"
      missing=$((missing + 1))
    fi
  done
  return "$missing"
}

clean() {
  log "Removing UAT fixtures from /tmp"
  for f in "${FIXTURES[@]}"; do
    rm -f "$f" && log "  removed $f" || warn "could not remove $f"
  done
}

find_first_existing() {
  # Echo the first path that exists, or nothing.
  for p in "$@"; do
    if [ -f "$p" ]; then echo "$p"; return 0; fi
  done
  return 1
}

create_photo() {
  local src
  src=$(find_first_existing \
    "$HOME/Pictures/uat-photo.png" \
    "$HOME/Desktop/uat-photo.png" \
    "/System/Library/Desktop Pictures/Solid Colors/Black.png" \
    "/System/Library/Desktop Pictures/Hello.heic" \
  ) || true

  if [ -n "${src:-}" ]; then
    cp "$src" "$PHOTO"
    log "photo: copied from $src"
  elif command -v curl >/dev/null; then
    log "photo: no local source found, fetching from gstatic"
    curl -s -f -o "$PHOTO" https://www.gstatic.com/webp/gallery/1.webp \
      || { warn "curl failed; falling back to first PNG in ~/Pictures"; \
           find "$HOME/Pictures" -iname "*.png" -type f -print -quit 2>/dev/null \
             | xargs -I{} cp {} "$PHOTO"; }
  else
    warn "no curl and no local PNG — Scenario 2 will fail"
    return 1
  fi
}

create_voice() {
  if ! command -v ffmpeg >/dev/null; then
    warn "ffmpeg not installed — skipping voice .ogg (Scenario 3 will fail at stat or upload)"
    warn "  install: brew install ffmpeg"
    return 0
  fi
  ffmpeg -hide_banner -loglevel error \
    -f lavfi -i "sine=frequency=440:duration=1" \
    -c:a libopus -b:a 24k "$VOICE" -y
  log "voice: synthesized 1s 440Hz Opus → $VOICE"
}

create_music() {
  if ! command -v ffmpeg >/dev/null; then
    warn "ffmpeg not installed — skipping audio .mp3 (Scenario 4 will fail at stat or upload)"
    return 0
  fi
  ffmpeg -hide_banner -loglevel error \
    -f lavfi -i "sine=frequency=440:duration=1" \
    -c:a libmp3lame -b:a 64k -metadata title="UAT 440Hz" "$MUSIC" -y
  log "music: synthesized 1s 440Hz MP3 → $MUSIC"
}

create_doc() {
  local src
  src=$(find_first_existing \
    "$HOME/Documents/uat-doc.pdf" \
    "$HOME/Desktop/uat-doc.pdf" \
  ) || true

  if [ -n "${src:-}" ]; then
    cp "$src" "$DOC"
    log "doc: copied from $src"
    return 0
  fi

  # Find any PDF on the user's machine as fallback
  local any
  any=$(find "$HOME/Documents" "$HOME/Desktop" -maxdepth 3 -iname "*.pdf" -type f -print -quit 2>/dev/null || true)
  if [ -n "$any" ]; then
    cp "$any" "$DOC"
    log "doc: copied from $any"
  else
    # Minimal PDF — Telegram accepts but renders blank
    printf '%%PDF-1.4\n1 0 obj<</Type/Catalog/Pages 2 0 R>>endobj\n2 0 obj<</Type/Pages/Kids[]/Count 0>>endobj\nxref\n0 3\n0000000000 65535 f\n0000000010 00000 n\n0000000053 00000 n\ntrailer<</Size 3/Root 1 0 R>>\nstartxref\n95\n%%%%EOF\n' > "$DOC"
    log "doc: wrote minimal PDF stub → $DOC (will render blank in Telegram)"
  fi
}

create_oversize() {
  # 21 MiB > 20 MiB photo cap → forces D-15 size pre-check
  dd if=/dev/zero of="$OVERSIZE" bs=1m count=21 status=none 2>/dev/null \
    || dd if=/dev/zero of="$OVERSIZE" bs=1M count=21 status=none
  log "oversize: 21 MiB → $OVERSIZE (trips D-15 size cap)"
}

main() {
  case "${1:-}" in
    --clean)
      clean
      exit 0
      ;;
    --verify)
      log "Verifying fixtures"
      verify && log "All fixtures present" \
            || warn "Some fixtures missing — re-run without --verify to create"
      exit $?
      ;;
    -h|--help)
      sed -n '2,15p' "$0"
      exit 0
      ;;
    "")
      ;;
    *)
      warn "Unknown flag: $1 (use --clean | --verify | --help)"
      exit 2
      ;;
  esac

  log "Creating UAT fixtures for Phase 36.17.2.2"
  create_photo    || warn "photo creation reported failure"
  create_voice
  create_music
  create_doc
  create_oversize

  log "Done. Verifying:"
  if verify; then
    log "All fixtures present — ready for live UAT"
    log "Open: crates/ironhermes-gateway/tests/telegram_media_uat.md"
  else
    warn "Some fixtures missing — re-run after installing ffmpeg / providing sources"
    exit 1
  fi
}

main "$@"
