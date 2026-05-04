use dioxus::prelude::*;
#[cfg(target_arch = "wasm32")]
use dioxus::core::use_drop;
#[cfg(target_arch = "wasm32")]
use wasm_bindgen::JsCast;

use crate::components::shell::{
    AgentPanel, BlockStream, CommandPalette, InputBox, StatusBar, TitleBar,
};
use crate::state::{
    demo_block_entries, demo_messages, now_time,
    Block, BlockEntry, Mode, PaletteItem, PaletteState, Personality, ShellSettings,
    Tab, TokenBudget,
};

/// Top-level desktop/web shell composer — Phase 4 integration.
///
/// Per CONTEXT D-01: hybrid state model with 12 local signals declared
/// here. Per CONTEXT D-02: ONE `use_context_provider` bundle
/// (`ShellSettings { personality }`) read by `AgentPanel` and `StatusBar`.
/// Per CONTEXT D-17: ONE global keydown listener installed via
/// `use_effect`, removed via `use_drop`. Per CONTEXT D-33: auto-scroll
/// `use_effect` watching `blocks.read().len()`.
///
/// Submit routing (D-13): trim → empty? return; clear input; pulse
/// scanner; pulse token; spawn appropriate mock based on mode. Borrow-
/// then-await discipline (D-06): every signal `.read()` / `.write()`
/// drops at `;` before any `.await`.
#[component]
pub fn WarpHermes() -> Element {
    // ── 12 Phase 4 signals (D-01). ──
    let mut input          = use_signal(String::new);
    let mut blocks         = use_signal(demo_block_entries);
    let messages           = use_signal(demo_messages);
    #[allow(unused_mut)] // required for mode.set via Callback closure capture
    #[allow(unused_mut)] // required for mode.set via Callback closure capture
    let mut mode               = use_signal(|| Mode::Shell);
    let mut pal_open       = use_signal(|| false);
    let mut pal_query      = use_signal(String::new);
    let mut pal_state      = use_signal(|| PaletteState::Browse);
    let scanner_active     = use_signal(|| false);
    let focused            = use_signal(|| false);
    let active_tab         = use_signal(|| 0_usize);
    let tokens             = use_signal(|| TokenBudget { used: 12_300, max: 128_000 });
    let mut next_id        = use_signal(|| 1000_u64);

    // ── Fetch real data from server functions (Plan 03 wiring). ──

    // Fetch real slash commands from CommandRouter via server function.
    let slash_commands = use_server_future(move || {
        crate::server::api::list_slash_commands()
    })?;

    // Fetch real config (model/provider/context_length) from server.
    let config_summary = use_server_future(move || {
        crate::server::api::get_config_summary()
    })?;

    // Fetch real sessions from StateStore via server function.
    let sessions = use_server_future(move || {
        crate::server::api::list_sessions()
    })?;

    // Convert server data to UI types — map inside component, no cross-module From impls.
    let palette_items: Vec<PaletteItem> = match slash_commands() {
        Some(Ok(cmds)) => cmds
            .into_iter()
            .map(|cmd| PaletteItem {
                section: "slash".into(),
                cmd: format!("/{}", cmd.name),
                label: cmd.description,
                kbd: vec![],
            })
            .collect(),
        _ => vec![], // Loading or error — empty palette until data arrives
    };

    let tabs: Vec<Tab> = match sessions() {
        Some(Ok(sessions)) if !sessions.is_empty() => sessions
            .into_iter()
            .map(|s| Tab {
                label: s.title.unwrap_or(s.id),
                live: true,
            })
            .collect(),
        _ => vec![Tab { label: "New Session".into(), live: true }],
    };

    let (model_name, provider_name) = match config_summary() {
        Some(Ok(cfg)) => (cfg.model, cfg.provider),
        _ => ("loading...".to_string(), "...".to_string()),
    };

    // ── ShellSettings via use_context_provider (D-02 + Pattern 5). ──
    let mut personality = use_signal(|| Personality::Default);
    use_context_provider(|| ShellSettings { personality });

    // ── Global keydown listener (D-17 + Pattern 3). ──
    // Closure stored in Signal<Option<Closure<_>>> so it outlives use_effect
    // and can be removed in use_drop. NOT cb.forget() (leaks + prevents cleanup).
    //
    // Entire listener setup is cfg-gated for wasm32 because:
    // 1. wasm_bindgen::Closure only exists on wasm32
    // 2. web_sys::window() / add_event_listener only work in browser
    // 3. Dioxus hooks inside compile-time cfg blocks are fine —
    //    the binary is built for one target, so hook ordering is consistent
    #[cfg(target_arch = "wasm32")]
    {
        use web_sys::KeyboardEvent as WebKeyboardEvent;

        let mut listener_slot: Signal<
            Option<wasm_bindgen::closure::Closure<dyn FnMut(WebKeyboardEvent)>>,
        > = use_signal(|| None);

        use_effect(move || {
            let Some(window) = web_sys::window() else { return; };
            let cb = wasm_bindgen::closure::Closure::<dyn FnMut(WebKeyboardEvent)>::new(
                move |ev: WebKeyboardEvent| {
                    let key = ev.key();
                    let lower = key.to_lowercase();
                    let code = ev.code(); // physical key code — independent of Option-key æöü mappings
                    if (ev.meta_key() || ev.ctrl_key()) && code == "KeyK" {
                        ev.prevent_default();
                        let cur = pal_open();
                        pal_open.set(!cur);
                        pal_query.set(String::new());
                        return;
                    }
                    if key == "Escape" {
                        pal_open.set(false);
                        pal_state.set(PaletteState::Browse);
                        return;
                    }
                    if ev.alt_key() && code == "KeyM" && focused() {
                        ev.prevent_default();
                        let next = match mode() {
                            Mode::Shell => Mode::Agent,
                            Mode::Agent => Mode::Shell,
                        };
                        mode.set(next);
                        return;
                    }
                },
            );
            let _ = window.add_event_listener_with_callback(
                "keydown",
                cb.as_ref().unchecked_ref(),
            );
            listener_slot.set(Some(cb));
        });

        use_drop(move || {
            if let Some(cb) = listener_slot.write().take() {
                if let Some(window) = web_sys::window() {
                    let _ = window.remove_event_listener_with_callback(
                        "keydown",
                        cb.as_ref().unchecked_ref(),
                    );
                }
                // cb drops here, releasing the JS-side reference.
            }
        });
    }

    // ── Auto-scroll on new blocks (D-33 + Common Op 3 + Pitfall 5 triple-guard). ──
    use_effect(move || {
        let len = blocks.read().len();
        if len > 0 {
            #[cfg(target_arch = "wasm32")]
            {
                if let Some(window) = web_sys::window() {
                    if let Some(doc) = window.document() {
                        if let Ok(Some(el)) = doc.query_selector(
                            ".wh-stream-scroll .wh-block:last-child",
                        ) {
                            el.scroll_into_view_with_bool(false);
                        }
                    }
                }
            }
        }
    });

    // ── pulse_scanner (D-14 + Pattern 4). ──
    let pulse_scanner = move |ms: u32, mut sa: Signal<bool>| {
        sa.set(true);
        spawn(async move {
            crate::platform::timer::sleep(ms).await;
            sa.set(false);
        });
    };

    // ── pulse_token (D-26): saturating +amount per submission. ──
    let pulse_token = move |amount: u32, mut t: Signal<TokenBudget>| {
        // Read current values before any mutation; TokenBudget is Copy.
        let cur = t();
        let new_used = cur.used.saturating_add(amount).min(cur.max);
        t.set(TokenBudget { used: new_used, max: cur.max });
    };

    // ── on_rerun handler (D-24): re-run a Cmd block by id. ──
    let on_rerun = move |id: u64| {
        // Find the entry; clone tokens out before any spawn — no read borrow held across spawn body.
        let cmd_text: Option<String> = {
            let bs = blocks.read();
            bs.iter()
                .find(|e| e.id == id)
                .and_then(|e| match &e.block {
                    Block::Cmd { command } => Some(
                        command
                            .tokens
                            .iter()
                            .map(|t| t.text().to_string())
                            .collect::<Vec<_>>()
                            .join(" "),
                    ),
                    _ => None,
                })
        };
        if let Some(text) = cmd_text {
            pulse_scanner(2000, scanner_active);
            pulse_token(120, tokens);
            spawn(async move {
                crate::mocks::run_shell(text, blocks, next_id, scanner_active).await;
            });
        }
    };

    // ── submit handler (Common Op 1 + D-13). ──
    let mut submit = move || {
        // Read input clone, trim, return early if empty. No live borrow held.
        let text = {
            let s = input.read();
            let t = s.trim().to_string();
            if t.is_empty() {
                return;
            }
            t
        };
        input.set(String::new());
        pulse_scanner(2000, scanner_active);
        pulse_token(120, tokens);

        let cur_mode = mode();
        let cur_personality = *personality.read();
        spawn(async move {
            match cur_mode {
                Mode::Agent => {
                    crate::mocks::run_agent_steps(text, cur_personality, messages).await;
                }
                Mode::Shell => {
                    crate::mocks::run_shell(text, blocks, next_id, scanner_active).await;
                }
            }
        });
    };

    // ── pick handler (Common Op 4 + D-27..D-32). ──
    let palette_items_for_pick = palette_items.clone();
    let mut pick = move |item: PaletteItem| {
        // PersonalityPick row: cmd is "/concise", "/technical", etc. Look up.
        if pal_state() == PaletteState::PersonalityPick {
            let target = item.cmd.trim_start_matches('/').to_string();
            if let Some(p) = Personality::ALL.iter().find(|p| p.label() == target) {
                personality.set(*p);
                pal_state.set(PaletteState::Browse);
                pal_open.set(false);
                pal_query.set(String::new());
            }
            return;
        }

        match item.cmd.as_str() {
            "/clear" => {
                blocks.set(Vec::new());
                pal_open.set(false);
                pal_query.set(String::new());
            }
            "/status" => {
                // Reserve an id; write WITHOUT holding next_id across anything else.
                let id = {
                    let cur = next_id();
                    next_id.set(cur + 1);
                    cur
                };
                blocks.write().push(BlockEntry {
                    id,
                    block: Block::Out {
                        author: Some("ironhermes".into()),
                        time: Some(now_time()),
                        text: crate::mocks::shell_outputs::STATUS_TEXT.into(),
                    },
                });
                pal_open.set(false);
                pal_query.set(String::new());
            }
            "/help" => {
                // Format slash commands from PALETTE_ITEMS (D-29).
                let mut help_text = String::from("Available commands\n");
                for it in palette_items_for_pick.iter().filter(|p| p.section == "slash") {
                    help_text.push_str(&format!("  {:<14}  {}\n", it.cmd, it.label));
                }
                let id = {
                    let cur = next_id();
                    next_id.set(cur + 1);
                    cur
                };
                blocks.write().push(BlockEntry {
                    id,
                    block: Block::Out {
                        author: Some("help".into()),
                        time: Some(now_time()),
                        text: help_text,
                    },
                });
                pal_open.set(false);
                pal_query.set(String::new());
            }
            "/personality" => {
                // Stay open; transition to PersonalityPick (D-20).
                pal_state.set(PaletteState::PersonalityPick);
                pal_query.set(String::new());
            }
            _ => {
                // /doctor, /quit, all workflow items: fill input box, close palette.
                input.set(item.cmd.clone());
                pal_open.set(false);
                pal_query.set(String::new());
            }
        }
    };

    rsx! {
        div {
            class: "wh-app",
            "data-theme": "cyan",
            "data-density": "comfy",
            "data-block": "framed",
            "data-agent": "right",
            TitleBar { tabs: tabs, active_tab: active_tab(), show_traffic_lights: true }
            div { class: "wh-main",
                div { class: "wh-col",
                    BlockStream {
                        blocks: blocks,
                        on_rerun: move |id: u64| on_rerun(id),
                    }
                    InputBox {
                        value: input,
                        mode: mode,
                        focused: focused,
                        on_submit: move |_| submit(),
                    }
                    StatusBar {
                        mode: "Chat".to_string(),
                        model: model_name.clone(),
                        provider: provider_name.clone(),
                        tokens: tokens,
                        scanner_active: scanner_active,
                        hint: "/help · ⌃C cancel · ⌘K palette".to_string(),
                    }
                }
                AgentPanel { messages: messages }
            }
            CommandPalette {
                items: palette_items,
                query: pal_query,
                open: pal_open,
                state: pal_state,
                on_pick: move |item: PaletteItem| pick(item),
            }
        }
    }
}
