---
created: 2026-04-17T03:11:41.262Z
title: Configuration and setup wizard improvements
area: cli
files: []
---

## Problem

Need an interactive setup wizard for first-run configuration (provider selection, API keys, model choice, tool availability), config set/get/show CLI commands for managing config.yaml values, config migrate to scan skills for unconfigured settings, and profile isolation so each profile gets its own HERMES_HOME, config, memory, sessions, and gateway PID.

Covers requirements: CFG-01, CFG-02, CFG-03, CFG-04.

## Solution

Implement `ironhermes setup` interactive wizard. Add `ironhermes config set/get/show` subcommands. Add `ironhermes config migrate` for skills settings discovery. Implement profile isolation with per-profile HERMES_HOME directories.
