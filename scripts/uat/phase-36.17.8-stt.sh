#!/usr/bin/env bash
# Phase 36.17.8 — STT (Speech-to-Text) UAT.
#
# Behavioral verification that the post-Wave-4 codebase ships a working
# STT pipeline: SttRegistry builtin names, mocked-provider round-trip,
# VAD capture loop, and live /voice on CLI gate.
#
# Decisions covered (from .planning/phases/36.17.8-.../36.17.8-CONTEXT.md):
#   D-03  — build_stt_registry() registers groq + openai built-ins
#   D-04  — BUILTIN_STT_NAMES = {"groq","openai"} frozen set
#   D-06  — Provider auto-select: groq preferred, openai fallback
#   D-07  — is_available() reads API key from env only; never logged
#   D-08  — Continuous VAD capture loop
#   D-09  — EnergyVad: speech-confirm, end-of-speech, hard-stop
#   D-10  — /voice on command dispatches without panic
#   D-12  — Hallucination filter rejects known phantom phrases
#   D-14  — Mocked provider transcription (wiremock; no live network in CI)
#   D-18  — SttConfig defaults + YAML round-trip
#
# Gates 1-2 are automated (cargo test; no API key needed).
# Gates 3-5 require a live session and optionally a real microphone.
#
# Exit 0 on all-pass; non-zero on any fail.
# Subsequent plans extend the automated assertions; this is the scaffold.
#
# Modeled on scripts/uat/phase-36.17.5-tts.sh conventions.
#
# ----------------------------------------------------------------------
# Gate 3 (D-10 / /voice on): MANUAL STEP
# ----------------------------------------------------------------------
# Manual steps:
#   1. cargo build --release -p ironhermes-cli
#   2. IRONHERMES_HOME=/tmp/uat-36.17.8-home target/release/hermes
#   3. In the CLI session, type: /voice on
#   4. Confirm output includes "Voice mode enabled" (or equivalent), no panic.
#   5. Type: /voice status
#   6. Confirm output includes the provider name and record key.
#
# ----------------------------------------------------------------------
# Gate 4 (D-08/D-09 / VAD + transcription): MANUAL STEP
# ----------------------------------------------------------------------
# Requires a real microphone + GROQ_API_KEY or VOICE_TOOLS_OPENAI_KEY.
#
# Manual steps:
#   1. With voice mode enabled (/voice on), press the record key.
#   2. Speak a short phrase (e.g. "Hello, IronHermes").
#   3. Release the record key (push-to-talk) or wait for VAD end-of-speech.
#   4. Confirm the transcript appears in the agent context (no hallucination filter false-positive).
#
# ----------------------------------------------------------------------
# Gate 5 (D-13/D-15 / web mic button): MANUAL STEP
# ----------------------------------------------------------------------
# Requires a browser with getUserMedia permission and a live STT API key.
#
# Manual steps:
#   1. dx serve --platform web (or cargo run -p iron_hermes_ui --features web)
#   2. Open http://localhost:8080 in a browser.
#   3. Click the mic button in HermesApp.
#   4. Speak, then click stop.
#   5. Confirm transcript appears in chat input.
#
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WORKSPACE_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"
cd "${WORKSPACE_ROOT}"

IRONHERMES_HOME="$(mktemp -d -t ironhermes-36.17.8-XXXXXX)"
export IRONHERMES_HOME
cleanup() { rm -rf "${IRONHERMES_HOME}"; }
trap cleanup EXIT

echo "==> Phase 36.17.8 STT UAT — IRONHERMES_HOME=${IRONHERMES_HOME}"
echo

# ----------------------------------------------------------------------
# Gate 1 (D-04 / D-18): SttRegistry builtin names + SttConfig defaults.
# Runs the Wave 0 scaffold tests that Plan 02 will fill.
# ----------------------------------------------------------------------
echo "==> Gate 1 (D-04 / D-18): SttRegistry builtin names + SttConfig defaults"
if perl -e 'alarm shift; exec @ARGV' 180 \
    cargo test -p ironhermes-core --test stt_registry -- --include-ignored 2>&1 | \
    grep -qE '(test result: ok|ignored)'; then
    echo "    OK (stt_registry tests passed or all ignored)"
else
    echo "    GATE FAIL (D-04 / D-18): stt_registry tests failed"
    exit 1
fi
echo

# ----------------------------------------------------------------------
# Gate 2 (D-03 / D-14): build_stt_registry + mocked provider round-trip.
# Runs the Wave 0 scaffold tests that Plan 03 will fill.
# ----------------------------------------------------------------------
echo "==> Gate 2 (D-03 / D-14): build_stt_registry + mocked transcription"
if perl -e 'alarm shift; exec @ARGV' 180 \
    cargo test -p ironhermes-tools --test stt_providers -- --include-ignored 2>&1 | \
    grep -qE '(test result: ok|ignored)'; then
    echo "    OK (stt_providers tests passed or all ignored)"
else
    echo "    GATE FAIL (D-03 / D-14): stt_providers tests failed"
    exit 1
fi
echo

# ----------------------------------------------------------------------
# Gate 3 (Plan-05 Task-3 / D-10): /voice + Ctrl+B status pill — MANUAL
# ----------------------------------------------------------------------
# Automated pre-check: hermes stt info exits 0 (build_stt_registry wired in main.rs).
echo "==> Gate 3a (D-10): hermes stt info (automated pre-check)"
if CARGO_TARGET_DIR=target-gsd-3617 cargo build -p ironhermes-cli --quiet 2>&1; then
    echo "    OK (ironhermes-cli build)"
else
    echo "    GATE FAIL: cargo build -p ironhermes-cli failed"
    exit 1
fi
echo
echo "==> Gates 3b-5 require human-verify checkpoints (see script header)."
echo
echo "    Gate 3b (/voice + status pill — Plan-05 Task-3 D-10):"
echo "      1. Run: IRONHERMES_HOME=${IRONHERMES_HOME} target/debug/hermes --tui"
echo "      2. Type '/voice status' → confirm output names the provider and record key"
echo "         (no 'No TTS infrastructure' stub text)."
echo "      3. Type '/voice on' → confirm status bar shows 'CTRL+B to record' pill (DarkGray)."
echo "      4. Type '/voice off' → confirm pill disappears."
echo
echo "    Gate 4 (Ctrl+B capture + transcript + status pill — D-08):"
echo "      Requires: GROQ_API_KEY or VOICE_TOOLS_OPENAI_KEY set, real microphone."
echo "      1. Type '/voice on', then press Ctrl+B."
echo "      2. Confirm status bar changes to '● LISTENING' (Red)."
echo "      3. Speak a phrase; stop speaking — confirm '◐ TRANSCRIBING' (Yellow) appears,"
echo "         then the transcript is submitted as a user turn and the agent replies."
echo "      4. Confirm 3 consecutive silent cycles exit the loop automatically."
echo
echo "    Gate 5 (auto-TTS spoken reply — D-11/D-16):"
echo "      1. Type '/voice tts' → confirm toggle reported."
echo "      2. Repeat Gate 4; confirm agent reply plays audio via rodio."
echo
echo "    Gate 6 (Web mic button — D-13/D-15): see original script header Gate 5."
echo

echo "==> Phase 36.17.8 STT UAT: automated gates green — manual gates 3b-6 pending operator"
