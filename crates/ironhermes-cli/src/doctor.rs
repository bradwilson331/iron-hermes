//! `hermes doctor` — configuration and dependency health check.
//!
//! Extracted from main.rs `cmd_doctor` (Phase 35.1 D-03).
//!
//! Called from `setup::run_setup` at wizard exit and from `main` dispatch.

use anyhow::Result;
use colored::Colorize;
use ironhermes_core::config::Config;

/// Run the IronHermes doctor preflight check.
///
/// Prints a formatted health summary covering home directory, config file,
/// .env file, API keys, state database, and gateway PID liveness. Always
/// returns `Ok(())` — issues are reported as `MISSING` lines, not errors.
pub fn run_doctor_check() -> Result<()> {
    println!("{}", "IronHermes Doctor".bold().cyan());
    // Phase 24 D-16: show which profile this doctor run is inspecting.
    println!("Profile: {}", ironhermes_cli::status_cmd::current_profile());
    println!("{}", "─".repeat(40));

    // Check home directory
    let home = ironhermes_core::get_hermes_home();
    print_check("Home directory", home.exists());

    // Check config
    let config_path = Config::config_path();
    print_check("Config file", config_path.exists());

    // Check .env
    let env_path = Config::env_path();
    print_check(".env file", env_path.exists());

    // Check API keys
    print_check(
        "OpenRouter API key",
        std::env::var("OPENROUTER_API_KEY").is_ok(),
    );
    print_check(
        "Anthropic API key",
        std::env::var("ANTHROPIC_API_KEY").is_ok(),
    );

    // Check state database
    let db_path = home.join("state.db");
    print_check("State database", db_path.exists());

    // Phase 24 D-16: gateway.pid liveness check (active profile only — no
    // cross-profile sweep per the deferred-ideas list).
    let pid_path = home.join("gateway.pid");
    if pid_path.exists() {
        let pid_ok = ironhermes_gateway::pid::read_gateway_pid(&home)
            .ok()
            .flatten()
            .map(|r| {
                matches!(
                    ironhermes_gateway::pid::is_pid_alive(r.pid),
                    ironhermes_gateway::pid::PidLiveness::Live
                        | ironhermes_gateway::pid::PidLiveness::LiveOtherUser
                )
            })
            .unwrap_or(false);
        print_check("Gateway PID (gateway.pid → live process)", pid_ok);
    } else {
        // Absent file = healthy (no gateway running). Use the "OK" branch.
        print_check("Gateway PID (not running)", true);
    }

    println!();
    println!("{}", "Run `ironhermes status` for more details.".dimmed());

    Ok(())
}

fn print_check(name: &str, ok: bool) {
    let icon = if ok { "OK".green() } else { "MISSING".yellow() };
    println!("  [{icon}] {name}");
}
