use std::sync::Arc;

use handy_sdk::app::updater_thread::Session;
use handy_sdk::roblox::instance::{
    child_by_class, child_by_name, children, data_model, instance_class, instance_name, resolve_path,
};
use handy_sdk::roblox::memory::Memory;
use handy_sdk::roblox::offsets::Offsets;
use handy_sdk::roblox::read::{gui_text, local_player, part_position, player_gui, workspace_of};
use handy_sdk::roblox::screen::Vec3;

pub const HIDDEN: i8 = -1;
pub const FLAG: i8 = -2;

/// One tile Part as read: its centre, BasePart.Color3, the count its NumberGui shows (None without one), and
/// whether a flag (a Model of any skin) stands on it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TileRead {
    pub pos: Vec3,
    pub rgb: [u8; 3],
    pub number: Option<i32>,
    pub flagged: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Board {
    /// columns along X
    pub w: i32,
    /// rows along Z
    pub h: i32,
    /// centre of the tile at column 0, row 0 (lowest X and Z)
    pub origin: Vec3,
    /// studs between tile centres; the tiles are cubes this size
    pub step: f32,
    /// row-major: HIDDEN, FLAG, or the revealed count 0..8
    pub cells: Vec<i8>,
    /// the whole board's mine count, from the HUD; None when it can't be read
    pub mines: Option<i32>,
}

impl Board {
    pub fn at(&self, col: i32, row: i32) -> i8 {
        self.cells[row as usize * self.w as usize + col as usize]
    }
}

// shortcut: hidden tiles are the brown checker (R about 55 over G), revealed ones yellow (R = G), as measured in the
// default theme; read the theme's colors when another theme turns up.
fn hidden_color(c: &[u8; 3]) -> bool {
    i32::from(c[0]) > i32::from(c[1]) + 30
}

/// Fills t.number and t.flagged from the part's NumberGui and flag children.
fn read_marks(m: &dyn Memory, o: &Offsets, part: usize, t: &mut TileRead) {
    let kids = children(m, o, part);
    if kids.len() < 2 {
        return; // only its TouchInterest: skips the name reads on most tiles
    }
    for c in kids {
        // each flag skin is a Model named after it (Stub, Rose, Speedrun, MVP, ...); nothing else on a tile is a Model
        if instance_class(m, o, c).as_deref() == Some("Model") {
            t.flagged = true;
        }
        if instance_name(m, o, c).as_deref() != Some("NumberGui") {
            continue;
        }
        let label = children(m, o, c);
        let text = label.first().and_then(|l| gui_text(m, o, *l)).unwrap_or_default();
        t.number = Some(text.bytes().find(u8::is_ascii_digit).map_or(0, |d| i32::from(d - b'0')));
    }
}

/// The HUD's "Mines: 100" (PlayerGui.InfoGui.Frame.Mines); None when it isn't there or shows no number.
fn mine_count(m: &dyn Memory, o: &Offsets, dm: usize) -> Option<i32> {
    let players = child_by_class(m, o, dm, "Players");
    let gui = player_gui(m, o, local_player(m, o, players));
    let label = if gui != 0 {
        resolve_path(m, o, gui, &["InfoGui", "Frame", "Mines"]).inst
    } else {
        0
    };
    let text = gui_text(m, o, label)?;
    let mut n: Option<i32> = None;
    for ch in text.bytes() {
        if ch.is_ascii_digit() && n.unwrap_or(0) < 100_000 {
            n = Some(n.unwrap_or(0) * 10 + i32::from(ch - b'0'));
        }
    }
    n
}

/// The grid the tiles form; None when they don't fill a whole rectangle once each (e.g. mid-rebuild).
pub fn build_board(tiles: &[TileRead]) -> Option<Board> {
    let first = tiles.first()?;
    let mut b = Board {
        origin: first.pos,
        ..Board::default()
    };
    for t in tiles {
        b.origin.x = b.origin.x.min(t.pos.x);
        b.origin.z = b.origin.z.min(t.pos.z);
    }
    b.step = f32::INFINITY;
    for t in tiles {
        for d in [t.pos.x - b.origin.x, t.pos.z - b.origin.z] {
            if d > 0.5 && d < b.step {
                b.step = d;
            }
        }
    }
    if !b.step.is_finite() {
        b.step = 1.0; // a single row or column: any step places it
    }

    let mut at = Vec::with_capacity(tiles.len());
    for t in tiles {
        let (cx, cz) = ((t.pos.x - b.origin.x) / b.step, (t.pos.z - b.origin.z) / b.step);
        let (col, row) = (cx.round() as i32, cz.round() as i32);
        if (cx - col as f32).abs() > 0.1 || (cz - row as f32).abs() > 0.1 {
            return None; // off the grid
        }
        at.push((col, row));
        b.w = b.w.max(col + 1);
        b.h = b.h.max(row + 1);
    }
    if b.w as usize * b.h as usize != tiles.len() {
        return None;
    }
    b.cells = vec![0; tiles.len()];
    let mut seen = vec![false; tiles.len()];
    for (t, &(col, row)) in tiles.iter().zip(&at) {
        let k = row as usize * b.w as usize + col as usize;
        if seen[k] || t.number.is_some_and(|n| !(0..=8).contains(&n)) {
            return None;
        }
        seen[k] = true;
        b.cells[k] = match t.number {
            Some(n) => n as i8,
            None if t.flagged => FLAG,
            None if hidden_color(&t.rgb) => HIDDEN,
            None => 0,
        };
    }
    Some(b)
}

/// board_text as bytes, so solution_text can mark it by position whatever the cells hold.
pub(crate) fn board_bytes(b: &Board) -> Vec<u8> {
    let mut s = Vec::new();
    for r in 0..b.h {
        for c in 0..b.w {
            let v = b.at(c, r);
            s.push(match v {
                HIDDEN => b'#',
                FLAG => b'F',
                0 => b'.',
                _ => (b'0' as i8).wrapping_add(v) as u8,
            });
        }
        s.push(b'\n');
    }
    s
}

/// One line per row: '#' hidden, 'F' flagged, '.' revealed blank, else the count.
pub fn board_text(b: &Board) -> String {
    String::from_utf8_lossy_owned(board_bytes(b))
}

/// The inverse of board_text (no position or mine count).
pub fn board_from_text(text: &str) -> Board {
    let mut b = Board::default();
    // as std::getline: split on '\n' only, no empty row after a final '\n'
    let rows = text.strip_suffix('\n').unwrap_or(text);
    if text.is_empty() {
        return b;
    }
    for row in rows.split('\n') {
        b.w = row.len() as i32;
        b.cells.extend(row.bytes().map(|ch| match ch {
            b'#' => HIDDEN,
            b'F' => FLAG,
            b'.' => 0,
            _ => (i32::from(ch as i8) - i32::from(b'0')) as i8,
        }));
        b.h += 1;
    }
    b
}

/// "25x25 · 15 revealed · 609 hidden · 1 flagged · 99 mines"
pub fn board_summary(b: &Board) -> String {
    let hidden = b.cells.iter().filter(|&&v| v == HIDDEN).count();
    let flagged = b.cells.iter().filter(|&&v| v == FLAG).count();
    format!(
        "{}x{} \u{b7} {} revealed \u{b7} {} hidden \u{b7} {} flagged \u{b7} {} mines",
        b.w,
        b.h,
        b.cells.len() - hidden - flagged,
        hidden,
        flagged,
        b.mines.map_or_else(|| "?".to_owned(), |n| n.to_string())
    )
}

/// Reads Workspace.Flag.Parts. On None, failed_at (when given) names the step that failed.
pub fn read_board(m: &dyn Memory, o: &Offsets, dm: usize, failed_at: Option<&mut String>) -> Option<Board> {
    let fail = |step: &str| -> Option<Board> {
        if let Some(f) = failed_at {
            step.clone_into(f);
        }
        None
    };
    let ws = workspace_of(m, o, dm);
    if ws == 0 {
        return fail("Workspace");
    }
    let flag = child_by_name(m, o, ws, "Flag");
    if flag == 0 {
        return fail("Flag");
    }
    let parts = child_by_name(m, o, flag, "Parts");
    if parts == 0 {
        return fail("Flag.Parts");
    }

    let mut tiles = Vec::new();
    for p in children(m, o, parts) {
        let pos = part_position(m, o, p);
        let rgb = m.get::<[u8; 3]>(o.at(p, "BasePart.Color3")); // Color3uint8
        let (Some(pos), Some(rgb)) = (pos, rgb) else {
            return fail("a tile");
        };
        let mut t = TileRead {
            pos,
            rgb,
            ..TileRead::default()
        };
        read_marks(m, o, p, &mut t);
        tiles.push(t);
    }
    let Some(mut b) = build_board(&tiles) else {
        return fail("the grid");
    };
    b.mines = mine_count(m, o, dm);
    Some(b)
}

/// For handy_sdk's SessionReader.
pub struct BoardReader {
    pub session: Arc<Session>,
}

impl BoardReader {
    pub fn new(session: &Arc<Session>) -> Self {
        Self {
            session: session.clone(),
        }
    }

    pub fn read(&self, failed_at: Option<&mut String>) -> Option<Board> {
        let (p, o) = (&self.session.proc, &self.session.offsets);
        read_board(p, o, data_model(p, o), failed_at)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use handy_sdk::roblox::screen::vec3;
    use handy_sdk::testing::{FakeMemory, fake_instance_offsets};

    const BROWN: [u8; 3] = [180, 125, 88];
    const YELLOW: [u8; 3] = [255, 255, 125];

    // Test-only field offsets past the 0x40 FakeMemory::instance uses.
    fn offs() -> Offsets {
        let mut o = fake_instance_offsets();
        o.values.extend(
            [
                ("DataModel.Workspace", 0x48),
                ("BasePart.Primitive", 0x50),
                ("BasePart.Color3", 0x58),
                ("GuiObject.Text", 0x60),
                ("Player.LocalPlayer", 0x68),
                ("Primitive.Position", 0x10),
            ]
            .map(|(k, v)| (k.to_owned(), v)),
        );
        o
    }

    fn label(m: &mut FakeMemory, name: &str, text: &str) -> usize {
        let l = m.instance("TextLabel", name, &[]);
        let (size, cap) = (text.len() as u64, 15u64); // an inline MSVC string in the label's Text field
        let mut z = text.as_bytes().to_vec();
        z.push(0);
        m.put(l + 0x60, &z);
        m.put_val(l + 0x70, &size);
        m.put_val(l + 0x78, &cap);
        l
    }

    fn tile(m: &mut FakeMemory, pos: Vec3, rgb: [u8; 3], number: Option<&str>, flag: Option<&str>) -> usize {
        let mut kids = vec![m.instance("TouchTransmitter", "TouchInterest", &[])];
        if let Some(flag) = flag {
            let primary = m.instance("Part", "Primary", &[]);
            kids.push(m.instance("Model", flag, &[primary])); // named after its skin
        }
        if let Some(number) = number {
            let l = label(m, "TextLabel", number);
            kids.push(m.instance("SurfaceGui", "NumberGui", &[l]));
        }
        let (p, prim) = (m.instance("Part", "Part", &kids), m.alloc(0x40));
        m.put_ptr(p + 0x50, prim);
        m.put_val(prim + 0x10, &pos);
        m.put(p + 0x58, &rgb);
        p
    }

    fn tr(pos: Vec3, rgb: [u8; 3], number: Option<i32>, flagged: bool) -> TileRead {
        TileRead {
            pos,
            rgb,
            number,
            flagged,
        }
    }

    #[test]
    fn board_build_places_shuffled_tiles_on_the_grid() {
        let t = [
            tr(vec3(65., 70., 90.), YELLOW, Some(3), false),
            tr(vec3(55., 70., 85.), BROWN, None, false),
            tr(vec3(60., 70., 90.), YELLOW, None, false),
            tr(vec3(65., 70., 85.), BROWN, None, true),
            tr(vec3(55., 70., 90.), YELLOW, Some(1), false),
            tr(vec3(60., 70., 85.), BROWN, None, false),
        ];
        let b = build_board(&t);
        assert!(b.is_some());
        let b = b.unwrap();
        assert!(b.w == 3 && b.h == 2);
        assert_eq!(b.step, 5.0);
        assert!(b.origin.x == 55.0 && b.origin.y == 70.0 && b.origin.z == 85.0);
        assert_eq!(board_text(&b), "##F\n1.3\n");
        assert_eq!(
            board_summary(&b),
            "3x2 \u{b7} 3 revealed \u{b7} 2 hidden \u{b7} 1 flagged \u{b7} ? mines"
        );
    }

    #[test]
    fn board_build_rejects_a_missing_duplicate_or_off_grid_tile() {
        let mut t = vec![
            tr(vec3(0., 0., 0.), BROWN, None, false),
            tr(vec3(5., 0., 0.), BROWN, None, false),
            tr(vec3(0., 0., 5.), BROWN, None, false),
        ];
        assert!(build_board(&t).is_none()); // 2x2 with one missing
        t.push(tr(vec3(0., 0., 5.), BROWN, None, false));
        assert!(build_board(&t).is_none()); // the missing one's place taken twice
        t.last_mut().unwrap().pos = vec3(5., 0., 7.);
        assert!(build_board(&t).is_none()); // between rows
        t.last_mut().unwrap().pos = vec3(5., 0., 5.);
        assert!(build_board(&t).is_some());
        t.last_mut().unwrap().number = Some(9);
        assert!(build_board(&t).is_none());
        assert!(build_board(&[]).is_none());
    }

    #[test]
    fn board_read_walks_flag_parts_and_reads_numbers() {
        let mut m = FakeMemory::default();
        let kids = [
            tile(&mut m, vec3(0., 70., 0.), BROWN, None, Some("Stub")),
            tile(&mut m, vec3(5., 70., 0.), YELLOW, Some("2"), None),
            tile(&mut m, vec3(0., 70., 5.), BROWN, None, Some("Speedrun")),
            tile(&mut m, vec3(5., 70., 5.), YELLOW, Some(""), None),
        ];
        let parts = m.instance("Folder", "Parts", &kids);
        let explosion = m.instance("Sound", "Explosion", &[]);
        let flag = m.instance("Flag", "Flag", &[explosion, parts]);
        let board = m.instance("Model", "Board", &[]);
        let ws = m.instance("Workspace", "Workspace", &[board, flag]);
        let dm = m.instance("DataModel", "Game", &[ws]);
        m.put_ptr(dm + 0x48, ws);

        let b = read_board(&m, &offs(), dm, None);
        assert!(b.is_some());
        let b = b.unwrap();
        assert_eq!(board_text(&b), "F2\nF.\n"); // every flag skin counts
        assert!(b.mines.is_none()); // no HUD yet

        let mines = label(&mut m, "Mines", "Mines: 12");
        let frame = m.instance("Frame", "Frame", &[mines]);
        let hud = m.instance("ScreenGui", "InfoGui", &[frame]);
        let pg = m.instance("PlayerGui", "PlayerGui", &[hud]);
        let me = m.instance("Player", "Me", &[pg]);
        let players = m.instance("Players", "Players", &[me]);
        m.put_ptr(players + 0x68, me);
        let dm2 = m.instance("DataModel", "Game", &[ws, players]);
        m.put_ptr(dm2 + 0x48, ws);
        let b = read_board(&m, &offs(), dm2, None);
        assert!(b.is_some_and(|b| b.mines == Some(12)));

        let empty = m.instance("Workspace", "Workspace", &[]);
        m.put_ptr(dm + 0x48, empty);
        let mut failed = String::new();
        assert!(read_board(&m, &offs(), dm, Some(&mut failed)).is_none());
        assert_eq!(failed, "Flag");
    }
}
