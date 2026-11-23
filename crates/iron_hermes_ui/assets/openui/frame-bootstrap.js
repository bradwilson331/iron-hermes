// Phase 54.9 Plan 08 (D-13): frame side of the OpenUI host bridge. Runs inside the
// sandboxed (opaque-origin) /openui/frame document after openui-bundle.min.js defines
// window.__OpenUI. Plain ES2020, no imports, no network, no storage.
//
//   host -> frame: {type:"openui-push", text} {type:"openui-reset"} {type:"openui-end"} {type:"openui-theme", vars, mode:"dark"|"light"}
//   frame -> host: {type:"openui-action", action_type, label, form_state}   (the ONLY outbound type)
//
// D-15: Renderer is deliberately given no tool provider, so a generated Query()/Mutation()
// stays inert and a view can never call a tool.
// ADR-0007 "changed": only the continue_conversation (@ToAssistant) action is forwarded;
// every other action type (notably the @OpenUrl link action) is dropped here.
(function () {
  var O = window.__OpenUI;
  var h = O.React.createElement;
  var root = O.createRoot(document.getElementById("openui-root"));
  var text = "";
  var streaming = true;
  // Theme variables the host may set; anything else is ignored.
  var THEME_VARS = ["--bg", "--bg-card", "--text", "--text-dim", "--teal", "--border", "--green", "--red", "--amber", "--font"];

  function onAction(a) {
    if (!a || a.type !== "continue_conversation") return;
    window.parent.postMessage(
      {
        type: "openui-action",
        action_type: String(a.type),
        label: String(a.humanFriendlyMessage || ""),
        form_state: a.formState || {},
      },
      "*"
    );
  }

  // ADR-0007 "Parse states in the frame": judged only once the stream has ended, from the
  // Renderer's own onParseResult / onError, so no second outbound message type is needed.
  //   fallback = nothing parsed (plain prose): show the raw text, pill TEXT FALLBACK
  //   partial  = parsed, but components were skipped or references are unresolved: pill PARTIAL
  var parsed = null;
  var errors = [];
  var lastState = "ok";
  var pill = document.createElement("div");
  pill.style.cssText = "display:none;margin:0 0 8px 0;font-size:12px;line-height:1.4;color:var(--text-dim,#555)";
  var rootEl = document.getElementById("openui-root");
  rootEl.parentNode.insertBefore(pill, rootEl);

  function parseState() {
    if (streaming) return "ok";
    if (!parsed || !parsed.root) return text.trim() ? "fallback" : "ok";
    var unresolved = parsed.meta && parsed.meta.unresolved && parsed.meta.unresolved.length > 0;
    return errors.length > 0 || unresolved ? "partial" : "ok";
  }

  function showPill(state) {
    pill.textContent = "";
    if (state === "ok") {
      pill.style.display = "none";
      return;
    }
    var tag = document.createElement("strong");
    tag.textContent = state === "fallback" ? "TEXT FALLBACK" : "PARTIAL";
    // Amber mixed toward the body text so the label stays >= 4.5:1 on a light and a dark card.
    tag.style.color = "color-mix(in srgb, var(--amber, #b45309) 55%, var(--text, #1d2228))";
    pill.appendChild(tag);
    pill.appendChild(
      document.createTextNode(
        state === "fallback"
          ? " The reply was not valid OpenUI. Showing it as text instead."
          : " Some components were skipped because they were not valid."
      )
    );
    pill.style.display = "block";
  }

  // Renderer callbacks arrive after a render; re-judge, and re-render only when the verdict
  // crosses into or out of the text fallback (that render has no Renderer, so it cannot loop).
  function onParseResult(r) {
    parsed = r || null;
    refresh();
  }
  function onError(e) {
    errors = Array.isArray(e) ? e : [];
    refresh();
  }
  function refresh() {
    var state = parseState();
    if ((state === "fallback") !== (lastState === "fallback")) {
      render();
    } else {
      lastState = state;
      showPill(state);
    }
  }

  function render() {
    var state = parseState();
    lastState = state;
    if (state === "fallback") {
      root.render(h("pre", { style: { whiteSpace: "pre-wrap", margin: 0 } }, text));
    } else {
      root.render(
        h(O.Renderer, {
          response: text,
          library: O.openuiChatLibrary,
          isStreaming: streaming,
          onAction: onAction,
          onParseResult: onParseResult,
          onError: onError,
        })
      );
    }
    showPill(state);
  }

  window.addEventListener("message", function (event) {
    if (event.source !== window.parent) return; // only the embedding host may drive the frame
    var m = event.data;
    if (!m || typeof m !== "object") return;
    if (m.type === "openui-push" && typeof m.text === "string") {
      text += m.text;
      parsed = null;
      errors = [];
      render();
    } else if (m.type === "openui-reset") {
      text = "";
      streaming = true;
      parsed = null;
      errors = [];
      render();
    } else if (m.type === "openui-end") {
      streaming = false;
      render();
    } else if (m.type === "openui-theme" && m.vars && typeof m.vars === "object") {
      THEME_VARS.forEach(function (name) {
        if (typeof m.vars[name] === "string") {
          document.documentElement.style.setProperty(name, m.vars[name]);
        }
      });
      // The stock stylesheet picks its palette from the OS (prefers-color-scheme), which is
      // unrelated to the app theme. Pin it to the app's light/dark so component text and the
      // frame background always come from the same palette.
      if (m.mode === "dark" || m.mode === "light") {
        document.documentElement.setAttribute("data-openui-theme", m.mode);
        document.documentElement.style.colorScheme = m.mode;
      }
    }
  });

  render();
})();
