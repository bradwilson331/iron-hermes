//! Static render-time stub data for the 10 visual-only screens in
//! Phase 26.2.1 (Plan 08 consumers).
//!
//! Per CONTEXT D-04: these are the **production** data sources for the 10
//! visual-stub screens — not test-only fixtures. Follow-up phases
//! (26.2.2..26.2.11 per the Deferred Ideas list) replace each `pub fn`
//! body with a server-function call; the typed return shape and the
//! `&'static str` field types stay the same so the swap is impl-only.
//!
//! Source-of-truth for each row is the corresponding `<section>` markup
//! in `crates/iron_hermes_ui/filestoimport/ironhermes-design-system/project/app.html`
//! (`screen-agents` ~501, `screen-skills` ~594, `screen-models` ~691,
//! `screen-memory` ~797, `screen-soul` ~885, `screen-tools` ~960,
//! `screen-schedules` ~1074, `screen-gateway` ~1150, `screen-office`
//! ~1266). Providers has no `<section>` in app.html (CONTEXT D-05) — its
//! rows are derived from `project/screens/Providers.tsx` plus the
//! prototype's surrounding model/provider vocabulary.
//!
//! Status badges (`ACTIVE`/`IDLE`/`PAUSED`/`CONNECTED`/`DISCONNECTED`/
//! etc.) stay as `&'static str` per the plan's anti-pattern list — Plan
//! 08's RSX does CSS-class lookup on the string, no logic switches on
//! the value.
//!
//! Module-level `#![allow(dead_code, unused_imports)]` because the
//! consumers land in Wave 3 (Plan 08). Without this the default
//! `cargo check` would warn on every unused factory.

#![allow(dead_code, unused_imports)]

// ---------------------------------------------------------------------------
// Agents (app.html `screen-agents` ~501)
// ---------------------------------------------------------------------------

/// One row in the Agents grid (CONTEXT D-04 visual-stub).
#[derive(Clone, PartialEq, Debug)]
pub struct AgentStub {
    pub name: &'static str,
    pub avatar_letter: char,
    pub avatar_color: &'static str,
    pub status: &'static str,
    pub model: &'static str,
    pub skills_count: u32,
    pub summary: &'static str,
}

/// 4 agent cards verbatim from app.html `screen-agents` (lines 501-590).
pub fn agents() -> Vec<AgentStub> {
    vec![
        AgentStub {
            name: "default",
            avatar_letter: '▓',
            avatar_color: "shield",
            status: "ACTIVE",
            model: "Openrouter · nemotron-3-nano-30b-a3b:free",
            skills_count: 57,
            summary: "General-purpose operator — broad skills, persistent memory, no platform integrations.",
        },
        AgentStub {
            name: "financial_analyst",
            avatar_letter: 'F',
            avatar_color: "amber",
            status: "IDLE",
            model: "Openrouter · nemotron-3-nano-30b-a3b:free",
            skills_count: 51,
            summary: "Quant-focused — bound to ledger memory store and Bloomberg gateway.",
        },
        AgentStub {
            name: "marketing_manager",
            avatar_letter: 'M',
            avatar_color: "green",
            status: "IDLE",
            model: "Local · gemma-3-4b",
            skills_count: 51,
            summary: "Copy & campaign lead — Slack + Discord gateways for content review threads.",
        },
        AgentStub {
            name: "social_media_manager",
            avatar_letter: 'S',
            avatar_color: "purple",
            status: "IDLE",
            model: "Local · llama-3.1-8b-instant",
            skills_count: 51,
            summary: "Scheduled posting — cron-driven digest delivery via Telegram.",
        },
    ]
}

// ---------------------------------------------------------------------------
// Skills (app.html `screen-skills` ~594)
// ---------------------------------------------------------------------------

/// One row in the Skills grid.
#[derive(Clone, PartialEq, Debug)]
pub struct SkillStub {
    pub name: &'static str,
    pub category: &'static str,
    pub status: &'static str,
    pub summary: &'static str,
    pub version: &'static str,
}

/// 6 skill cards verbatim from app.html `screen-skills` (lines 620-686).
pub fn skills() -> Vec<SkillStub> {
    vec![
        SkillStub {
            name: "/research",
            category: "bundled · 5 tools",
            status: "ENABLED",
            summary: "Multi-source web research with independence scoring and citation extraction.",
            version: "v3.2",
        },
        SkillStub {
            name: "/summarize",
            category: "bundled · 1 tool",
            status: "ENABLED",
            summary: "Compress long threads or documents with configurable detail level.",
            version: "v2.1",
        },
        SkillStub {
            name: "/code",
            category: "bundled · 3 tools",
            status: "ENABLED",
            summary: "Edit, lint, and run Python/JS snippets in an isolated sandbox.",
            version: "v4.0",
        },
        SkillStub {
            name: "/recall",
            category: "bundled · memory",
            status: "DISABLED",
            summary: "Semantic search across this agent's memory store.",
            version: "v1.4",
        },
        SkillStub {
            name: "/cite",
            category: "installed · 2 tools",
            status: "DISABLED",
            summary: "Inline source attribution for any factual claim.",
            version: "v1.0",
        },
        SkillStub {
            name: "/translate",
            category: "installed · 1 tool",
            status: "DISABLED",
            summary: "Translate between 47 languages with idiom preservation.",
            version: "v2.0",
        },
    ]
}

// ---------------------------------------------------------------------------
// Models (app.html `screen-models` ~691)
// ---------------------------------------------------------------------------

/// One row in the Models grid.
#[derive(Clone, PartialEq, Debug)]
pub struct ModelStub {
    pub id: &'static str,
    pub family: &'static str,
    pub context_window: &'static str,
    pub status: &'static str,
}

/// 5 model configs verbatim from app.html `screen-models` (lines 704-793).
pub fn models() -> Vec<ModelStub> {
    vec![
        ModelStub {
            id: "nemotron-3-nano-30b-a3b:free",
            family: "Openrouter",
            context_window: "128k",
            status: "DEFAULT",
        },
        ModelStub {
            id: "gpt-5",
            family: "Openrouter",
            context_window: "128k",
            status: "AVAILABLE",
        },
        ModelStub {
            id: "claude-sonnet-4",
            family: "Openrouter",
            context_window: "200k",
            status: "AVAILABLE",
        },
        ModelStub {
            id: "gemma-3-4b",
            family: "Local (llama.cpp)",
            context_window: "8k",
            status: "AVAILABLE",
        },
        ModelStub {
            id: "llama-3.1-8b-instant",
            family: "Local (llama.cpp)",
            context_window: "8k",
            status: "AVAILABLE",
        },
    ]
}

// ---------------------------------------------------------------------------
// Memory (app.html `screen-memory` ~797)
// ---------------------------------------------------------------------------

/// One row in the Memory entries panel.
#[derive(Clone, PartialEq, Debug)]
pub struct MemoryEntryStub {
    pub scope: &'static str,
    pub key: &'static str,
    pub value_preview: &'static str,
    pub updated: &'static str,
}

/// 5 memory entries verbatim from app.html `screen-memory` (lines 819-853).
pub fn memory_entries() -> Vec<MemoryEntryStub> {
    vec![
        MemoryEntryStub {
            scope: "PREFERENCE",
            key: "monospace-output",
            value_preview: "Operator prefers monospace output for code blocks, no syntax highlighting unless explicitly requested.",
            updated: "2026-05-12 · 14:08 UTC",
        },
        MemoryEntryStub {
            scope: "FACT",
            key: "production-db",
            value_preview: "Production database is postgres 16 on db-prod-01.internal. Migrations under /srv/hermes/migrations.",
            updated: "2026-05-11 · 09:33 UTC",
        },
        MemoryEntryStub {
            scope: "CONTEXT",
            key: "apt-29-investigation",
            value_preview: "Currently investigating APT-29 IOCs. Cross-referencing destination 185.220.101.47 with internal subnet 10.14.0.0/24.",
            updated: "2026-05-10 · 22:41 UTC",
        },
        MemoryEntryStub {
            scope: "PREFERENCE",
            key: "report-length",
            value_preview: "Reports must be under 500 words and lead with the recommendation, not the evidence chain.",
            updated: "2026-05-09 · 11:18 UTC",
        },
        MemoryEntryStub {
            scope: "FACT",
            key: "team-stack",
            value_preview: "Team uses Linear for tracking, Slack for comms, 1Password for credentials.",
            updated: "2026-05-08 · 03:17 UTC",
        },
    ]
}

// ---------------------------------------------------------------------------
// Soul (app.html `screen-soul` ~885)
// ---------------------------------------------------------------------------

/// One row in the Soul personas panel.
#[derive(Clone, PartialEq, Debug)]
pub struct SoulPersonaStub {
    pub id: &'static str,
    pub label: &'static str,
    pub blurb: &'static str,
    pub active: bool,
}

/// 4 personas — the prototype shows a single SOUL.md editor for `default`;
/// these rows surface the multi-persona picker that 26.2.6 will wire to
/// per-profile SOUL.md files. Labels derived from the agent set in
/// `screen-agents` so the two screens cross-reference cleanly.
pub fn soul_personas() -> Vec<SoulPersonaStub> {
    vec![
        SoulPersonaStub {
            id: "default",
            label: "Hermes — default profile",
            blurb: "Operator-aligned intelligence shell. Calm, direct, technically literate.",
            active: true,
        },
        SoulPersonaStub {
            id: "financial_analyst",
            label: "Hermes — financial_analyst",
            blurb: "Quant-focused. Lead with the recommendation, then the evidence chain.",
            active: false,
        },
        SoulPersonaStub {
            id: "marketing_manager",
            label: "Hermes — marketing_manager",
            blurb: "Copy & campaign lead. Calm voice, brand-aware, channel-appropriate tone.",
            active: false,
        },
        SoulPersonaStub {
            id: "social_media_manager",
            label: "Hermes — social_media_manager",
            blurb: "Scheduled posting persona. Concise, on-brand, audience-aware.",
            active: false,
        },
    ]
}

// ---------------------------------------------------------------------------
// Tools (app.html `screen-tools` ~960)
// ---------------------------------------------------------------------------

/// One row in the Tools grid.
#[derive(Clone, PartialEq, Debug)]
pub struct ToolStub {
    pub name: &'static str,
    pub group: &'static str,
    pub status: &'static str,
    pub summary: &'static str,
}

/// 12 tool cards verbatim from app.html `screen-tools` (lines 974-1069).
pub fn tools() -> Vec<ToolStub> {
    vec![
        ToolStub {
            name: "Web Search",
            group: "Information",
            status: "ENABLED",
            summary: "Search the web and extract content from URLs.",
        },
        ToolStub {
            name: "Browser",
            group: "Information",
            status: "DISABLED",
            summary: "Navigate, click, type, and interact with web pages.",
        },
        ToolStub {
            name: "Terminal",
            group: "Execution",
            status: "ENABLED",
            summary: "Execute shell commands and scripts.",
        },
        ToolStub {
            name: "File Operations",
            group: "Execution",
            status: "DISABLED",
            summary: "Read, write, search, and manage files.",
        },
        ToolStub {
            name: "Code Execution",
            group: "Execution",
            status: "ENABLED",
            summary: "Execute Python and shell code directly.",
        },
        ToolStub {
            name: "Vision",
            group: "Multimodal",
            status: "DISABLED",
            summary: "Analyze images and visual content.",
        },
        ToolStub {
            name: "Image Generation",
            group: "Multimodal",
            status: "DISABLED",
            summary: "Generate images with DALL-E and other models.",
        },
        ToolStub {
            name: "Text-to-Speech",
            group: "Multimodal",
            status: "DISABLED",
            summary: "Convert text to spoken audio.",
        },
        ToolStub {
            name: "Skills",
            group: "Agent",
            status: "DISABLED",
            summary: "Create, manage, and execute reusable skills.",
        },
        ToolStub {
            name: "Memory",
            group: "Agent",
            status: "ENABLED",
            summary: "Store and recall persistent knowledge.",
        },
        ToolStub {
            name: "Session Search",
            group: "Agent",
            status: "DISABLED",
            summary: "Search across past conversations.",
        },
        ToolStub {
            name: "Clarifying Questions",
            group: "Agent",
            status: "DISABLED",
            summary: "Ask the user for clarification when needed.",
        },
    ]
}

// ---------------------------------------------------------------------------
// Schedules (app.html `screen-schedules` ~1074)
// ---------------------------------------------------------------------------

/// One row in the Schedules list.
#[derive(Clone, PartialEq, Debug)]
pub struct ScheduleStub {
    pub id: &'static str,
    pub cron: &'static str,
    pub target: &'static str,
    pub status: &'static str,
    pub last_run: &'static str,
}

/// 5 schedule rows verbatim from app.html `screen-schedules` (lines 1091-1145).
pub fn schedules() -> Vec<ScheduleStub> {
    vec![
        ScheduleStub {
            id: "Daily competitor digest",
            cron: "0 9 * * 1-5",
            target: "Telegram → @social_ops",
            status: "ACTIVE",
            last_run: "May 13 · 09:00 UTC",
        },
        ScheduleStub {
            id: "Weekly market brief",
            cron: "0 7 * * 1",
            target: "Email → ops@stark.io",
            status: "ACTIVE",
            last_run: "May 18 · 07:00 UTC",
        },
        ScheduleStub {
            id: "Threat intel sweep",
            cron: "*/30 * * * *",
            target: "Slack → #sec-feed",
            status: "ACTIVE",
            last_run: "May 13 · 03:30 UTC",
        },
        ScheduleStub {
            id: "Quarterly persona retune",
            cron: "0 0 1 */3 *",
            target: "Email → operator@stark.io",
            status: "PAUSED",
            last_run: "Jul 1 · 00:00 UTC",
        },
        ScheduleStub {
            id: "Memory garbage collect",
            cron: "0 4 * * 0",
            target: "— (silent)",
            status: "DISABLED",
            last_run: "May 19 · 04:00 UTC",
        },
    ]
}

// ---------------------------------------------------------------------------
// Gateway (app.html `screen-gateway` ~1150)
// ---------------------------------------------------------------------------

/// One row in the Gateway platforms grid.
#[derive(Clone, PartialEq, Debug)]
pub struct GatewayPlatformStub {
    pub name: &'static str,
    pub status: &'static str,
    pub chats_connected: u32,
}

/// 6 platform cards verbatim from app.html `screen-gateway` (lines 1163-1262).
pub fn gateway_platforms() -> Vec<GatewayPlatformStub> {
    vec![
        GatewayPlatformStub {
            name: "Slack",
            status: "CONNECTED",
            chats_connected: 3,
        },
        GatewayPlatformStub {
            name: "Discord",
            status: "CONNECTED",
            chats_connected: 1,
        },
        GatewayPlatformStub {
            name: "Telegram",
            status: "CONNECTED",
            chats_connected: 2,
        },
        GatewayPlatformStub {
            name: "Email (SMTP/IMAP)",
            status: "DISCONNECTED",
            chats_connected: 0,
        },
        GatewayPlatformStub {
            name: "SMS (Twilio)",
            status: "DISCONNECTED",
            chats_connected: 0,
        },
        GatewayPlatformStub {
            name: "Webhook",
            status: "CONNECTED",
            chats_connected: 1,
        },
    ]
}

// ---------------------------------------------------------------------------
// Office (app.html `screen-office` ~1266)
// ---------------------------------------------------------------------------

/// One row in the Office workspaces list.
#[derive(Clone, PartialEq, Debug)]
pub struct OfficeWorkspaceStub {
    pub name: &'static str,
    pub kind: &'static str,
    pub last_active: &'static str,
}

/// 4 workspace rows verbatim from app.html `screen-office` Devices panel
/// (lines 1309-1337) — Plan 08 surfaces the device mesh + calibration
/// panel together; this stub powers the device list.
pub fn office_workspaces() -> Vec<OfficeWorkspaceStub> {
    vec![
        OfficeWorkspaceStub {
            name: "Projector — UST-01",
            kind: "Projector · 4K · 144Hz",
            last_active: "LIVE",
        },
        OfficeWorkspaceStub {
            name: "IR Camera Array",
            kind: "×4 · synced 144fps",
            last_active: "LOCKED",
        },
        OfficeWorkspaceStub {
            name: "Surface Mic Pair",
            kind: "XLR · gain −18 dB",
            last_active: "LIVE",
        },
        OfficeWorkspaceStub {
            name: "Haptic Floor Pad",
            kind: "USB-C · low-freq",
            last_active: "IDLE",
        },
    ]
}

// ---------------------------------------------------------------------------
// Providers — derived from project/screens/Providers.tsx (CONTEXT D-05)
// ---------------------------------------------------------------------------
//
// app.html has no `#screen-providers` section (D-05). The TSX reference
// exposes per-provider env keys, model picker, and a shared credential
// pool. For Plan 02 we surface the provider list itself — the row count,
// label, status, and latency mirror the prototype's surrounding
// vocabulary (Openrouter / Anthropic / OpenAI / Local llama.cpp) so the
// Providers visual stub in Plan 08 has a coherent dataset.

/// One row in the Providers grid.
#[derive(Clone, PartialEq, Debug)]
pub struct ProviderStub {
    pub id: &'static str,
    pub label: &'static str,
    pub status: &'static str,
    pub model_count: u32,
    pub latency_p50_ms: u32,
}

/// 5 provider rows derived from `project/screens/Providers.tsx` plus the
/// model-family vocabulary in `screen-models` (Openrouter / Local /
/// Anthropic / OpenAI).
pub fn providers() -> Vec<ProviderStub> {
    vec![
        ProviderStub {
            id: "openrouter",
            label: "Openrouter",
            status: "ACTIVE",
            model_count: 3,
            latency_p50_ms: 218,
        },
        ProviderStub {
            id: "anthropic",
            label: "Anthropic",
            status: "ACTIVE",
            model_count: 4,
            latency_p50_ms: 388,
        },
        ProviderStub {
            id: "openai",
            label: "OpenAI",
            status: "IDLE",
            model_count: 6,
            latency_p50_ms: 412,
        },
        ProviderStub {
            id: "local-llamacpp",
            label: "Local (llama.cpp)",
            status: "ACTIVE",
            model_count: 2,
            latency_p50_ms: 52,
        },
        ProviderStub {
            id: "local-ollama",
            label: "Local (Ollama)",
            status: "DISCONNECTED",
            model_count: 0,
            latency_p50_ms: 0,
        },
    ]
}
