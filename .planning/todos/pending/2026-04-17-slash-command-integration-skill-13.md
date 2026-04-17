---
created: 2026-04-17T03:11:41.262Z
title: Slash command integration (SKILL-13)
area: skills
files: []
---

## Problem

SKILL-13 was explicitly backlogged from v1.1. Slash command router needs to intercept `/`-prefixed messages before AgentLoop — platform-agnostic across CLI, gateway, and ACP. Core commands needed: /help, /reset, /personality, /skills, /memory, /sessions, /search, /model, /stop, /new. Also needs alias and prefix matching via resolve_command().

Covers requirements: SKILL-12, SKILL-13, SKILL-14.

## Solution

Implement slash command router in ironhermes-core or ironhermes-tools. Wire into CLI prompt handler and gateway message guard. Use resolve_command() pattern from hermes-agent for alias/prefix matching.
