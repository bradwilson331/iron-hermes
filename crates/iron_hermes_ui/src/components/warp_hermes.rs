#[cfg(target_arch = "wasm32")]
use dioxus::core::use_drop;
use dioxus::prelude::*;
#[cfg(target_arch = "wasm32")]
use wasm_bindgen::JsCast;

use crate::components::shell::{
    AgentPanel, BlockStream, CommandPalette, InputBox, StatusBar, TitleBar,
};
use crate::state::{
    now_time, Block, BlockEntry, Message, Mode, PaletteItem, PaletteState, Personality,
    ShellSettings, Tab, TokenBudget, ToolCall as UiToolCall, ToolStatus,
};

const DISCONNECT_NOTICE: &str =
    "Connection interrupted. Please retry your message once reconnected.";

fn push_disconnect_notice(blocks: &mut Signal<Vec<BlockEntry>>, next_id: &mut Signal<u64>) {
    let id = {
        let cur = next_id();
        next_id.set(cur + 1);
        cur
    };
    blocks.write().push(BlockEntry {
        id,
        block: Block::Err {
            author: Some("hermes".into()),
            time: Some(now_time()),
            exit_code: 1,
            message: DISCONNECT_NOTICE.to_string(),
        },
    });
}

/// Top-level desktop/web shell composer — Phase 4 integration + Plan 04 WebSocket wiring.
///
/// Per CONTEXT D-01: hybrid state model with 12 local signals declared
/// here. Per CONTEXT D-02: ONE `use_context_provider` bundle
/// (`ShellSettings { personality }`) read by `AgentPanel` and `StatusBar`.
/// Per CONTEXT D-17: ONE global keydown listener installed via
/// `use_effect`, removed via `use_drop`. Per CONTEXT D-33: auto-scroll
/// `use_effect` watching `blocks.read().len()`.
///
/// Plan 04: Submit sends messages over WebSocket to real AgentLoop.
/// Streaming deltas update block stream in real-time. Tool calls
/// update agent panel. No mock data in production path.
#[component]
pub fn WarpHermes() -> Element {
    // ── 12 Phase 4 signals (D-01) — blocks and messages start empty (no demo data). ──
    let mut input = use_signal(String::new);
    let mut blocks = use_signal(Vec::<BlockEntry>::new);
    let mut messages = use_signal(Vec::<Message>::new);
    #[allow(unused_mut)] // required for mode.set via Callback closure capture
    #[allow(unused_mut)] // required for mode.set via Callback closure capture
    let mut mode = use_signal(|| Mode::Shell);
    let mut pal_open = use_signal(|| false);
    let mut pal_query = use_signal(String::new);
    let mut pal_state = use_signal(|| PaletteState::Browse);
    let mut scanner_active = use_signal(|| false);
    let focused = use_signal(|| false);
    let active_tab = use_signal(|| 0_usize);
    let mut tokens = use_signal(|| TokenBudget {
        used: 0,
        max: 128_000,
    });
    let mut next_id = use_signal(|| 1000_u64);

    // ── Session ID signal — created once on mount via server function. ──
    let mut session_id = use_signal(|| "pending".to_string());

    // Create session on mount.
    use_effect(move || {
        spawn(async move {
            match crate::server::api::create_session().await {
                Ok(sid) => session_id.set(sid),
                Err(e) => {
                    // Log error to console on wasm, stderr on native.
                    #[cfg(target_arch = "wasm32")]
                    web_sys::console::log_1(&format!("Session creation failed: {e}").into());
                    let _ = e;
                }
            }
        });
    });

    // ── Track current streaming block id — accumulates deltas into one Block::Ai. ──
    let mut streaming_block_id = use_signal(|| Option::<u64>::None);

    // ── WebSocket connection to server for streaming chat. ──
    let mut ws = dioxus_fullstack::use_websocket(move || {
        crate::server::ws::ws_chat(
            dioxus_fullstack::WebSocketOptions::new().with_automatic_reconnect(),
        )
    });

    // ── WebSocket receiver loop — processes ChatStreamEvent from server. ──
    // Runs in a use_future so it continuously reads from the WS.
    // Uses recv_raw/send_raw with manual JSON serialization to avoid
    // type inference issues with generic Websocket<In, Out> parameters.
    use_future(move || async move {
        let mut disconnect_notified = false;

        loop {
            let state = ws.connect().await;
            if ws.is_err() {
                scanner_active.set(false);
                streaming_block_id.set(None);

                if !disconnect_notified {
                    #[cfg(target_arch = "wasm32")]
                    web_sys::console::warn_1(
                        &format!("WebSocket connect failed; retrying: {:?}", state).into(),
                    );
                    #[cfg(not(target_arch = "wasm32"))]
                    eprintln!("WebSocket connect failed; retrying: {:?}", state);

                    push_disconnect_notice(&mut blocks, &mut next_id);
                    disconnect_notified = true;
                }

                continue;
            }

            loop {
                match ws.recv_raw().await {
                    Ok(raw_msg) => {
                        // Extract text from the raw message.
                        let msg_text = match raw_msg {
                            dioxus_fullstack::Message::Text(t) => {
                                disconnect_notified = false;
                                t
                            }
                            dioxus_fullstack::Message::Binary(b) => {
                                disconnect_notified = false;
                                String::from_utf8_lossy(&b).to_string()
                            }
                            dioxus_fullstack::Message::Close { .. } => {
                                scanner_active.set(false);
                                streaming_block_id.set(None);

                                if !disconnect_notified {
                                    #[cfg(target_arch = "wasm32")]
                                    web_sys::console::warn_1(
                                        &"WebSocket closed; reconnecting".into(),
                                    );
                                    #[cfg(not(target_arch = "wasm32"))]
                                    eprintln!("WebSocket closed; reconnecting");

                                    push_disconnect_notice(&mut blocks, &mut next_id);
                                    disconnect_notified = true;
                                }

                                break;
                            }
                            _ => continue, // Skip ping/pong
                        };

                        // Parse the JSON-encoded ChatStreamEvent.
                        let event: crate::protocol::ChatStreamEvent =
                            match serde_json::from_str(&msg_text) {
                                Ok(e) => e,
                                Err(_) => continue, // Skip malformed messages
                            };

                        match event {
                            crate::protocol::ChatStreamEvent::Delta { text } => {
                                // Accumulate deltas into a single Block::Ai entry.
                                let current_streaming_id = streaming_block_id();
                                if let Some(sid) = current_streaming_id {
                                    // Append to existing streaming block.
                                    let mut bs = blocks.write();
                                    if let Some(entry) = bs.iter_mut().find(|e| e.id == sid) {
                                        if let Block::Ai {
                                            ref mut markdown, ..
                                        } = entry.block
                                        {
                                            markdown.push_str(&text);
                                        }
                                    }
                                } else {
                                    // First delta — create a new Block::Ai.
                                    let id = {
                                        let cur = next_id();
                                        next_id.set(cur + 1);
                                        cur
                                    };
                                    streaming_block_id.set(Some(id));
                                    blocks.write().push(BlockEntry {
                                        id,
                                        block: Block::Ai {
                                            author: Some("Hermes".into()),
                                            time: Some(now_time()),
                                            markdown: text,
                                        },
                                    });
                                }
                            }
                            crate::protocol::ChatStreamEvent::ToolCallStart { name, args } => {
                                // Add a Tool block to the block stream.
                                let id = {
                                    let cur = next_id();
                                    next_id.set(cur + 1);
                                    cur
                                };
                                blocks.write().push(BlockEntry {
                                    id,
                                    block: Block::Tool {
                                        call: UiToolCall {
                                            name: name.clone(),
                                            args_summary: args.clone(),
                                            status: ToolStatus::Running,
                                        },
                                    },
                                });
                                // Also add to messages for agent panel.
                                messages.write().push(Message {
                                    who: "hermes".into(),
                                    time: now_time(),
                                    body: String::new(),
                                    tool: Some(UiToolCall {
                                        name,
                                        args_summary: args,
                                        status: ToolStatus::Running,
                                    }),
                                });
                            }
                            crate::protocol::ChatStreamEvent::ToolCallEnd { name, success } => {
                                // Update the last matching tool call block status.
                                let new_status = if success {
                                    ToolStatus::Done
                                } else {
                                    ToolStatus::Failed
                                };
                                {
                                    let mut bs = blocks.write();
                                    for entry in bs.iter_mut().rev() {
                                        if let Block::Tool { ref mut call } = entry.block {
                                            if call.name == name && call.status == ToolStatus::Running {
                                                call.status = new_status.clone();
                                                break;
                                            }
                                        }
                                    }
                                }
                                // Also update in messages.
                                {
                                    let mut ms = messages.write();
                                    for msg in ms.iter_mut().rev() {
                                        if let Some(ref mut tool) = msg.tool {
                                            if tool.name == name && tool.status == ToolStatus::Running {
                                                tool.status = new_status;
                                                break;
                                            }
                                        }
                                    }
                                }
                            }
                            crate::protocol::ChatStreamEvent::Finished { total_tokens } => {
                                // Update token budget.
                                tokens.set(TokenBudget {
                                    used: total_tokens,
                                    max: 128_000,
                                });
                                scanner_active.set(false);
                                // Reset streaming block id for next turn.
                                streaming_block_id.set(None);
                                // Push the accumulated AI response to messages.
                                let ai_text: Option<String> = {
                                    let bs = blocks.read();
                                    bs.iter().rev().find_map(|e| match &e.block {
                                        Block::Ai { markdown, .. } if !markdown.is_empty() => {
                                            Some(markdown.clone())
                                        }
                                        _ => None,
                                    })
                                };
                                if let Some(text) = ai_text {
                                    messages.write().push(Message {
                                        who: "hermes".into(),
                                        time: now_time(),
                                        body: text,
                                        tool: None,
                                    });
                                }
                            }
                            crate::protocol::ChatStreamEvent::Error { message } => {
                                let id = {
                                    let cur = next_id();
                                    next_id.set(cur + 1);
                                    cur
                                };
                                blocks.write().push(BlockEntry {
                                    id,
                                    block: Block::Err {
                                        author: Some("hermes".into()),
                                        time: Some(now_time()),
                                        exit_code: 1,
                                        message,
                                    },
                                });
                                scanner_active.set(false);
                                streaming_block_id.set(None);
                            }
                        }
                    }
                    Err(err) => {
                        scanner_active.set(false);
                        streaming_block_id.set(None);

                        if !disconnect_notified {
                            #[cfg(target_arch = "wasm32")]
                            web_sys::console::warn_1(
                                &format!("WebSocket receive failed; reconnecting: {err}").into(),
                            );
                            #[cfg(not(target_arch = "wasm32"))]
                            eprintln!("WebSocket receive failed; reconnecting: {err}");

                            push_disconnect_notice(&mut blocks, &mut next_id);
                            disconnect_notified = true;
                        }

                        break;
                    }
                }
            }
        }
    });

    // ── Fetch real data from server functions (Plan 03 wiring). ──

    // Fetch real slash commands from CommandRouter via server function.
    let slash_commands = use_server_future(move || crate::server::api::list_slash_commands())?;

    // Fetch real config (model/provider/context_length) from server.
    let config_summary = use_server_future(move || crate::server::api::get_config_summary())?;

    // Fetch real sessions from StateStore via server function.
    let sessions = use_server_future(move || crate::server::api::list_sessions())?;

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
        _ => vec![Tab {
            label: "New Session".into(),
            live: true,
        }],
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
            let Some(window) = web_sys::window() else {
                return;
            };
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
            let _ = window.add_event_listener_with_callback("keydown", cb.as_ref().unchecked_ref());
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
                        if let Ok(Some(el)) =
                            doc.query_selector(".wh-stream-scroll .wh-block:last-child")
                        {
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

    // ── on_rerun handler (D-24): re-run a Cmd block by id via WebSocket. ──
    let mut on_rerun = move |id: u64| {
        // Find the entry; clone tokens out before any spawn — no read borrow held across spawn body.
        let cmd_text: Option<String> = {
            let bs = blocks.read();
            bs.iter().find(|e| e.id == id).and_then(|e| match &e.block {
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
            scanner_active.set(true);
            streaming_block_id.set(None);
            // Send via WebSocket.
            let sid = session_id();
            let req = crate::protocol::ChatRequest {
                session_id: sid,
                message: text,
            };
            let json = serde_json::to_string(&req).unwrap_or_default();
            spawn(async move {
                let _ = ws.send_raw(dioxus_fullstack::Message::Text(json)).await;
            });
        }
    };

    // ── submit handler (Plan 04: sends via WebSocket to real AgentLoop). ──
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
        scanner_active.set(true);
        streaming_block_id.set(None);

        // Add user's command/message to the block stream immediately.
        {
            let id = {
                let cur = next_id();
                next_id.set(cur + 1);
                cur
            };
            let cur_mode = mode();
            match cur_mode {
                Mode::Shell => {
                    // Tokenize as a shell command block.
                    let tokens_vec: Vec<crate::state::Token> = {
                        let mut iter = text.split_whitespace();
                        let mut out = Vec::new();
                        if let Some(first) = iter.next() {
                            out.push(crate::state::Token::Bin(first.into()));
                        }
                        for tok in iter {
                            if tok.starts_with('-') {
                                out.push(crate::state::Token::Flag(tok.into()));
                            } else {
                                out.push(crate::state::Token::Arg(tok.into()));
                            }
                        }
                        out
                    };
                    blocks.write().push(BlockEntry {
                        id,
                        block: Block::Cmd {
                            command: crate::state::CommandLine {
                                tokens: tokens_vec,
                                time: Some("…".into()),
                                cwd: None,
                                glyph: Some("❯".into()),
                            },
                        },
                    });
                }
                Mode::Agent => {
                    // In agent mode, show the user message as an Out block.
                    blocks.write().push(BlockEntry {
                        id,
                        block: Block::Out {
                            author: Some("you".into()),
                            time: Some(now_time()),
                            text: text.clone(),
                        },
                    });
                }
            }
        }

        // Add to agent panel messages.
        messages.write().push(Message {
            who: "user".into(),
            time: now_time(),
            body: text.clone(),
            tool: None,
        });

        // Send ChatRequest over WebSocket.
        let sid = session_id();
        let req = crate::protocol::ChatRequest {
            session_id: sid,
            message: text,
        };
        let json = serde_json::to_string(&req).unwrap_or_default();
        spawn(async move {
            let _ = ws.send_raw(dioxus_fullstack::Message::Text(json)).await;
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
                // Build status text from the already-fetched config summary (real data).
                let status_text = match config_summary() {
                    Some(Ok(ref cfg)) => format!(
                        "IronHermes Status\n\
                         ────────────────────────────────────────\n  \
                         Model:    {}\n  \
                         Provider: {}\n  \
                         Context:  {} tokens",
                        cfg.model, cfg.provider, cfg.context_length,
                    ),
                    _ => "IronHermes Status\n  Loading...".to_string(),
                };
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
                        text: status_text,
                    },
                });
                pal_open.set(false);
                pal_query.set(String::new());
            }
            "/help" => {
                // Format slash commands from PALETTE_ITEMS (D-29).
                let mut help_text = String::from("Available commands\n");
                for it in palette_items_for_pick
                    .iter()
                    .filter(|p| p.section == "slash")
                {
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
