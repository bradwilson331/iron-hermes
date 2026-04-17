---
created: 2026-04-17T03:11:41.262Z
title: Tool registry improvements
area: tools
files: []
---

## Problem

Tool registry needs toolset management (named groups with platform-specific presets), is_available() check functions so tools are silently excluded when prerequisites are absent, and setup wizard integration to guide users through missing prerequisites (API keys, env vars). Agent-intercepted tools (memory, session_search, delegate_task, todo) should be handled before registry dispatch.

Covers requirements: TOOL-01, TOOL-02, TOOL-03, TOOL-04, TOOL-05.

## Solution

Add is_available() to Tool trait. Organize tools into named toolsets. Wire setup wizard to check tool availability and prompt for missing config. Route agent-intercepted tools before registry dispatch.
