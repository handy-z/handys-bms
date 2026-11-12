use std::path::Path;

use handy_sdk::util::ini::Ini;
use handy_sdk::util::shared_settings::{SaveSettings, SharedSettings};

use crate::solver::solver::OpenerWeights;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MarkShape {
    #[default]
    Square,
    Circle,
    Diamond,
}

const SHAPE_NAMES: [&str; 3] = ["square", "circle", "diamond"]; // MarkShape order
const SHAPES: [MarkShape; 3] = [MarkShape::Square, MarkShape::Circle, MarkShape::Diamond];

/// [overlay] in config.ini: how the marks look.
#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    pub shape: MarkShape,
    /// of the tile's width
    pub size_pct: i32,
    /// fill opacity; 0 = outline only
    pub fill_pct: i32,
    pub outline: bool,
    pub show_safe: bool,
    pub show_mines: bool,
    pub show_hint: bool,
    pub show_opener: bool,
    /// [macro] x2=: holding mouse X2 spams left click (macro/clicker); F2 in Roblox flips it
    pub click_x2: bool,
    /// [overlay] on=: the marks at all; F1 in Roblox flips it
    pub overlay_on: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            shape: MarkShape::Square,
            size_pct: 80,
            fill_pct: 45,
            outline: true,
            show_safe: true,
            show_mines: true,
            show_hint: true,
            show_opener: true,
            click_x2: true,
            overlay_on: true,
        }
    }
}

pub const MIN_SIZE_PCT: i32 = 10;
pub const MAX_SIZE_PCT: i32 = 100;

/// Every number is clamped to the dashboard's range on load; an unknown shape loads as the default.
pub fn load_settings(file: &Path) -> Settings {
    let ini = Ini::with_section(file, "overlay");
    let (d, mut s) = (Settings::default(), Settings::default());
    let shape = ini.get_str("shape", SHAPE_NAMES[d.shape as usize]);
    for (name, sh) in SHAPE_NAMES.iter().zip(SHAPES) {
        if shape == *name {
            s.shape = sh;
        }
    }
    s.size_pct = ini.get_int_clamped("size", d.size_pct, MIN_SIZE_PCT, MAX_SIZE_PCT);
    s.fill_pct = ini.get_int_clamped("fill", d.fill_pct, 0, 100);
    s.outline = ini.get_bool("outline", d.outline);
    s.show_safe = ini.get_bool("safe", d.show_safe);
    s.show_mines = ini.get_bool("mines", d.show_mines);
    s.show_hint = ini.get_bool("hint", d.show_hint);
    s.show_opener = ini.get_bool("opener", d.show_opener);
    s.click_x2 = Ini::with_section(file, "macro").get_bool("x2", d.click_x2);
    s.overlay_on = ini.get_bool("on", d.overlay_on);
    s
}

pub fn save_settings(file: &Path, s: &Settings) {
    let ini = Ini::with_section(file, "overlay");
    ini.put_str("shape", SHAPE_NAMES[s.shape as usize]);
    ini.put_int("size", s.size_pct);
    ini.put_int("fill", s.fill_pct);
    ini.put_bool("outline", s.outline);
    ini.put_bool("safe", s.show_safe);
    ini.put_bool("mines", s.show_mines);
    ini.put_bool("hint", s.show_hint);
    ini.put_bool("opener", s.show_opener);
    Ini::with_section(file, "macro").put_bool("x2", s.click_x2);
    ini.put_bool("on", s.overlay_on);
}

impl SaveSettings for Settings {
    fn save(&self, ini: &Path) {
        save_settings(ini, self);
    }
}

pub type SettingsStore = SharedSettings<Settings>;

/// strtof: the longest leading number after any whitespace; None when there is none.
// shortcut: Rust's float syntax, so a hex float ("0x1p2") reads as its leading 0; the tuner writes decimals.
fn leading_float(text: &str) -> Option<f32> {
    let t = text.trim_start();
    t.char_indices()
        .map(|(i, c)| i + c.len_utf8())
        .rev()
        .find_map(|end| t[..end].parse::<f32>().ok())
}

/// [opener] zero= and neighbours= in config.ini, written by tools/debug/board_stats.py's tuning (not the dashboard);
/// a missing, unreadable or out-of-range (past 0..4) one is 1.
pub fn load_opener_weights(file: &Path) -> OpenerWeights {
    let ini = Ini::with_section(file, "opener");
    let weight = |key: &str| {
        // past the tuner's range is a broken file: the default
        leading_float(&ini.get_str(key, "1"))
            .filter(|v| (0.0..=4.0).contains(v))
            .unwrap_or(1.0)
    };
    OpenerWeights {
        zero: weight("zero"),
        neighbours: weight("neighbours"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn temp_ini(name: &str) -> PathBuf {
        let p = std::env::temp_dir().join(name);
        let _ = std::fs::remove_file(&p);
        p
    }

    #[test]
    fn config_defaults_when_missing() {
        assert_eq!(
            load_settings(&temp_ini("handys-bms-rs-missing.ini")),
            Settings::default()
        );
    }

    #[test]
    fn config_round_trips() {
        let p = temp_ini("handys-bms-rs-roundtrip.ini");
        let s = Settings {
            shape: MarkShape::Diamond,
            size_pct: 55,
            fill_pct: 0,
            outline: false,
            show_safe: true,
            show_mines: false,
            show_hint: false,
            show_opener: false,
            click_x2: false,
            overlay_on: false,
        };
        save_settings(&p, &s);
        assert_eq!(load_settings(&p), s);
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn config_reads_the_tuned_opener_weights() {
        let p = temp_ini("handys-bms-rs-opener.ini");
        assert!(load_opener_weights(&p).zero == 1.0 && load_opener_weights(&p).neighbours == 1.0); // none tuned yet
        std::fs::write(&p, "[opener]\nzero=1.5\nneighbours=x\n").unwrap();
        assert!(load_opener_weights(&p).zero == 1.5 && load_opener_weights(&p).neighbours == 1.0);
        std::fs::write(&p, "[opener]\nzero=9\nneighbours=0.25\n").unwrap();
        assert!(load_opener_weights(&p).zero == 1.0 && load_opener_weights(&p).neighbours == 0.25);
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn config_clamps_hand_edits() {
        let p = temp_ini("handys-bms-rs-clamp.ini");
        std::fs::write(&p, "[overlay]\nshape=hexagon\nsize=500\nfill=-3\n").unwrap();
        let s = load_settings(&p);
        assert_eq!(s.shape, MarkShape::Square);
        assert_eq!(s.size_pct, MAX_SIZE_PCT);
        assert_eq!(s.fill_pct, 0);
        let _ = std::fs::remove_file(&p);
    }
}
