use handy_sdk::roblox::screen::{ScreenView, Vec2, vec3, world_to_screen};

use crate::app::config::{MarkShape, Settings};
use crate::game::board::{Board, FLAG, HIDDEN};
use crate::solver::solver::{Mark, Solution};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MarkKind {
    Safe,
    Mine,
    WrongFlag,
    Hint,
    Opener,
    Chance,
}

/// A marked tile's shape on its top face, on screen in render pixels (ScreenView::size).
#[derive(Clone, Debug, PartialEq)]
pub struct MarkPolygon {
    pub pts: Vec<Vec2>,
    pub kind: MarkKind,
    /// for the hint's and a Chance's label; -1 = none
    pub mine_chance: f32,
    /// for the opener's label (also on a hint that is the opener too); -1 = none
    pub open_chance: f32,
}

const CIRCLE_POINTS: i32 = 24; // smooth at a tile's on-screen size

/// The shape's outline around a tile's centre on X/Z, for a half-width r.
fn outline(shape: MarkShape, r: f32) -> Vec<(f32, f32)> {
    match shape {
        MarkShape::Diamond => vec![(0.0, -r), (r, 0.0), (0.0, r), (-r, 0.0)],
        MarkShape::Circle => (0..CIRCLE_POINTS)
            .map(|i| {
                let a = 2.0 * std::f32::consts::PI * i as f32 / CIRCLE_POINTS as f32;
                (r * a.cos(), r * a.sin())
            })
            .collect(),
        MarkShape::Square => vec![(-r, -r), (r, -r), (r, r), (-r, r)],
    }
}

/// A number touches it, so its chance is its own; the tiles no number touches all share one.
fn beside_number(b: &Board, col: i32, row: i32) -> bool {
    for r in 0.max(row - 1)..=(b.h - 1).min(row + 1) {
        for c in 0.max(col - 1)..=(b.w - 1).min(col + 1) {
            if b.at(c, r) >= 0 {
                return true;
            }
        }
    }
    false
}

/// One polygon per Safe or Mine tile the settings show, in their shape and size, laid on the tile's top face so it
/// follows the camera's perspective; a tile with any point behind the camera is left out. A flagged mine gets none
/// (its flag marks it); a flagged tile the solver proves safe is a WrongFlag, shown whatever the settings hide. The
/// solver's hint is a Hint when show_hint is on, and its opener an Opener when show_opener is on (a tile that is both
/// is one Hint carrying both chances). While there is a hint (nothing is sure safe), every other unsure hidden tile
/// beside a number is a Chance carrying its mine chance, when show_hint is on: a label with no shape.
pub fn mark_polygons(v: &ScreenView, b: &Board, s: &Solution, st: &Settings) -> Vec<MarkPolygon> {
    let mut out = Vec::new();
    let (half, top) = (b.step / 2.0, b.origin.y + b.step / 2.0); // tiles are cubes `step` wide
    let shape = outline(st.shape, half * st.size_pct as f32 / 100.0);
    let mut opens = vec![-1.0f32; b.cells.len()]; // each opener's 0 chance
    if st.show_opener {
        for &(tile, p) in &s.openers {
            opens[tile as usize] = p;
        }
    }
    let mut hinted = vec![false; b.cells.len()]; // the board's best guess and each stuck area's
    if s.hint >= 0 {
        hinted[s.hint as usize] = true;
    }
    for &h in &s.hints {
        hinted[h as usize] = true;
    }
    for row in 0..b.h {
        for col in 0..b.w {
            let cell = (row * b.w + col) as usize;
            let m = s.marks[cell];
            let flagged = b.at(col, row) == FLAG;
            let stuck = !s.stuck.is_empty() && s.stuck[cell];
            let kind = if hinted[cell] && st.show_hint {
                MarkKind::Hint
            } else if opens[cell] >= 0.0 {
                MarkKind::Opener
            } else if m == Mark::Safe && flagged {
                MarkKind::WrongFlag // always shown: a wrong flag is worth seeing whatever else is hidden
            } else if m == Mark::Safe && st.show_safe {
                MarkKind::Safe
            } else if m == Mark::Mine && !flagged && st.show_mines {
                MarkKind::Mine // a flagged mine is already marked by its flag
            } else if st.show_hint
                && (stuck || (s.hint >= 0 && beside_number(b, col, row)))
                && m == Mark::None
                && b.at(col, row) == HIDDEN
                && s.mine_chance[cell] >= 0.0
            {
                MarkKind::Chance // a guess is due here: the other choices with theirs, to weigh against the hint
            } else {
                continue;
            };
            let (x, z) = (b.origin.x + col as f32 * b.step, b.origin.z + row as f32 * b.step);
            let chance = kind == MarkKind::Hint || kind == MarkKind::Chance;
            let mut p = MarkPolygon {
                pts: Vec::new(),
                kind,
                mine_chance: if chance { s.mine_chance[cell] } else { -1.0 },
                open_chance: opens[cell],
            };
            for &(dx, dz) in &shape {
                let Some(at) = world_to_screen(v, vec3(x + dx, top, z + dz)) else {
                    break;
                };
                p.pts.push(at);
            }
            if p.pts.len() == shape.len() {
                out.push(p);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    // Looks straight down with no perspective: screen x = (x + 1) * 400, screen y = (z + 1) * 300.
    fn top_down() -> ScreenView {
        ScreenView {
            m: [1., 0., 0., 0., 0., 0., -1., 0., 0., 0., 0., 0., 0., 0., 0., 1.],
            size: Vec2 { x: 800., y: 600. },
        }
    }

    // 2x1 board of half-stud tiles centred at x = 0 and 0.5, z = 0.
    fn two_tiles() -> Board {
        Board {
            w: 2,
            h: 1,
            step: 0.5,
            origin: vec3(0., 0., 0.),
            cells: vec![HIDDEN, HIDDEN],
            mines: None,
        }
    }

    fn marks(a: Mark, b: Mark) -> Solution {
        Solution {
            marks: vec![a, b],
            ..Solution::default()
        }
    }

    fn close_to(a: Vec2, x: f32, y: f32) -> bool {
        (a.x - x).abs() < 1e-3 && (a.y - y).abs() < 1e-3
    }

    #[test]
    fn marks_square_covers_the_top_face_at_its_size() {
        let p = mark_polygons(
            &top_down(),
            &two_tiles(),
            &marks(Mark::None, Mark::Mine),
            &Settings::default(),
        );
        assert_eq!(p.len(), 1);
        assert!(p[0].kind == MarkKind::Mine && p[0].pts.len() == 4);
        // tile at x 0.5: half width 0.25, at the default 80% -> x 0.3..0.7, z -0.2..0.2
        assert!(close_to(p[0].pts[0], 1.3 * 400., 0.8 * 300.));
        assert!(close_to(p[0].pts[2], 1.7 * 400., 1.2 * 300.));

        let full = Settings {
            size_pct: 100,
            ..Settings::default()
        };
        assert!(close_to(
            mark_polygons(&top_down(), &two_tiles(), &marks(Mark::None, Mark::Mine), &full)[0].pts[0],
            1.25 * 400.,
            0.75 * 300.
        ));
    }

    #[test]
    fn marks_diamond_and_circle_shapes() {
        let mut st = Settings {
            shape: MarkShape::Diamond,
            ..Settings::default()
        };
        let p = mark_polygons(&top_down(), &two_tiles(), &marks(Mark::Safe, Mark::None), &st);
        assert!(p.len() == 1 && p[0].pts.len() == 4);
        assert!(close_to(p[0].pts[0], 400., 0.8 * 300.)); // straight "up" in z from the tile at x 0
        assert!(close_to(p[0].pts[1], 1.2 * 400., 300.));

        st.shape = MarkShape::Circle;
        let p = mark_polygons(&top_down(), &two_tiles(), &marks(Mark::Safe, Mark::None), &st);
        assert!(p.len() == 1 && p[0].pts.len() == 24);
        assert!(close_to(p[0].pts[0], 1.2 * 400., 300.)); // angle 0: +x
        for pt in &p[0].pts {
            // every point 0.2 studs from the centre
            let (dx, dz) = (pt.x / 400. - 1., pt.y / 300. - 1.);
            assert!((dx.hypot(dz) - 0.2).abs() < 1e-4);
        }
    }

    #[test]
    fn marks_hide_what_the_settings_turn_off() {
        let mut st = Settings {
            show_safe: false,
            ..Settings::default()
        };
        let p = mark_polygons(&top_down(), &two_tiles(), &marks(Mark::Safe, Mark::Mine), &st);
        assert!(p.len() == 1 && p[0].kind == MarkKind::Mine);
        st.show_safe = true;
        st.show_mines = false;
        let p = mark_polygons(&top_down(), &two_tiles(), &marks(Mark::Safe, Mark::Mine), &st);
        assert!(p.len() == 1 && p[0].kind == MarkKind::Safe);
    }

    #[test]
    fn marks_skip_flagged_mines_and_warn_on_wrong_flags() {
        let mut b = two_tiles();
        b.cells = vec![FLAG, FLAG];
        let p = mark_polygons(&top_down(), &b, &marks(Mark::Mine, Mark::Safe), &Settings::default());
        assert!(p.len() == 1 && p[0].kind == MarkKind::WrongFlag); // the flagged mine is left to its flag
        assert!(close_to(p[0].pts[0], 1.3 * 400., 0.8 * 300.)); // on the second tile

        let hidden = Settings {
            show_safe: false,
            show_mines: false,
            ..Settings::default()
        };
        let p = mark_polygons(&top_down(), &b, &marks(Mark::None, Mark::Safe), &hidden);
        assert!(p.len() == 1 && p[0].kind == MarkKind::WrongFlag); // shown whatever the settings hide
    }

    #[test]
    fn marks_show_the_hint_with_its_chance() {
        let mut s = marks(Mark::None, Mark::None);
        s.mine_chance = vec![0.25, 0.5];
        s.hint = 0;
        let p = mark_polygons(&top_down(), &two_tiles(), &s, &Settings::default());
        assert!(p.len() == 1 && p[0].kind == MarkKind::Hint && p[0].mine_chance == 0.25);
        let off = Settings {
            show_hint: false,
            ..Settings::default()
        };
        assert!(mark_polygons(&top_down(), &two_tiles(), &s, &off).is_empty());
    }

    #[test]
    fn marks_label_the_other_guesses_beside_numbers_while_a_guess_is_due() {
        // "#1##": the 1's two tiles are the guesses, the last tile touches no number
        let b = Board {
            w: 4,
            h: 1,
            step: 0.5,
            origin: vec3(0., 0., 0.),
            cells: vec![HIDDEN, 1, HIDDEN, HIDDEN],
            mines: None,
        };
        let mut s = Solution {
            marks: vec![Mark::None, Mark::None, Mark::None, Mark::None],
            mine_chance: vec![0.5, -1.0, 0.5, 0.3],
            hint: 3,
            ..Solution::default()
        };
        let p = mark_polygons(&top_down(), &b, &s, &Settings::default());
        assert_eq!(p.len(), 3);
        assert!(p[0].kind == MarkKind::Chance && p[0].mine_chance == 0.5 && p[0].open_chance == -1.0);
        assert!(p[1].kind == MarkKind::Chance && p[1].mine_chance == 0.5);
        assert_eq!(p[2].kind, MarkKind::Hint);

        s.hint = -1; // something is sure safe: no guess due, no chances
        s.marks[0] = Mark::Safe;
        let p = mark_polygons(&top_down(), &b, &s, &Settings::default());
        assert!(p.len() == 1 && p[0].kind == MarkKind::Safe);

        s.hint = 3;
        s.marks[0] = Mark::None;
        let off = Settings {
            show_hint: false,
            ..Settings::default()
        };
        assert!(mark_polygons(&top_down(), &b, &s, &off).is_empty());

        // safe tiles elsewhere (no board-wide guess) but this area is stuck: its own hint, chances on its other tiles
        s.hint = -1;
        s.hints = vec![2];
        s.stuck = vec![true, false, true, false];
        let p = mark_polygons(&top_down(), &b, &s, &Settings::default());
        assert!(
            p.len() == 2 && p[0].kind == MarkKind::Chance && p[1].kind == MarkKind::Hint && p[1].mine_chance == 0.5
        );
    }

    #[test]
    fn marks_show_the_opener_and_merge_it_into_the_hint() {
        let mut s = marks(Mark::None, Mark::None);
        s.mine_chance = vec![0.25, 0.5];
        s.hint = 0;
        s.openers = vec![(1, 0.4)];
        let p = mark_polygons(&top_down(), &two_tiles(), &s, &Settings::default());
        assert_eq!(p.len(), 2);
        assert!(p[0].kind == MarkKind::Hint && p[0].open_chance == -1.0);
        assert!(p[1].kind == MarkKind::Opener && p[1].open_chance == 0.4 && p[1].mine_chance == -1.0);

        s.openers = vec![(0, 0.4)]; // the same tile: one hint with both chances
        let p = mark_polygons(&top_down(), &two_tiles(), &s, &Settings::default());
        assert!(p.len() == 1 && p[0].kind == MarkKind::Hint && p[0].mine_chance == 0.25 && p[0].open_chance == 0.4);

        let off = Settings {
            show_opener: false,
            ..Settings::default()
        };
        let p = mark_polygons(&top_down(), &two_tiles(), &s, &off);
        assert!(p.len() == 1 && p[0].open_chance == -1.0);

        s = marks(Mark::Safe, Mark::Safe); // a sure-safe opener: purple instead of green, labelled with its percent only
        s.openers = vec![(1, 0.62)];
        let p = mark_polygons(&top_down(), &two_tiles(), &s, &Settings::default());
        assert!(
            p.len() == 2 && p[0].kind == MarkKind::Safe && p[1].kind == MarkKind::Opener && p[1].mine_chance == -1.0
        );

        s.openers = vec![(0, 0.62), (1, 0.55)]; // every opener shows, each with its own chance
        let p = mark_polygons(&top_down(), &two_tiles(), &s, &Settings::default());
        assert!(p.len() == 2 && p[0].open_chance == 0.62 && p[1].open_chance == 0.55);
    }

    #[test]
    fn marks_leave_out_tiles_behind_the_camera() {
        let mut v = top_down();
        v.m[12] = 1.;
        v.m[15] = 0.; // w = x: only points past x = 0.1 are in front
        let p = mark_polygons(&v, &two_tiles(), &marks(Mark::Safe, Mark::Safe), &Settings::default());
        assert_eq!(p.len(), 1); // the tile at x 0 has corners at x -0.2
    }
}
