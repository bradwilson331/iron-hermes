#!/usr/bin/env bash
# Phase 36.17.5 — TTS functions UAT.
#
# Behavioral verification that the post-Wave-4 codebase ships a working
# Edge TTS synthesis path, audio_cache dir creation, provider registry,
# and CLI/Local playback arm. Designed to be run locally after all four
# plans (36.17.5-01 through 36.17.5-04) have been applied.
#
# Decisions covered (from .planning/phases/36.17.5-.../36.17.5-CONTEXT.md):
#   D-03  — Built-in provider lineup v1 = Edge TTS + ElevenLabs
#   D-04  — ffmpeg optional, opt-in; Edge falls back to sendAudio if absent
#   D-05  — Tool-only (Path B); no auto-speak hook
#   D-08  — Trait/registry in ironhermes-core; impls in ironhermes-tools
#   D-10  — BUILTIN_TTS_NAMES built-ins-always-win invariant
#   D-15  — send_audio dispatches on SessionKey platform
#   D-16  — audio_cache under $IRONHERMES_HOME/audio_cache/<uuid>.<ext>
#
# Gates 1-5 wired in PLAN 04. PLAN 01 ships skeleton + banner echo + helpers.
#
# Exit 0 on all-pass; non-zero on any fail. Mirrors scripts/uat/phase-36.17-web-logging.sh
# conventions (shebang, set -euo pipefail, SCRIPT_DIR/WORKSPACE_ROOT, gate echo
# pattern, GATE FAIL exit pattern).
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WORKSPACE_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"
cd "${WORKSPACE_ROOT}"

IRONHERMES_HOME="$(mktemp -d -t ironhermes-36.17.5-XXXXXX)"
export IRONHERMES_HOME
cleanup() { rm -rf "${IRONHERMES_HOME}"; }
trap cleanup EXIT

echo "==> Phase 36.17.5 TTS UAT — IRONHERMES_HOME=${IRONHERMES_HOME}"
echo "==> Gate 0: banner echo only (PLAN 01 skeleton — gates 1-5 wired in PLAN 04)"
echo "    OK"
echo

echo "==> Phase 36.17.5 TTS UAT: skeleton green (gates 1-5 pending PLAN 04 wiring)"
exit 0
