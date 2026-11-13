#![windows_subsystem = "windows"]

use std::cell::Cell;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use handy_sdk::app::args::arg_value;
use handy_sdk::app::crash_guard::on_crash;
use handy_sdk::app::self_update::{SelfUpdate, Startup, UpdateState};
use handy_sdk::app::session_reader::SessionReader;
use handy_sdk::app::single_instance::SingleInstance;
use handy_sdk::app::status_snapshot::{snapshot_allowed, status_line, wait_for};
use handy_sdk::app::updater_thread::{AppStatus, Updater, production_smoke_test, status_name, status_tone};
use handy_sdk::input::focus::foreground_is;
use handy_sdk::input::hooks::KeyPoller;
use handy_sdk::input::key::MOUSE_LEFT;
use handy_sdk::input::send::release;
use handy_sdk::roblox::http::win_http_get;
use handy_sdk::roblox::screen::read_screen_view;
use handy_sdk::ui::dashboard::{Dashboard, Spec, check, choice, note, number, page, section, value};
use handy_sdk::ui::overlay::StatusOverlay;
use handy_sdk::util::paths::{app_data_dir, set_app_name};
use handy_sdk::util::thread::StopThread;
use handys_bms::app::config::{
    MAX_SIZE_PCT, MIN_SIZE_PCT, MarkShape, SettingsStore, load_opener_weights, load_settings,
};
use handys_bms::app::offset_keys::required_keys;
use handys_bms::game::board::{BoardReader, board_summary};
use handys_bms::r#macro::clicker::Clicker;
use handys_bms::overlay::engine::Engine;
use handys_bms::overlay::marks::mark_polygons;
use handys_bms::solver::replay::score_replay;
use handys_bms::solver::solver::{SEARCH_LIMIT, Solution, solution_summary, solution_text, solve};
use windows::Win32::UI::Input::KeyboardAndMouse::{VK_F1, VK_F2};
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWDEFAULT;

const MUTEX: &str = "Local\\handys-bms";
const DASHBOARD_CLASS: &str = "handys-bms-dashboard";
const ID_ROBLOX: i32 = 1;
const ID_BOARD: i32 = 2;
const ID_SOLVER: i32 = 3;
const ID_OVERLAY: i32 = 4;
const ID_SAFE: i32 = 5;
const ID_MINES: i32 = 6;
const ID_GUESS: i32 = 7;
const ID_OPENER: i32 = 8;
const ID_SHAPE: i32 = 9;
const ID_SIZE: i32 = 12;
const ID_FILL: i32 = 13;
const ID_OUTLINE: i32 = 14;
const ID_OVERLAY_ON: i32 = 15;
const ID_CLICK: i32 = 16;
const ID_HINT: i32 = 17;

fn config_ini() -> PathBuf {
    app_data_dir().join("config.ini")
}

fn new_updater() -> Arc<Updater> {
    Arc::new(Updater::new(
        Arc::new(win_http_get),
        production_smoke_test,
        app_data_dir(),
        &required_keys(),
    ))
}

/// `--status <file>`: read-only snapshot (no hooks, no input). Waits for Ready or 15 s.
fn status_snapshot(out: &Path) -> i32 {
    if !snapshot_allowed(MUTEX, "handy's bms", out) {
        return 1;
    }
    let updater = new_updater();
    let thread = {
        let u = updater.clone();
        StopThread::spawn(move |st| u.run(st))
    };
    wait_for(
        || updater.status() == AppStatus::Ready,
        Duration::from_secs(15),
        Duration::from_millis(50),
    );
    let mut f = format!(
        "version: {}\nstatus: {}\n",
        env!("CARGO_PKG_VERSION"),
        status_line(&updater)
    ); // then a line per thing the game reads

    let mut reader = SessionReader::for_updater(&updater, BoardReader::new);
    let mut failed = "no session".to_owned();
    let t0 = Instant::now();
    let r = reader.get().map(|r| &*r);
    let board = r.and_then(|r| r.read(Some(&mut failed)));
    let ms = t0.elapsed().as_secs_f64() * 1000.0;
    let s = board.as_ref().map_or_else(Solution::default, |b| {
        solve(b, SEARCH_LIMIT, load_opener_weights(&config_ini()))
    });
    if let Some(sess) = r.map(|r| &r.session) {
        match read_screen_view(&sess.proc, &sess.offsets, sess.proc.base()) {
            None => f += "camera: unreadable\n",
            Some(view) => {
                f += &format!("camera: {:.0}x{:.0}", view.size.x, view.size.y);
                if let Some(board) = &board {
                    let marks = mark_polygons(&view, board, &s, &load_settings(&config_ini()));
                    let inside = marks
                        .iter()
                        .filter(|m| {
                            m.pts
                                .iter()
                                .all(|p| p.x >= 0.0 && p.y >= 0.0 && p.x <= view.size.x && p.y <= view.size.y)
                        })
                        .count();
                    f += &format!(" \u{b7} {inside} of {} marks on screen", marks.len());
                }
                f += "\n";
            }
        }
    }
    match &board {
        Some(board) => {
            f += &format!("board: {} \u{b7} read {ms:.1} ms\n", board_summary(board));
            f += &format!("solver: {}\n{}", solution_summary(&s), solution_text(board, &s));
        }
        None => f += &format!("board: not found ({failed})\n"),
    }
    let _ = std::fs::write(out, f);
    drop(thread);
    if updater.status() == AppStatus::Ready { 0 } else { 1 }
}

fn run() -> i32 {
    set_app_name("handy's bms");
    on_crash(|| release(&MOUSE_LEFT)); // a crash mid-click must not leave the left button held down in Roblox
    if let Some(out) = arg_value("--status") {
        return if out.is_empty() {
            2
        } else {
            status_snapshot(Path::new(&out))
        };
    }
    if let Some(input) = arg_value("--replay") {
        // offline: scores the solver on a file, reads no game
        let Some(text) = (!input.is_empty()).then(|| std::fs::read(&input).ok()).flatten() else {
            return 2;
        };
        // the opener as the app has it now
        let scores = score_replay(&String::from_utf8_lossy(&text), load_opener_weights(&config_ini()));
        return if std::fs::write(format!("{input}.scores"), scores).is_ok() {
            0
        } else {
            1
        };
    }

    // Only builds made by release.cmd update themselves (dev builds and build.cmd never do). Skipped while another
    // copy runs: its exe is in use and it would be replaced under it.
    let enabled = option_env!("HANDY_RELEASE").is_some() && !SingleInstance::is_running(MUTEX);
    let update = match SelfUpdate::start("handy-z", "handys-bms", env!("CARGO_PKG_VERSION"), enabled) {
        Startup::Restarting => return 0, // an update staged earlier is starting; exit before anything else runs
        Startup::Running(u) => u,
    };

    let mut instance = SingleInstance::new(MUTEX, DASHBOARD_CLASS);
    if !instance.acquire() {
        return 0; // brought the running copy's window up
    }

    let updater = new_updater();
    let updater_thread = {
        let u = updater.clone();
        StopThread::spawn(move |st| u.run(st))
    };
    let settings = Arc::new(SettingsStore::new(load_settings(&config_ini())));
    let engine = Arc::new(Engine::new(updater.clone(), settings.clone(), config_ini()));
    let engine_thread = {
        let e = engine.clone();
        StopThread::spawn(move |st| e.run(st))
    };
    let clicker = Arc::new(Clicker::new(updater.clone(), settings.clone()));
    let clicker_thread = {
        let c = clicker.clone();
        StopThread::spawn(move |st| c.run(st))
    };

    let first = Cell::new(true);
    let (f1, f2) = (Cell::new(KeyPoller::default()), Cell::new(KeyPoller::default())); // a key held at startup is not a press
    let pill = StatusOverlay::new(); // the autoclick's state, since it shows nothing on the board
    pill.create();
    let d = Dashboard::new(Spec {
        title: "handy's bms".into(),
        badge: update.badge().text,
        window_class: DASHBOARD_CLASS.into(),
        rows: vec![
            page("Main"),
            section("On / off (keys work in Roblox)"),
            check(ID_OVERLAY_ON, "Overlay\t[F1]"),
            check(ID_CLICK, "Hold mouse X2 to spam left click\t[F2]"),
            section("bLockerman's Minesweeper"),
            value(ID_ROBLOX, "Roblox"),
            value(ID_BOARD, "Board"),
            value(ID_SOLVER, "Solver"),
            value(ID_OVERLAY, "Overlay"),
            page("Overlay"),
            section("Show"),
            check(ID_SAFE, "Safe tiles"),
            check(ID_MINES, "Mines"),
            check(ID_GUESS, "Best guess and chances when nothing is safe"),
            check(ID_OPENER, "Area openers"),
            section("Look"),
            choice(ID_SHAPE, "Shape", &["Square", "Circle", "Diamond"]),
            number(ID_SIZE, "Size (% of tile)", MIN_SIZE_PCT, MAX_SIZE_PCT),
            number(ID_FILL, "Fill opacity (%)", 0, 100),
            check(ID_OUTLINE, "Outline"),
            note(ID_HINT),
        ],
        hint_row: Some(ID_HINT),
        ..Default::default()
    });
    d.on_tick(|ui| {
        let b = update.badge();
        ui.set_badge(&b.text, b.tone, &b.tip);
        let session = updater.session();
        // only presses made in Roblox flip them
        let in_roblox = session.is_some_and(|s| foreground_is(s.proc.pid()));
        let pressed = |p: &Cell<KeyPoller>, vk: u16| {
            let mut k = p.get();
            let down = k.pressed(u32::from(vk));
            p.set(k);
            down
        };
        let flip_overlay = pressed(&f1, VK_F1.0) && in_roblox;
        let flip_click = pressed(&f2, VK_F2.0) && in_roblox;
        if flip_overlay || flip_click {
            settings.update(|s| {
                if flip_overlay {
                    s.overlay_on = !s.overlay_on;
                }
                if flip_click {
                    s.click_x2 = !s.click_x2;
                }
            });
            settings.save(&config_ini());
        }
        let st = updater.status();
        ui.set_status(status_tone(st), status_name(st), "");
        ui.set_status_tip(&updater.detail());
        ui.set_text(
            ID_ROBLOX,
            if st == AppStatus::Ready {
                "ready"
            } else {
                status_name(st)
            },
        );
        let v = engine.view();
        ui.set_text(
            ID_BOARD,
            &v.board
                .as_ref()
                .map_or_else(|| format!("not found ({})", v.failed_at), |b| board_summary(b)),
        );
        ui.set_text(
            ID_SOLVER,
            &v.board
                .as_ref()
                .map_or_else(|| "-".to_owned(), |_| solution_summary(&v.solution)),
        );
        ui.set_text(
            ID_OVERLAY,
            &if v.frame_ms > 0.0 {
                format!("drawing \u{b7} {:.1} ms a frame", v.frame_ms)
            } else {
                "hidden while Roblox isn't in front".to_owned()
            },
        );
        let s = settings.get();
        ui.set_check(ID_OVERLAY_ON, s.overlay_on);
        ui.set_check(ID_CLICK, s.click_x2);
        pill.set(
            if !s.click_x2 {
                "Autoclick off"
            } else if clicker.clicking() {
                "Autoclick: clicking"
            } else {
                "Autoclick on (hold X2)"
            },
            s.click_x2,
        );
        ui.set_check(ID_SAFE, s.show_safe);
        ui.set_check(ID_MINES, s.show_mines);
        ui.set_check(ID_GUESS, s.show_hint);
        ui.set_check(ID_OPENER, s.show_opener);
        ui.set_choice(ID_SHAPE, s.shape as usize);
        ui.set_check(ID_OUTLINE, s.outline);
        if first.replace(false) {
            // only once: setting them every tick would fight typing
            ui.set_number(ID_SIZE, s.size_pct);
            ui.set_number(ID_FILL, s.fill_pct);
        }
    });
    d.on_badge_click(|ui| {
        if matches!(update.state(), UpdateState::Ready { .. }) && clicker.clicking() {
            return ui.flash(); // not mid-click
        }
        if update.request_restart() {
            ui.close();
        } else {
            handy_sdk::ui::open_url(&update.release_url());
        }
    });
    d.on_check(|_, id, checked| {
        settings.update(|s| match id {
            ID_OVERLAY_ON => s.overlay_on = checked,
            ID_CLICK => s.click_x2 = checked,
            ID_SAFE => s.show_safe = checked,
            ID_MINES => s.show_mines = checked,
            ID_GUESS => s.show_hint = checked,
            ID_OPENER => s.show_opener = checked,
            ID_OUTLINE => s.outline = checked,
            _ => {}
        });
        settings.save(&config_ini());
    });
    d.on_choice(|_, _, index| {
        settings.update(|s| {
            s.shape = [MarkShape::Square, MarkShape::Circle, MarkShape::Diamond][index.min(2)];
        });
        settings.save(&config_ini());
    });
    d.on_number(|_, id, value| {
        settings.update(|s| {
            if id == ID_SIZE {
                s.size_pct = value;
            } else {
                s.fill_pct = value;
            }
        });
        settings.save(&config_ini());
    });
    d.on_end_session(|| release(&MOUSE_LEFT)); // logoff/shutdown can end us without WM_DESTROY
    let rc = d.run(SW_SHOWDEFAULT);

    // The Updater join can wait out a WinHTTP timeout (up to 10 s): let a relaunch start meanwhile.
    instance.release();
    drop(clicker_thread); // it and the engine read the Updater's session, so they go first
    drop(engine_thread);
    drop(updater_thread);
    update.finish(); // after the mutex is released: starts the staged update
    rc
}

fn main() {
    std::process::exit(run());
}
