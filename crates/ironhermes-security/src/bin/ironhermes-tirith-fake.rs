//! Test helper: a fake `ironhermes-tirith` that speaks the plan-01 JSONL protocol
//! and misbehaves on demand. MIT, no tirith code. Never shipped or spawned in
//! production; the supervised-client tests pass `--mode <mode>` through
//! `SidecarOptions::extra_args`.
//!
//! Modes (checks are counted; ping and version never are):
//! - `ok`: answer everything, checks with no findings
//! - `sleep`: read lines, never answer
//! - `garbage`: answer every line with `not json`
//! - `crash-after=N`: behave like `echo`, exit right after answering the Nth check
//! - `delay-first-ms=N`: behave like `echo`, but the first check's answer is sent
//!   from a side thread after N ms (later checks are answered promptly)
//! - `echo`: one finding per check whose title is the request text
//! - `echo-env`: one finding per check for each env var name the child can see
//! - `count`: one finding per check titled `count:<n>`, `n` = checks seen so far
//!   including this one (proves a refused subject was never written)
//! - `fixed=<SEV>`: one finding per check, rule `FakeRule`, that severity
//! - `delay-ms=N`: answer every check like `echo`, but sleep N ms on the main loop
//!   thread first, so checks are answered strictly in order, as tirith does
//! - `err=<code>`: answer every check with the error code `<code>` (for example
//!   `unsupported_subject`, `too_large`, `internal`); the value is a string

use std::io::{self, BufRead, Write};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ironhermes_security_proto::{
    ERR_PROTOCOL, Op, PROTOCOL_VERSION, Request, Response, SidecarVersion, WireFinding,
    WireSeverity,
};

enum Mode {
    Ok,
    Sleep,
    Garbage,
    CrashAfter(u64),
    DelayFirstMs(u64),
    Echo,
    EchoEnv,
    Count,
    Fixed(WireSeverity),
    DelayMs(u64),
    Err(String),
}

fn parse_mode(raw: &str) -> Mode {
    let (name, value) = match raw.split_once('=') {
        Some((n, v)) => (n, Some(v)),
        None => (raw, None),
    };
    let number = || {
        value
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or_else(|| panic!("mode {name} needs a numeric =N"))
    };
    match name {
        "ok" => Mode::Ok,
        "sleep" => Mode::Sleep,
        "garbage" => Mode::Garbage,
        "crash-after" => Mode::CrashAfter(number()),
        "delay-first-ms" => Mode::DelayFirstMs(number()),
        "delay-ms" => Mode::DelayMs(number()),
        "err" => Mode::Err(
            value
                .unwrap_or_else(|| panic!("mode err needs =<code>"))
                .to_string(),
        ),
        "echo" => Mode::Echo,
        "echo-env" => Mode::EchoEnv,
        "count" => Mode::Count,
        "fixed" => {
            let sev = value.unwrap_or_else(|| panic!("mode fixed needs =<SEV>"));
            let sev: WireSeverity = serde_json::from_str(&format!("\"{sev}\""))
                .unwrap_or_else(|_| panic!("unknown severity {sev}"));
            Mode::Fixed(sev)
        }
        other => panic!("unknown mode {other}"),
    }
}

fn mode_from_args() -> Mode {
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--mode" {
            let value = args.next().expect("--mode needs a value");
            return parse_mode(&value);
        }
        if let Some(value) = arg.strip_prefix("--mode=") {
            return parse_mode(value);
        }
    }
    Mode::Ok
}

fn finding(rule: &str, sev: WireSeverity, title: impl Into<String>) -> WireFinding {
    WireFinding {
        rule: rule.to_string(),
        sev,
        title: title.into(),
    }
}

type Out = Arc<Mutex<io::Stdout>>;

fn send_raw(out: &Out, bytes: &[u8]) {
    let mut guard = out.lock().unwrap_or_else(|p| p.into_inner());
    let _ = guard.write_all(bytes);
    let _ = guard.write_all(b"\n");
    let _ = guard.flush();
}

fn send(out: &Out, resp: &Response) {
    let line = serde_json::to_vec(resp).expect("response serialises");
    send_raw(out, &line);
}

fn check_findings(mode: &Mode, text: &str, checks_seen: u64) -> Vec<WireFinding> {
    match mode {
        Mode::Ok | Mode::Sleep | Mode::Garbage | Mode::Err(_) => Vec::new(),
        Mode::Echo | Mode::CrashAfter(_) | Mode::DelayFirstMs(_) | Mode::DelayMs(_) => {
            vec![finding("Echo", WireSeverity::Info, text)]
        }
        Mode::EchoEnv => {
            let mut names: Vec<String> = std::env::vars_os()
                .map(|(k, _)| k.to_string_lossy().into_owned())
                .collect();
            names.sort();
            names
                .into_iter()
                .map(|n| finding("Env", WireSeverity::Info, n))
                .collect()
        }
        Mode::Count => vec![finding(
            "Count",
            WireSeverity::Info,
            format!("count:{checks_seen}"),
        )],
        Mode::Fixed(sev) => vec![finding("FakeRule", *sev, "fixed")],
    }
}

fn main() {
    let mode = mode_from_args();
    let out: Out = Arc::new(Mutex::new(io::stdout()));
    let mut checks_seen: u64 = 0;

    for line in io::stdin().lock().lines() {
        let Ok(line) = line else { break };
        match mode {
            Mode::Sleep => continue,
            Mode::Garbage => {
                send_raw(&out, b"not json");
                continue;
            }
            _ => {}
        }
        let Ok(req) = serde_json::from_str::<Request>(&line) else {
            send(&out, &Response::err(0, ERR_PROTOCOL));
            continue;
        };
        match req.op {
            Op::Ping => send(&out, &Response::ok(req.id)),
            Op::Version => {
                let mut resp = Response::ok(req.id);
                resp.version = Some(SidecarVersion {
                    sidecar: "fake".to_string(),
                    tirith_core: "fake".to_string(),
                    protocol: PROTOCOL_VERSION,
                    threat_db: "absent".to_string(),
                });
                send(&out, &resp);
            }
            Op::Check => {
                checks_seen += 1;
                let text = req.text.as_deref().unwrap_or("");
                if let Mode::DelayMs(ms) = mode {
                    std::thread::sleep(Duration::from_millis(ms));
                }
                if let Mode::Err(code) = &mode {
                    send(&out, &Response::err(req.id, code));
                    continue;
                }
                let mut resp = Response::ok(req.id);
                resp.findings = check_findings(&mode, text, checks_seen);
                match mode {
                    Mode::DelayFirstMs(ms) if checks_seen == 1 => {
                        let out = Arc::clone(&out);
                        std::thread::spawn(move || {
                            std::thread::sleep(Duration::from_millis(ms));
                            send(&out, &resp);
                        });
                    }
                    _ => send(&out, &resp),
                }
                if let Mode::CrashAfter(n) = mode
                    && checks_seen >= n
                {
                    std::process::exit(0);
                }
            }
        }
    }
}
