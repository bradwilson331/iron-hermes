// Phase 54.9 Plan 08 (D-09): offline generator for crates/iron_hermes_ui/assets/openui/system-prompt.txt.
// No network call is made. Emits library.prompt(openuiChatPromptOptions), the locally generated prompt
// (never the managed hosted prompt, never Autofix), then self-checks the local streaming parser:
// a known-good sample must give 0 errors, a known-bad one more than 0.
//
// Usage: node scripts/openui/gen-system-prompt.mjs --modules <scratch node_modules dir> \
//          --out crates/iron_hermes_ui/assets/openui/system-prompt.txt [--file <openui-lang file to parse>]
//
// The dev-only packages are NOT a repo dependency. Install them into a scratch dir OUTSIDE the repo,
// scripts disabled, telemetry off, pinned to the versions ADR-0007 records (operator-approved):
//   mkdir -p "$SCRATCH/openui" && cd "$SCRATCH/openui"
//   OPENUI_TELEMETRY_DISABLED=1 DO_NOT_TRACK=1 npm install --ignore-scripts --no-audit --no-fund \
//     @openuidev/react-ui@0.18.1 @openuidev/react-lang@1.0.2 @openuidev/lang-core@1.0.2 react@19 react-dom@19 zod
// After any @openuidev/* upgrade re-run this so the prompt matches the vendored bundle's library.
import { pathToFileURL } from "node:url";
import { writeFileSync, readFileSync } from "node:fs";

const arg = (k) => {
  const i = process.argv.indexOf(k);
  return i > 0 ? process.argv[i + 1] : undefined;
};
const modules = arg("--modules");
const out = arg("--out");
if (!modules || !out) {
  console.error("usage: --modules <node_modules dir> --out <path> [--file <lang file to parse>]");
  process.exit(2);
}
const load = (rel) => import(pathToFileURL(`${modules}/${rel}`).href);
const { openuiChatLibrary, openuiChatPromptOptions } = await load("@openuidev/react-ui/dist/genui-lib/index.mjs");
const { createStreamingParser } = await load("@openuidev/lang-core/dist/index.mjs");

const prompt = openuiChatLibrary.prompt(openuiChatPromptOptions);
writeFileSync(out, prompt);
console.log(`PROMPT_CHARS=${prompt.length}`);

const GOOD = [
  'root = Card([title, body, btn])',
  'title = TextContent("Hello from OpenUI Lang", "large-heavy")',
  'body = TextContent("CSP spike")',
  'btn = Button("Do thing", @ToAssistant("do the thing"), "primary")',
].join("\n");
const BAD = 'root = Card([NoSuchComponent("x")])\n';

function check(label, text) {
  const sp = createStreamingParser(openuiChatLibrary.toJSONSchema());
  const r = sp.push(text, true);
  const errs = r.meta?.errors ?? [];
  console.log(`PARSE_${label}_ERRORS=${errs.length}` + (errs[0] ? ` first=${JSON.stringify(errs[0]).slice(0, 160)}` : ""));
  return errs.length;
}
const g = check("GOOD", GOOD);
const b = check("BAD", BAD);
const f = arg("--file");
if (f) check("FILE", readFileSync(f, "utf8"));
if (g !== 0 || b === 0) {
  console.error("SELF_CHECK_FAILED: good must be 0 errors and bad must be > 0");
  process.exit(1);
}
console.log("SELF_CHECK=ok");
