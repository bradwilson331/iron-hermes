//! `hermes kanban boards` subcommand surface (Phase 36.3.7.9 Plan 03).
//!
//! Six leaf verbs: list, create, switch, show, rename, rm.
//! All filesystem mutations live here; store operations delegate to
//! `ironhermes_kanban::KanbanStore`.

use anyhow::{Context, Result};
use clap::Subcommand;
use ironhermes_kanban::{KanbanStore, board, paths};

// ---------------------------------------------------------------------------
// BoardsCommands clap enum
// ---------------------------------------------------------------------------

/// Manage boards (multi-project isolation).
#[derive(Subcommand)]
pub enum BoardsCommands {
    /// List all boards (* = active)
    #[command(name = "list")]
    List {
        /// Output as JSON array
        #[arg(long)]
        json: bool,
    },

    /// Create a new named board
    #[command(name = "create")]
    Create {
        /// Board slug (lowercase alnum + - + _)
        slug: String,
        /// Human-readable display name
        #[arg(long)]
        name: Option<String>,
        /// Board description
        #[arg(long)]
        description: Option<String>,
        /// Switch to this board after creating it
        #[arg(long = "switch")]
        switch: bool,
        /// Output result as JSON
        #[arg(long)]
        json: bool,
    },

    /// Switch the active board (writes ~/.ironhermes/kanban/current)
    #[command(name = "switch")]
    Switch {
        /// Board slug to switch to
        slug: String,
    },

    /// Show the resolved active board context
    #[command(name = "show")]
    Show {
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },

    /// Rename a board's display name (slug is immutable)
    #[command(name = "rename")]
    Rename {
        /// Board slug (immutable identifier)
        slug: String,
        /// New display name
        new_name: String,
    },

    /// Remove a board (archives by default; --delete for hard removal)
    #[command(name = "rm")]
    Rm {
        /// Board slug to remove
        slug: String,
        /// Permanently delete instead of archiving (refuses if open tasks exist)
        #[arg(long)]
        delete: bool,
    },
}

// ---------------------------------------------------------------------------
// cmd_boards_list
// ---------------------------------------------------------------------------

pub async fn cmd_boards_list(json: bool) -> Result<i32> {
    use ironhermes_kanban::paths::{board_dir, list_boards};

    let ctx = board::resolve_board_context(None).context("Failed to resolve board context")?;
    let named = list_boards().context("Failed to read boards directory")?;

    // Synthesize the display set: "default" always first, then sorted named boards.
    let mut all: Vec<String> = vec!["default".to_string()];
    all.extend(named);

    if json {
        let items: Vec<serde_json::Value> = all
            .iter()
            .map(|slug| {
                let active = slug == &ctx.slug;
                let db_path = ironhermes_kanban::paths::board_db_path_for_slug(slug);
                let display_name = read_board_display_name(slug);
                serde_json::json!({
                    "slug": slug,
                    "db_path": db_path.to_string_lossy(),
                    "active": active,
                    "display_name": display_name,
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&items)?);
    } else {
        for slug in &all {
            let active = slug == &ctx.slug;
            let prefix = if active { "* " } else { "  " };
            let display = read_board_display_name(slug)
                .map(|n| format!("  {}", n))
                .unwrap_or_default();
            println!("{}{}{}", prefix, slug, display);
        }
    }

    Ok(0)
}

// ---------------------------------------------------------------------------
// cmd_boards_create
// ---------------------------------------------------------------------------

pub async fn cmd_boards_create(
    slug: String,
    name: Option<String>,
    description: Option<String>,
    switch: bool,
    _json: bool,
) -> Result<i32> {
    // Pitfall 3: CLI-layer reject for "default"
    if slug == "default" {
        eprintln!("'default' is the built-in board; choose a different slug");
        return Ok(1);
    }

    // T-1: validate slug at CLI boundary (defense in depth)
    let validated_slug =
        board::validate_board_slug(&slug).map_err(|e| anyhow::anyhow!("{}", e))?;

    let dir = paths::board_dir(&validated_slug);

    // Idempotent guard
    if dir.exists() {
        eprintln!(
            "board '{}' already exists at {}",
            validated_slug,
            dir.display()
        );
        return Ok(1);
    }

    // Step 1: create directory
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("create board dir {}", dir.display()))?;

    // Step 2: initialize DB (T-3 ordering: dir first, then DB, then current file)
    KanbanStore::open_for_board(&validated_slug).context("Failed to initialize board DB")?;

    // Step 3: write board.toml if name or description provided
    if name.is_some() || description.is_some() {
        write_board_toml(&dir, name.as_deref(), description.as_deref())?;
    }

    // Step 4: write current file ONLY after DB init succeeds (Pitfall 8 / T-3)
    if switch {
        write_current_board_atomic(&validated_slug)?;
    }

    let switch_suffix = if switch { " (now current)" } else { "" };
    println!(
        "created board '{}' at {}{}",
        validated_slug,
        dir.display(),
        switch_suffix
    );

    Ok(0)
}

// ---------------------------------------------------------------------------
// cmd_boards_switch
// ---------------------------------------------------------------------------

pub async fn cmd_boards_switch(slug: String) -> Result<i32> {
    let validated_slug =
        board::validate_board_slug(&slug).map_err(|e| anyhow::anyhow!("{}", e))?;

    // "default" is always valid — no on-disk dir to verify
    if validated_slug != "default" {
        let dir = paths::board_dir(&validated_slug);
        if !dir.exists() {
            eprintln!(
                "board '{}' does not exist; create it with `boards create`",
                validated_slug
            );
            return Ok(1);
        }
    }

    write_current_board_atomic(&validated_slug)?;
    println!("switched to board '{}'", validated_slug);

    Ok(0)
}

// ---------------------------------------------------------------------------
// cmd_boards_show
// ---------------------------------------------------------------------------

pub async fn cmd_boards_show(json: bool) -> Result<i32> {
    let ctx = board::resolve_board_context(None).context("Failed to resolve board context")?;

    if json {
        let obj = serde_json::json!({
            "slug": ctx.slug,
            "db_path": ctx.db_path.to_string_lossy(),
            "source": ctx.source.as_str(),
        });
        println!("{}", serde_json::to_string_pretty(&obj)?);
    } else {
        println!("slug:    {}", ctx.slug);
        println!("db_path: {}", ctx.db_path.display());
        println!("source:  {}", ctx.source.as_str());
    }

    Ok(0)
}

// ---------------------------------------------------------------------------
// cmd_boards_rename
// ---------------------------------------------------------------------------

pub async fn cmd_boards_rename(slug: String, new_name: String) -> Result<i32> {
    let validated_slug =
        board::validate_board_slug(&slug).map_err(|e| anyhow::anyhow!("{}", e))?;

    if validated_slug == "default" {
        eprintln!("cannot rename the built-in 'default' board");
        return Ok(1);
    }

    let dir = paths::board_dir(&validated_slug);
    if !dir.exists() {
        eprintln!("board '{}' does not exist", validated_slug);
        return Ok(1);
    }

    // Read existing board.toml to preserve description field (if any)
    let existing_desc = read_board_description(&dir);

    write_board_toml(&dir, Some(&new_name), existing_desc.as_deref())?;

    println!(
        "renamed '{}' display name to '{}'",
        validated_slug, new_name
    );

    Ok(0)
}

// ---------------------------------------------------------------------------
// cmd_boards_rm
// ---------------------------------------------------------------------------

pub async fn cmd_boards_rm(slug: String, delete: bool) -> Result<i32> {
    let validated_slug =
        board::validate_board_slug(&slug).map_err(|e| anyhow::anyhow!("{}", e))?;

    // D-01 invariant: cannot remove the built-in default board
    if validated_slug == "default" {
        eprintln!("cannot remove the built-in 'default' board");
        return Ok(1);
    }

    let board_path = paths::board_dir(&validated_slug);

    // T-6: symlink check BEFORE any mutation
    match std::fs::symlink_metadata(&board_path) {
        Ok(meta) => {
            if meta.file_type().is_symlink() {
                eprintln!(
                    "refusing to operate on '{}': path is a symbolic link",
                    validated_slug
                );
                return Ok(1);
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            // Board doesn't exist — handled below after symlink check
        }
        Err(e) => {
            return Err(anyhow::anyhow!("failed to stat board path: {}", e));
        }
    }

    if !board_path.exists() {
        eprintln!("board '{}' does not exist", validated_slug);
        return Ok(1);
    }

    if delete {
        // T-7: advisory lock via create_new — prevents concurrent rm operations
        let lock_path = board_path.join("kanban.db.rm-lock");
        let _lock_file = match std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&lock_path)
        {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                eprintln!(
                    "another rm in progress for '{}'; remove kanban.db.rm-lock if stale",
                    validated_slug
                );
                return Ok(1);
            }
            Err(e) => {
                return Err(anyhow::anyhow!("failed to create rm lock: {}", e));
            }
        };

        // Pre-flight count inside the lock (D-07)
        let count = {
            let store = KanbanStore::open_for_board(&validated_slug)
                .context("Failed to open board for pre-flight count")?;
            store
                .open_tasks_count()
                .context("Failed to count open tasks")?
        };

        if count > 0 {
            // Drop the lock file (it's inside board_path, will be cleaned up manually)
            drop(_lock_file);
            let _ = std::fs::remove_file(&lock_path);
            eprintln!(
                "board '{}' has {} open task(s); refusing hard delete. Archive instead (omit --delete) or close tasks first.",
                validated_slug, count
            );
            return Ok(1);
        }

        // count == 0: remove the entire directory (lock file is inside and removed too)
        std::fs::remove_dir_all(&board_path)
            .with_context(|| format!("delete board dir {}", board_path.display()))?;
        println!("deleted board '{}'", validated_slug);
    } else {
        // Archive path (default)
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let archive = paths::archive_dir();
        let dest = archive.join(format!("{}-{}", validated_slug, ts));

        std::fs::create_dir_all(&archive)
            .with_context(|| format!("create archive dir {}", archive.display()))?;
        std::fs::rename(&board_path, &dest)
            .with_context(|| format!("archive board '{}' to {}", validated_slug, dest.display()))?;

        println!("archived {} -> _archived/{}-{}", validated_slug, validated_slug, ts);
    }

    Ok(0)
}

// ---------------------------------------------------------------------------
// Private helpers
// ---------------------------------------------------------------------------

/// Atomically write the slug to the current-board file via tmp+rename (T-3 / PATTERNS §D).
fn write_current_board_atomic(slug: &str) -> Result<()> {
    let current = paths::current_board_file();
    // Ensure parent dir exists
    if let Some(parent) = current.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create kanban dir {}", parent.display()))?;
    }
    let tmp = current.with_extension("tmp");
    std::fs::write(&tmp, slug.as_bytes())
        .with_context(|| format!("write tmp current-board file {}", tmp.display()))?;
    std::fs::rename(&tmp, &current)
        .with_context(|| format!("atomic rename current-board file {}", current.display()))?;
    Ok(())
}

/// Write board.toml with the given name and description fields.
/// Uses atomic tmp+rename for crash safety.
fn write_board_toml(dir: &std::path::Path, name: Option<&str>, description: Option<&str>) -> Result<()> {
    let toml_path = dir.join("board.toml");
    let tmp_path = dir.join("board.toml.tmp");

    let mut content = String::new();
    if let Some(n) = name {
        // Escape any double-quotes in name
        let escaped = n.replace('\\', "\\\\").replace('"', "\\\"");
        content.push_str(&format!("name = \"{}\"\n", escaped));
    }
    if let Some(d) = description {
        let escaped = d.replace('\\', "\\\\").replace('"', "\\\"");
        content.push_str(&format!("description = \"{}\"\n", escaped));
    }

    std::fs::write(&tmp_path, content.as_bytes())
        .with_context(|| format!("write tmp board.toml {}", tmp_path.display()))?;
    std::fs::rename(&tmp_path, &toml_path)
        .with_context(|| format!("rename board.toml {}", toml_path.display()))?;
    Ok(())
}

/// Read the display name from board.toml (if present).
fn read_board_display_name(slug: &str) -> Option<String> {
    if slug == "default" {
        return None;
    }
    let path = paths::board_dir(slug).join("board.toml");
    let content = std::fs::read_to_string(path).ok()?;
    for line in content.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("name") {
            let rest = rest.trim_start_matches(|c: char| c.is_whitespace() || c == '=');
            let value = rest.trim_matches('"');
            return Some(value.to_string());
        }
    }
    None
}

/// Read the description field from board.toml (if present).
fn read_board_description(dir: &std::path::Path) -> Option<String> {
    let path = dir.join("board.toml");
    let content = std::fs::read_to_string(path).ok()?;
    for line in content.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("description") {
            let rest = rest.trim_start_matches(|c: char| c.is_whitespace() || c == '=');
            let value = rest.trim_matches('"');
            return Some(value.to_string());
        }
    }
    None
}
