//! Phase 54.10 Plan 01 (D-01/D-02/D-03/D-08): the Dashboard board store.
//!
//! A board is one JSON file under the ACTIVE profile's home
//! (`<profile home>/dashboard/boards/<uuid>.json`), resolved per call by the ADR-0007 rule shared
//! with saved views (`openui_views_api::profile_home_for_slug`). The schema below is plain serde
//! data outside the `feature = "server"` store, exactly like `OpenUiView`, so the wasm client
//! uses the same types.
//!
//! D-02: an AI widget item stores only the saved view's id plus geometry; the prompt and the
//! snapshot stay on the saved view. D-08: tile settings are typed enums, and every container
//! denies unknown fields, so a hand-edited or stale file with an unknown key fails to load
//! instead of being carried around as an untyped blob. `BOARD_SCHEMA_VERSION` keeps a later
//! format change a versioned migration.

use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

/// Version of the on-disk board format.
pub const BOARD_SCHEMA_VERSION: u32 = 1;

/// One board. Times are unix seconds.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Board {
    pub version: u32,
    pub id: String,
    pub name: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub items: Vec<BoardItem>,
}

/// One item on the canvas: world-unit geometry plus what it shows.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct BoardItem {
    pub id: String,
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
    pub kind: ItemKind,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ItemKind {
    /// A native live tile.
    Tile { tile: TileSpec },
    /// An AI widget: a reference to a saved OpenUI view (D-02).
    Widget { view_id: String },
    /// A free-text note.
    Note { text: String },
}

/// Which native tile, with its typed settings (D-08).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(tag = "tile", rename_all = "snake_case", deny_unknown_fields)]
pub enum TileSpec {
    Usage {
        #[serde(default)]
        window: UsageWindow,
    },
    Sessions {
        #[serde(default)]
        show: SessionsShow,
    },
    Kanban {
        #[serde(default = "all_kanban_columns")]
        columns: Vec<KanbanColumn>,
    },
    Cron {},
    Gateway {},
    Reports {
        #[serde(default)]
        report: Option<String>,
    },
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum UsageWindow {
    #[serde(rename = "24h")]
    Day,
    #[default]
    #[serde(rename = "7d")]
    Week,
    #[serde(rename = "30d")]
    Month,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SessionsShow {
    #[default]
    Recent,
    Active,
}

/// Kanban columns a tile can count. No `failed`: the problem count is `blocked` only
/// (operator resolution 5).
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[repr(u8)]
pub enum KanbanColumn {
    Triage,
    Todo,
    Ready,
    Running,
    Blocked,
    Done,
}

pub(crate) fn all_kanban_columns() -> Vec<KanbanColumn> {
    use KanbanColumn::*;
    vec![Triage, Todo, Ready, Running, Blocked, Done]
}

impl TileSpec {
    /// The tile title shown in the item header (UI-SPEC Copywriting, "Tile titles").
    #[allow(dead_code)] // consumed by the canvas item plans (54.10-02+)
    pub fn title(&self) -> String {
        match self {
            TileSpec::Usage { .. } => "Usage".to_string(),
            TileSpec::Sessions { .. } => "Sessions".to_string(),
            TileSpec::Kanban { .. } => "Kanban".to_string(),
            TileSpec::Cron {} => "Cron".to_string(),
            TileSpec::Gateway {} => "Gateway health".to_string(),
            TileSpec::Reports { report: Some(r) } => format!("Reports: {r}"),
            TileSpec::Reports { report: None } => "Reports".to_string(),
        }
    }
}

/// At most this many boards per profile (T-54.10-02).
pub const BOARDS_MAX: usize = 12;
/// At most this many items per board.
pub const BOARD_ITEMS_MAX: usize = 40;
pub const BOARD_NAME_MAX_CHARS: usize = 40;
pub const NOTE_TEXT_MAX_CHARS: usize = 2000;
/// Largest serialized board file, in bytes (checked by the server store only).
#[cfg(feature = "server")]
pub const BOARD_FILE_MAX_BYTES: usize = 256 * 1024;
/// Item coordinates stay within +-COORD_MAX world units.
pub const COORD_MAX: i32 = 100_000;
/// Item width and height never exceed this.
pub const ITEM_SIDE_MAX: i32 = 4096;

/// UI-SPEC "Board management" copy, shared by the server check and the client pre-check.
pub const BOARD_NAME_ERROR: &str = "Board name must be 1 to 40 characters.";
pub const BOARD_ITEMS_FULL_ERROR: &str =
    "This board is full (40 items). Remove one to add another.";

/// UI-SPEC: `You have {n} boards, the maximum. Delete one to create another.`
pub fn boards_full_error(n: usize) -> String {
    format!("You have {n} boards, the maximum. Delete one to create another.")
}

/// Smallest `(w, h)` an item of this kind may have (UI-SPEC Spacing).
pub fn item_min_size(kind: &ItemKind) -> (i32, i32) {
    match kind {
        ItemKind::Tile { .. } => (192, 144),
        ItemKind::Widget { .. } => (256, 192),
        ItemKind::Note { .. } => (160, 96),
    }
}

/// Default `(w, h)` for a newly added item of this kind.
#[allow(dead_code)] // consumed by the Add menu plans (54.10-02+)
pub fn item_default_size(kind: &ItemKind) -> (i32, i32) {
    match kind {
        ItemKind::Tile { .. } => (320, 240),
        ItemKind::Widget { .. } => (480, 320),
        ItemKind::Note { .. } => (224, 160),
    }
}

/// `^[a-z0-9][a-z0-9._-]{0,63}$` with no `..`. Same rule as the ironhermes-tools report store,
/// duplicated because that crate is native-only and this check must run on wasm too.
pub fn valid_report_name(name: &str) -> bool {
    let mut chars = name.chars();
    let first_ok = chars
        .next()
        .is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit());
    first_ok
        && name.len() <= 64
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || "._-".contains(c))
        && !name.contains("..")
}

fn check_uuid(id: &str, what: &str) -> Result<(), String> {
    uuid::Uuid::parse_str(id)
        .map(|_| ())
        .map_err(|_| format!("Invalid {what} id"))
}

/// Every server-side rule for a board. Pure and wasm-safe so the client can pre-check; the
/// store runs it again before any write (T-54.10-01/02/04).
pub fn validate_board(board: &Board) -> Result<(), String> {
    if board.version != BOARD_SCHEMA_VERSION {
        return Err("Unsupported board version".to_string());
    }
    check_uuid(&board.id, "board")?;
    let name = board.name.trim();
    let name_chars = name.chars().count();
    if name_chars == 0 || name_chars > BOARD_NAME_MAX_CHARS || name.chars().any(char::is_control) {
        return Err(BOARD_NAME_ERROR.to_string());
    }
    if board.items.len() > BOARD_ITEMS_MAX {
        return Err(BOARD_ITEMS_FULL_ERROR.to_string());
    }
    let mut seen = std::collections::HashSet::new();
    for item in &board.items {
        check_uuid(&item.id, "item")?;
        if !seen.insert(item.id.as_str()) {
            return Err("Duplicate item id".to_string());
        }
        if item.x.abs() > COORD_MAX || item.y.abs() > COORD_MAX {
            return Err("An item is outside the board".to_string());
        }
        let (min_w, min_h) = item_min_size(&item.kind);
        if !(min_w..=ITEM_SIDE_MAX).contains(&item.w) || !(min_h..=ITEM_SIDE_MAX).contains(&item.h)
        {
            return Err("An item has an invalid size".to_string());
        }
        match &item.kind {
            ItemKind::Widget { view_id } => check_uuid(view_id, "view")?,
            ItemKind::Note { text } if text.chars().count() > NOTE_TEXT_MAX_CHARS => {
                return Err(format!(
                    "A note is limited to {NOTE_TEXT_MAX_CHARS} characters."
                ));
            }
            ItemKind::Tile {
                tile: TileSpec::Reports { report: Some(r) },
            } if !valid_report_name(r) => return Err("Invalid report name".to_string()),
            ItemKind::Tile {
                tile: TileSpec::Kanban { columns },
            } => {
                let mut cols = std::collections::HashSet::new();
                if !columns.iter().all(|c| cols.insert(*c as u8)) {
                    return Err("Duplicate Kanban column".to_string());
                }
            }
            _ => {}
        }
    }
    Ok(())
}

/// At most this many card ids in a saved Gateway order (T-54.10-22).
pub const GATEWAY_ORDER_MAX: usize = 64;
const GATEWAY_ID_MAX_CHARS: usize = 200;
const GATEWAY_ID_PREFIXES: [&str; 5] =
    ["schedules", "api_server", "platform:", "route:", "teaser:"];

/// Rules for a saved Gateway card order (D-18, T-54.10-22). The ids are JSON content only and
/// never become paths, so this bounds size and shape rather than the route-name charset.
pub fn validate_gateway_order(ids: &[String]) -> Result<(), String> {
    if ids.len() > GATEWAY_ORDER_MAX {
        return Err("Too many cards in the saved order".to_string());
    }
    for id in ids {
        let chars = id.chars().count();
        if chars == 0
            || chars > GATEWAY_ID_MAX_CHARS
            || id.chars().any(char::is_control)
            || !GATEWAY_ID_PREFIXES.iter().any(|p| id.starts_with(p))
        {
            return Err("Invalid card id in the saved order".to_string());
        }
    }
    Ok(())
}

#[cfg(feature = "server")]
mod store {
    use super::*;
    use std::path::{Path, PathBuf};

    /// `<home>/dashboard/boards`.
    pub(crate) fn boards_dir_for_home(home: &Path) -> PathBuf {
        home.join("dashboard").join("boards")
    }

    /// The ACTIVE profile's boards directory. Loads config.yaml, so call from `spawn_blocking`.
    pub(crate) fn boards_dir() -> Result<PathBuf, String> {
        Ok(boards_dir_for_home(
            &crate::server::openui_views_api::active_profile_home()?,
        ))
    }

    pub(crate) fn valid_id(id: &str) -> Result<(), String> {
        uuid::Uuid::parse_str(id)
            .map(|_| ())
            .map_err(|_| "Invalid board id".to_string())
    }

    /// Newest `updated_at` first. Unreadable, non-JSON, non-UUID-named and id-mismatched files
    /// are skipped, never fatal.
    pub(crate) fn list_boards_at(dir: &Path) -> Vec<Board> {
        let mut boards: Vec<Board> = std::fs::read_dir(dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| {
                let path = e.path();
                let stem = path.file_stem()?.to_str()?.to_string();
                if path.extension()? != "json" || valid_id(&stem).is_err() {
                    return None;
                }
                let board: Board = serde_json::from_slice(&std::fs::read(&path).ok()?).ok()?;
                (board.id == stem).then_some(board)
            })
            .collect();
        boards.sort_by(|a, b| {
            b.updated_at
                .cmp(&a.updated_at)
                .then_with(|| b.id.cmp(&a.id))
        });
        boards
    }

    fn now_secs() -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0)
    }

    fn file_for(dir: &Path, id: &str) -> Result<PathBuf, String> {
        valid_id(id)?;
        Ok(dir.join(format!("{id}.json")))
    }

    pub(crate) fn load_board_at(dir: &Path, id: &str) -> Result<Board, String> {
        let bytes = std::fs::read(file_for(dir, id)?).map_err(|_| "Board not found".to_string())?;
        serde_json::from_slice(&bytes).map_err(|_| "Board not found".to_string())
    }

    /// Serializes every board / gateway-order write in this process, so the board-count cap
    /// check and the write are one step and two saves of one file never interleave.
    static WRITE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// Write `bytes` to `path` via a unique temp file + rename; the temp file is removed when
    /// either step fails. Callers hold `WRITE_LOCK`.
    fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
        let tmp = path.with_extension(format!("{}.tmp", uuid::Uuid::new_v4()));
        let result = std::fs::write(&tmp, bytes).and_then(|_| std::fs::rename(&tmp, path));
        if result.is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
        result
    }

    /// Validate, cap, stamp times and write atomically (temp file + rename). Whole-board write,
    /// last writer wins (UI-SPEC A13). `created_at` of an existing board is kept.
    pub(crate) fn save_board_at(dir: &Path, mut board: Board) -> Result<Board, String> {
        validate_board(&board)?;
        let _guard = WRITE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let path = file_for(dir, &board.id)?;
        board.name = board.name.trim().to_string();
        let existing = load_board_at(dir, &board.id).ok();
        if existing.is_none() && !path.exists() && list_boards_at(dir).len() >= BOARDS_MAX {
            return Err(boards_full_error(BOARDS_MAX));
        }
        let now = now_secs();
        board.created_at = existing.map_or(now, |e| e.created_at);
        board.updated_at = now;
        let json =
            serde_json::to_vec_pretty(&board).map_err(|e| format!("Could not save board: {e}"))?;
        if json.len() > BOARD_FILE_MAX_BYTES {
            return Err("Board is too large to save".to_string());
        }
        std::fs::create_dir_all(dir).map_err(|e| format!("Could not save board: {e}"))?;
        write_atomic(&path, &json).map_err(|e| format!("Could not save board: {e}"))?;
        Ok(board)
    }

    pub(crate) fn delete_board_at(dir: &Path, id: &str) -> Result<(), String> {
        std::fs::remove_file(file_for(dir, id)?).map_err(|_| "Board not found".to_string())
    }

    /// `<home>/dashboard/gateway-card-order.json`.
    pub(crate) fn gateway_order_path(home: &Path) -> PathBuf {
        home.join("dashboard").join("gateway-card-order.json")
    }

    /// The saved order, or empty when the file is missing or unreadable (the screen then shows
    /// the default order; a parse failure is logged).
    pub(crate) fn load_gateway_order_at(path: &Path) -> Vec<String> {
        let Ok(bytes) = std::fs::read(path) else {
            return Vec::new();
        };
        match serde_json::from_slice::<Vec<String>>(&bytes) {
            Ok(ids) if validate_gateway_order(&ids).is_ok() => ids,
            _ => {
                tracing::warn!(path = %path.display(), "ignoring unreadable gateway card order");
                Vec::new()
            }
        }
    }

    /// Validate, then write atomically (temp file + rename).
    pub(crate) fn save_gateway_order_at(path: &Path, ids: &[String]) -> Result<(), String> {
        validate_gateway_order(ids)?;
        let _guard = WRITE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let json = serde_json::to_vec(ids).map_err(|e| format!("Could not save order: {e}"))?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("Could not save order: {e}"))?;
        }
        write_atomic(path, &json).map_err(|e| format!("Could not save order: {e}"))
    }

    /// Run `f` on the ACTIVE profile's gateway order path off the async runtime.
    pub(crate) async fn with_gateway_order_path<T: Send + 'static>(
        f: impl FnOnce(PathBuf) -> Result<T, String> + Send + 'static,
    ) -> Result<T, ServerFnError> {
        tokio::task::spawn_blocking(move || {
            crate::server::openui_views_api::active_profile_home()
                .and_then(|home| f(gateway_order_path(&home)))
        })
        .await
        .map_err(|e| ServerFnError::new(format!("join error: {e}")))?
        .map_err(ServerFnError::new)
    }

    /// Run a store op against the active profile's directory off the async runtime.
    pub(crate) async fn with_boards_dir<T: Send + 'static>(
        f: impl FnOnce(PathBuf) -> Result<T, String> + Send + 'static,
    ) -> Result<T, ServerFnError> {
        tokio::task::spawn_blocking(move || boards_dir().and_then(f))
            .await
            .map_err(|e| ServerFnError::new(format!("join error: {e}")))?
            .map_err(ServerFnError::new)
    }
}

#[cfg(feature = "server")]
pub(crate) use store::*;

/// The active profile's boards, newest first.
#[server]
pub async fn list_dashboard_boards() -> Result<Vec<Board>, ServerFnError> {
    with_boards_dir(|dir| Ok(list_boards_at(&dir))).await
}

/// Create or replace a board of the active profile. Returns the stored board (times stamped).
#[server]
pub async fn save_dashboard_board(board: Board) -> Result<Board, ServerFnError> {
    with_boards_dir(move |dir| save_board_at(&dir, board)).await
}

/// Delete a board of the active profile.
#[server]
pub async fn delete_dashboard_board(id: String) -> Result<(), ServerFnError> {
    with_boards_dir(move |dir| delete_board_at(&dir, &id)).await
}

/// The active profile's saved Gateway card order (empty when none is saved).
#[server]
pub async fn get_gateway_card_order() -> Result<Vec<String>, ServerFnError> {
    with_gateway_order_path(|path| Ok(load_gateway_order_at(&path))).await
}

/// Save the active profile's Gateway card order (an empty list restores the default).
#[server]
pub async fn set_gateway_card_order(order: Vec<String>) -> Result<(), ServerFnError> {
    with_gateway_order_path(move |path| save_gateway_order_at(&path, &order)).await
}

#[cfg(all(test, feature = "server"))]
mod tests {
    use super::*;

    pub(super) fn board(id: &str, name: &str, updated_at: i64) -> Board {
        Board {
            version: BOARD_SCHEMA_VERSION,
            id: id.to_string(),
            name: name.to_string(),
            created_at: 1,
            updated_at,
            items: vec![],
        }
    }

    #[test]
    fn boards_list_reads_board_files_from_the_active_profile_home() {
        let home = tempfile::tempdir().unwrap();
        let dir = boards_dir_for_home(home.path());
        assert_eq!(dir, home.path().join("dashboard/boards"));
        std::fs::create_dir_all(&dir).unwrap();
        let id = uuid::Uuid::new_v4().to_string();
        let b = board(&id, "Ops", 10);
        std::fs::write(
            dir.join(format!("{id}.json")),
            serde_json::to_vec(&b).unwrap(),
        )
        .unwrap();
        // A file whose stem is not a UUID is skipped even when its content is a valid board.
        std::fs::write(dir.join("notes.json"), serde_json::to_vec(&b).unwrap()).unwrap();
        assert_eq!(list_boards_at(&dir), vec![b]);
    }

    #[test]
    fn tile_titles_follow_the_copy_contract() {
        assert_eq!(TileSpec::Gateway {}.title(), "Gateway health");
        assert_eq!(TileSpec::Reports { report: None }.title(), "Reports");
        assert_eq!(
            TileSpec::Reports {
                report: Some("redteam".into())
            }
            .title(),
            "Reports: redteam"
        );
    }

    use crate::protocol::{ActivationScope, ActiveProfileRecord};
    use crate::server::profile_activation_api::{
        clear_active_profile_record, write_active_profile_record,
    };

    /// Module-local RAII env guard (each test module is its own namespace in this crate).
    struct ScopedEnv {
        key: String,
        prev: Option<String>,
    }
    impl ScopedEnv {
        fn set(key: &str, value: &str) -> Self {
            let prev = std::env::var(key).ok();
            // SAFETY: tests run with --test-threads=1.
            unsafe { std::env::set_var(key, value) };
            Self {
                key: key.to_string(),
                prev,
            }
        }
    }
    impl Drop for ScopedEnv {
        fn drop(&mut self) {
            // SAFETY: tests run with --test-threads=1.
            match &self.prev {
                Some(v) => unsafe { std::env::set_var(&self.key, v) },
                None => unsafe { std::env::remove_var(&self.key) },
            }
        }
    }

    fn record(name: &str) -> ActiveProfileRecord {
        ActiveProfileRecord {
            name: name.into(),
            scope: ActivationScope::ChatOnly,
            updated_at_ms: 1,
        }
    }

    fn uid() -> String {
        uuid::Uuid::new_v4().to_string()
    }

    fn new_board(name: &str) -> Board {
        board(&uid(), name, 0)
    }

    fn item(kind: ItemKind) -> BoardItem {
        let (w, h) = item_default_size(&kind);
        BoardItem {
            id: uid(),
            x: 0,
            y: 0,
            w,
            h,
            kind,
        }
    }

    fn note(text: &str) -> ItemKind {
        ItemKind::Note { text: text.into() }
    }

    #[test]
    fn boards_round_trip_through_disk() {
        let dir = tempfile::tempdir().unwrap();
        let mut b = new_board("Ops");
        b.items = vec![
            item(ItemKind::Widget { view_id: uid() }),
            item(ItemKind::Tile {
                tile: TileSpec::Usage {
                    window: UsageWindow::Month,
                },
            }),
            item(note("hello")),
        ];
        let saved = save_board_at(dir.path(), b.clone()).unwrap();
        assert!(dir.path().join(format!("{}.json", b.id)).is_file());
        assert!(saved.updated_at > 0 && saved.created_at > 0);
        let loaded = load_board_at(dir.path(), &b.id).unwrap();
        assert_eq!(loaded, saved);
        assert_eq!(loaded.items, b.items);
        // Re-save keeps created_at (even when the client sends a different one), refreshes updated_at.
        let mut again = loaded.clone();
        again.created_at = 7;
        again.updated_at = 7;
        let resaved = save_board_at(dir.path(), again).unwrap();
        assert_eq!(resaved.created_at, loaded.created_at);
        assert!(resaved.updated_at >= loaded.updated_at && resaved.updated_at != 7);
    }

    #[test]
    fn boards_list_newest_first_and_skips_garbage() {
        let dir = tempfile::tempdir().unwrap();
        for (i, n) in ["a", "b", "c"].iter().enumerate() {
            let b = board(&uid(), n, 100 + i as i64);
            std::fs::create_dir_all(dir.path()).unwrap();
            std::fs::write(
                dir.path().join(format!("{}.json", b.id)),
                serde_json::to_vec(&b).unwrap(),
            )
            .unwrap();
        }
        std::fs::write(dir.path().join("x.json"), "{}").unwrap();
        std::fs::write(dir.path().join("not-a-uuid.txt"), "junk").unwrap();
        std::fs::write(dir.path().join(format!("{}.json", uid())), "{ not json").unwrap();
        // Valid board JSON under a different (valid) UUID stem: id != stem, skipped.
        let stray = board(&uid(), "stray", 500);
        std::fs::write(
            dir.path().join(format!("{}.json", uid())),
            serde_json::to_vec(&stray).unwrap(),
        )
        .unwrap();
        let names: Vec<String> = list_boards_at(dir.path())
            .into_iter()
            .map(|b| b.name)
            .collect();
        assert_eq!(names, ["c", "b", "a"]);
    }

    #[test]
    fn concurrent_saves_respect_the_cap_and_leave_no_temp_files() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().to_path_buf();
        let ok: usize = (0..BOARDS_MAX + 6)
            .map(|i| {
                let p = path.clone();
                std::thread::spawn(move || save_board_at(&p, new_board(&format!("b{i}"))).is_ok())
            })
            .collect::<Vec<_>>()
            .into_iter()
            .map(|h| usize::from(h.join().unwrap()))
            .sum();
        assert_eq!(ok, BOARDS_MAX, "the cap holds under concurrent creates");
        assert_eq!(list_boards_at(&path).len(), BOARDS_MAX);
        let strays = std::fs::read_dir(&path)
            .unwrap()
            .flatten()
            .filter(|e| e.path().extension().is_some_and(|x| x == "tmp"))
            .count();
        assert_eq!(strays, 0);
    }

    #[test]
    fn boards_enforce_caps() {
        let dir = tempfile::tempdir().unwrap();
        // Board count: the 13th NEW board is refused, re-saving an existing one is allowed.
        let mut first = None;
        for i in 0..BOARDS_MAX {
            let b = save_board_at(dir.path(), new_board(&format!("b{i}"))).unwrap();
            first.get_or_insert(b);
        }
        assert_eq!(
            save_board_at(dir.path(), new_board("one too many")).unwrap_err(),
            "You have 12 boards, the maximum. Delete one to create another."
        );
        assert!(save_board_at(dir.path(), first.unwrap()).is_ok());

        let scratch = tempfile::tempdir().unwrap();
        let try_save = |b: Board| save_board_at(scratch.path(), b);

        let mut b = new_board("items");
        b.items = (0..=BOARD_ITEMS_MAX).map(|_| item(note(""))).collect();
        assert_eq!(
            try_save(b).unwrap_err(),
            "This board is full (40 items). Remove one to add another."
        );

        for name in ["", "   ", &"n".repeat(41)] {
            assert_eq!(
                try_save(new_board(name)).unwrap_err(),
                "Board name must be 1 to 40 characters."
            );
        }
        assert!(try_save(new_board(&"n".repeat(40))).is_ok());

        let with = |it: BoardItem| {
            let mut b = new_board("x");
            b.items = vec![it];
            b
        };
        assert!(try_save(with(item(note(&"t".repeat(2000))))).is_ok());
        assert!(try_save(with(item(note(&"t".repeat(2001))))).is_err());
        let mut far = item(note(""));
        far.x = 100_001;
        assert!(try_save(with(far)).is_err());
        let mut small = item(note(""));
        small.w = 159;
        assert!(try_save(with(small)).is_err());
        let mut small_tile = item(ItemKind::Tile {
            tile: TileSpec::Cron {},
        });
        small_tile.h = 143;
        assert!(try_save(with(small_tile)).is_err());
        let mut wide = item(note(""));
        wide.w = 4097;
        assert!(try_save(with(wide)).is_err());
        // 40 four-byte-char notes: within every item cap, above the 256 KiB file cap.
        let mut huge = new_board("huge");
        huge.items = (0..BOARD_ITEMS_MAX)
            .map(|_| item(note(&"😀".repeat(NOTE_TEXT_MAX_CHARS))))
            .collect();
        assert!(validate_board(&huge).is_ok());
        assert_eq!(try_save(huge).unwrap_err(), "Board is too large to save");
    }

    #[test]
    fn boards_reject_non_uuid_ids() {
        let dir = tempfile::tempdir().unwrap();
        let outside = dir.path().join("config.json");
        std::fs::write(&outside, "{}").unwrap();
        let boards = dir.path().join("boards");
        std::fs::create_dir_all(&boards).unwrap();
        for bad in ["../x", "a/b", ""] {
            let mut b = new_board("x");
            b.id = bad.to_string();
            assert!(save_board_at(&boards, b).is_err(), "{bad:?}");
            assert!(load_board_at(&boards, bad).is_err());
            assert!(delete_board_at(&boards, bad).is_err());
        }
        let mut bad_item = new_board("x");
        bad_item.items = vec![item(note(""))];
        bad_item.items[0].id = "nope".into();
        assert!(save_board_at(&boards, bad_item).is_err());
        let mut bad_view = new_board("x");
        bad_view.items = vec![item(ItemKind::Widget {
            view_id: "../../etc".into(),
        })];
        assert!(save_board_at(&boards, bad_view).is_err());
        let mut dup = new_board("x");
        let one = item(note(""));
        dup.items = vec![one.clone(), one];
        assert!(save_board_at(&boards, dup).is_err());
        assert!(outside.is_file());
        assert_eq!(std::fs::read_dir(&boards).unwrap().count(), 0);
    }

    #[test]
    fn tile_settings_reject_unknown_keys() {
        let parse = |j: &str| serde_json::from_str::<TileSpec>(j);
        assert_eq!(
            parse(r#"{"tile":"usage"}"#).unwrap(),
            TileSpec::Usage {
                window: UsageWindow::Week
            }
        );
        assert!(parse(r#"{"tile":"usage","window":"24h"}"#).is_ok());
        assert!(parse(r#"{"tile":"usage","window":"1y"}"#).is_err());
        assert!(parse(r#"{"tile":"usage","window":"7d","extra":1}"#).is_err());
        assert!(parse(r#"{"tile":"kanban","columns":["failed"]}"#).is_err());
        assert!(parse(r#"{"tile":"kanban","columns":["blocked","done"]}"#).is_ok());
        assert!(parse(r#"{"tile":"cron","extra":true}"#).is_err());
        assert!(parse(r#"{"tile":"sessions","show":"everything"}"#).is_err());
        // The same strictness applies to items and boards.
        let item_json = |extra: &str| {
            format!(
                r#"{{"id":"{}","x":0,"y":0,"w":160,"h":96,"kind":{{"type":"note","text":""{extra}}}}}"#,
                uid()
            )
        };
        assert!(serde_json::from_str::<BoardItem>(&item_json("")).is_ok());
        assert!(serde_json::from_str::<BoardItem>(&item_json(r#","prompt":"p""#)).is_err());
    }

    #[test]
    fn widget_item_stores_only_view_id_and_geometry() {
        let it = item(ItemKind::Widget { view_id: uid() });
        let v = serde_json::to_value(&it).unwrap();
        let mut keys: Vec<&str> = v.as_object().unwrap().keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(keys, ["h", "id", "kind", "w", "x", "y"]);
        let mut kind_keys: Vec<&str> = v["kind"]
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        kind_keys.sort_unstable();
        assert_eq!(kind_keys, ["type", "view_id"]);
    }

    #[test]
    fn report_names_follow_the_report_store_rule() {
        for ok in ["a", "redteam-1", "r.v2_x", &"a".repeat(64)] {
            assert!(valid_report_name(ok), "{ok}");
        }
        for bad in ["", "A", "-a", ".a", "a..b", "a/b", &"a".repeat(65)] {
            assert!(!valid_report_name(bad), "{bad}");
        }
    }

    #[test]
    fn boards_saved_under_profile_a_not_listed_under_profile_b() {
        let home = tempfile::tempdir().unwrap();
        let _g = ScopedEnv::set("IRONHERMES_HOME", home.path().to_str().unwrap());
        for p in ["alpha", "beta"] {
            std::fs::create_dir_all(home.path().join("profiles").join(p)).unwrap();
        }
        assert_ne!(ironhermes_core::current_profile(), "alpha");

        write_active_profile_record(&record("alpha")).unwrap();
        let a = boards_dir().unwrap();
        assert_eq!(a, home.path().join("profiles/alpha/dashboard/boards"));
        let saved = save_board_at(&a, new_board("A")).unwrap();

        write_active_profile_record(&record("beta")).unwrap();
        let b = boards_dir().unwrap();
        assert_eq!(b, home.path().join("profiles/beta/dashboard/boards"));
        assert!(list_boards_at(&b).is_empty());
        assert!(load_board_at(&b, &saved.id).is_err());
        assert!(delete_board_at(&b, &saved.id).is_err());

        write_active_profile_record(&record("alpha")).unwrap();
        assert_eq!(list_boards_at(&boards_dir().unwrap()), vec![saved]);

        clear_active_profile_record().unwrap();
        let root = boards_dir().unwrap();
        assert_eq!(root, home.path().join("dashboard/boards"));
        assert!(list_boards_at(&root).is_empty());
    }

    #[test]
    fn boards_dir_refuses_an_invalid_active_profile_name() {
        let home = tempfile::tempdir().unwrap();
        let _g = ScopedEnv::set("IRONHERMES_HOME", home.path().to_str().unwrap());
        write_active_profile_record(&record("../evil")).unwrap();
        assert!(boards_dir().is_err());
        assert!(
            !home.path().join("dashboard").exists() && !home.path().join("profiles").exists(),
            "no directory is created for a refused slug"
        );
        clear_active_profile_record().unwrap();
    }

    #[test]
    fn delete_board_removes_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let saved = save_board_at(dir.path(), new_board("gone")).unwrap();
        delete_board_at(dir.path(), &saved.id).unwrap();
        assert!(list_boards_at(dir.path()).is_empty());
        assert!(delete_board_at(dir.path(), &saved.id).is_err());
        assert!(delete_board_at(dir.path(), "../../etc/passwd").is_err());
    }

    #[test]
    fn gateway_order_round_trips_and_validates() {
        let dir = tempfile::tempdir().unwrap();
        let path = gateway_order_path(dir.path());
        assert_eq!(path, dir.path().join("dashboard/gateway-card-order.json"));
        assert!(load_gateway_order_at(&path).is_empty());
        let ids: Vec<String> = [
            "api_server",
            "route:a b/../c",
            "platform:slack",
            "teaser:email",
        ]
        .map(String::from)
        .to_vec();
        save_gateway_order_at(&path, &ids).unwrap();
        assert_eq!(load_gateway_order_at(&path), ids);
        // An empty list is a valid "reset".
        save_gateway_order_at(&path, &[]).unwrap();
        assert!(load_gateway_order_at(&path).is_empty());

        let many: Vec<String> = (0..=GATEWAY_ORDER_MAX)
            .map(|i| format!("route:{i}"))
            .collect();
        let bad: [Vec<String>; 4] = [
            many,
            vec![String::new()],
            vec!["route:a\nb".to_string()],
            vec!["other:x".to_string()],
        ];
        for b in bad {
            assert!(save_gateway_order_at(&path, &b).is_err(), "{b:?}");
        }
        assert!(validate_gateway_order(&[format!("route:{}", "x".repeat(194))]).is_ok());
        assert!(validate_gateway_order(&[format!("route:{}", "x".repeat(195))]).is_err());
        // A tampered file on disk falls back to the default order.
        std::fs::write(&path, r#"["nope:1"]"#).unwrap();
        assert!(load_gateway_order_at(&path).is_empty());
    }
}
