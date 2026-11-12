use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use handy_sdk::app::session_reader::SessionReader;
use handy_sdk::app::updater_thread::{AppStatus, Updater};
use handy_sdk::input::focus::{foreground_is, process_window};
use handy_sdk::roblox::screen::read_screen_view;
use handy_sdk::ui::d2d::{Vector2, dwrite_factory};
use handy_sdk::ui::gpu_overlay::GpuOverlay;
use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Direct2D::Common::{
    D2D_RECT_F, D2D1_COLOR_F, D2D1_FIGURE_BEGIN_FILLED, D2D1_FIGURE_END_CLOSED,
};
use windows::Win32::Graphics::Direct2D::{D2D1_DRAW_TEXT_OPTIONS_NONE, ID2D1DeviceContext};
use windows::Win32::Graphics::DirectWrite::{
    DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_WEIGHT_BOLD, DWRITE_MEASURING_MODE_NATURAL,
    DWRITE_PARAGRAPH_ALIGNMENT_CENTER, DWRITE_TEXT_ALIGNMENT_CENTER, IDWriteTextFormat,
};
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::WindowsAndMessaging::IsWindow;
use windows::core::w;

use crate::app::config::{Settings, SettingsStore, load_opener_weights};
use crate::game::board::{Board, BoardReader};
use crate::overlay::marks::{MarkKind, MarkPolygon, mark_polygons};
use crate::solver::solver::{OpenerWeights, SEARCH_LIMIT, Solution, percent_text, solve};

#[derive(Clone, Debug)]
pub struct EngineView {
    // Arc: the view is cloned into `view` every drawn frame, and the board and solution are large.
    pub board: Option<Arc<Board>>,
    pub solution: Arc<Solution>,
    /// why there is no board
    pub failed_at: String,
    /// last drawn frame: camera read, draw and present; 0 = not drawing
    pub frame_ms: f32,
}

impl Default for EngineView {
    fn default() -> Self {
        Self {
            board: None,
            solution: Arc::default(),
            failed_at: "no session".into(),
            frame_ms: 0.0,
        }
    }
}

const READ_EVERY: Duration = Duration::from_millis(100); // a click shows within a few frames; a full read is about 4 ms

const fn color(r: f32, g: f32, b: f32, a: f32) -> D2D1_COLOR_F {
    D2D1_COLOR_F { r, g, b, a }
}
const SAFE: D2D1_COLOR_F = color(0.2, 0.85, 0.35, 1.0);
const SAFE_EDGE: D2D1_COLOR_F = color(0.1, 0.5, 0.2, 0.9);
const MINE: D2D1_COLOR_F = color(0.95, 0.2, 0.2, 1.0);
const MINE_EDGE: D2D1_COLOR_F = color(0.55, 0.05, 0.05, 0.9);
const WRONG_FLAG: D2D1_COLOR_F = color(1.0, 0.7, 0.1, 1.0); // amber: a warning
const WRONG_FLAG_EDGE: D2D1_COLOR_F = color(0.6, 0.35, 0.0, 0.9);
const HINT: D2D1_COLOR_F = color(0.25, 0.6, 1.0, 1.0);
const HINT_EDGE: D2D1_COLOR_F = color(0.1, 0.3, 0.75, 0.9);
const OPENER: D2D1_COLOR_F = color(0.7, 0.4, 1.0, 1.0); // purple
const OPENER_EDGE: D2D1_COLOR_F = color(0.4, 0.15, 0.7, 0.9);
const LABEL: D2D1_COLOR_F = color(1.0, 1.0, 1.0, 1.0);
const LABEL_SHADE: D2D1_COLOR_F = color(0.0, 0.0, 0.0, 0.75);
const CHANCE_LABEL: D2D1_COLOR_F = color(1.0, 0.7, 0.7, 1.0); // light red: a mine chance, unlike the opener's white 0 chance

/// What one draw_marks call paints.
struct Look {
    fill: D2D1_COLOR_F,
    edge: D2D1_COLOR_F,
    sx: f32,
    sy: f32,
    width: f32,
}

// All of one mark's polygons as one geometry, so a frame is two fills and two outlines however many tiles are marked.
fn draw_marks(dc: &ID2D1DeviceContext, o: &GpuOverlay, ps: &[MarkPolygon], which: MarkKind, look: Look, st: &Settings) {
    let Look {
        mut fill,
        edge,
        sx,
        sy,
        width,
    } = look;
    let Ok(f) = (unsafe { dc.GetFactory() }) else { return };
    let Ok(g) = (unsafe { f.CreatePathGeometry() }) else {
        return;
    };
    let Ok(sink) = (unsafe { g.Open() }) else { return };
    let mut any = false;
    for p in ps.iter().filter(|p| p.kind == which) {
        any = true;
        unsafe {
            sink.BeginFigure(
                Vector2 {
                    X: p.pts[0].x * sx,
                    Y: p.pts[0].y * sy,
                },
                D2D1_FIGURE_BEGIN_FILLED,
            );
            for pt in &p.pts[1..] {
                sink.AddLine(Vector2 {
                    X: pt.x * sx,
                    Y: pt.y * sy,
                });
            }
            sink.EndFigure(D2D1_FIGURE_END_CLOSED);
        }
    }
    if unsafe { sink.Close() }.is_err() || !any {
        return;
    }
    fill.a = st.fill_pct as f32 / 100.0;
    if let Some(b) = o.brush(fill)
        && st.fill_pct > 0
    {
        unsafe { dc.FillGeometry(&g, &b, None) };
    }
    if let Some(b) = o.brush(edge)
        && st.outline
    {
        unsafe { dc.DrawGeometry(&g, &b, width, None) };
    }
}

// The hint's mine chance ("33% mine"), the opener's 0 chance ("47%") and each Chance tile's mine chance ("25%", light
// red, no shape) at the middle of the tile, over a dark shadow so they read on any tile.
fn draw_labels(
    dc: &ID2D1DeviceContext,
    o: &GpuOverlay,
    ps: &[MarkPolygon],
    fmt: Option<&IDWriteTextFormat>,
    sx: f32,
    sy: f32,
) {
    let (white, red, shade) = (o.brush(LABEL), o.brush(CHANCE_LABEL), o.brush(LABEL_SHADE));
    let (Some(fmt), Some(white), Some(red), Some(shade)) = (fmt, white, red, shade) else {
        return;
    };
    for p in ps {
        if p.mine_chance < 0.0 && p.open_chance < 0.0 {
            continue;
        }
        let (mut cx, mut cy) = (0.0f32, 0.0f32);
        for pt in &p.pts {
            cx += pt.x * sx;
            cy += pt.y * sy;
        }
        cx /= p.pts.len() as f32;
        cy /= p.pts.len() as f32;
        let chance = p.kind == MarkKind::Chance;
        let mut label = if p.mine_chance >= 0.0 {
            percent_text(p.mine_chance) + if chance { "" } else { " mine" }
        } else {
            String::new()
        };
        if p.open_chance >= 0.0 {
            label += if label.is_empty() { "" } else { "\n" };
            label += &percent_text(p.open_chance);
        }
        let s: Vec<u16> = label.encode_utf16().collect();
        let bx = D2D_RECT_F {
            left: cx - 100.0,
            top: cy - 30.0,
            right: cx + 100.0,
            bottom: cy + 30.0,
        }; // the format centres the text in it
        let shadow = D2D_RECT_F {
            left: bx.left + 1.0,
            top: bx.top + 1.0,
            right: bx.right + 1.0,
            bottom: bx.bottom + 1.0,
        };
        unsafe {
            dc.DrawText(
                &s,
                fmt,
                &shadow,
                &shade,
                D2D1_DRAW_TEXT_OPTIONS_NONE,
                DWRITE_MEASURING_MODE_NATURAL,
            );
            dc.DrawText(
                &s,
                fmt,
                &bx,
                if chance { &red } else { &white },
                D2D1_DRAW_TEXT_OPTIONS_NONE,
                DWRITE_MEASURING_MODE_NATURAL,
            );
        }
    }
}

/// Owns the overlay on its own thread. Reads and solves the board every 100 ms; while Roblox is in front, waits for
/// the display to take a frame, then reads the camera and draws the marks at once. Only reads Roblox memory.
pub struct Engine {
    updater: Arc<Updater>,
    settings: Arc<SettingsStore>, // read every frame, so a change shows at once
    config: PathBuf,
    view: Mutex<EngineView>,
}

impl Engine {
    /// config: config.ini, re-read for the opener's weights whenever the tuner rewrites it
    pub fn new(updater: Arc<Updater>, settings: Arc<SettingsStore>, config: PathBuf) -> Self {
        Self {
            updater,
            settings,
            config,
            view: Mutex::new(EngineView::default()),
        }
    }

    pub fn view(&self) -> EngineView {
        self.view.lock().unwrap().clone()
    }

    pub fn run(&self, stop: &AtomicBool) {
        let overlay = GpuOverlay::new("handys-bms-overlay"); // drawn from this thread only
        overlay.create();
        let mut reader = SessionReader::for_updater(&self.updater, BoardReader::new);
        let dwrite = dwrite_factory();
        let mut label: Option<IDWriteTextFormat> = None; // made again when Roblox's window moves to a monitor with another DPI
        let mut label_dpi = 0u32;
        let mut roblox: Option<HWND> = None;
        let mut roblox_pid = 0u32;
        let mut v = EngineView::default();
        let mut next_read = Instant::now();
        let mut weights = OpenerWeights::default();
        let mut solved_with = weights; // the weights v.solution was solved with
        let mut config_time: Option<SystemTime> = None; // config.ini's write time when the weights were read
        let publish = |v: &EngineView| *self.view.lock().unwrap() = v.clone();
        while !stop.load(Relaxed) {
            overlay.pump();
            let r: Option<&BoardReader> = if self.updater.status() == AppStatus::Ready {
                reader.get().map(|r| &*r)
            } else {
                None
            };
            let session = r.map(|r| r.session.clone());
            let pid = session.as_ref().map_or(0, |s| s.proc.pid());
            if pid != roblox_pid
                || roblox.is_some_and(|w| !unsafe { IsWindow(Some(w)) }.as_bool())
                || (roblox.is_none() && pid != 0)
            {
                roblox_pid = pid;
                roblox = process_window(pid); // the window can appear after the process
            }
            let now = Instant::now();
            if now >= next_read {
                // shortcut: solves on this thread, so a board whose search runs long holds one frame back; move
                // solving to its own thread if that shows.
                if let Ok(t) = std::fs::metadata(&self.config).and_then(|m| m.modified())
                    && config_time != Some(t)
                {
                    config_time = Some(t);
                    weights = load_opener_weights(&self.config); // a round end may have tuned them
                }
                v.failed_at = if r.is_some() { "" } else { "no session" }.into();
                let board = r.and_then(|r| r.read(Some(&mut v.failed_at)));
                // most reads see the same board: solving it again gives the same answer, so keep the last one
                if board.as_ref() != v.board.as_deref() || weights != solved_with {
                    solved_with = weights;
                    v.solution = Arc::new(
                        board
                            .as_ref()
                            .map_or_else(Solution::default, |b| solve(b, SEARCH_LIMIT, weights)),
                    );
                    v.board = board.map(Arc::new);
                }
                next_read = now + READ_EVERY;
            }
            // switched off (F1): no marks, but the board is still read and solved for the dashboard
            // (no session means pid 0, which is never in front)
            let shown = v.board.is_some() && roblox.is_some() && foreground_is(pid) && self.settings.get().overlay_on;
            let (true, Some(board), Some(target), Some(session)) = (shown, v.board.as_ref(), roblox, session.as_ref())
            else {
                overlay.hide();
                v.frame_ms = 0.0;
                publish(&v);
                std::thread::sleep(Duration::from_millis(50));
                continue;
            };
            // Paced by the display: the camera is read right before drawing, so the marks follow it as closely as they
            // can.
            overlay.wait_for_frame();
            let t0 = Instant::now();
            if let Some(view) = read_screen_view(&session.proc, &session.offsets, session.proc.base()) {
                let cfg = self.settings.get();
                let marks = mark_polygons(&view, board, &v.solution, &cfg);
                let dpi = unsafe { GetDpiForWindow(target) };
                let width = dpi as f32 / 96.0 * 1.5;
                if dpi != label_dpi
                    && let Some(dw) = &dwrite
                {
                    label_dpi = dpi;
                    label = unsafe {
                        dw.CreateTextFormat(
                            w!("Segoe UI"),
                            None,
                            DWRITE_FONT_WEIGHT_BOLD,
                            DWRITE_FONT_STYLE_NORMAL,
                            DWRITE_FONT_STRETCH_NORMAL,
                            14.0 * dpi as f32 / 96.0,
                            w!(""),
                        )
                    }
                    .ok();
                    if let Some(l) = &label {
                        unsafe {
                            let _ = l.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER);
                            let _ = l.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER);
                        }
                    }
                }
                overlay.show(target, |dc, client| {
                    let (sx, sy) = (client.width / view.size.x, client.height / view.size.y);
                    let look = |fill, edge| Look {
                        fill,
                        edge,
                        sx,
                        sy,
                        width,
                    };
                    draw_marks(dc, &overlay, &marks, MarkKind::Safe, look(SAFE, SAFE_EDGE), &cfg);
                    draw_marks(dc, &overlay, &marks, MarkKind::Mine, look(MINE, MINE_EDGE), &cfg);
                    draw_marks(
                        dc,
                        &overlay,
                        &marks,
                        MarkKind::WrongFlag,
                        look(WRONG_FLAG, WRONG_FLAG_EDGE),
                        &cfg,
                    );
                    draw_marks(dc, &overlay, &marks, MarkKind::Hint, look(HINT, HINT_EDGE), &cfg);
                    draw_marks(dc, &overlay, &marks, MarkKind::Opener, look(OPENER, OPENER_EDGE), &cfg);
                    draw_labels(dc, &overlay, &marks, label.as_ref(), sx, sy);
                });
            } else {
                overlay.hide();
            }
            v.frame_ms = t0.elapsed().as_secs_f32() * 1000.0;
            publish(&v);
        }
    }
}
