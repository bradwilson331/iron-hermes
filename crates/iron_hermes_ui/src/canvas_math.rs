//! Phase 54.10 Plan 02 (D-13/D-16/D-17/D-18): pure canvas interaction maths.
//!
//! Drag, zoom, snap, Fit, reading order, keyboard nudge/resize, placement, the shortcut
//! predicate, frame-cap selection and list reorder live here so they are natively testable
//! (the project has no DOM harness and native builds are blind to wasm code, RESEARCH
//! Pattern 6). No dioxus, web_sys, axum or serde_yaml: this module compiles for wasm and
//! native alike, same rule as `menu_layout.rs`.
//!
//! Item rects are integer world units (`i32`, CSS px at 100% zoom, multiples of [`GRID`]);
//! the view (pan/zoom) is `f64`. Every function treats non-finite input as a no-op.

// Plans 03-14 consume this surface (canvas screen, Gateway reorder).
#![allow(dead_code)]

/// Snap grid step in world units (UI-SPEC "Spacing Scale": the `md` token).
pub const GRID: i32 = 16;

/// Largest pointer delta (world units) honoured in one update; keeps `f64 -> i32` and the
/// additions below far from overflow on hostile input (T-54.10-06).
const DELTA_LIMIT: f64 = 1.0e7;

/// An item rect in world units.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

/// Pan and zoom of the world layer: `screen = world * zoom + pan`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct View {
    pub pan_x: f64,
    pub pan_y: f64,
    pub zoom: f64,
}

impl Default for View {
    fn default() -> Self {
        View {
            pan_x: 0.0,
            pan_y: 0.0,
            zoom: 1.0,
        }
    }
}

/// Nearest multiple of [`GRID`]; ties round away from zero.
pub fn snap(v: i32) -> i32 {
    ((f64::from(v) / f64::from(GRID)).round() as i32).saturating_mul(GRID)
}

pub fn world_to_screen(p: (f64, f64), v: View) -> (f64, f64) {
    (p.0 * v.zoom + v.pan_x, p.1 * v.zoom + v.pan_y)
}

pub fn screen_to_world(p: (f64, f64), v: View) -> (f64, f64) {
    ((p.0 - v.pan_x) / v.zoom, (p.1 - v.pan_y) / v.zoom)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DragMode {
    Move,
    Resize,
}

/// One pointer gesture. `last` is the most recent rect handed to the caller, so a cancel or
/// window blur ends at the last position the user saw (D-13), never a stale one.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum DragState {
    Idle,
    Active {
        mode: DragMode,
        start_ptr: (f64, f64),
        start: Rect,
        last: Rect,
        min: (i32, i32),
    },
}

impl DragState {
    pub fn begin(mode: DragMode, ptr: (f64, f64), rect: Rect, min: (i32, i32)) -> DragState {
        DragState::Active {
            mode,
            start_ptr: ptr,
            start: rect,
            last: rect,
            min,
        }
    }

    /// New rect for a pointer at `ptr` (screen px): the delta is divided by `zoom` into world
    /// units, applied to x/y (Move) or w/h (Resize, clamped to `min`), then snapped unless
    /// `bypass_snap` (Alt). `None` when idle or the inputs are not finite.
    pub fn update(&mut self, ptr: (f64, f64), zoom: f64, bypass_snap: bool) -> Option<Rect> {
        let DragState::Active {
            mode,
            start_ptr,
            start,
            last,
            min,
        } = self
        else {
            return None;
        };
        if !(ptr.0.is_finite() && ptr.1.is_finite() && zoom.is_finite() && zoom > 0.0) {
            return None;
        }
        let dx = ((ptr.0 - start_ptr.0) / zoom)
            .clamp(-DELTA_LIMIT, DELTA_LIMIT)
            .round() as i32;
        let dy = ((ptr.1 - start_ptr.1) / zoom)
            .clamp(-DELTA_LIMIT, DELTA_LIMIT)
            .round() as i32;
        let fix = |v: i32| if bypass_snap { v } else { snap(v) };
        let next = match mode {
            DragMode::Move => Rect {
                x: fix(start.x.saturating_add(dx)),
                y: fix(start.y.saturating_add(dy)),
                ..*start
            },
            DragMode::Resize => Rect {
                w: fix(start.w.saturating_add(dx)).max(min.0),
                h: fix(start.h.saturating_add(dy)).max(min.1),
                ..*start
            },
        };
        *last = next;
        Some(next)
    }

    /// Pointer released: the final rect, and back to idle.
    pub fn end(&mut self) -> Option<Rect> {
        let out = match self {
            DragState::Active { last, .. } => Some(*last),
            DragState::Idle => None,
        };
        *self = DragState::Idle;
        out
    }

    /// `pointercancel` / window blur: ends at the last snapped position, then idle.
    pub fn cancel(&mut self) -> Option<Rect> {
        self.end()
    }
}

pub const ZOOM_MIN: f64 = 0.25;
pub const ZOOM_MAX: f64 = 2.0;
/// Button / key zoom steps (UI-SPEC A3).
pub const ZOOM_STEPS: [f64; 7] = [0.25, 0.5, 0.75, 1.0, 1.25, 1.5, 2.0];
/// Zoom factor per wheel notch.
pub const WHEEL_FACTOR: f64 = 1.1;
pub const FIT_MARGIN: f64 = 48.0;
/// Keyboard pan step when the canvas region itself has focus.
pub const PAN_KEY_STEP: f64 = 64.0;
/// Reading-order band height: `(floor(y / 64), x, y)`.
pub const READING_BAND: i32 = 64;
/// Below this zoom an item shows only title + hero and no frame mounts.
pub const LOD_ZOOM: f64 = 0.5;
/// Viewports narrower than this render the read-only stacked board (D-17).
pub const NARROW_BREAKPOINT: f64 = 900.0;

fn finite2(p: (f64, f64)) -> bool {
    p.0.is_finite() && p.1.is_finite()
}

/// Zoom by `factor` around `cursor_screen`, keeping the world point under it fixed. Zoom is
/// clamped to [`ZOOM_MIN`]..=[`ZOOM_MAX`]; non-finite input returns the view unchanged.
pub fn zoom_at(view: View, factor: f64, cursor_screen: (f64, f64)) -> View {
    if !(factor.is_finite() && factor > 0.0 && finite2(cursor_screen) && view.zoom > 0.0) {
        return view;
    }
    let zoom = (view.zoom * factor).clamp(ZOOM_MIN, ZOOM_MAX);
    let (wx, wy) = screen_to_world(cursor_screen, view);
    View {
        pan_x: cursor_screen.0 - wx * zoom,
        pan_y: cursor_screen.1 - wy * zoom,
        zoom,
    }
}

/// Walk [`ZOOM_STEPS`] one step in or out, around `viewport_center`. At a bound: unchanged.
pub fn step_zoom(view: View, dir_in: bool, viewport_center: (f64, f64)) -> View {
    if !(view.zoom.is_finite() && view.zoom > 0.0) {
        return view;
    }
    const EPS: f64 = 1e-9;
    let target = if dir_in {
        ZOOM_STEPS.iter().copied().find(|s| *s > view.zoom + EPS)
    } else {
        ZOOM_STEPS
            .iter()
            .rev()
            .copied()
            .find(|s| *s < view.zoom - EPS)
    };
    match target {
        Some(t) => zoom_at(view, t / view.zoom, viewport_center),
        None => view,
    }
}

/// Frame all `rects` in `viewport` (w, h) with [`FIT_MARGIN`], zoom capped at `max_zoom`
/// (1.0 on board open, 2.0 for Present). No rects: the default view.
pub fn fit(rects: &[Rect], viewport: (f64, f64), max_zoom: f64) -> View {
    if rects.is_empty() || !finite2(viewport) || viewport.0 <= 0.0 || viewport.1 <= 0.0 {
        return View::default();
    }
    let max_zoom = if max_zoom.is_finite() {
        max_zoom.clamp(ZOOM_MIN, ZOOM_MAX)
    } else {
        1.0
    };
    let x0 = rects.iter().map(|r| r.x).min().unwrap_or(0);
    let y0 = rects.iter().map(|r| r.y).min().unwrap_or(0);
    let x1 = rects
        .iter()
        .map(|r| r.x.saturating_add(r.w))
        .max()
        .unwrap_or(0);
    let y1 = rects
        .iter()
        .map(|r| r.y.saturating_add(r.h))
        .max()
        .unwrap_or(0);
    let bw = (f64::from(x1) - f64::from(x0)).max(1.0);
    let bh = (f64::from(y1) - f64::from(y0)).max(1.0);
    let avail_w = (viewport.0 - 2.0 * FIT_MARGIN).max(1.0);
    let avail_h = (viewport.1 - 2.0 * FIT_MARGIN).max(1.0);
    let zoom = (avail_w / bw)
        .min(avail_h / bh)
        .min(max_zoom)
        .clamp(ZOOM_MIN, ZOOM_MAX);
    View {
        pan_x: (viewport.0 - bw * zoom) / 2.0 - f64::from(x0) * zoom,
        pan_y: (viewport.1 - bh * zoom) / 2.0 - f64::from(y0) * zoom,
        zoom,
    }
}

/// Full item anatomy (header buttons, resize handle) shows from [`LOD_ZOOM`] up.
pub fn lod_full(zoom: f64) -> bool {
    zoom >= LOD_ZOOM
}

/// The snap grid is only worth drawing while items are in full detail.
pub fn grid_visible(zoom: f64) -> bool {
    lod_full(zoom)
}

/// D-17: below [`NARROW_BREAKPOINT`] the board is the read-only stacked view.
pub fn narrow(viewport_w: f64) -> bool {
    viewport_w < NARROW_BREAKPOINT
}

/// Item ids in reading order: sort by `(floor(y / 64), x, y)`; ties keep input order.
pub fn reading_order(items: &[(String, Rect)]) -> Vec<String> {
    let mut v: Vec<&(String, Rect)> = items.iter().collect();
    v.sort_by_key(|(_, r)| (r.y.div_euclid(READING_BAND), r.x, r.y));
    v.into_iter().map(|(id, _)| id.clone()).collect()
}

/// Arrow-key move: whole [`GRID`] steps.
pub fn nudge(rect: Rect, dx_steps: i32, dy_steps: i32) -> Rect {
    Rect {
        x: rect.x.saturating_add(dx_steps.saturating_mul(GRID)),
        y: rect.y.saturating_add(dy_steps.saturating_mul(GRID)),
        ..rect
    }
}

/// Shift+arrow resize: whole [`GRID`] steps, never below `min` (w, h).
pub fn resize_step(rect: Rect, dw_steps: i32, dh_steps: i32, min: (i32, i32)) -> Rect {
    Rect {
        w: rect
            .w
            .saturating_add(dw_steps.saturating_mul(GRID))
            .max(min.0),
        h: rect
            .h
            .saturating_add(dh_steps.saturating_mul(GRID))
            .max(min.1),
        ..rect
    }
}

/// Add-menu placement (UI-SPEC A8): `size` centred on `center_world`, snapped, then nudged
/// +16/+16 for every existing item already sitting on that spot so stacks stay findable.
pub fn place_at_center_stack(
    existing: &[Rect],
    center_world: (i32, i32),
    size: (i32, i32),
) -> Rect {
    let mut x = snap(center_world.0.saturating_sub(size.0 / 2));
    let mut y = snap(center_world.1.saturating_sub(size.1 / 2));
    // Each pass moves to a spot not yet tried, so at most `existing.len()` passes happen.
    while existing.iter().any(|r| r.x == x && r.y == y) {
        x = x.saturating_add(GRID);
        y = y.saturating_add(GRID);
    }
    Rect {
        x,
        y,
        w: size.0,
        h: size.1,
    }
}

/// Preset layout (UI-SPEC A8): left to right, wrapping down when the next item would pass
/// `origin.0 + width`; [`GRID`] gaps. Sizes and origin on the grid keep every rect on it.
pub fn flow_pack(sizes: &[(i32, i32)], origin: (i32, i32), width: i32) -> Vec<Rect> {
    let right = origin.0.saturating_add(width);
    let (mut x, mut y, mut row_h) = (origin.0, origin.1, 0);
    let mut out = Vec::with_capacity(sizes.len());
    for &(w, h) in sizes {
        if x > origin.0 && x.saturating_add(w) > right {
            x = origin.0;
            y = y.saturating_add(row_h).saturating_add(GRID);
            row_h = 0;
        }
        out.push(Rect { x, y, w, h });
        x = x.saturating_add(w).saturating_add(GRID);
        row_h = row_h.max(h);
    }
    out
}

/// Most AI frames mounted at once (UI-SPEC A5; each is a ~3.6 MB document, RESEARCH
/// Pattern 5). One constant to change.
pub const FRAME_CAP: usize = 6;
/// A frame also mounts when within this fraction of the viewport size outside it.
pub const FRAME_MARGIN: f64 = 0.25;

/// Ids of the items whose frame should be mounted (UI-SPEC A5). `viewport_world` is
/// (x, y, w, h) in world units. At zoom >= [`LOD_ZOOM`] an item qualifies when it overlaps the
/// viewport grown by [`FRAME_MARGIN`]; `forced` ids (selected, Present current/next) are always
/// candidates. At most `cap` ids are returned: forced first, then the nearest the viewport
/// centre, evicting the farthest. Result keeps `items` order.
pub fn visible_set(
    viewport_world: (f64, f64, f64, f64),
    zoom: f64,
    items: &[(String, Rect)],
    forced: &[String],
    cap: usize,
) -> Vec<String> {
    let (vx, vy, vw, vh) = viewport_world;
    let finite = [vx, vy, vw, vh, zoom].iter().all(|v| v.is_finite());
    let (cx, cy) = (vx + vw / 2.0, vy + vh / 2.0);
    let (mx, my) = (vw * FRAME_MARGIN, vh * FRAME_MARGIN);
    let dist = |r: &Rect| {
        let dx = f64::from(r.x) + f64::from(r.w) / 2.0 - cx;
        let dy = f64::from(r.y) + f64::from(r.h) / 2.0 - cy;
        dx * dx + dy * dy
    };
    let qualifies = |r: &Rect| {
        finite
            && lod_full(zoom)
            && f64::from(r.x) < vx + vw + mx
            && f64::from(r.x) + f64::from(r.w) > vx - mx
            && f64::from(r.y) < vy + vh + my
            && f64::from(r.y) + f64::from(r.h) > vy - my
    };
    let mut forced_c: Vec<(f64, &str)> = Vec::new();
    let mut near: Vec<(f64, &str)> = Vec::new();
    for (id, r) in items {
        if forced.contains(id) {
            forced_c.push((dist(r), id));
        } else if qualifies(r) {
            near.push((dist(r), id));
        }
    }
    forced_c.sort_by(|a, b| a.0.total_cmp(&b.0));
    near.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut keep: Vec<&str> = forced_c.iter().take(cap).map(|(_, id)| *id).collect();
    keep.extend(near.iter().take(cap - keep.len()).map(|(_, id)| *id));
    items
        .iter()
        .filter(|(id, _)| keep.contains(&id.as_str()))
        .map(|(id, _)| id.clone())
        .collect()
}

/// Move the element at `from` so it ends at index `to`, keeping the rest in order. Shared by
/// the canvas z-order and the Gateway card list (D-18). Out-of-range indices: unchanged.
pub fn reorder<T: Clone>(list: &[T], from: usize, to: usize) -> Vec<T> {
    let mut out = list.to_vec();
    if from < out.len() && to < out.len() {
        let el = out.remove(from);
        out.insert(to, el);
    }
    out
}

/// Slot (0..=len) a pointer at `pointer` drops into, given each card's midpoint: before the
/// first midpoint it has not passed, after the last for anything beyond. This is an insertion
/// slot; a caller moving an element from `from` to a later slot passes `slot - 1` to
/// [`reorder`].
pub fn drop_index(midpoints: &[f64], pointer: f64) -> usize {
    midpoints.iter().filter(|m| **m < pointer).count()
}

/// A canvas-level single-key command (UI-SPEC A10).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CanvasCmd {
    Palette,
    Note,
    ZoomIn,
    ZoomOut,
    ZoomReset,
    Fit,
    Present,
}

/// Map a key to a canvas command, or `None` while the key must be left alone: focus in an
/// input/textarea/select/contenteditable (`target_editable`), inside an active AI frame
/// (`frame_active`), or a modifier other than Shift held. Shift alone never suppresses.
pub fn canvas_shortcut(
    key: &str,
    ctrl_or_meta: bool,
    alt: bool,
    target_editable: bool,
    frame_active: bool,
) -> Option<CanvasCmd> {
    if ctrl_or_meta || alt || target_editable || frame_active {
        return None;
    }
    match key {
        "/" => Some(CanvasCmd::Palette),
        "n" | "N" => Some(CanvasCmd::Note),
        "+" | "=" => Some(CanvasCmd::ZoomIn),
        "-" => Some(CanvasCmd::ZoomOut),
        "0" => Some(CanvasCmd::ZoomReset),
        "f" | "F" => Some(CanvasCmd::Fit),
        "p" | "P" => Some(CanvasCmd::Present),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: i32, y: i32, w: i32, h: i32) -> Rect {
        Rect { x, y, w, h }
    }

    #[test]
    fn snap_rounds_to_the_16_grid() {
        assert_eq!(snap(23), 16);
        assert_eq!(snap(24), 32);
        assert_eq!(snap(-9), -16);
        assert_eq!(snap(0), 0);
    }

    #[test]
    fn world_screen_round_trip() {
        for zoom in [0.25, 1.0, 2.0] {
            let v = View {
                pan_x: 37.5,
                pan_y: -120.25,
                zoom,
            };
            let p = (123.0, -45.0);
            let back = screen_to_world(world_to_screen(p, v), v);
            assert!((back.0 - p.0).abs() < 1e-9 && (back.1 - p.1).abs() < 1e-9);
        }
    }

    #[test]
    fn drag_moves_by_pointer_delta_divided_by_zoom_and_snaps() {
        let mut d = DragState::begin(
            DragMode::Move,
            (100.0, 100.0),
            rect(16, 16, 320, 240),
            (192, 144),
        );
        assert_eq!(
            d.update((140.0, 108.0), 0.5, false),
            Some(rect(96, 32, 320, 240))
        );
    }

    #[test]
    fn drag_with_alt_bypasses_snap() {
        let mut d = DragState::begin(
            DragMode::Move,
            (0.0, 0.0),
            rect(16, 16, 320, 240),
            (192, 144),
        );
        assert_eq!(
            d.update((10.0, 6.0), 1.0, true),
            Some(rect(26, 22, 320, 240))
        );
        assert_eq!(
            d.update((10.0, 6.0), 1.0, false),
            Some(rect(32, 16, 320, 240))
        );
    }

    #[test]
    fn cancel_ends_at_last_snapped_position() {
        let mut d = DragState::begin(DragMode::Move, (0.0, 0.0), rect(0, 0, 320, 240), (192, 144));
        d.update((40.0, 0.0), 1.0, false);
        let last = d.update((70.0, 17.0), 1.0, false);
        assert_eq!(last, Some(rect(64, 16, 320, 240)));
        assert_eq!(d.cancel(), last);
        assert_eq!(d, DragState::Idle);
        assert_eq!(d.cancel(), None);
    }

    #[test]
    fn resize_clamps_to_minimum() {
        let mut d = DragState::begin(
            DragMode::Resize,
            (0.0, 0.0),
            rect(0, 0, 320, 240),
            (192, 144),
        );
        assert_eq!(
            d.update((-500.0, -500.0), 1.0, false),
            Some(rect(0, 0, 192, 144))
        );
    }

    fn item(id: &str, x: i32, y: i32) -> (String, Rect) {
        (id.to_string(), rect(x, y, 320, 240))
    }

    #[test]
    fn zoom_at_keeps_the_cursor_world_point_fixed() {
        let v = View {
            pan_x: 40.0,
            pan_y: -20.0,
            zoom: 1.0,
        };
        let cursor = (300.0, 200.0);
        let before = screen_to_world(cursor, v);
        let z = zoom_at(v, 1.1, cursor);
        let after = screen_to_world(cursor, z);
        assert!((before.0 - after.0).abs() < 1e-9 && (before.1 - after.1).abs() < 1e-9);
        assert!((z.zoom - 1.1).abs() < 1e-12);
        assert_eq!(zoom_at(v, 1000.0, cursor).zoom, ZOOM_MAX);
        assert_eq!(zoom_at(v, 0.0001, cursor).zoom, ZOOM_MIN);
        assert_eq!(zoom_at(v, f64::NAN, cursor), v);
        assert_eq!(zoom_at(v, 1.1, (f64::INFINITY, 0.0)), v);
    }

    #[test]
    fn step_zoom_walks_the_fixed_steps() {
        let at = |zoom| View {
            zoom,
            ..View::default()
        };
        let c = (0.0, 0.0);
        assert_eq!(step_zoom(at(1.0), true, c).zoom, 1.25);
        assert_eq!(step_zoom(at(1.0), false, c).zoom, 0.75);
        assert_eq!(step_zoom(at(0.3), true, c).zoom, 0.5);
        assert_eq!(step_zoom(at(2.0), true, c).zoom, 2.0);
        assert_eq!(step_zoom(at(0.25), false, c).zoom, 0.25);
    }

    #[test]
    fn fit_frames_items_with_margin_and_cap() {
        let vp = (1000.0, 600.0);
        let far = [rect(0, 0, 320, 240), rect(2000, 1000, 320, 240)];
        let v = fit(&far, vp, 1.0);
        assert!(v.zoom <= 1.0);
        for r in &far {
            let (sx0, sy0) = world_to_screen((f64::from(r.x), f64::from(r.y)), v);
            let (sx1, sy1) = world_to_screen((f64::from(r.x + r.w), f64::from(r.y + r.h)), v);
            assert!(sx0 >= FIT_MARGIN - 1e-6 && sy0 >= FIT_MARGIN - 1e-6);
            assert!(sx1 <= vp.0 - FIT_MARGIN + 1e-6 && sy1 <= vp.1 - FIT_MARGIN + 1e-6);
        }
        // One small item: zoom 1.0 and centred.
        let one = [rect(160, 160, 320, 240)];
        let v = fit(&one, vp, 1.0);
        assert_eq!(v.zoom, 1.0);
        let (cx, cy) = world_to_screen((320.0, 280.0), v);
        assert!((cx - 500.0).abs() < 1e-9 && (cy - 300.0).abs() < 1e-9);
        // Present cap 2.0 zooms a small item up.
        assert_eq!(fit(&[rect(0, 0, 160, 96)], vp, 2.0).zoom, 2.0);
        assert_eq!(fit(&[], vp, 1.0), View::default());
        assert_eq!(fit(&one, (f64::NAN, 600.0), 1.0), View::default());
    }

    #[test]
    fn lod_and_grid_threshold() {
        assert!(!lod_full(0.49));
        assert!(lod_full(0.5));
        assert!(!grid_visible(0.49) && grid_visible(0.5));
        assert!(narrow(899.0));
        assert!(!narrow(900.0));
    }

    #[test]
    fn reading_order_sorts_by_band_then_x() {
        let items = [item("a", 300, 10), item("b", 0, 70), item("c", 0, 0)];
        assert_eq!(reading_order(&items), ["c", "a", "b"]);
    }

    #[test]
    fn nudge_and_resize_step() {
        let r = rect(32, 32, 224, 160);
        assert_eq!(nudge(r, 1, 0), rect(48, 32, 224, 160));
        assert_eq!(nudge(r, 0, -1), rect(32, 16, 224, 160));
        assert_eq!(resize_step(r, 1, 0, (160, 96)).w, 240);
        assert_eq!(resize_step(r, -1, 0, (160, 96)).w, 208);
        assert_eq!(
            resize_step(rect(0, 0, 160, 96), -1, -1, (160, 96)),
            rect(0, 0, 160, 96)
        );
    }

    #[test]
    fn place_at_center_stack_nudges_off_occupied_spots() {
        let size = (224, 160);
        let cand = place_at_center_stack(&[], (1000, 500), size);
        assert_eq!((cand.x, cand.y), (snap(1000 - 112), snap(500 - 80)));
        let existing = [
            rect(cand.x, cand.y, 320, 240),
            rect(cand.x + 16, cand.y + 16, 320, 240),
        ];
        let placed = place_at_center_stack(&existing, (1000, 500), size);
        assert_eq!((placed.x, placed.y), (cand.x + 32, cand.y + 32));
        assert_eq!((placed.w, placed.h), size);
    }

    #[test]
    fn flow_pack_places_left_to_right_then_down() {
        let out = flow_pack(&[(320, 240), (320, 240), (480, 320)], (0, 0), 1000);
        assert_eq!(
            out,
            [
                rect(0, 0, 320, 240),
                rect(336, 0, 320, 240),
                rect(0, 256, 480, 320)
            ]
        );
        for (i, a) in out.iter().enumerate() {
            assert!([a.x, a.y, a.w, a.h].iter().all(|v| v % GRID == 0));
            for b in &out[i + 1..] {
                let apart =
                    a.x + a.w <= b.x || b.x + b.w <= a.x || a.y + a.h <= b.y || b.y + b.h <= a.y;
                assert!(apart, "{a:?} overlaps {b:?}");
            }
        }
    }

    #[test]
    fn canvas_shortcut_suppression() {
        let k = |key| canvas_shortcut(key, false, false, false, false);
        assert_eq!(k("/"), Some(CanvasCmd::Palette));
        assert_eq!(k("n"), Some(CanvasCmd::Note));
        assert_eq!(k("N"), Some(CanvasCmd::Note));
        assert_eq!(k("+"), Some(CanvasCmd::ZoomIn));
        assert_eq!(k("="), Some(CanvasCmd::ZoomIn));
        assert_eq!(k("-"), Some(CanvasCmd::ZoomOut));
        assert_eq!(k("0"), Some(CanvasCmd::ZoomReset));
        assert_eq!(k("f"), Some(CanvasCmd::Fit));
        assert_eq!(k("p"), Some(CanvasCmd::Present));
        assert_eq!(k("x"), None);
        for key in ["/", "n", "+", "-", "0", "f", "p"] {
            assert_eq!(
                canvas_shortcut(key, false, false, true, false),
                None,
                "editable {key}"
            );
            assert_eq!(
                canvas_shortcut(key, false, false, false, true),
                None,
                "frame {key}"
            );
            assert_eq!(
                canvas_shortcut(key, true, false, false, false),
                None,
                "ctrl {key}"
            );
            assert_eq!(
                canvas_shortcut(key, false, true, false, false),
                None,
                "alt {key}"
            );
        }
    }

    const VP: (f64, f64, f64, f64) = (0.0, 0.0, 1000.0, 600.0);

    fn ids(v: &[String]) -> Vec<&str> {
        v.iter().map(String::as_str).collect()
    }

    #[test]
    fn visible_set_caps_and_prefers_the_centre() {
        // w0..w7 sit progressively further right of the centre; all inside the viewport.
        let items: Vec<_> = (0..8)
            .map(|i| item(&format!("w{i}"), 100 + i * 80, 100))
            .collect();
        // Centre (500, 300): tile centres are x+160, so w3 (340+160=500) is nearest.
        let got = visible_set(VP, 1.0, &items, &[], 6);
        assert_eq!(got.len(), 6);
        assert!(got.contains(&"w3".to_string()));
        // w7 is the farthest (320 from the centre); w0 and w6 tie at 240, one of them is evicted too.
        assert!(!got.contains(&"w7".to_string()));
        assert!(got.contains(&"w1".to_string()) && got.contains(&"w5".to_string()));
    }

    #[test]
    fn visible_set_respects_margin_and_zoom() {
        // Viewport right edge 1000, margin 250: x=1100 is within, x=1300 is beyond.
        let items = [item("in", 1100, 100), item("out", 1300, 100)];
        assert_eq!(ids(&visible_set(VP, 1.0, &items, &[], 6)), ["in"]);
        assert!(visible_set(VP, 0.4, &items, &[], 6).is_empty());
        assert_eq!(
            ids(&visible_set(VP, 0.4, &items, &["out".to_string()], 6)),
            ["out"]
        );
        assert!(visible_set((f64::NAN, 0.0, 1000.0, 600.0), 1.0, &items, &[], 6).is_empty());
    }

    #[test]
    fn visible_set_includes_forced_ids() {
        let mut items: Vec<_> = (0..8)
            .map(|i| item(&format!("w{i}"), 100 + i * 80, 100))
            .collect();
        items.push(item("far", 5000, 5000));
        let got = visible_set(VP, 1.0, &items, &["far".to_string()], 6);
        assert_eq!(got.len(), 6);
        assert!(got.contains(&"far".to_string()));
        // The forced id took a slot, so the farthest qualifying item is evicted.
        assert!(!got.contains(&"w7".to_string()));
        // More forced than the cap still never exceeds it.
        let all: Vec<String> = items.iter().map(|(id, _)| id.clone()).collect();
        assert_eq!(visible_set(VP, 1.0, &items, &all, 3).len(), 3);
    }

    #[test]
    fn reorder_moves_one_element() {
        let l = ["a", "b", "c", "d"];
        assert_eq!(reorder(&l, 0, 2), ["b", "c", "a", "d"]);
        assert_eq!(reorder(&l, 3, 0), ["d", "a", "b", "c"]);
        assert_eq!(reorder(&l, 4, 0), l);
        assert_eq!(reorder(&l, 0, 4), l);
    }

    #[test]
    fn drop_index_uses_midpoints() {
        let mids = [50.0, 150.0, 250.0];
        assert_eq!(drop_index(&mids, 120.0), 1);
        assert_eq!(drop_index(&mids, 10.0), 0);
        assert_eq!(drop_index(&mids, 400.0), 3);
    }

    #[test]
    fn non_finite_input_is_a_no_op() {
        let mut d = DragState::begin(DragMode::Move, (0.0, 0.0), rect(0, 0, 320, 240), (192, 144));
        assert_eq!(d.update((f64::NAN, 0.0), 1.0, false), None);
        assert_eq!(d.update((1.0, 0.0), 0.0, false), None);
        assert_eq!(d.update((f64::INFINITY, 0.0), 1.0, false), None);
    }
}
