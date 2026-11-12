use crate::game::board::{Board, FLAG, HIDDEN, board_bytes};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mark {
    #[default]
    None,
    Safe,
    Mine,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Solution {
    /// false when no placement of mines fits the numbers (e.g. read mid-update)
    pub consistent: bool,
    /// false when a group of tiles hit the search limit (its tiles stay None)
    pub complete: bool,
    /// the board's mine count weighed the chances and marks
    pub weighted: bool,
    /// per cell, as Board::cells; only hidden and flagged cells get Safe or Mine
    pub marks: Vec<Mark>,
    /// per cell; -1 where unknown (no number beside it without a mine count)
    pub mine_chance: Vec<f32>,
    /// the best guess (cell index) when no tile is sure safe; -1 = none
    pub hint: i32,
    /// the tile whose click opens the most, a sure-safe one if any; -1 = none
    pub opener: i32,
    /// the opener's chance of being a 0
    pub open_chance: f32,
    /// Every opener to show, best first (the opener above): every one within 5% of the best, then for the players
    /// spread over one board each next best that is a 0 at least half the time and 3+ tiles from those before it. With
    /// its 0 chance.
    pub openers: Vec<(i32, f32)>,
    /// Everyone plays one board, so one player can be stuck while another has sure-safe tiles. A stuck area is a run
    /// of edge tiles (hidden, beside a number, joined through the numbers they share) with no sure-safe tile in or
    /// beside it: `hints` holds each one's least likely mines (all within 2 points of the best), `stuck` marks its
    /// unsure tiles (per cell).
    pub hints: Vec<i32>,
    pub stuck: Vec<bool>,
}

impl Default for Solution {
    fn default() -> Self {
        Self {
            consistent: true,
            complete: true,
            weighted: false,
            marks: Vec::new(),
            mine_chance: Vec::new(),
            hint: -1,
            opener: -1,
            open_chance: 0.0,
            openers: Vec::new(),
            hints: Vec::new(),
            stuck: Vec::new(),
        }
    }
}

/// The opener score's powers: 0 chance^zero x hidden neighbours^neighbours. 1 and 1 measured best by hand on the
/// first rounds; tools/debug/board_stats.py tunes them on the recorded rounds into config.ini's [opener] after every
/// round.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OpenerWeights {
    pub zero: f32,
    pub neighbours: f32,
}

impl Default for OpenerWeights {
    fn default() -> Self {
        Self {
            zero: 1.0,
            neighbours: 1.0,
        }
    }
}

pub const SEARCH_LIMIT: usize = 1_000_000;

// The C runtime's lgamma (std has no stable ln_gamma), so the opener weights are exact.
unsafe extern "C" {
    safe fn lgamma(x: f64) -> f64;
}

/// A revealed number: exactly `need` mines among `vars` (its hidden and flagged neighbours).
struct Constraint {
    vars: Vec<usize>,
    need: i32,
}

fn unknown_cell(v: i8) -> bool {
    v == HIDDEN || v == FLAG
}

/// Sets every var each constraint alone decides, until nothing changes. false on a contradiction.
fn propagate(cs: &[Constraint], known: &mut [i8]) -> bool {
    let mut changed = true;
    while changed {
        changed = false;
        for c in cs {
            let (mut mines, mut open) = (0, 0);
            for &v in &c.vars {
                if known[v] == 1 {
                    mines += 1;
                } else if known[v] < 0 {
                    open += 1;
                }
            }
            let rest = c.need - mines;
            if rest < 0 || rest > open {
                return false;
            }
            if open == 0 || (rest != 0 && rest != open) {
                continue;
            }
            for &v in &c.vars {
                if known[v] < 0 {
                    known[v] = if rest == 0 { 0 } else { 1 };
                }
            }
            changed = true;
        }
    }
    true
}

/// One group's placements, split by how many mines they put in the group.
#[derive(Default)]
struct Group {
    /// the group's vars
    order: Vec<usize>,
    /// [k]: placements with k mines
    total: Vec<f64>,
    /// [position in order][k]: those with that var a mine
    mine_count: Vec<Vec<f64>>,
}

/// Counts every mine placement over one group's vars that meets its constraints.
struct Search<'a> {
    cs: &'a [Constraint],
    touching: &'a [Vec<usize>], // constraint indices per var
    known: &'a mut [i8],
    g: &'a mut Group,
    mines: Vec<i32>, // per constraint, over the current partial placement
    open: Vec<i32>,
    placed: i32, // mines in the current partial placement
    nodes: usize,
    limit: usize,
}

impl Search<'_> {
    fn fits(&self, v: usize) -> bool {
        self.touching[v]
            .iter()
            .all(|&c| !(self.mines[c] > self.cs[c].need || self.mines[c] + self.open[c] < self.cs[c].need))
    }

    fn set(&mut self, v: usize, mine: i8, dir: i32) {
        for &c in &self.touching[v] {
            self.open[c] -= dir;
            self.mines[c] += i32::from(mine) * dir;
        }
        self.placed += i32::from(mine) * dir;
        self.known[v] = if dir > 0 { mine } else { -1 };
    }

    /// false once the limit is hit
    fn run(&mut self, i: usize) -> bool {
        self.nodes += 1;
        if self.nodes > self.limit {
            return false;
        }
        if i == self.g.order.len() {
            let placed = self.placed as usize;
            self.g.total[placed] += 1.0;
            for k in 0..self.g.order.len() {
                if self.known[self.g.order[k]] == 1 {
                    self.g.mine_count[k][placed] += 1.0;
                }
            }
            return true;
        }
        let v = self.g.order[i];
        for mine in [0i8, 1] {
            self.set(v, mine, 1);
            let ok = !self.fits(v) || self.run(i + 1);
            self.set(v, mine, -1);
            if !ok {
                return false;
            }
        }
        true
    }
}

fn convolve(a: &[f64], b: &[f64]) -> Vec<f64> {
    let mut out = vec![0.0; a.len() + b.len() - 1];
    for i in 0..a.len() {
        for j in 0..b.len() {
            out[i + j] += a[i] * b[j];
        }
    }
    out
}

/// Which mine counts some placement reaches; kept apart from the weights so an underflowed weight never reads as
/// "impossible" and turns into a sure mark.
fn reachable(a: &[bool], b: &[f64]) -> Vec<bool> {
    let mut out = vec![false; a.len() + b.len() - 1];
    for i in 0..a.len() {
        for j in 0..b.len() {
            if a[i] && b[j] > 0.0 {
                out[i + j] = true;
            }
        }
    }
    out
}

fn max_of(v: &[f64]) -> f64 {
    // as std::max_element: the first largest
    let mut top = v[0];
    for &x in &v[1..] {
        if top < x {
            top = x;
        }
    }
    top
}

/// Marks and chances from each group alone: every placement that fits the numbers counts the same.
fn mark_evenly(groups: &[Group], s: &mut Solution) {
    for g in groups {
        let mut t = 0.0;
        for &x in &g.total {
            t += x;
        }
        for i in 0..g.order.len() {
            let mut m = 0.0;
            for &x in &g.mine_count[i] {
                m += x; // whole counts, so the comparisons below are exact
            }
            s.mine_chance[g.order[i]] = (m / t) as f32;
            if m == 0.0 {
                s.marks[g.order[i]] = Mark::Safe;
            }
            if m == t {
                s.marks[g.order[i]] = Mark::Mine;
            }
        }
    }
}

/// Marks and chances weighing each placement by the ways the mines left over fit in the `interior` unknown cells no
/// number touches, given `left` mines outside what propagation fixed. false when no split fits (e.g. the count is
/// stale), leaving s untouched.
fn mark_by_count(groups: &[Group], interior: &[usize], left: i32, s: &mut Solution) -> bool {
    let u = interior.len() as i32;
    let fits_left = |j: i32| j <= left && left - j <= u;
    let log_ways =
        |j: i32| lgamma(f64::from(u) + 1.) - lgamma(f64::from(left - j) + 1.) - lgamma(f64::from(u - left + j) + 1.);

    // each group's totals over its largest, so products stay in range
    let scaled: Vec<Vec<f64>> = groups
        .iter()
        .map(|g| {
            let top = max_of(&g.total);
            g.total.iter().map(|x| x / top).collect()
        })
        .collect();
    // all groups but `skip` together: weights and reachable counts
    let product = |skip: usize| {
        let mut p: (Vec<f64>, Vec<bool>) = (vec![1.0], vec![true]);
        for (c, sc) in scaled.iter().enumerate() {
            if c != skip {
                p = (convolve(&p.0, sc), reachable(&p.1, sc));
            }
        }
        p
    };
    let (all, all_reach) = product(groups.len());

    let mut top = f64::NEG_INFINITY; // the largest weight's log, taken out of every weight
    for (j, &a) in all.iter().enumerate() {
        if a > 0.0 && fits_left(j as i32) {
            let x = a.ln() + log_ways(j as i32);
            if top < x {
                top = x;
            }
        }
    }
    if !top.is_finite() {
        return false;
    }
    let ways = |j: i32| {
        if fits_left(j) { (log_ways(j) - top).exp() } else { 0.0 }
    };
    let mut weight = 0.0;
    for (j, &a) in all.iter().enumerate() {
        weight += a * ways(j as i32);
    }

    for (c, g) in groups.iter().enumerate() {
        let (rest, rest_reach) = product(c);
        let scale = max_of(&g.total);
        // per k mines in this group: the other groups' weight with it, and whether any reachable split fits
        let mut with = vec![0.0; g.total.len()];
        let mut fits = vec![false; g.total.len()];
        for k in 0..g.total.len() {
            for j in 0..rest.len() {
                with[k] += rest[j] * ways((k + j) as i32) / scale;
                fits[k] = fits[k] || (rest_reach[j] && fits_left((k + j) as i32));
            }
        }
        for i in 0..g.order.len() {
            let mut mine = 0.0;
            let (mut can_mine, mut can_safe) = (false, false);
            for k in 0..g.total.len() {
                mine += g.mine_count[i][k] * with[k];
                if !fits[k] {
                    continue;
                }
                can_mine |= g.mine_count[i][k] > 0.0;
                can_safe |= g.total[k] > g.mine_count[i][k];
            }
            let v = g.order[i];
            s.mine_chance[v] = (mine / weight) as f32;
            s.marks[v] = if !can_mine {
                Mark::Safe
            } else if !can_safe {
                Mark::Mine
            } else {
                Mark::None
            };
        }
    }
    if u > 0 {
        let mut mine = 0.0;
        let (mut can_mine, mut can_safe) = (false, false);
        for (j, &a) in all.iter().enumerate() {
            let j = j as i32;
            mine += a * ways(j) * f64::from(left - j) / f64::from(u);
            if !all_reach[j as usize] || !fits_left(j) {
                continue;
            }
            can_mine |= left - j > 0;
            can_safe |= left - j < u;
        }
        for &v in interior {
            s.mine_chance[v] = (mine / weight) as f32;
            s.marks[v] = if !can_mine {
                Mark::Safe
            } else if !can_safe {
                Mark::Mine
            } else {
                Mark::None
            };
        }
    }
    s.weighted = true;
    true
}

/// Flags count as unknown, so a wrong flag comes out Safe. With Board::mines, each placement on the frontier is
/// weighed by the ways the remaining mines fit in the cells no number touches: exact chances everywhere, and marks
/// the numbers alone can't give (e.g. the last mines). Without it, every placement counts the same and only the
/// frontier gets chances. Sure marks never depend on rounding. When no tile is sure safe, `hint` is the hidden,
/// unflagged tile least likely a mine, a frontier one on a tie (opening it tells more). `opener` is the tile whose
/// click opens the most, scored as its 0 chance times its hidden neighbours (a 0 opens them all and cascades), each
/// raised to its OpenerWeights power: among the sure-safe tiles when there are any, else among all. Before the first
/// click it is the middle at 100%, since the game always opens an area there.
pub fn solve(b: &Board, search_limit: usize, ow: OpenerWeights) -> Solution {
    let n = (b.w * b.h) as usize;
    let mut s = Solution {
        marks: vec![Mark::None; n],
        mine_chance: vec![-1.0; n],
        ..Solution::default()
    };
    let none = Solution {
        consistent: false,
        marks: vec![Mark::None; n],
        mine_chance: vec![-1.0; n],
        ..Solution::default()
    };

    let mut cs: Vec<Constraint> = Vec::new();
    let mut touching: Vec<Vec<usize>> = vec![Vec::new(); n];
    for r in 0..b.h {
        for c in 0..b.w {
            if b.at(c, r) < 0 {
                continue;
            }
            let mut k = Constraint {
                vars: Vec::new(),
                need: i32::from(b.at(c, r)),
            };
            for dr in -1..=1 {
                for dc in -1..=1 {
                    let (rr, cc) = (r + dr, c + dc);
                    if (dr != 0 || dc != 0) && rr >= 0 && rr < b.h && cc >= 0 && cc < b.w && unknown_cell(b.at(cc, rr))
                    {
                        k.vars.push((rr * b.w + cc) as usize);
                    }
                }
            }
            if k.vars.is_empty() {
                if k.need != 0 {
                    return none;
                }
                continue;
            }
            for &v in &k.vars {
                touching[v].push(cs.len());
            }
            cs.push(k);
        }
    }

    let mut known = vec![-1i8; n]; // -1 unknown, 0 safe, 1 mine
    if !propagate(&cs, &mut known) {
        return none;
    }

    // The frontier vars propagation left open, in groups joined by shared constraints; each is searched on its own.
    let mut groups: Vec<Group> = Vec::new();
    let mut grouped = vec![false; n];
    for start in 0..n {
        if grouped[start] || known[start] >= 0 || touching[start].is_empty() {
            continue;
        }
        let mut g = Group::default();
        grouped[start] = true;
        g.order.push(start);
        let mut i = 0;
        while i < g.order.len() {
            // breadth-first: neighbours get tried close together
            for &c in &touching[g.order[i]] {
                for &v in &cs[c].vars {
                    if !grouped[v] && known[v] < 0 {
                        grouped[v] = true;
                        g.order.push(v);
                    }
                }
            }
            i += 1;
        }
        g.total = vec![0.0; g.order.len() + 1];
        g.mine_count = vec![vec![0.0; g.order.len() + 1]; g.order.len()];
        let (mut mines, mut open) = (vec![0; cs.len()], vec![0; cs.len()]);
        for (c, k) in cs.iter().enumerate() {
            for &v in &k.vars {
                if known[v] == 1 {
                    mines[c] += 1;
                } else if known[v] < 0 {
                    open[c] += 1;
                }
            }
        }
        let mut q = Search {
            cs: &cs,
            touching: &touching,
            known: &mut known,
            g: &mut g,
            mines,
            open,
            placed: 0,
            nodes: 0,
            limit: search_limit,
        };
        if !q.run(0) {
            s.complete = false; // this group stays unmarked
            continue;
        }
        if g.total.iter().all(|&x| x == 0.0) {
            return none;
        }
        groups.push(g);
    }

    let mut interior = Vec::new(); // unknown cells no number touches
    let mut fixed_mines = 0;
    for v in 0..n {
        if unknown_cell(b.cells[v]) && touching[v].is_empty() {
            interior.push(v);
        }
        fixed_mines += i32::from(known[v] == 1);
    }
    // a group past the limit has no counts to weigh, so then every group falls back to counting evenly
    let by_count = match b.mines {
        Some(m) if s.complete => mark_by_count(&groups, &interior, m - fixed_mines, &mut s),
        _ => false,
    };
    if !by_count {
        mark_evenly(&groups, &mut s);
    }
    for (v, &k) in known.iter().enumerate() {
        if k >= 0 {
            s.marks[v] = if k != 0 { Mark::Mine } else { Mark::Safe };
            s.mine_chance[v] = f32::from(k);
        }
    }

    let any_safe = s.marks.contains(&Mark::Safe);
    let any_open = b.cells.iter().any(|&v| v >= 0);
    const TIE: f32 = 1e-5;
    if !any_open {
        // the game's first click always opens an area (17 of 17 rounds): the middle has most room to spread
        if n > 0 {
            s.opener = b.h / 2 * b.w + b.w / 2;
            s.open_chance = 1.0;
            s.openers = vec![(s.opener, 1.0)];
        }
        return s;
    }
    // The opener: the tile whose click opens the most, about its 0 chance times the hidden neighbours a 0 opens. On
    // 1852 recorded boards with safe tiles it opened 15.9 tiles a click against 13.8 for the likeliest 0 alone
    // (looking two rings out scored no better). Among the sure-safe tiles when there are any, so the click is free;
    // else any tile.
    let mut top = 0.0f32;
    let mut scored: Vec<(f32, i32, f32)> = Vec::new(); // score, tile, 0 chance
    for v in 0..n {
        if b.cells[v] != HIDDEN || s.mine_chance[v] < 0.0 {
            continue;
        }
        if if any_safe {
            s.marks[v] != Mark::Safe
        } else {
            s.marks[v] == Mark::Mine
        } {
            continue;
        }
        let v = v as i32;
        let p = zero_chance(b, &s, v);
        let score =
            (if p < 0.0 { 0.0 } else { p }).powf(ow.zero) * (hidden_neighbours(b, v) as f32).powf(ow.neighbours);
        if score > top + TIE || (score >= top - TIE && score > TIE && p > s.open_chance + TIE) {
            top = score;
            s.opener = v;
            s.open_chance = p;
        }
        if score > TIE {
            scored.push((score, v, p));
        }
    }
    if s.opener >= 0 {
        s.openers = vec![(s.opener, s.open_chance)];
    }
    scored.sort_by(|a, c| c.0.total_cmp(&a.0)); // stable, best first
    // More: every one about as good as the best (within 5%), then for the rest of the board each next best that is a
    // 0 at least half the time and 3+ tiles from those already shown.
    for &(score, v, p) in &scored {
        if v == s.opener {
            continue;
        }
        let apart = s
            .openers
            .iter()
            .all(|o| (o.0 / b.w - v / b.w).abs().max((o.0 % b.w - v % b.w).abs()) >= 3);
        if score >= top * 0.95 || (p >= 0.5 && apart) {
            s.openers.push((v, p));
        }
    }

    mark_stuck_areas(b, &mut s);
    if any_safe {
        return s; // a sure-safe tile beats any guess
    }
    for v in 0..n {
        if b.cells[v] != HIDDEN || s.marks[v] != Mark::None || s.mine_chance[v] < 0.0 {
            continue;
        }
        let c = s.mine_chance[v];
        let best = if s.hint < 0 {
            2.0
        } else {
            s.mine_chance[s.hint as usize]
        };
        // (with no hint yet the first test always holds, so touching[hint] is only read for a real one)
        if c < best - TIE || (c <= best + TIE && !touching[v].is_empty() && touching[s.hint as usize].is_empty()) {
            s.hint = v as i32;
        }
    }
    s
}

/// The cells around `v` (not itself), row by row.
fn around(b: &Board, v: usize) -> impl Iterator<Item = usize> + '_ {
    let (row, col) = (v as i32 / b.w, v as i32 % b.w);
    (-1..=1).flat_map(move |dr| {
        (-1..=1).filter_map(move |dc| {
            let (r, c) = (row + dr, col + dc);
            ((dr != 0 || dc != 0) && r >= 0 && r < b.h && c >= 0 && c < b.w).then_some((r * b.w + c) as usize)
        })
    })
}

/// Fills s.hints and s.stuck from s.marks and s.mine_chance (solve calls it).
// shortcut: multiplies the cells' safe chances as if independent, within about 0.1% of the joint count for a few
// cells among hundreds; count placements jointly if a small endgame needs it exact.
pub fn mark_stuck_areas(b: &Board, s: &mut Solution) {
    let n = (b.w * b.h) as usize;
    let edge = |v: usize| b.cells[v] < 0 && around(b, v).any(|u| b.cells[u] >= 0);
    s.hints.clear();
    s.stuck = vec![false; n];
    let mut seen = vec![false; n];
    for start in 0..n {
        if seen[start] || !edge(start) {
            continue;
        }
        let mut area = vec![start];
        seen[start] = true;
        let mut i = 0;
        while i < area.len() {
            // through every number beside it, to that number's other edge tiles
            for num in around(b, area[i]) {
                if b.cells[num] >= 0 {
                    for w in around(b, num) {
                        if !seen[w] && edge(w) {
                            seen[w] = true;
                            area.push(w);
                        }
                    }
                }
            }
            i += 1;
        }
        let mut free = false; // a sure-safe tile in it or beside it: this area has a move
        for &v in &area {
            free |= s.marks[v] == Mark::Safe;
            for u in around(b, v) {
                free |= b.cells[u] < 0 && s.marks[u] == Mark::Safe;
            }
        }
        if free {
            continue;
        }
        let mut best: Option<usize> = None;
        for &v in &area {
            if b.cells[v] != HIDDEN || s.marks[v] != Mark::None || s.mine_chance[v] < 0.0 {
                continue;
            }
            s.stuck[v] = true;
            if best.is_none_or(|bst| s.mine_chance[v] < s.mine_chance[bst]) {
                best = Some(v);
            }
        }
        let Some(best) = best else { continue };
        const NEAR: f32 = 0.02; // as good a guess as the best: let the players around pick the closest
        for &v in &area {
            if s.stuck[v] && s.mine_chance[v] <= s.mine_chance[best] + NEAR {
                s.hints.push(v as i32);
            }
        }
    }
}

/// How many of `cell`'s neighbours are hidden (flags don't open with a 0).
pub fn hidden_neighbours(b: &Board, cell: i32) -> i32 {
    let mut n = 0;
    for dr in -1..=1 {
        for dc in -1..=1 {
            let (r, c) = (cell / b.w + dr, cell % b.w + dc);
            n += i32::from((dr != 0 || dc != 0) && r >= 0 && r < b.h && c >= 0 && c < b.w && b.at(c, r) == HIDDEN);
        }
    }
    n
}

/// The chance `cell` is a 0: it and its hidden neighbours all safe (revealed ones are). -1 when a neighbour has no
/// chance (no mine count and off the frontier).
pub fn zero_chance(b: &Board, s: &Solution, cell: i32) -> f32 {
    let mut p = 1.0f32;
    for dr in -1..=1 {
        for dc in -1..=1 {
            let (r, c) = (cell / b.w + dr, cell % b.w + dc);
            if r < 0 || r >= b.h || c < 0 || c >= b.w || b.at(c, r) >= 0 {
                continue; // revealed ones are safe
            }
            let m = s.mine_chance[(r * b.w + c) as usize];
            if m < 0.0 {
                return -1.0; // a neighbour with no chance: this tile can't be scored
            }
            p *= 1.0 - m;
        }
    }
    p
}

/// "3 safe · 2 mines", "no mines fit the numbers", plus " · by the mine count", " · best guess 33%",
/// " · opener 47%" and " · search limit hit".
pub fn solution_summary(s: &Solution) -> String {
    if !s.consistent {
        return "no mines fit the numbers".into();
    }
    let safe = s.marks.iter().filter(|&&m| m == Mark::Safe).count();
    let mines = s.marks.iter().filter(|&&m| m == Mark::Mine).count();
    let mut t = format!(
        "{safe} safe \u{b7} {mines}{}",
        if mines == 1 { " mine" } else { " mines" }
    );
    if s.weighted {
        t += " \u{b7} by the mine count";
    }
    if s.hint >= 0 {
        t += &format!(" \u{b7} best guess {}", percent_text(s.mine_chance[s.hint as usize]));
    }
    if s.opener >= 0 {
        t += &format!(" \u{b7} opener {}", percent_text(s.open_chance));
    }
    if !s.complete {
        t += " \u{b7} search limit hit";
    }
    t
}

/// A mine chance as a rounded percentage: "33%".
pub fn percent_text(chance: f32) -> String {
    format!("{}%", (chance * 100.0).round() as i32)
}

/// board_text with sure marks over it: 'S' safe, '*' mine (where hidden or flagged), '?' the hint, 'O' the opener.
pub fn solution_text(b: &Board, s: &Solution) -> String {
    let mut t = board_bytes(b);
    let w = b.w as usize;
    let at = |v: i32| (v / b.w) as usize * (w + 1) + (v % b.w) as usize;
    for r in 0..b.h as usize {
        for c in 0..w {
            let m = s.marks[r * w + c];
            if m != Mark::None {
                t[r * (w + 1) + c] = if m == Mark::Safe { b'S' } else { b'*' };
            }
        }
    }
    for &(v, _) in &s.openers {
        t[at(v)] = b'O';
    }
    for &h in &s.hints {
        t[at(h)] = b'?'; // over an opener on the same tile
    }
    if s.hint >= 0 {
        t[at(s.hint)] = b'?';
    }
    String::from_utf8_lossy_owned(t)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::board::{board_from_text, board_text};

    fn parse(text: &str) -> Board {
        board_from_text(text)
    }

    fn sol(b: &Board) -> Solution {
        solve(b, SEARCH_LIMIT, OpenerWeights::default())
    }

    fn solved(b: &Board) -> String {
        solution_text(b, &sol(b))
    }

    fn solved_text(text: &str) -> String {
        solved(&parse(text))
    }

    #[test]
    fn solver_single_numbers_decide_mines_and_safes() {
        assert_eq!(solved_text("#\n1\n"), "*\n1\n");
        // the bottom 1 finds the mine, so the top 1's other tile is safe (it has no hidden neighbour to open: no
        // opener)
        assert_eq!(solved_text("#\n1\n#\n1\n"), "S\n1\n*\n1\n");
    }

    #[test]
    fn solver_one_two_one_needs_the_search() {
        // A+B=1, A+B+C=2, B+C=1: no number decides alone, together A and C are mines and B is safe.
        assert_eq!(solved_text("###\n121\n...\n"), "*S*\n121\n...\n");
        assert_eq!(
            solution_summary(&sol(&parse("###\n121\n...\n"))),
            "1 safe \u{b7} 2 mines"
        );
    }

    #[test]
    fn solver_gives_chances_where_nothing_is_sure() {
        let b = parse("###\n#1#\n###\n");
        let s = sol(&b);
        assert!(s.consistent && s.complete);
        // all tie at 1 in 8, so all eight are the area's hints (the first is the board's); the opener is an edge tile
        // (4 hidden neighbours beat a corner's 2), under its hint
        assert!(solution_text(&b, &s) == "???\n?1?\n???\n" && s.hints.len() == 8 && s.opener == 1);
        assert!((s.mine_chance[0] - 0.125).abs() < 1e-6);
        assert_eq!(s.mine_chance[4], -1.0); // the number itself
    }

    #[test]
    fn solver_checks_flags_instead_of_trusting_them() {
        assert_eq!(solved_text("F\n1\n"), "*\n1\n");
        assert_eq!(solved_text("F\n1\n#\n1\n"), "S\n1\n*\n1\n"); // a wrong flag comes out safe
    }

    #[test]
    fn solver_reports_a_board_no_mines_fit() {
        for t in ["#\n2\n", "##\n1.\n", "#.\n1#\n"] {
            let b = parse(t);
            let s = sol(&b);
            assert!(!s.consistent);
            assert_eq!(solution_text(&b, &s), board_text(&b)); // no marks
        }
    }

    #[test]
    fn solver_weighs_by_the_boards_mine_count() {
        // The 1 has one mine on its two sides; the three cells on the right touch no number.
        let mut b = parse("#1####\n");
        let mut s = sol(&b);
        assert!(!s.weighted && s.mine_chance[0] == 0.5 && s.mine_chance[3] == -1.0);

        b.mines = Some(1); // the 1's mine is the only one: the rest are safe
        s = sol(&b);
        assert_eq!(solution_text(&b, &s), "#1#SOS\n");
        assert!(s.weighted && s.mine_chance[0] == 0.5 && s.mine_chance[3] == 0.0);
        b.mines = Some(2); // one of three on the right
        s = sol(&b);
        // the far cells (1 in 3) beat the 1's sides (1 in 2): the hint goes to the first far cell
        // (and the 1's own stuck area gets its least likely tile, the first of its two halves)
        assert!(solution_text(&b, &s) == "?1??O#\n" && (s.mine_chance[4] - 1.0 / 3.0).abs() < 1e-6);
        b.mines = Some(4); // all three on the right
        assert_eq!(solved(&b), "?1?***\n"); // the 1's two sides tie at 1 in 2: both are hints
        b.mines = Some(5); // more than fit: a stale count is ignored
        s = sol(&b);
        assert!(s.consistent && !s.weighted && s.mine_chance[0] == 0.5);
        assert_eq!(
            solution_summary(&sol(&parse("###\n121\n...\n"))),
            "1 safe \u{b7} 2 mines"
        );
    }

    #[test]
    fn solver_weighs_frontier_placements_by_the_mines_they_leave() {
        // X 1 Y 1 Z then two cells no number touches. Either Y alone (1 mine) or X and Z (2 mines). With 2 mines on the
        // board, Y leaves 1 mine for the 2 far cells (2 ways) and X+Z leaves none (1 way): Y is twice as likely.
        let mut b = parse("#1#1###\n");
        assert_eq!(sol(&b).mine_chance[2], 0.5); // evenly, without the count
        b.mines = Some(2);
        let s = sol(&b);
        assert!(s.weighted && solution_text(&b, &s) == "?1#1?O#\n"); // X and Z tie at 1 in 3: both hints
        assert!((s.mine_chance[2] - 2.0 / 3.0).abs() < 1e-6);
        assert!((s.mine_chance[0] - 1.0 / 3.0).abs() < 1e-6 && (s.mine_chance[4] - 1.0 / 3.0).abs() < 1e-6);
        assert!((s.mine_chance[5] - 1.0 / 3.0).abs() < 1e-6); // (2/3 of the time, 1 of 2 cells)
    }

    #[test]
    fn solver_hints_the_least_likely_mine_when_nothing_is_sure() {
        // As on a live endgame board: Y serves both 1s, two far cells, 3 mines left. Y as the mine leaves 2 for the 2
        // far cells (1 way); X and Z leave 1 (2 ways): Y is a mine 1 time in 3, everything else 2 in 3.
        let mut b = parse("#1#1###\n");
        b.mines = Some(3);
        let s = sol(&b);
        assert!(s.hint == 2 && (s.mine_chance[2] - 1.0 / 3.0).abs() < 1e-6);
        assert_eq!(solution_text(&b, &s), "#1?1O#O\n"); // the far end ties with Z as an opener: both show
        assert_eq!(
            solution_summary(&s),
            "0 safe \u{b7} 0 mines \u{b7} by the mine count \u{b7} best guess 33% \u{b7} opener 11%"
        );
        assert_eq!(s.opener, 4); // Y has no hidden neighbour to open; Z and its far neighbour are both safe 1 time in 9

        b.mines = Some(2); // X, Z and the far cells tie at 1 in 3: a frontier tile wins, it tells more when opened
        assert_eq!(sol(&b).hint, 0);
    }

    #[test]
    fn solver_gives_no_hint_with_a_safe_tile_or_before_the_first_click() {
        assert_eq!(sol(&parse("###\n121\n...\n")).hint, -1);
        let mut fresh = parse("##\n##\n");
        fresh.mines = Some(1);
        assert_eq!(sol(&fresh).hint, -1);
        assert_eq!(sol(&parse("F\n1\n#\n1\n")).hint, -1); // the wrong flag is the safe move
    }

    #[test]
    fn solver_opener_is_the_tile_that_opens_the_most() {
        // Before the first click the game always opens an area: the middle, with the most room to spread, sure to.
        let mut fresh = parse("####\n####\n####\n####\n");
        fresh.mines = Some(1);
        let s = sol(&fresh);
        assert!(s.hint == -1 && s.opener == 10 && s.open_chance == 1.0);
        assert_eq!(solution_text(&fresh, &s), "####\n####\n##O#\n####\n");

        // #1####, 2 mines: the 1's sides are mines half the time, the far cells a third. The left corner is a 0 half
        // the time but has no hidden neighbour left to open; the middle far cell is a 0 8 times in 27 and opens two:
        // 0.59 tiles against the corner's 0. Opener and best guess (the first far cell) differ.
        let mut b = parse("#1####\n");
        b.mines = Some(2);
        let s = sol(&b);
        assert!(s.opener == 4 && (s.open_chance - 8.0 / 27.0).abs() < 1e-5 && s.hint == 3);
        assert!(hidden_neighbours(&b, 4) == 2 && hidden_neighbours(&b, 0) == 0);
    }

    #[test]
    fn solver_hints_each_stuck_area_even_with_safe_tiles_elsewhere() {
        // Two players' areas on one board: the left one's edge (0 and 2, around the 1) has a sure-safe tile, the right
        // one's (4 and 6) has none. Only the right one is stuck: its least likely mine is the hint, its tiles get
        // chances.
        let b = parse("#1###1#\n");
        let mut s = Solution {
            marks: vec![
                Mark::Safe,
                Mark::None,
                Mark::None,
                Mark::None,
                Mark::None,
                Mark::None,
                Mark::None,
            ],
            mine_chance: vec![0.0, -1.0, 0.5, 0.4, 0.3, -1.0, 0.6],
            ..Solution::default()
        };
        mark_stuck_areas(&b, &mut s);
        assert_eq!(s.hints, vec![4]);
        assert!(!s.stuck[2] && !s.stuck[3] && s.stuck[4] && s.stuck[6]);

        s.mine_chance[6] = 0.31; // within 2 points of the best: as good a guess, so a hint too
        mark_stuck_areas(&b, &mut s);
        assert_eq!(s.hints, vec![4, 6]);

        // a sure-safe tile beside the area frees it too: #1####'s safe far cells sit next to the 1's right side
        let mut one = parse("#1####\n");
        one.mines = Some(1);
        assert!(sol(&one).hints.is_empty());
        one.mines = Some(2); // nothing safe anywhere: the area around the 1 is stuck as well as the board's best guess
        let t = sol(&one);
        assert!(t.hint == 3 && t.hints == vec![0, 2]); // the 1's two sides tie at 1 in 2
    }

    #[test]
    fn solver_opener_follows_its_weights() {
        // #1####, 2 mines: by default (0 chance x hidden neighbours) the middle far tile; with neighbours weighed 0 the
        // score is the 0 chance alone, and the left corner (a 0 half the time) wins.
        let mut b = parse("#1####\n");
        b.mines = Some(2);
        assert_eq!(sol(&b).opener, 4);
        let w = OpenerWeights {
            zero: 1.0,
            neighbours: 0.0,
        };
        assert_eq!(solve(&b, SEARCH_LIMIT, w).opener, 0);
    }

    #[test]
    fn solver_shows_an_opener_for_every_part_of_the_board() {
        // #1######, 1 mine: the right five are safe. Cells 4, 5 and 6 each open two sure 0 neighbours: all as good, all
        // shown (4 is the opener). Cell 7 opens one, and sits too close to them to add another.
        let mut b = parse("#1######\n");
        b.mines = Some(1);
        let s = sol(&b);
        assert!(s.opener == 4 && s.openers.len() == 3 && s.openers[1].0 == 5 && s.openers[2].0 == 6);
        assert_eq!(solution_text(&b, &s), "#1#SOOOS\n");

        // in the 1-in-3 guess Z and the far end tie as openers (11% each): both show, nothing else
        b = parse("#1#1###\n");
        b.mines = Some(3);
        assert_eq!(sol(&b).openers.len(), 2);
    }

    #[test]
    fn solver_opener_picks_the_safe_tile_that_opens_the_most() {
        // #1####, 1 mine: it's beside the 1, so the right three are safe. The one next to the 1's unsure side is a 0
        // half the time; the other two have only safe neighbours, so they are sure 0s. No guess with a safe tile
        // around.
        let mut b = parse("#1####\n");
        b.mines = Some(1);
        let s = sol(&b);
        assert!(s.opener == 4 && s.open_chance == 1.0 && s.hint == -1);
        assert_eq!(solution_text(&b, &s), "#1#SOS\n");

        // the 1-2-1's safe tile has a mine on each side: it can't be a 0, so no opener
        assert_eq!(sol(&parse("###\n121\n...\n")).opener, -1);
    }

    #[test]
    fn solver_leaves_a_group_unmarked_past_the_search_limit() {
        let b = parse("###\n121\n...\n");
        let s = solve(&b, 3, OpenerWeights::default());
        assert!(s.consistent && !s.complete);
        assert_eq!(solution_text(&b, &s), board_text(&b));
    }
}
