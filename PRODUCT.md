# Product

## Register

both

## Users

Developers and power users who live in the terminal. They run builds, read logs, and navigate systems with keyboard shortcuts — this is their environment, not a foreign interface. They're context-switching from an editor or shell when they open IronHermes, and they expect the same precision.

Secondary surface: a public-facing layer where the product needs to speak for itself and communicate capability before anyone opens the app.

## Product Purpose

IronHermes is a terminal-native AI agent interface. Users interact with an AI (Hermes) through a streaming shell where commands, responses, tool calls, and errors surface as typed blocks. Multiple sessions, personality presets, and a keyboard-driven command palette make it a configurable, expert-level tool.

Success: feels like a first-class shell citizen, not a chat window dressed up as one.

## Brand Personality

Precise · Austere · Powerful

Voice: terse and confident. No exclamation points. No "Let's get started!" copy. Every word earns its place. The interface should feel like it was built by someone who uses it themselves.

## Anti-references

- ChatGPT / Claude.ai web UI — rounded bubbles, avatar thumbnails, friendly-AI softness
- Notion / Linear — polished SaaS softness, rounded corners, warm grays
- Generic AI dashboard templates — gradient hero metric cards, glowing blue orbs, hero animations
- Any interface that feels like it's explaining itself to someone who wouldn't use it anyway

## Design Principles

1. **Terminal-native first** — conventions from shell environments (monospace everywhere, color-coded output, keyboard over pointer) are not aesthetic choices, they're correctness.
2. **Density earns trust** — showing more information, not less, signals that the tool respects the user's ability to parse it.
3. **Silence is design** — every interaction that doesn't require a modal, tooltip, or animation shouldn't have one.
4. **Semantic color only** — color communicates meaning (success/error/warning/accent), never decoration.
5. **Both surfaces must be coherent** — the public landing and the app shell should feel like the same product, not one designed to attract and another designed to retain.

## Accessibility & Inclusion

WCAG AA minimum. Keyboard navigability is core to the product (not optional accessibility). All interactive elements must be reachable without a pointer device. Reduced motion via `prefers-reduced-motion` for the scanner animation.
