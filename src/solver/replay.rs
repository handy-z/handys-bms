use std::fmt::Write;

use crate::game::board::{Board, HIDDEN, board_from_text};
use crate::solver::solver::{Mark, OpenerWeights, SEARCH_LIMIT, hidden_neighbours, solve, zero_chance};

fn mine_at(layout: &[&str], col: i32, row: i32) -> bool {
    row >= 0
        && (row as usize) < layout.len()
        && col >= 0
        && (col as usize) < layout[row as usize].len()
        && layout[row as usize].as_bytes()[col as usize] == b'*'
}

/// It and every neighbour mine-free: opening it cascades.
fn zero_at(layout: &[&str], col: i32, row: i32) -> bool {
    for dr in -1..=1 {
        for dc in -1..=1 {
            if mine_at(layout, col + dc, row + dr) {
                return false;
            }
        }
    }
    true
}

/// How many hidden tiles clicking `cell` opens on the real layout: it, and on a 0 its neighbours, cascading.
fn cascade(layout: &[&str], b: &Board, cell: i32) -> i32 {
    let mut seen = vec![false; b.cells.len()];
    let mut todo = vec![cell];
    seen[cell as usize] = true;
    let mut opened = 0;
    while let Some(v) = todo.pop() {
        let (col, row) = (v % b.w, v / b.w);
        opened += 1;
        if !zero_at(layout, col, row) {
            continue;
        }
        for dr in -1..=1 {
            for dc in -1..=1 {
                let (r, c) = (row + dr, col + dc);
                let u = r * b.w + c;
                if r < 0 || r >= b.h || c < 0 || c >= b.w || seen[u as usize] || b.cells[u as usize] != HIDDEN {
                    continue; // flags stay
                }
                seen[u as usize] = true;
                todo.push(u);
            }
        }
    }
    opened
}

/// Lines as std::getline gives them: split on '\n' only, none after a final '\n'.
struct Lines<'a>(&'a str);

impl<'a> Lines<'a> {
    fn next_line(&mut self) -> Option<&'a str> {
        if self.0.is_empty() {
            return None;
        }
        let (line, rest) = self.0.split_once('\n').unwrap_or((self.0, ""));
        self.0 = rest;
        Some(line)
    }

    /// `h` more lines, empty past the end.
    fn rows(&mut self, h: i32) -> Vec<&'a str> {
        (0..h.max(0)).map(|_| self.next_line().unwrap_or("")).collect()
    }
}

/// Scores the solver on recorded rounds (tools/debug/board_stats.py writes them from board_log.py's replays):
///   round <id> <w> <h> <mines, -1 unknown>   then h rows of the real layout ('*' mine, '.' safe)
///   state <k>                                then h rows of board_text, as the players saw it
/// Returns a line per sure mark the layout proves wrong ("wrong <round> <k> <cell> safe|mine"), per state with no
/// sure-safe tile ("guess <round> <k> <hint> <its chance> <was a mine> <opener> <its 0 chance> <was a 0> <was a
/// mine> <unsure hidden tiles>"), per sure-safe hidden tile ("safe <round> <k> <cell> <its 0 chance> <hidden
/// neighbours> <tiles clicking it opens on the layout> <is the opener>"), then "calib <lo %> <tiles> <mines>" per 5%
/// band of unsure hidden tiles' chances and "total <states> <guesses> <inconsistent> <search limit hit> <wrong>".
pub fn score_replay(input: &str, ow: OpenerWeights) -> String {
    let mut out = String::new();
    let mut calib = [[0i64; 2]; 20]; // per 5% band: tiles, mines
    let (mut states, mut guesses, mut inconsistent, mut limited, mut wrong) = (0i64, 0i64, 0i64, 0i64, 0i64);
    let mut round = String::new();
    let (mut w, mut h, mut mines) = (0i32, 0i32, -1i32);
    let mut layout: Vec<&str> = Vec::new();
    let mut lines = Lines(input);
    while let Some(line) = lines.next_line() {
        let mut ls = line.split_ascii_whitespace();
        let kind = ls.next().unwrap_or("");
        if kind == "round" {
            // as `>>` reads them: a missing field and those after it stay as they were; one that isn't a number
            // reads as 0 and ends the line
            if let Some(r) = ls.next() {
                r.clone_into(&mut round);
                for f in [&mut w, &mut h, &mut mines] {
                    let Some(t) = ls.next() else { break };
                    match t.parse() {
                        Ok(v) => *f = v,
                        Err(_) => {
                            *f = 0;
                            break;
                        }
                    }
                }
            }
            layout = lines.rows(h);
            continue;
        }
        if kind != "state" {
            continue;
        }
        let k = ls.next().unwrap_or("");
        let mut text = String::new();
        for r in lines.rows(h) {
            text += r;
            text += "\n";
        }
        let mut b = board_from_text(&text);
        if mines >= 0 {
            b.mines = Some(mines);
        }
        let s = solve(&b, SEARCH_LIMIT, ow);
        states += 1;
        if !s.consistent {
            // a torn read, or the layout's round got mixed up: nothing to score
            inconsistent += 1;
            continue;
        }
        limited += i64::from(!s.complete);
        let mut any_safe = false;
        let mut hidden = 0;
        for i in 0..b.cells.len() {
            let mine = mine_at(&layout, i as i32 % b.w, i as i32 / b.w);
            any_safe |= s.marks[i] == Mark::Safe;
            hidden += i32::from(b.cells[i] == HIDDEN && s.marks[i] == Mark::None); // only sure mines left is no guess
            if (s.marks[i] == Mark::Safe && mine) || (s.marks[i] == Mark::Mine && !mine) {
                wrong += 1;
                let _ = writeln!(out, "wrong {round} {k} {i} {}", if mine { "safe" } else { "mine" });
            }
            if b.cells[i] == HIDDEN && s.marks[i] == Mark::None && s.mine_chance[i] >= 0.0 {
                let band = &mut calib[19.min((s.mine_chance[i] * 20.0) as i32) as usize];
                band[0] += 1;
                band[1] += i64::from(mine);
            }
        }
        if any_safe {
            for i in 0..b.cells.len() as i32 {
                if b.cells[i as usize] != HIDDEN || s.marks[i as usize] != Mark::Safe {
                    continue;
                }
                let _ = writeln!(
                    out,
                    "safe {round} {k} {i} {:.4} {} {} {}",
                    zero_chance(&b, &s, i),
                    hidden_neighbours(&b, i),
                    cascade(&layout, &b, i),
                    i32::from(i == s.opener)
                );
            }
        }
        if any_safe || hidden == 0 || s.hint < 0 {
            continue;
        }
        guesses += 1;
        let (hc, hr, oc, orow) = (s.hint % b.w, s.hint / b.w, s.opener % b.w, s.opener / b.w);
        let has_opener = s.opener >= 0;
        let _ = writeln!(
            out,
            "guess {round} {k} {} {:.4} {} {} {:.4} {} {} {hidden}",
            s.hint,
            s.mine_chance[s.hint as usize],
            i32::from(mine_at(&layout, hc, hr)),
            s.opener,
            s.open_chance,
            i32::from(has_opener && zero_at(&layout, oc, orow)),
            i32::from(has_opener && mine_at(&layout, oc, orow)),
        );
    }
    for (i, band) in calib.iter().enumerate() {
        if band[0] != 0 {
            let _ = writeln!(out, "calib {} {} {}", i * 5, band[0], band[1]);
        }
    }
    let _ = writeln!(out, "total {states} {guesses} {inconsistent} {limited} {wrong}");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scored(input: &str) -> String {
        score_replay(input, OpenerWeights::default())
    }

    #[test]
    fn replay_scores_guesses_and_counts_states() {
        // "#1#": one mine in two tiles, a coin flip; the layout puts it on the left
        let out = scored("round r7 3 1 -1\n*..\nstate 0\n#1#\n");
        assert!(out.starts_with("guess r7 0 "), "{out}");
        assert!(out.contains(" 0.5000 "));
        assert!(out.contains("total 1 1 0 0 0\n"));
    }

    #[test]
    fn replay_reports_sure_marks_the_layout_disproves() {
        // ".1#": the 1 makes the right tile a sure mine
        let out = scored("round r1 3 1 -1\n..*\nstate 4\n#1#\nstate 5\n.1#\n");
        assert!(!out.contains("wrong")); // as laid out
        assert!(out.contains("total 2 1 0 0 0\n")); // state 4 a coin flip, state 5 nothing left to guess
        let bad = scored("round r1 3 1 -1\n*..\nstate 5\n.1#\n");
        assert!(bad.contains("wrong r1 5 2 mine")); // marked a mine, but the layout has it safe
        assert!(bad.contains("total 1 0 0 0 1\n"));
    }

    #[test]
    fn replay_skips_boards_no_layout_fits() {
        assert_eq!(scored("round r 2 1 -1\n..\nstate 0\n22\n"), "total 1 0 1 0 0\n");
    }
}
