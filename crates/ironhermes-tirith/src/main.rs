// SPDX-License-Identifier: AGPL-3.0-only
//
// ironhermes-tirith: a long-lived detection sidecar.
//
// This program links tirith-core 0.4.2 (AGPL-3.0-only) and is distributed as a
// SEPARATE program from the MIT-licensed IronHermes binaries. It talks to them
// only through newline-delimited JSON on stdio, defined by the MIT crate
// `ironhermes-security-proto`. See LICENSE and NOTICE in this crate directory.
//
// Protocol: one JSON `Request` per stdin line, exactly one JSON `Response` per
// request on stdout, keyed by the request `id`. A malformed line is answered
// with `{"ok":false,"err":"protocol_error"}` and the loop keeps serving.
//
// Anti-bypass: analysis goes through
// `engine::analyze_without_bypass_returning_policy` with `interactive: false`,
// so an inline `TIRITH=0 <cmd>` prefix in agent-controlled text is never
// honoured. Raw tirith severities are returned; tirith's own `action` is never
// consulted (the caller applies its own severity x DEFCON table).
//
// This binary never reads `TIRITH`, `TIRITH_SERVER_URL` or `TIRITH_API_KEY`;
// the caller spawns it with a scrubbed environment.

use std::io::{self, BufRead, Write};
use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::time::Instant;

use ironhermes_security_proto::{
    ERR_INTERNAL, ERR_PROTOCOL, ERR_TOO_LARGE, ERR_UNSUPPORTED_SUBJECT, MAX_SUBJECT_BYTES, Op,
    PROTOCOL_VERSION, Request, Response, SidecarVersion, SubjectKind, WireFinding, WireSeverity,
};
use tirith_core::clipboard::ClipboardSourceState;
use tirith_core::content_kind::{self, ContentKind};
use tirith_core::engine::{self, AnalysisContext};
use tirith_core::extract::ScanContext;
use tirith_core::redact::{self, CompiledCustomPatterns};
use tirith_core::scan::{self, GuardedScanOutcome, ScanFileOutcome};
use tirith_core::tokenize::ShellType;
use tirith_core::verdict::{self, Finding, Severity};

/// The pinned tirith-core version (kept equal to the `=0.4.2` pin in Cargo.toml;
/// the round-trip test cross-checks it against Cargo.lock).
const TIRITH_CORE_VERSION: &str = "0.4.2";

/// A request line longer than this is answered `too_large` without parsing.
const MAX_LINE_BYTES: usize = MAX_SUBJECT_BYTES + 4096;

/// Suffixes the sidecar refuses for `file` subjects: PDFs, archives and binaries
/// are never handed to tirith (RUSTSEC-2026-0187 stays unreachable).
const REFUSED_SUFFIXES: &[&str] = &[
    "pdf", "zip", "tar", "gz", "tgz", "bz2", "xz", "zst", "7z", "rar", "whl", "jar", "exe", "dll",
    "so", "dylib", "bin",
];

enum LineRead {
    Eof,
    Line(Vec<u8>),
    TooLong,
}

/// Read one `\n`-terminated line, keeping at most `cap` bytes. A longer line is
/// drained to its terminator and reported as [`LineRead::TooLong`], so the stream
/// stays in sync and memory stays bounded.
fn read_bounded_line<R: BufRead>(reader: &mut R, cap: usize) -> io::Result<LineRead> {
    let mut buf: Vec<u8> = Vec::new();
    let mut overflow = false;
    let mut saw_any = false;
    loop {
        let chunk = reader.fill_buf()?;
        if chunk.is_empty() {
            return Ok(if !saw_any {
                LineRead::Eof
            } else if overflow {
                LineRead::TooLong
            } else {
                LineRead::Line(buf)
            });
        }
        saw_any = true;
        let (take, found_newline) = match chunk.iter().position(|b| *b == b'\n') {
            Some(i) => (i, true),
            None => (chunk.len(), false),
        };
        if !overflow {
            if buf.len() + take > cap {
                overflow = true;
                buf.clear();
            } else {
                buf.extend_from_slice(&chunk[..take]);
            }
        }
        let consume = if found_newline { take + 1 } else { take };
        reader.consume(consume);
        if found_newline {
            return Ok(if overflow {
                LineRead::TooLong
            } else {
                LineRead::Line(buf)
            });
        }
    }
}

fn wire_severity(sev: Severity) -> WireSeverity {
    match sev {
        Severity::Info => WireSeverity::Info,
        Severity::Low => WireSeverity::Low,
        Severity::Medium => WireSeverity::Medium,
        Severity::High => WireSeverity::High,
        Severity::Critical => WireSeverity::Critical,
    }
}

/// Map findings to wire form: rule id, raw severity, title. Evidence and
/// description text never cross the wire.
fn to_wire(findings: &[Finding]) -> Vec<WireFinding> {
    findings
        .iter()
        .map(|f| WireFinding {
            rule: format!("{:?}", f.rule_id),
            sev: wire_severity(f.severity),
            title: f.title.clone(),
        })
        .collect()
}

fn analyze_text(id: u64, kind: SubjectKind, text: &str) -> Response {
    let started = Instant::now();
    let (scan_context, raw_bytes) = match kind {
        SubjectKind::Exec => (ScanContext::Exec, None),
        // Paste context runs the byte scan (ANSI, bidi, zero-width, Unicode tags).
        _ => (ScanContext::Paste, Some(text.as_bytes().to_vec())),
    };
    let ctx = AnalysisContext {
        input: text.to_string(),
        shell: ShellType::Posix,
        scan_context,
        raw_bytes,
        // Non-interactive: bypass is off by default, and the no-bypass entry
        // point below ignores inline `TIRITH=0` regardless.
        interactive: false,
        cwd: None,
        file_path: None,
        repo_root: None,
        is_config_override: false,
        clipboard_html: None,
        card_ref: None,
        clipboard_source: ClipboardSourceState::Unread,
    };
    let (mut verdict, _policy) = engine::analyze_without_bypass_returning_policy(&ctx);
    redact::redact_verdict_with_compiled(&mut verdict, &CompiledCustomPatterns::new_silent(&[]));
    verdict::bound_verdict_for_output(&mut verdict);

    let mut resp = Response::ok(id);
    resp.findings = to_wire(&verdict.findings);
    resp.tier = Some(verdict.tier_reached);
    resp.ms = Some(started.elapsed().as_secs_f64() * 1000.0);
    resp
}

/// A bare basename: no separator, no `..`, no NUL, not empty or `.`.
fn is_bare_basename(name: &str) -> bool {
    !name.is_empty() && name != "." && !name.contains(['/', '\\', '\0']) && !name.contains("..")
}

fn has_refused_suffix(name: &str) -> bool {
    Path::new(name)
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .is_some_and(|ext| REFUSED_SUFFIXES.contains(&ext.as_str()))
}

/// Private scratch directory under `$HOME`, created 0700.
fn scratch_dir() -> io::Result<PathBuf> {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| io::Error::other("HOME is not set"))?;
    let dir = home.join(format!(".ironhermes-tirith-scratch-{}", std::process::id()));
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(&dir)?;
    Ok(dir)
}

fn analyze_file(id: u64, name: Option<&str>, text: &str) -> Response {
    let Some(name) = name else {
        return Response::err(id, ERR_PROTOCOL);
    };
    if !is_bare_basename(name) || has_refused_suffix(name) {
        return Response::err(id, ERR_UNSUPPORTED_SUBJECT);
    }
    // Defence in depth against a PDF/archive/binary hidden behind a text name:
    // use tirith's own byte classifier and require plain text.
    let class = content_kind::classify_with_ambiguity(text.as_bytes());
    if class.kind != ContentKind::Text || class.pdf_header_offset.is_some() {
        return Response::err(id, ERR_UNSUPPORTED_SUBJECT);
    }

    let started = Instant::now();
    let Ok(dir) = scratch_dir() else {
        return Response::err(id, ERR_INTERNAL);
    };
    let path = dir.join(name);
    if std::fs::write(&path, text.as_bytes()).is_err() {
        return Response::err(id, ERR_INTERNAL);
    }
    let outcome = scan::scan_single_file_guarded(&path);
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_dir(&dir);

    let mut result = match outcome {
        GuardedScanOutcome::Completed(ScanFileOutcome::Scanned(r)) => r,
        // Skipped or panicked: the file was not analysed, which must never read
        // as clean.
        GuardedScanOutcome::Completed(ScanFileOutcome::Skipped(_))
        | GuardedScanOutcome::RulePanic(_) => return Response::err(id, ERR_INTERNAL),
    };
    if result.findings.is_empty() && result.analysis_incomplete() {
        return Response::err(id, ERR_INTERNAL);
    }
    redact::redact_findings_with_compiled(
        &mut result.findings,
        &CompiledCustomPatterns::new_silent(&[]),
    );
    verdict::bound_findings_for_output(&mut result.findings);

    let mut resp = Response::ok(id);
    resp.findings = to_wire(&result.findings);
    resp.ms = Some(started.elapsed().as_secs_f64() * 1000.0);
    resp
}

fn handle(req: Request) -> Response {
    match req.op {
        Op::Ping => Response::ok(req.id),
        Op::Version => {
            let mut resp = Response::ok(req.id);
            resp.version = Some(SidecarVersion {
                sidecar: env!("CARGO_PKG_VERSION").to_string(),
                tirith_core: TIRITH_CORE_VERSION.to_string(),
                protocol: PROTOCOL_VERSION,
                // Research OQ5: no tirith threat database ships in this phase.
                threat_db: "absent".to_string(),
            });
            resp
        }
        Op::Check => {
            let (Some(kind), Some(text)) = (req.kind, req.text.as_deref()) else {
                return Response::err(req.id, ERR_PROTOCOL);
            };
            if text.len() > MAX_SUBJECT_BYTES {
                return Response::err(req.id, ERR_TOO_LARGE);
            }
            match kind {
                SubjectKind::Exec | SubjectKind::Paste => analyze_text(req.id, kind, text),
                SubjectKind::File => analyze_file(req.id, req.name.as_deref(), text),
            }
        }
    }
}

/// Run one request, converting a panic inside analysis into an `internal`
/// response so one bad subject cannot kill the loop.
fn handle_guarded(req: Request) -> Response {
    let id = req.id;
    panic::catch_unwind(AssertUnwindSafe(|| handle(req)))
        .unwrap_or_else(|_| Response::err(id, ERR_INTERNAL))
}

fn write_response<W: Write>(out: &mut W, resp: &Response) -> io::Result<()> {
    let mut line = serde_json::to_vec(resp).map_err(io::Error::other)?;
    line.push(b'\n');
    out.write_all(&line)?;
    out.flush()
}

fn serve() -> io::Result<()> {
    let stdin = io::stdin();
    let mut input = stdin.lock();
    let stdout = io::stdout();
    let mut out = stdout.lock();
    loop {
        let resp = match read_bounded_line(&mut input, MAX_LINE_BYTES)? {
            LineRead::Eof => return Ok(()),
            LineRead::TooLong => Response::err(0, ERR_TOO_LARGE),
            LineRead::Line(bytes) => match serde_json::from_slice::<Request>(&bytes) {
                Ok(req) => handle_guarded(req),
                Err(_) => Response::err(0, ERR_PROTOCOL),
            },
        };
        write_response(&mut out, &resp)?;
    }
}

fn main() {
    if std::env::args().nth(1).as_deref() == Some("--version") {
        println!(
            "ironhermes-tirith {} (tirith-core {}, protocol {})",
            env!("CARGO_PKG_VERSION"),
            TIRITH_CORE_VERSION,
            PROTOCOL_VERSION
        );
        return;
    }
    if let Err(e) = serve() {
        eprintln!("ironhermes-tirith: i/o error: {e}");
        std::process::exit(1);
    }
}
