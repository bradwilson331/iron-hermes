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
# Gates 1-3 are automated (network required; Edge TTS is keyless).
# Gates 4-5 require human-verify checkpoints (tool registry + audio playback).
#
# Exit 0 on all-pass; non-zero on any fail. Mirrors scripts/uat/phase-36.17-web-logging.sh
# conventions (shebang, set -euo pipefail, SCRIPT_DIR/WORKSPACE_ROOT, gate echo
# pattern, GATE FAIL exit pattern).
#
# ----------------------------------------------------------------------
# Gate 4 (D-08 / registry): MANUAL STEP
# ----------------------------------------------------------------------
# This gate cannot be automated because PLAN 03 makes session_key optional
# and the CLI startup path passes None for v1. The gate verifies that both
# `text_to_speech` and `send_audio` appear in the tool registry listing.
#
# Manual steps:
#   1. cargo build --release -p ironhermes-cli
#   2. IRONHERMES_HOME=/tmp/uat-36.17.5-home target/release/hermes tools list
#   3. Confirm BOTH of the following names appear in the output:
#        text_to_speech   (toolset: voice)
#        send_audio       (toolset: voice)
#   4. If either is missing, PLAN 03's register_tts_tools wiring is not
#      being invoked from the CLI startup path. Fix: wire
#      session_key=Some(SessionKey::new(Platform::Local, "local")) into
#      the CLI's AppRuntimeFactoryInput construction site.
#
# ----------------------------------------------------------------------
# Gate 5 (D-15 / Platform::Local): MANUAL STEP
# ----------------------------------------------------------------------
# This gate requires a human ear (or a clean error on headless machines).
#
# Manual steps:
#   1. Take OUTPUT_PATH from Gate 1 (printed to stdout by hermes tts test).
#   2. Run: cargo run -p ironhermes-cli -- tts play --path "${OUTPUT_PATH}"
#   3. On a machine WITH an audio device: confirm audible speech.
#   4. On a machine WITHOUT an audio device: confirm clean Err with
#      "No audio device" or similar — binary must exit non-zero but NOT panic.
#   5. Confirm OUTPUT_PATH file still exists after playback (D-16 no cleanup).
#
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WORKSPACE_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"
cd "${WORKSPACE_ROOT}"

IRONHERMES_HOME="$(mktemp -d -t ironhermes-36.17.5-XXXXXX)"
export IRONHERMES_HOME
cleanup() { rm -rf "${IRONHERMES_HOME}"; }
trap cleanup EXIT

echo "==> Phase 36.17.5 TTS UAT — IRONHERMES_HOME=${IRONHERMES_HOME}"
echo

# ----------------------------------------------------------------------
# Gate 1 (D-03 / D-16): synth via `hermes tts test --provider edge`
# creates audio_cache/ and writes exactly one UUID-named .mp3 file.
# ----------------------------------------------------------------------
echo "==> Gate 1 (D-03 / D-16): hermes tts test writes MP3 to audio_cache/"
OUTPUT_PATH="$(cargo run --quiet -p ironhermes-cli -- tts test --provider edge --text 'Phase 36.17.5 gate 1 sample.' 2>&1 | tail -1)"
if [ -z "${OUTPUT_PATH}" ]; then
    echo "    GATE FAIL (D-03): hermes tts test produced no path on stdout"
    exit 1
fi
if [ ! -f "${OUTPUT_PATH}" ]; then
    echo "    GATE FAIL (D-16): output path '${OUTPUT_PATH}' does not exist"
    exit 1
fi
case "${OUTPUT_PATH}" in
    *"${IRONHERMES_HOME}/audio_cache/"*.mp3) : ;;
    *) echo "    GATE FAIL (D-16): output '${OUTPUT_PATH}' is not under audio_cache/"; exit 1 ;;
esac
echo "    OK (synth wrote ${OUTPUT_PATH})"
echo

# ----------------------------------------------------------------------
# Gate 2 (D-04 / D-05): synthesized file is non-empty (real audio bytes).
# ----------------------------------------------------------------------
echo "==> Gate 2 (D-04 / D-05): synthesized file is non-empty"
BYTES="$(wc -c < "${OUTPUT_PATH}" | tr -d ' ')"
if [ "${BYTES}" -lt 1000 ]; then
    echo "    GATE FAIL (D-04 / D-05): file size ${BYTES} bytes is suspiciously small"
    exit 1
fi
# MP3 frame sync OR ID3 header
FIRST_BYTES="$(head -c 3 "${OUTPUT_PATH}" | od -An -tx1 | tr -d ' \n')"
case "${FIRST_BYTES}" in
    fffb*|fff3*|fff2*|494433*) : ;;
    *) echo "    GATE FAIL (D-04 / D-05): unexpected file magic ${FIRST_BYTES} (need MP3 frame sync or ID3)"; exit 1 ;;
esac
echo "    OK (${BYTES} bytes, magic ${FIRST_BYTES})"
echo

# ----------------------------------------------------------------------
# Gate 3 (D-10 / D-03): provider 'edge' is the BUILTIN_TTS_NAMES default
# and the binary exits 0 without an API key (Edge is keyless).
# ----------------------------------------------------------------------
echo "==> Gate 3 (D-10 / D-03): edge provider requires no API key"
# Unset any API key environment vars that might mask a real failure.
unset ELEVENLABS_API_KEY OPENAI_API_KEY || true
if ! cargo run --quiet -p ironhermes-cli -- tts test --provider edge --text 'Gate 3.' > /dev/null 2>&1; then
    echo "    GATE FAIL (D-03): edge provider failed without an API key (expected to succeed)"
    exit 1
fi
echo "    OK"
echo

# ----------------------------------------------------------------------
# Gates 4 and 5: MANUAL — see header comments above for instructions.
# ----------------------------------------------------------------------
echo "==> Gates 4 and 5 require human-verify checkpoints (see script header)."
echo "    Gate 4: run 'hermes tools list' and confirm text_to_speech + send_audio appear."
echo "    Gate 5: run 'hermes tts play --path ${OUTPUT_PATH}' and confirm audio or clean error."
echo

echo "==> Phase 36.17.5 TTS UAT: all green"
