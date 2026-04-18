---
title: "SkillRegistry name validation rejects Title Case skills"
area: skills
created: 2026-04-18
source: runtime-warnings
priority: medium
---

## Problem

SkillRegistry enforces kebab-case name regex `^[a-z0-9]([a-z0-9-]*[a-z0-9])?$` but 8 skills
use Title Case names and get silently rejected at startup:

- "Command Development", "Skill Development", "Plugin Settings", "Plugin Structure"
- "Writing Hookify Rules", "Hook Development", "MCP Integration", "Agent Development"

## Options

1. **Normalize names on load** — `to_lowercase().replace(' ', '-')` before validation, preserving
   the original display name separately
2. **Relax the regex** — allow spaces/caps in the name field, add a separate `slug` field for lookups
3. **Fix the skill files** — rename the `name:` field in each SKILL.md to kebab-case

## Also noted

- `cron/jobs.json` parse warning (empty/missing file — low priority)
- `hermes-agent/SKILL.md` blocked by skill_pattern_22/25/28 but WARN-BUT-LOAD proceeds (security scanner false positive on trusted skill — investigate patterns)
