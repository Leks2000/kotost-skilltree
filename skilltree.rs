// Kotost Skill Tree: a port of Passive Skill Tree (Daripher, GPL-3.0) to Horizon, written from
// the mod's own data files (skills, texts, textures: `data/`, baked by kotost_skilltree_tools/bake.py).
// 
// Server: the points (bought with experience), the learned skills, their bonuses, the save file.
// Client: the tree's screen (O, or /skilltree).
// Bonuses this version runs: vanilla attributes, damage, critical hits, evasion, blocking, block
// break speed, jump height, experience per minute, healing on a hit, gems. The others show grey.
// Gems (0.2.0): the items are the content mod `kotost_gems`; here they drop from ores, go into
// sockets on an anvil (an item with a gem on the right) and give their bonus while worn or held.
use super::Game;
use horizon_client::hud::HudBatch;
use horizon_client::items::ItemStack;
use horizon_client::mod_api::{
    EquipmentSlot, NativeCommand, NativeLootDrops, NativeScreenView, NativeServerPlayer, ServerTickPhase,
};
use horizon_client::text::Text;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

pub const MOD: &str = "kotost_skilltree";
const CH_ASK: &str = "kotost_skilltree:ask";
const CH_LEARN: &str = "kotost_skilltree:learn";
const CH_BUY: &str = "kotost_skilltree:buy";
const CH_STATE: &str = "kotost_skilltree:state";
const CH_NOTE: &str = "kotost_skilltree:note";

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

// ---- the tree (both sides) --------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq)]
enum Dc {
    Any,
    Melee,
    Projectile,
    Magic,
}

#[derive(Clone, PartialEq)]
enum Cond {
    None,
    Hand(String),
    Equip(String),
    Hp(f64, f64),
    /// Gems in the worn or held items of a type: at least, at most (-1: no bound).
    Gems(String, f64, f64),
}

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Attr,
    Dmg,
    Crit,
    CritDmg,
    Evasion,
    Blocking,
    XpPerMinute,
    /// Healing on a melee hit.
    Heal,
    /// A chance (`v`) for `x` more gems from an ore.
    GemLoot,
    /// Gems put into items of type `e` are stronger by `v` (read at the anvil).
    GemPower,
    /// `v` more sockets in items of type `e` (read at the anvil).
    Sockets,
}

struct Fx {
    kind: Kind,
    v: f64,
    op: u8,
    attr: String,
    dc: Dc,
    cond: Cond,
    x: f64,
    e: String,
    /// "For each gem in items of this type": the value is multiplied by that count.
    per_gem: Option<String>,
}

fn parse_fx(v: &serde_json::Value) -> Option<Fx> {
    let f = |v: &serde_json::Value| v.as_f64().unwrap_or(0.0);
    let c = &v["c"];
    let e = || c["e"].as_str().unwrap_or("any").to_owned();
    let cond = match c["k"].as_str() {
        Some("hand") => Cond::Hand(e()),
        Some("equip") => Cond::Equip(e()),
        Some("hp") => Cond::Hp(f(&c["min"]), f(&c["max"])),
        Some("gems") => Cond::Gems(e(), f(&c["min"]), f(&c["max"])),
        _ => Cond::None,
    };
    let kind = match v["t"].as_str()? {
        "attr" => Kind::Attr,
        "dmg" => Kind::Dmg,
        "crit" => Kind::Crit,
        "critdmg" => Kind::CritDmg,
        "evasion" => Kind::Evasion,
        "blocking" => Kind::Blocking,
        "exp_per_minute" => Kind::XpPerMinute,
        "heal" => Kind::Heal,
        "gemloot" => Kind::GemLoot,
        "gempower" => Kind::GemPower,
        "sockets" => Kind::Sockets,
        _ => return None,
    };
    let dc = match v["dc"].as_str() {
        Some("melee") => Dc::Melee,
        Some("projectile") => Dc::Projectile,
        Some("magic") => Dc::Magic,
        _ => Dc::Any,
    };
    Some(Fx {
        kind,
        v: f(&v["v"]),
        op: v["op"].as_u64().unwrap_or(0) as u8,
        attr: v["a"].as_str().unwrap_or("").to_owned(),
        dc,
        cond,
        x: f(&v["x"]),
        e: v["e"].as_str().unwrap_or("any").to_owned(),
        per_gem: v["m"].as_str().map(str::to_owned),
    })
}

struct Skill {
    id: String,
    x: f32,
    y: f32,
    size: f32,
    frame: usize,
    icon: u32,
    start: bool,
    direct: Vec<u16>,
    long: Vec<u16>,
    one_way: Vec<u16>,
    name: [String; 2],
    lines: [Vec<(String, u8)>; 2],
    fx: Vec<Fx>,
}

struct Tree {
    skills: Vec<Skill>,
    frames: Vec<[f32; 4]>,
    bg: [f32; 4],
    /// Who each skill can be reached from (direct and long links both ways, one-way links forward).
    from: Vec<Vec<u16>>,
}

fn tree() -> &'static Tree {
    static TREE: OnceLock<Tree> = OnceLock::new();
    TREE.get_or_init(|| parse_tree(crate::native_data!("kotost_skilltree", "tree.json")))
}

fn parse_tree(bytes: &[u8]) -> Tree {
    use serde_json::Value;
    let doc: Value = serde_json::from_slice(bytes).unwrap_or(Value::Null);
    let f = |v: &Value| v.as_f64().unwrap_or(0.0);
    let rect = |v: &Value| -> [f32; 4] {
        let a = v.as_array().cloned().unwrap_or_default();
        let g = |i: usize| a.get(i).map(&f).unwrap_or(0.0) as f32;
        [g(0), g(1), g(2), g(3)]
    };
    let ids = |v: &Value| -> Vec<u16> {
        v.as_array().map(|a| a.iter().map(|x| x.as_u64().unwrap_or(0) as u16).collect()).unwrap_or_default()
    };
    let lines = |v: &Value| -> Vec<(String, u8)> {
        v.as_array()
            .map(|a| {
                a.iter()
                    .map(|l| (l[0].as_str().unwrap_or("").to_owned(), l[1].as_u64().unwrap_or(0) as u8))
                    .collect()
            })
            .unwrap_or_default()
    };
    let mut skills = Vec::new();
    for s in doc["skills"].as_array().cloned().unwrap_or_default() {
        skills.push(Skill {
            id: s["id"].as_str().unwrap_or("").to_owned(),
            x: f(&s["x"]) as f32,
            y: f(&s["y"]) as f32,
            size: f(&s["s"]) as f32,
            frame: s["f"].as_u64().unwrap_or(0) as usize,
            icon: s["i"].as_u64().unwrap_or(0) as u32,
            start: s["st"].as_u64().unwrap_or(0) == 1,
            direct: ids(&s["d"]),
            long: ids(&s["l"]),
            one_way: ids(&s["o"]),
            name: [s["ru"][0].as_str().unwrap_or("").to_owned(), s["en"][0].as_str().unwrap_or("").to_owned()],
            lines: [lines(&s["lru"]), lines(&s["len"])],
            fx: s["fx"].as_array().map(|a| a.iter().filter_map(parse_fx).collect()).unwrap_or_default(),
        });
    }
    let n = skills.len();
    let mut from = vec![Vec::new(); n];
    for (i, s) in skills.iter().enumerate() {
        for &j in s.direct.iter().chain(&s.long) {
            if (j as usize) < n {
                from[j as usize].push(i as u16);
                from[i].push(j);
            }
        }
        for &j in &s.one_way {
            if (j as usize) < n {
                from[j as usize].push(i as u16);
            }
        }
    }
    let frames = doc["frames"].as_array().map(|a| a.iter().map(&rect).collect()).unwrap_or_default();
    Tree { skills, frames, bg: rect(&doc["bg"]), from }
}

/// Whether `i` can be learned next: a starting point when nothing is learned, else a neighbour of a learned skill.
/// How many starting skills (branches) one player may take.
const START_BRANCHES: usize = 3;

fn reachable(t: &Tree, learned: &[bool], i: usize) -> bool {
    if learned.get(i).copied().unwrap_or(true) {
        return false;
    }
    // Up to START_BRANCHES of the starting skills can be taken, each opening its own branch.
    if t.skills[i].start {
        let taken = t.skills.iter().zip(learned).filter(|&(s, &l)| s.start && l).count();
        if taken < START_BRANCHES {
            return true;
        }
    }
    t.from[i].iter().any(|&j| learned[j as usize])
}

// ---- server -----------------------------------------------------------------------------------------

struct Cfg {
    max_points: u32,
    first_cost: u32,
    last_cost: u32,
    /// By ore (`diamond`): the chance a gem drops, the lowest and the highest tier (0..5).
    gems: HashMap<String, (f64, u8, u8)>,
    /// The chance of a vacucite (it destroys the gems in an item) from emerald and diamond ores.
    vacucite: f64,
}

/// The ores that drop gems, cheapest first: chance, lowest tier, highest tier.
const GEM_ORES: [(&str, f64, u8, u8); 8] = [
    ("coal", 0.01, 0, 1),
    ("copper", 0.012, 0, 1),
    ("iron", 0.02, 0, 2),
    ("gold", 0.03, 1, 2),
    ("redstone", 0.03, 1, 3),
    ("lapis", 0.035, 1, 3),
    ("emerald", 0.08, 2, 4),
    ("diamond", 0.1, 2, 5),
];
const VACUCITE_CHANCE: f64 = 0.004;

fn gem_cfg_text() -> String {
    let mut s = String::from(
        "# Gems from ores (deepslate ores too; none with Silk Touch): gem_<ore> = chance lowest_tier highest_tier\n\
         # (tiers 0..5: crumbled ... exceptional). gem_vacucite: its chance from emerald and diamond ores.\n",
    );
    for (ore, chance, lo, hi) in GEM_ORES {
        s.push_str(&format!("gem_{ore} = {chance} {lo} {hi}\n"));
    }
    s.push_str(&format!("gem_vacucite = {VACUCITE_CHANCE}\n"));
    s
}

impl Cfg {
    /// The experience points the next point costs, having bought `bought`.
    fn cost(&self, bought: u32) -> u32 {
        if self.max_points <= 1 {
            return self.first_cost;
        }
        let t = bought.min(self.max_points - 1) as f64 / (self.max_points - 1) as f64;
        (self.first_cost as f64 + (self.last_cost as f64 - self.first_cost as f64) * t).round() as u32
    }
}

#[derive(Default, Clone)]
struct PlayerData {
    bought: u32,
    learned: Vec<u16>,
}

/// What a player's skills give right now, with their conditions already checked.
#[derive(Default, Clone)]
struct Snap {
    evasion: f64,
    blocking: f64,
    /// (kind, damage condition, value, operation)
    hits: Vec<(Kind, Dc, f64, u8)>,
    /// Health back on each melee hit.
    heal: f64,
    /// (chance, how many more) gems from an ore.
    gem_loot: Vec<(f64, f64)>,
}

enum Ask {
    Open(u128),
    State(u128),
    Learn(u128, u16),
    Buy(u128),
    Reset(u128),
    Grant(u128, u32),
    /// What the server sees of a player's gems (a check for tests).
    Gems(u128),
}

struct Server {
    cfg: Cfg,
    players: HashMap<u128, PlayerData>,
    /// The attribute modifiers each player has from us: (attribute, operation) -> amount.
    applied: HashMap<u128, HashMap<(String, u8), f64>>,
    xp_carry: HashMap<u128, f64>,
    ticks: u64,
    dirty: bool,
    /// Attribute commands that didn't run (shown by `/skilltree gems`).
    failed_commands: u64,
    last_failure: String,
}

static SERVER: Mutex<Option<Server>> = Mutex::new(None);
static ASKS: Mutex<Vec<Ask>> = Mutex::new(Vec::new());
/// Read by the damage hooks, which have no game: by player UUID and by entity id.
static SNAPS: Mutex<Option<(HashMap<u128, Snap>, HashMap<i32, u128>)>> = Mutex::new(None);

fn config_dir() -> std::path::PathBuf {
    horizon_client::logging::game_home().join("config")
}

const DEFAULT_CFG: &str = "# Kotost Skill Tree (port of Passive Skill Tree)\n\
# Most points a player can buy, and what the first and the last one cost in experience points.\n\
max_points = 85\nfirst_cost = 10\nlast_cost = 300\n";

fn load_server() -> Server {
    let path = config_dir().join("kotost_skilltree.txt");
    let mut text = std::fs::read_to_string(&path).unwrap_or_else(|_| {
        let _ = std::fs::create_dir_all(config_dir());
        let text = format!("{DEFAULT_CFG}{}", gem_cfg_text());
        let _ = std::fs::write(&path, &text);
        text
    });
    // A file from 0.1.0: the gem lines are added, so they can be changed there.
    if !text.contains("gem_") {
        text.push_str(&gem_cfg_text());
        let _ = std::fs::write(&path, &text);
    }
    let gems = GEM_ORES.iter().map(|&(ore, c, lo, hi)| (ore.to_owned(), (c, lo, hi))).collect();
    let mut cfg = Cfg { max_points: 85, first_cost: 10, last_cost: 300, gems, vacucite: VACUCITE_CHANCE };
    for line in text.lines() {
        let Some((k, v)) = line.split_once('=') else { continue };
        if let Some(ore) = k.trim().strip_prefix("gem_") {
            let w: Vec<f64> = v.split_whitespace().filter_map(|x| x.parse().ok()).collect();
            if ore == "vacucite" {
                cfg.vacucite = w.first().copied().unwrap_or(VACUCITE_CHANCE).clamp(0.0, 1.0);
            } else if let (Some(&c), Some(e)) = (w.first(), cfg.gems.get_mut(ore)) {
                let lo = w.get(1).map_or(e.1, |&t| t.clamp(0.0, 5.0) as u8);
                let hi = w.get(2).map_or(e.2, |&t| t.clamp(0.0, 5.0) as u8).max(lo);
                *e = (c.clamp(0.0, 1.0), lo, hi);
            }
            continue;
        }
        let Ok(v) = v.trim().parse::<u32>() else { continue };
        match k.trim() {
            "max_points" => cfg.max_points = v.clamp(1, 588),
            "first_cost" => cfg.first_cost = v,
            "last_cost" => cfg.last_cost = v,
            _ => {}
        }
    }
    let t = tree();
    let mut players = HashMap::new();
    let saved = std::fs::read_to_string(config_dir().join("kotost_skilltree_players.txt")).unwrap_or_default();
    for line in saved.lines() {
        let mut w = line.split_whitespace();
        let (Some(uuid), Some(bought)) = (w.next(), w.next()) else { continue };
        let (Ok(uuid), Ok(bought)) = (u128::from_str_radix(uuid, 16), bought.parse::<u32>()) else { continue };
        let mut learned: Vec<u16> =
            w.filter_map(|id| t.skills.iter().position(|s| s.id == id)).map(|i| i as u16).collect();
        learned.dedup();
        players.insert(uuid, PlayerData { bought: bought.max(learned.len() as u32), learned });
    }
    Server { cfg, players, applied: HashMap::new(), xp_carry: HashMap::new(), ticks: 0, dirty: false, failed_commands: 0, last_failure: String::new() }
}

fn save(s: &mut Server) {
    if !s.dirty {
        return;
    }
    let t = tree();
    let mut out = String::new();
    for (uuid, p) in &s.players {
        out.push_str(&format!("{uuid:032x} {}", p.bought));
        for &i in &p.learned {
            out.push(' ');
            out.push_str(&t.skills[i as usize].id);
        }
        out.push('\n');
    }
    let _ = std::fs::create_dir_all(config_dir());
    let path = config_dir().join("kotost_skilltree_players.txt");
    let tmp = config_dir().join("kotost_skilltree_players.tmp");
    if std::fs::write(&tmp, out).is_ok() && std::fs::rename(&tmp, &path).is_ok() {
        s.dirty = false;
    }
}

/// `Player.totalExperience` from the level and the progress (vanilla's level curve).
fn total_xp(level: i32, progress: f32) -> u32 {
    let l = level.max(0) as f64;
    let base = if level <= 16 {
        l * l + 6.0 * l
    } else if level <= 31 {
        2.5 * l * l - 40.5 * l + 360.0
    } else {
        4.5 * l * l - 162.5 * l + 2220.0
    };
    let next = if level <= 15 { 2.0 * l + 7.0 } else if level <= 30 { 5.0 * l - 38.0 } else { 9.0 * l - 158.0 };
    (base + (progress.clamp(0.0, 1.0) as f64 * next).round()).max(0.0) as u32
}

fn is_ru(game: &Game<'_>, uuid: u128) -> bool {
    game.native_player_client(uuid).is_some_and(|i| i.language.to_lowercase().starts_with("ru"))
}

fn note(game: &mut Game<'_>, uuid: u128, ru: &str, en: &str) {
    let line = if is_ru(game, uuid) { ru } else { en };
    let _ = game.native_net_send_to_player(uuid, CH_NOTE, line.as_bytes());
}

fn send_state(game: &mut Game<'_>, s: &Server, p: &NativeServerPlayer, open: bool) {
    let d = s.players.get(&p.uuid).cloned().unwrap_or_default();
    let mut out = vec![open as u8];
    let free = d.bought.saturating_sub(d.learned.len() as u32);
    out.extend_from_slice(&(free as u16).to_le_bytes());
    out.extend_from_slice(&(d.bought as u16).to_le_bytes());
    out.extend_from_slice(&(s.cfg.max_points as u16).to_le_bytes());
    out.extend_from_slice(&s.cfg.cost(d.bought).to_le_bytes());
    out.extend_from_slice(&total_xp(p.xp_level, p.xp_progress).to_le_bytes());
    out.extend_from_slice(&(d.learned.len() as u16).to_le_bytes());
    for i in &d.learned {
        out.extend_from_slice(&i.to_le_bytes());
    }
    let _ = game.native_net_send_to_player(p.uuid, CH_STATE, &out);
}

pub fn register_commands(commands: &mut Vec<NativeCommand>) {
    commands.push(NativeCommand::new("skilltree"));
    commands.push(NativeCommand::op("skilltree reset", 2));
    commands.push(NativeCommand::op("skilltree points <count:integer>", 2));
    commands.push(NativeCommand::new("skilltree gems"));
}

pub fn perform_command(_game: &mut Game<'_>, player: &NativeServerPlayer, command: &str, ran: &mut bool) {
    let mut words = command.split_whitespace();
    if words.next() != Some("skilltree") {
        return;
    }
    *ran = true;
    let ask = match (words.next(), words.next()) {
        (Some("reset"), _) if player.permission >= 2 => Ask::Reset(player.uuid),
        (Some("points"), Some(n)) if player.permission >= 2 => Ask::Grant(player.uuid, n.parse().unwrap_or(1)),
        (Some("gems"), _) => Ask::Gems(player.uuid),
        _ => Ask::Open(player.uuid),
    };
    lock(&ASKS).push(ask);
}

pub fn net_from_client(player: u128, channel: &str, data: &[u8]) {
    let ask = match channel {
        CH_ASK if data.first() == Some(&0) => Ask::State(player),
        CH_ASK => Ask::Open(player),
        CH_BUY => Ask::Buy(player),
        CH_LEARN if data.len() >= 2 => Ask::Learn(player, u16::from_le_bytes([data[0], data[1]])),
        _ => return,
    };
    let mut asks = lock(&ASKS);
    if asks.len() < 256 {
        asks.push(ask);
    }
}

/// A login or a respawn: their attribute modifiers are given again.
pub fn arrived(player: u128) {
    if let Some(s) = lock(&SERVER).as_mut() {
        s.applied.remove(&player);
    }
}

fn kind_of(item: &str) -> &str {
    item.rsplit(':').next().unwrap_or(item)
}

fn is_type(item: &str, ty: &str) -> bool {
    let i = kind_of(item);
    let melee = || i.ends_with("_sword") || i.ends_with("_axe") || i == "trident" || i == "mace";
    let ranged = || i == "bow" || i == "crossbow";
    let armor = || ["_helmet", "_chestplate", "_leggings", "_boots"].iter().any(|s| i.ends_with(s)) || i == "turtle_helmet";
    let tool = || ["_pickaxe", "_shovel", "_hoe", "_axe"].iter().any(|s| i.ends_with(s));
    match ty {
        "sword" => i.ends_with("_sword"),
        "axe" => i.ends_with("_axe") && !i.ends_with("_pickaxe"),
        "pickaxe" => i.ends_with("_pickaxe"),
        "shovel" => i.ends_with("_shovel"),
        "hoe" => i.ends_with("_hoe"),
        "bow" => i == "bow",
        "crossbow" => i == "crossbow",
        "trident" => i == "trident",
        "shield" => i == "shield",
        "helmet" => i.ends_with("_helmet"),
        "chestplate" => i.ends_with("_chestplate"),
        "leggings" => i.ends_with("_leggings"),
        "boots" => i.ends_with("_boots"),
        "melee_weapon" => melee(),
        "ranged_weapon" => ranged(),
        "weapon" => melee() || ranged(),
        "armor" => armor(),
        "tool" => tool(),
        _ => melee() || ranged() || armor() || tool() || i == "shield",
    }
}

fn holds(game: &Game<'_>, uuid: u128, slots: &[EquipmentSlot], ty: &str) -> bool {
    slots.iter().any(|&slot| game.native_player_equipment(uuid, slot).is_some_and(|s| s.count > 0 && is_type(&*s.item, ty)))
}

fn holds_cond(game: &Game<'_>, p: &NativeServerPlayer, c: &Cond) -> bool {
    use EquipmentSlot as S;
    match c {
        Cond::None => true,
        Cond::Hand(ty) => holds(game, p.uuid, &[S::MainHand, S::OffHand], ty),
        Cond::Equip(ty) => holds(game, p.uuid, &[S::MainHand, S::OffHand, S::Head, S::Chest, S::Legs, S::Feet], ty),
        // Against 20: the server's player record has no maximum, and reading it back would feed on our own modifier.
        Cond::Hp(min, max) => {
            let part = (p.health / 20.0) as f64;
            (*min < 0.0 || part >= *min) && (*max < 0.0 || part <= *max)
        }
        Cond::Gems(ty, min, max) => {
            let n = gems_in(game, p.uuid, ty) as f64;
            (*min < 0.0 || n >= *min) && (*max < 0.0 || n <= *max)
        }
    }
}

/// Gives (`Some`) or takes (`None`) one of our attribute modifiers, as the game puts on an item's:
/// a transient one (not saved with the player; given again after a login or a respawn), replaced in
/// place. Horizon's `/attribute` can't reach a player from the console of a dedicated server, so the
/// modifier is set on the player's body directly. Whether the player was reached.
fn attr_command(game: &mut Game<'_>, p: &NativeServerPlayer, attr: &str, op: u8, amount: Option<f64>, why: &mut String) -> bool {
    let id = format!("kotost_skilltree:{}_{op}", kind_of(attr));
    let operation = ["add_value", "add_multiplied_base", "add_multiplied_total"][op.min(2) as usize];
    let guest = game.lan.as_mut().and_then(|host| host.hub.guests.iter_mut().find(|g| g.uuid == p.uuid));
    let body = match guest {
        Some(g) => g.motion.as_deref_mut(),
        // Single player: the host's own body.
        None => game.player.as_mut(),
    };
    let Some(body) = body else {
        *why = format!("{}: no body to reach", p.name);
        return false;
    };
    match body.set_modifier(attr, &id, amount.unwrap_or(0.0), operation, amount.is_some()) {
        Ok(()) => true,
        Err(e) => {
            *why = format!("{attr} {id}: {e}");
            false
        }
    }
}

/// Every half second: each player's bonuses with their conditions, as attribute modifiers and the hooks' snapshot.
fn refresh(game: &mut Game<'_>, s: &mut Server, players: &[NativeServerPlayer]) {
    let t = tree();
    let mut snaps = HashMap::new();
    let mut by_entity = HashMap::new();
    let minute = s.ticks % 1200 == 0;
    for p in players {
        // Nothing learned may still mean gems; with neither, what an earlier session left goes.
        let d = s.players.get(&p.uuid);
        let mut want: HashMap<(String, u8), f64> = HashMap::new();
        let mut snap = Snap::default();
        let mut xp = 0.0;
        let mut add = |fx: &Fx, v: f64| match fx.kind {
            Kind::Attr => *want.entry((fx.attr.clone(), fx.op)).or_default() += v,
            Kind::Evasion => snap.evasion += v,
            Kind::Blocking => snap.blocking += v,
            Kind::XpPerMinute => xp += v,
            Kind::Heal => snap.heal += v,
            Kind::GemLoot => snap.gem_loot.push((v, fx.x)),
            Kind::GemPower | Kind::Sockets => {} // the anvil reads these
            k => snap.hits.push((k, fx.dc, v, fx.op)),
        };
        if let Some(d) = d {
            for &i in &d.learned {
                for fx in &t.skills[i as usize].fx {
                    if !holds_cond(game, p, &fx.cond) {
                        continue;
                    }
                    let v = match &fx.per_gem {
                        Some(ty) => fx.v * gems_in(game, p.uuid, ty) as f64,
                        None => fx.v,
                    };
                    add(fx, v);
                }
            }
        }
        // The gems in what they wear and hold, each in its own slot.
        for (slot, stack) in worn(game, p.uuid) {
            let Some(class) = gem_class(&stack.item) else { continue };
            if !slot_fits(slot, class) {
                continue;
            }
            for s in read_sockets(&stack).1 {
                if let Some(fx) = socket_line(&s, class).and_then(|l| l.fx.as_ref()) {
                    add(fx, fx.v * s.pow as f64);
                }
            }
        }
        if snap.blocking > 0.0 && !holds(game, p.uuid, &[EquipmentSlot::MainHand, EquipmentSlot::OffHand], "shield") {
            snap.blocking = 0.0;
        }
        let first = !s.applied.contains_key(&p.uuid);
        let have = s.applied.entry(p.uuid).or_default();
        // Only what the server took is kept as given: a command that didn't run is tried again.
        let mut given = HashMap::new();
        for (key, v) in &want {
            let old = have.get(key).copied();
            if old.is_some_and(|o| (o - v).abs() < 1e-9) {
                given.insert(key.clone(), *v);
                continue;
            }
            // A first sight adds only: a modifier their save kept stays as it is, with their health.
            if !first || old.is_some() {
                attr_command(game, p, &key.0, key.1, None, &mut s.last_failure);
            }
            if attr_command(game, p, &key.0, key.1, Some(*v), &mut s.last_failure) {
                given.insert(key.clone(), *v);
            } else {
                s.failed_commands += 1;
            }
        }
        let gone: Vec<_> = have.keys().filter(|k| !want.contains_key(*k)).cloned().collect();
        for key in gone {
            if !attr_command(game, p, &key.0, key.1, None, &mut s.last_failure) {
                given.insert(key.clone(), have[&key]);
            }
        }
        *have = given;
        if minute && xp > 0.0 {
            let carry = s.xp_carry.entry(p.uuid).or_default();
            *carry += xp;
            let whole = carry.floor();
            if whole >= 1.0 && game.native_give_experience(p.uuid, whole as u32) {
                *carry -= whole;
            }
        }
        if p.entity_id != 0 {
            by_entity.insert(p.entity_id, p.uuid);
        }
        snaps.insert(p.uuid, snap);
    }
    *lock(&SNAPS) = Some((snaps, by_entity));
}

fn answer(game: &mut Game<'_>, s: &mut Server, players: &[NativeServerPlayer], ask: Ask) {
    let t = tree();
    let uuid = match &ask {
        Ask::Open(u) | Ask::State(u) | Ask::Buy(u) | Ask::Reset(u) | Ask::Learn(u, _) | Ask::Grant(u, _) | Ask::Gems(u) => *u,
    };
    let Some(p) = players.iter().find(|p| p.uuid == uuid) else { return };
    let mut open = false;
    match ask {
        Ask::Open(_) => open = true,
        Ask::State(_) => {}
        Ask::Learn(_, i) => {
            let d = s.players.entry(uuid).or_default();
            let mut learned = vec![false; t.skills.len()];
            for &l in &d.learned {
                learned[l as usize] = true;
            }
            if (i as usize) >= t.skills.len() || d.learned.len() as u32 >= d.bought {
                note(game, uuid, "Нет свободных очков умений", "No skill points left");
            } else if !reachable(t, &learned, i as usize) {
                note(game, uuid, "Умение не связано с изученными", "This skill is not connected to a learned one");
            } else {
                d.learned.push(i);
                s.dirty = true;
            }
        }
        Ask::Buy(_) => {
            let d = s.players.entry(uuid).or_default();
            let cost = s.cfg.cost(d.bought);
            if d.bought >= s.cfg.max_points {
                note(game, uuid, "Куплены все очки умений", "Every skill point is bought");
            } else if p.game_mode != 1 && total_xp(p.xp_level, p.xp_progress) < cost {
                note(game, uuid, "Не хватает опыта", "Not enough experience");
            } else if p.game_mode == 1 || game.native_run_command(&format!("experience add {} -{cost} points", p.name)) {
                d.bought += 1;
                s.dirty = true;
            } else {
                note(game, uuid, "Сервер не смог списать опыт", "The server could not take the experience");
            }
        }
        Ask::Reset(_) => {
            if let Some(d) = s.players.get_mut(&uuid) {
                d.learned.clear();
                s.dirty = true;
            }
            let line = if is_ru(game, uuid) { "Древо умений сброшено, очки возвращены." } else { "Your skill tree has been reset." };
            let _ = game.native_tell(uuid, Text::literal(line).with_color(0xFFFF55));
        }
        Ask::Gems(_) => {
            let mut lines = vec![format!("failed attribute commands: {} {}", s.failed_commands, s.last_failure)];
            for (slot, stack) in worn(game, uuid) {
                let (cap, list) = read_sockets(&stack);
                let ids: Vec<String> = list.iter().map(|g| g.id.clone()).collect();
                lines.push(format!("{slot:?}: {} {:?} sockets {cap:?} [{}]", stack.item, gem_class(&stack.item), ids.join(", ")));
            }
            for ((attr, op), v) in s.applied.get(&uuid).into_iter().flatten() {
                lines.push(format!("given: {attr} op {op} = {v:.4}"));
            }
            for line in lines {
                let _ = game.native_tell(uuid, Text::literal(line).with_color(0xAAAAAA));
            }
            return;
        }
        Ask::Grant(_, n) => {
            let d = s.players.entry(uuid).or_default();
            d.bought = (d.bought + n).min(s.cfg.max_points);
            s.dirty = true;
        }
    }
    // The experience the command took shows in the next list of players; the client asks again after a buy.
    send_state(game, s, p, open);
}

fn server_tick(game: &mut Game<'_>) {
    let asks: Vec<Ask> = std::mem::take(&mut *lock(&ASKS));
    let mut guard = lock(&SERVER);
    let s = guard.get_or_insert_with(load_server);
    s.ticks += 1;
    let due = s.ticks % 10 == 0;
    if asks.is_empty() && !due {
        return;
    }
    let mut players = game.native_server_players();
    let asked = !asks.is_empty();
    for ask in asks {
        answer(game, s, &players, ask);
    }
    if asked {
        players = game.native_server_players();
    }
    if due || asked {
        refresh(game, s, &players);
    }
    if s.ticks % 200 == 0 || asked {
        save(s);
    }
}

fn rand01() -> f64 {
    static SEED: Mutex<u64> = Mutex::new(0);
    let mut s = lock(&SEED);
    if *s == 0 {
        *s = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0x9E3779B97F4A7C15)
            | 1;
    }
    *s ^= *s << 13;
    *s ^= *s >> 7;
    *s ^= *s << 17;
    (*s >> 11) as f64 / (1u64 << 53) as f64
}

fn boost(snap: &Snap, dc: Dc, amount: &mut f32) {
    let fits = |c: Dc| c == Dc::Any || c == dc;
    let (mut add, mut mul, mut chance, mut crit) = (0.0, 0.0, 0.0, 0.0);
    for &(kind, c, v, op) in &snap.hits {
        if !fits(c) {
            continue;
        }
        match kind {
            Kind::Dmg if op == 0 => add += v,
            Kind::Dmg => mul += v,
            Kind::Crit => chance += v,
            Kind::CritDmg => crit += v,
            _ => {}
        }
    }
    let mut out = (*amount as f64 + add) * (1.0 + mul);
    if chance > 0.0 && rand01() < chance {
        out *= 1.5 + crit;
    }
    if out.is_finite() {
        *amount = out.max(0.0) as f32;
    }
}

/// `native_living_incoming_damage_v2` (server): the acting player's melee hit on a mob.
pub fn melee_hit(game: &mut Game<'_>, amount: &mut f32) {
    let Some(p) = game.native_acting_player() else { return };
    let mut heal = 0.0;
    {
        let guard = lock(&SNAPS);
        if let Some(snap) = guard.as_ref().and_then(|(snaps, _)| snaps.get(&p.uuid)) {
            boost(snap, Dc::Melee, amount);
            heal = snap.heal;
        }
    }
    // The acting player's state is in place: their health is the one the hit's player sees.
    if heal > 0.0 && *amount > 0.0 {
        if let Some(me) = game.player.as_mut() {
            let h = me.health.health();
            if h > 0.0 {
                let _ = me.set_health(h + heal as f32, false);
            }
        }
    }
}

/// `native_living_incoming_damage` (server): a player's projectile or magic, and a hit on a player.
pub fn incoming(entity: i32, damage_type: &str, causing: i32, direct: i32, amount: &mut f32, canceled: &mut bool) {
    let guard = lock(&SNAPS);
    let Some((snaps, by_entity)) = guard.as_ref() else { return };
    let of = |id: i32| by_entity.get(&id).and_then(|u| snaps.get(u));
    if let Some(victim) = of(entity) {
        let attack = (causing != 0 || direct != 0) && causing != entity;
        if attack && (rand01() * 100.0 < victim.evasion || rand01() * 100.0 < victim.blocking) {
            *canceled = true;
            return;
        }
    }
    if causing != 0 && causing != entity {
        if let Some(attacker) = of(causing) {
            let ty = kind_of(damage_type);
            let dc = if ty.contains("magic") {
                Dc::Magic
            } else if ["arrow", "trident", "thrown", "fireball", "projectile", "firework"].iter().any(|k| ty.contains(k)) {
                Dc::Projectile
            } else {
                return; // melee goes through `melee_hit`
            };
            boost(attacker, dc, amount);
        }
    }
}

// ---- client -----------------------------------------------------------------------------------------

static CLIENT_RU: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn tr(r: &'static str, e: &'static str) -> &'static str {
    if CLIENT_RU.load(std::sync::atomic::Ordering::Relaxed) { r } else { e }
}

struct Client {
    learned: Vec<bool>,
    free: u32,
    bought: u32,
    max: u32,
    cost: u32,
    xp: u32,
    cam: [f32; 2],
    zoom: f32,
    /// A press: where the mouse and the camera were, and whether it moved since.
    drag: Option<([f32; 2], [f32; 2], bool)>,
    centered: bool,
    note: Option<(String, Instant)>,
    out: Vec<(&'static str, Vec<u8>)>,
    close: bool,
    refresh_at: Option<Instant>,
    born: Instant,
    /// When the icons were last put in the shared mod image (another mod, a minimap,
    /// may draw over it while the tree is open: they are put back every second).
    atlas_at: Instant,
}

static CLIENT: Mutex<Option<Client>> = Mutex::new(None);
static ATLAS: OnceLock<Option<Vec<u8>>> = OnceLock::new();

fn atlas() -> Option<&'static Vec<u8>> {
    ATLAS
        .get_or_init(|| {
            let bytes: &[u8] = crate::native_data!("kotost_skilltree", "atlas.png");
            let mut reader = png::Decoder::new(std::io::Cursor::new(bytes)).read_info().ok()?;
            let mut buf = vec![0u8; 1024 * 1024 * 4];
            let info = reader.next_frame(&mut buf).ok()?;
            (info.width == 1024 && info.height == 1024 && info.color_type == png::ColorType::Rgba && info.bit_depth == png::BitDepth::Eight)
                .then_some(buf)
        })
        .as_ref()
}

fn mine(game: &Game<'_>, screen: &NativeScreenView<'_>) -> bool {
    screen.id == "mod" && game.native_mod_screen() == Some(MOD)
}

impl Client {
    fn new() -> Self {
        Client {
            learned: vec![false; tree().skills.len()],
            free: 0,
            bought: 0,
            max: 0,
            cost: 0,
            xp: 0,
            cam: [0.0, 0.0],
            zoom: 1.5,
            drag: None,
            centered: false,
            note: None,
            out: Vec::new(),
            close: false,
            refresh_at: None,
            born: Instant::now(),
            atlas_at: Instant::now(),
        }
    }
    fn at(&self, w: f32, h: f32, p: [f32; 2]) -> [f32; 2] {
        [w / 2.0 + (p[0] - self.cam[0]) * self.zoom, h / 2.0 + (p[1] - self.cam[1]) * self.zoom]
    }
    fn hovered(&self, w: f32, h: f32, m: [f32; 2]) -> Option<usize> {
        if m[1] >= h - BAR {
            return None;
        }
        tree().skills.iter().position(|s| {
            let c = self.at(w, h, [s.x, s.y]);
            let r = s.size * self.zoom / 2.0;
            (m[0] - c[0]).abs() <= r && (m[1] - c[1]).abs() <= r
        })
    }
}

const BAR: f32 = 24.0;
const SPRITE: &str = horizon_client::hud::MOD_IMAGE_SPRITE;

fn buy_rect(w: f32, h: f32) -> [f32; 4] {
    [w - 206.0, h - BAR + 3.0, 200.0, BAR - 6.0]
}

fn inside(p: [f32; 2], r: [f32; 4]) -> bool {
    p[0] >= r[0] && p[0] < r[0] + r[2] && p[1] >= r[1] && p[1] < r[1] + r[3]
}

fn line(game: &Game<'_>, b: &mut HudBatch, p0: [f32; 2], p1: [f32; 2], width: f32, argb: u32) {
    let (dx, dy) = (p1[0] - p0[0], p1[1] - p0[1]);
    let len = (dx * dx + dy * dy).sqrt();
    if len < 0.01 {
        return;
    }
    let (nx, ny) = (-dy / len * width / 2.0, dx / len * width / 2.0);
    game.native_quad(
        b,
        [[p0[0] + nx, p0[1] + ny], [p1[0] + nx, p1[1] + ny], [p1[0] - nx, p1[1] - ny], [p0[0] - nx, p0[1] - ny]],
        argb,
    );
}

fn wrap(game: &Game<'_>, s: &str, w: f32) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for word in s.split(' ') {
        let next = if cur.is_empty() { word.to_owned() } else { format!("{cur} {word}") };
        if !cur.is_empty() && game.native_text_width(&next) > w {
            out.push(std::mem::replace(&mut cur, word.to_owned()));
        } else {
            cur = next;
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

fn tooltip(game: &Game<'_>, b: &mut HudBatch, c: &Client, i: usize, m: [f32; 2]) {
    let t = tree();
    let s = &t.skills[i];
    let lang = if CLIENT_RU.load(std::sync::atomic::Ordering::Relaxed) { 0 } else { 1 };
    let title = [0xFFFFFF, 0xFFD86A, 0x8AD8FF, 0xC08AFF, 0xFFA05A][s.frame.min(4)];
    let mut rows: Vec<(String, u32)> = vec![(s.name[lang].clone(), title)];
    let mut grey = false;
    for (text, flag) in &s.lines[lang] {
        let colour = match flag {
            1 => 0x9CB4FF,
            2 => 0xB0B0B0,
            _ => {
                grey = true;
                0x7A7A7A
            }
        };
        rows.extend(wrap(game, text, 210.0).into_iter().map(|l| (l, colour)));
    }
    if grey {
        rows.push((tr("Серые бонусы пока не работают в Horizon", "Grey bonuses don't work in Horizon yet").to_owned(), 0x5A5A5A));
    }
    if c.learned[i] {
        rows.push((tr("Изучено", "Learned").to_owned(), 0x55FF55));
    } else if !reachable(t, &c.learned, i) {
        let why = if c.learned.iter().any(|&l| l) {
            tr("Нет связи с изученными умениями", "Not connected to a learned skill")
        } else {
            tr("Начните с умения класса", "Start with a class skill")
        };
        rows.push((why.to_owned(), 0xFF5555));
    } else if c.free == 0 {
        rows.push((tr("Нет очков умений: купите очко внизу", "No skill points: buy one below").to_owned(), 0xFFAA00));
    } else {
        rows.push((tr("ЛКМ — изучить", "Click to learn").to_owned(), 0x55FF55));
    }
    let width = rows.iter().map(|(l, _)| game.native_text_width(l)).fold(0.0f32, f32::max) + 8.0;
    let height = rows.len() as f32 * 10.0 + 7.0;
    let x = if m[0] + 12.0 + width > b.width { (m[0] - 12.0 - width).max(0.0) } else { m[0] + 12.0 };
    let y = (m[1] - 4.0).min(b.height - height - 2.0).max(0.0);
    game.native_fill(b, [x - 1.0, y - 1.0, width + 2.0, height + 2.0], 0xFF3A2A5A);
    game.native_fill(b, [x, y, width, height], 0xF0100818);
    for (n, (l, colour)) in rows.iter().enumerate() {
        game.native_text(b, l, [x + 4.0, y + 4.0 + n as f32 * 10.0], *colour, true);
    }
}

pub fn render(game: &Game<'_>, b: &mut HudBatch, screen: &NativeScreenView<'_>) {
    if !mine(game, screen) {
        return;
    }
    let mut guard = lock(&CLIENT);
    let Some(c) = guard.as_mut() else { return };
    let t = tree();
    let (w, h) = (b.width, b.height);
    let m = screen.mouse;
    if !c.centered {
        c.centered = true;
        if let Some(i) = c.learned.iter().position(|&l| l) {
            c.cam = [t.skills[i].x, t.skills[i].y];
        }
    }
    if let Some((from, cam, moved)) = &mut c.drag {
        let d = [m[0] - from[0], m[1] - from[1]];
        if d[0].abs() + d[1].abs() > 3.0 {
            *moved = true;
        }
        if *moved {
            c.cam = [cam[0] - d[0] / c.zoom, cam[1] - d[1] / c.zoom];
        }
    }
    game.native_fill(b, [0.0, 0.0, w, h], 0xFF0A0612);
    let side = w.max(h);
    let _ = game.native_sprite(b, SPRITE, t.bg, [(w - side) / 2.0, (h - side) / 2.0, side, side]);
    let hover = c.hovered(w, h, m);
    let on = |p: [f32; 2], r: f32| p[0] > -r && p[1] > -r && p[0] < w + r && p[1] < h + r;
    for (i, s) in t.skills.iter().enumerate() {
        let p0 = c.at(w, h, [s.x, s.y]);
        for (links, long, one_way) in [(&s.direct, false, false), (&s.long, true, false), (&s.one_way, false, true)] {
            for &j in links {
                let o = &t.skills[j as usize];
                let p1 = c.at(w, h, [o.x, o.y]);
                if long && hover != Some(i) && hover != Some(j as usize) {
                    continue;
                }
                if !on(p0, 0.0) && !on(p1, 0.0) && !long {
                    continue;
                }
                let both = c.learned[i] && c.learned[j as usize];
                let one = c.learned[i] || c.learned[j as usize];
                let colour = match (long, both, one) {
                    (true, ..) => 0xA08A6AFF,
                    (_, true, _) => 0xFFE8B84A,
                    (_, _, true) => 0xFF8A7A5A,
                    _ if one_way => 0xFF5A4A72,
                    _ => 0xFF3E3A48,
                };
                line(game, b, p0, p1, (if both { 3.0 } else { 2.0 }) * c.zoom.clamp(0.6, 1.6), colour);
            }
        }
    }
    let blink = (c.born.elapsed().as_millis() / 500) % 2 == 0;
    for (i, s) in t.skills.iter().enumerate() {
        let p = c.at(w, h, [s.x, s.y]);
        let size = s.size * c.zoom;
        if !on(p, size) {
            continue;
        }
        let Some(&[fx, fy, fw, fh]) = t.frames.get(s.frame) else { continue };
        let dst = [p[0] - size / 2.0, p[1] - size / 2.0, size, size];
        let _ = game.native_sprite(b, SPRITE, [fx, fy, fw, fh], dst);
        let icon = size / 2.0;
        let src = [(s.icon % 64) as f32 * 16.0, (s.icon / 64) as f32 * 16.0, 16.0, 16.0];
        let _ = game.native_sprite(b, SPRITE, src, [p[0] - icon / 2.0, p[1] - icon / 2.0, icon, icon]);
        if c.learned[i] || (blink && c.free > 0 && reachable(t, &c.learned, i)) {
            let _ = game.native_sprite(b, SPRITE, [fx + fw * 2.0, fy, fw, fh], dst);
        }
    }
    // The bar: the points, the buy button.
    game.native_fill(b, [0.0, h - BAR, w, BAR], 0xE0100818);
    game.native_fill(b, [0.0, h - BAR, w, 1.0], 0xFF3A2A5A);
    let points = format!("{}: {}   ({}/{})", tr("Очков осталось", "Points left"), c.free, c.bought, c.max);
    game.native_text(b, &points, [8.0, h - BAR + 8.0], 0xFFE8B8, true);
    let r = buy_rect(w, h);
    let can = c.bought < c.max && c.xp >= c.cost;
    let hot = inside(m, r) && can;
    game.native_fill(b, r, if hot { 0xFF5A4A8A } else if can { 0xFF3A2A5A } else { 0xFF24202C });
    let label = if c.bought >= c.max {
        tr("Все очки куплены", "Every point is bought").to_owned()
    } else {
        format!("{}: {} / {} {}", tr("Купить очко", "Buy a point"), c.xp, c.cost, tr("опыта", "XP"))
    };
    let lw = game.native_text_width(&label);
    game.native_text(b, &label, [r[0] + (r[2] - lw) / 2.0, r[1] + 5.0], if can { 0xFFFFFF } else { 0x808080 }, true);
    let hint = tr("Колесо — масштаб, зажать ЛКМ — двигать, O или Esc — закрыть", "Wheel zooms, drag moves, O or Esc closes");
    game.native_text(b, hint, [6.0, 6.0], 0x8A8A9A, true);
    if let Some((text, at)) = &c.note {
        if at.elapsed().as_secs_f32() < 3.0 {
            let tw = game.native_text_width(text);
            game.native_fill(b, [(w - tw) / 2.0 - 6.0, 18.0, tw + 12.0, 16.0], 0xE0401010);
            game.native_text(b, text, [(w - tw) / 2.0, 22.0], 0xFFB0B0, true);
        }
    }
    if let (Some(i), true) = (hover, c.drag.as_ref().is_none_or(|d| !d.2)) {
        tooltip(game, b, c, i, m);
    }
}

pub fn press(game: &Game<'_>, screen: &NativeScreenView<'_>, button: i32, canceled: &mut bool) {
    if !mine(game, screen) {
        return;
    }
    *canceled = true;
    let mut guard = lock(&CLIENT);
    let (Some(c), Some(r)) = (guard.as_mut(), game.renderer.as_ref()) else { return };
    if button != 0 {
        return;
    }
    let b = HudBatch::new([r.config.width, r.config.height]);
    if inside(screen.mouse, buy_rect(b.width, b.height)) {
        if c.bought < c.max && c.xp >= c.cost {
            c.out.push((CH_BUY, Vec::new()));
            c.refresh_at = Some(Instant::now() + std::time::Duration::from_millis(400));
        }
        return;
    }
    c.drag = Some((screen.mouse, c.cam, false));
}

pub fn release(game: &Game<'_>, screen: &NativeScreenView<'_>, canceled: &mut bool) {
    if !mine(game, screen) {
        return;
    }
    *canceled = true;
    let mut guard = lock(&CLIENT);
    let (Some(c), Some(r)) = (guard.as_mut(), game.renderer.as_ref()) else { return };
    let Some((_, _, moved)) = c.drag.take() else { return };
    if moved {
        return;
    }
    let b = HudBatch::new([r.config.width, r.config.height]);
    if let Some(i) = c.hovered(b.width, b.height, screen.mouse) {
        if !c.learned[i] && c.free > 0 && reachable(tree(), &c.learned, i) {
            c.out.push((CH_LEARN, (i as u16).to_le_bytes().to_vec()));
        }
    }
}

pub fn scroll(game: &Game<'_>, screen: &NativeScreenView<'_>, delta: [f64; 2], canceled: &mut bool) {
    if !mine(game, screen) {
        return;
    }
    *canceled = true;
    let mut guard = lock(&CLIENT);
    let (Some(c), Some(r)) = (guard.as_mut(), game.renderer.as_ref()) else { return };
    let b = HudBatch::new([r.config.width, r.config.height]);
    let m = screen.mouse;
    // The point under the mouse stays under it.
    let under = [c.cam[0] + (m[0] - b.width / 2.0) / c.zoom, c.cam[1] + (m[1] - b.height / 2.0) / c.zoom];
    c.zoom = (c.zoom * 1.15f32.powf(delta[1].clamp(-4.0, 4.0) as f32)).clamp(0.3, 5.0);
    c.cam = [under[0] - (m[0] - b.width / 2.0) / c.zoom, under[1] - (m[1] - b.height / 2.0) / c.zoom];
}

/// A key over our screen: O closes it as Escape does.
pub fn screen_key(game: &Game<'_>, screen: &NativeScreenView<'_>, key: &str, canceled: &mut bool) {
    if !mine(game, screen) || key != "key.keyboard.o" {
        return;
    }
    *canceled = true;
    if let Some(c) = lock(&CLIENT).as_mut() {
        c.close = true;
    }
}

/// A key in the world: O asks the server for the tree.
pub fn key(game: &mut Game<'_>, key: &str, pressed: bool) {
    if !pressed || key != "key.keyboard.o" || !game.native_window() || !game.native_playing() {
        return;
    }
    let _ = game.native_net_send_to_server(CH_ASK, &[]);
}

pub fn net_from_server(game: &mut Game<'_>, channel: &str, data: &[u8]) {
    match channel {
        CH_NOTE => {
            let mut guard = lock(&CLIENT);
            let c = guard.get_or_insert_with(Client::new);
            c.note = Some((String::from_utf8_lossy(data).into_owned(), Instant::now()));
        }
        CH_STATE if data.len() >= 17 => {
            let u16at = |i: usize| u16::from_le_bytes([data[i], data[i + 1]]) as u32;
            let u32at = |i: usize| u32::from_le_bytes([data[i], data[i + 1], data[i + 2], data[i + 3]]);
            let open = data[0] == 1;
            {
                let mut guard = lock(&CLIENT);
                let c = guard.get_or_insert_with(Client::new);
                (c.free, c.bought, c.max, c.cost, c.xp) = (u16at(1), u16at(3), u16at(5), u32at(7), u32at(11));
                c.learned.iter_mut().for_each(|l| *l = false);
                for n in 0..u16at(15) as usize {
                    let at = 17 + n * 2;
                    if at + 1 < data.len() {
                        if let Some(l) = c.learned.get_mut(u16at(at) as usize) {
                            *l = true;
                        }
                    }
                }
                if open {
                    c.drag = None;
                    c.close = false;
                }
            }
            if open && game.native_mod_screen() != Some(MOD) {
                if let Some(rgba) = atlas() {
                    game.native_mod_image(1024, 1024, rgba);
                }
                game.native_mod_screen_open(MOD);
            }
        }
        _ => {}
    }
}

fn client_tick(game: &mut Game<'_>) {
    let ru = game.settings.language.to_lowercase().starts_with("ru");
    CLIENT_RU.store(ru, std::sync::atomic::Ordering::Relaxed);
    let (out, close) = {
        let mut guard = lock(&CLIENT);
        if guard.is_none() && game.native_playing() {
            // The learned skills, asked once: the anvil's window works out gem power and sockets with them.
            let mut c = Client::new();
            c.out.push((CH_ASK, vec![0]));
            *guard = Some(c);
        }
        let Some(c) = guard.as_mut() else { return };
        if c.refresh_at.is_some_and(|t| Instant::now() >= t) {
            c.refresh_at = None;
            c.out.push((CH_ASK, vec![0])); // the state only: the experience left after the buy
        }
        (std::mem::take(&mut c.out), std::mem::take(&mut c.close))
    };
    for (channel, data) in out {
        let _ = game.native_net_send_to_server(channel, &data);
    }
    if close {
        game.native_mod_screen_close(MOD);
    } else if game.native_mod_screen() == Some(MOD) {
        let due = lock(&CLIENT).as_mut().is_some_and(|c| {
            let due = c.atlas_at.elapsed() >= std::time::Duration::from_secs(1);
            if due {
                c.atlas_at = Instant::now();
            }
            due
        });
        if let (true, Some(rgba)) = (due, atlas()) {
            game.native_mod_image(1024, 1024, rgba);
        }
    }
}

pub fn tick(game: &mut Game<'_>) {
    if game.native_window() {
        client_tick(game);
    }
}

/// `native_server_tick`: the server's work at the end of each tick, where no player's state is
/// swapped in (commands, such as the attribute modifiers, don't run while one is).
pub fn server_tick_hook(game: &mut Game<'_>, phase: ServerTickPhase) {
    if matches!(phase, ServerTickPhase::ServerTickEnd { .. }) && game.lan_acting.is_none() {
        server_tick(game);
    }
}

// ---- gems (0.2.0) -----------------------------------------------------------------------------------
// The gem items are the content mod `kotost_gems` (its own id: there they are data, here the logic).
// An item's sockets live in its `minecraft:custom_data` under "kotost_gems":
// {sockets: Int, gems: [{id: "citrine_2", opt: Int (iriscite's rolled bonus, -1), pow: Float}]}.

const GEM_NS: &str = "kotost_gems";
const GEM_TYPES: [&str; 5] = ["citrine", "iriscite", "jade", "ruby", "sapphire"];
const CLASS_ORDER: [&str; 7] = ["weapon", "tool", "helmet", "chestplate", "leggings", "boots", "shield"];
const MAX_SOCKETS: u32 = 3;

struct GemLine {
    text: [String; 2],
    fx: Option<Fx>,
}

struct Gem {
    id: String,
    tier: u8,
    color: u32,
    name: [String; 2],
    by_class: HashMap<String, GemLine>,
    /// Iriscite: one of these, rolled when it goes in.
    random: Vec<GemLine>,
    /// Vacucite: it empties the item's sockets.
    removal: bool,
}

struct Gems {
    list: Vec<Gem>,
    classes: HashMap<String, [String; 2]>,
    ui: HashMap<String, [String; 2]>,
}

fn gems() -> &'static Gems {
    static GEMS: OnceLock<Gems> = OnceLock::new();
    GEMS.get_or_init(|| {
        use serde_json::Value;
        let doc: Value =
            serde_json::from_slice(crate::native_data!("kotost_skilltree", "gems.json")).unwrap_or(Value::Null);
        let s = |v: &Value| v.as_str().unwrap_or("").to_owned();
        let pair = |v: &Value| [s(&v[0]), s(&v[1])];
        let line = |v: &Value| GemLine { text: [s(&v["ru"]), s(&v["en"])], fx: parse_fx(&v["fx"]) };
        let pairs = |v: &Value| -> HashMap<String, [String; 2]> {
            v.as_object().map(|o| o.iter().map(|(k, v)| (k.clone(), pair(v))).collect()).unwrap_or_default()
        };
        let list = doc["gems"]
            .as_array()
            .map(|a| {
                a.iter()
                    .map(|g| Gem {
                        id: s(&g["id"]),
                        tier: g["t"].as_u64().unwrap_or(0) as u8,
                        color: g["col"].as_u64().unwrap_or(0xFFFFFF) as u32,
                        name: [s(&g["ru"]), s(&g["en"])],
                        by_class: g["cls"]
                            .as_object()
                            .map(|o| o.iter().map(|(k, v)| (k.clone(), line(v))).collect())
                            .unwrap_or_default(),
                        random: g["rnd"].as_array().map(|a| a.iter().map(&line).collect()).unwrap_or_default(),
                        removal: g["rm"].as_u64() == Some(1),
                    })
                    .collect()
            })
            .unwrap_or_default();
        Gems { list, classes: pairs(&doc["classes"]), ui: pairs(&doc["ui"]) }
    })
}

fn gem(id: &str) -> Option<&'static Gem> {
    gems().list.iter().find(|g| g.id == id)
}

/// The gem an item id (`kotost_gems:ruby_3`) is.
fn gem_of(item: &str) -> Option<&'static Gem> {
    gem(item.strip_prefix(GEM_NS)?.strip_prefix(':')?)
}

/// Which of a gem's bonuses an item with sockets takes; None for an item without sockets.
fn gem_class(item: &str) -> Option<&'static str> {
    let i = kind_of(item);
    Some(if is_type(item, "helmet") {
        "helmet"
    } else if is_type(item, "chestplate") {
        "chestplate"
    } else if is_type(item, "leggings") {
        "leggings"
    } else if is_type(item, "boots") {
        "boots"
    } else if i == "shield" {
        "shield"
    } else if is_type(item, "pickaxe") || is_type(item, "shovel") {
        "tool"
    } else if is_type(item, "sword") || is_type(item, "axe") || matches!(i, "bow" | "crossbow" | "trident" | "mace") {
        "weapon"
    } else {
        return None;
    })
}

/// An item's own sockets, by its material: 1 to 3 (the tree's "more sockets" adds, up to 3).
fn base_sockets(item: &str) -> u32 {
    let i = kind_of(item);
    if i.starts_with("diamond_") || i.starts_with("netherite_") || matches!(i, "trident" | "mace") {
        3
    } else if i.starts_with("iron_") || i.starts_with("copper_") || matches!(i, "bow" | "crossbow" | "shield") {
        2
    } else {
        1
    }
}

/// Whether an item of `class` gives its gems from `slot`: weapons and tools in the main hand, armour worn.
fn slot_fits(slot: EquipmentSlot, class: &str) -> bool {
    use EquipmentSlot as S;
    match slot {
        S::MainHand => matches!(class, "weapon" | "tool" | "shield"),
        S::OffHand => class == "shield",
        S::Head => class == "helmet",
        S::Chest => class == "chestplate",
        S::Legs => class == "leggings",
        S::Feet => class == "boots",
        _ => false,
    }
}

fn worn(game: &Game<'_>, uuid: u128) -> Vec<(EquipmentSlot, ItemStack)> {
    use EquipmentSlot as S;
    [S::MainHand, S::OffHand, S::Head, S::Chest, S::Legs, S::Feet]
        .into_iter()
        .filter_map(|slot| game.native_player_equipment(uuid, slot).filter(|s| s.count > 0).map(|s| (slot, s)))
        .collect()
}

/// The gems in a player's worn and held items of type `ty` (the tree's "if you have gems in ...").
fn gems_in(game: &Game<'_>, uuid: u128, ty: &str) -> usize {
    worn(game, uuid)
        .iter()
        .filter(|(slot, s)| is_type(&s.item, ty) && gem_class(&s.item).is_some_and(|c| slot_fits(*slot, c)))
        .map(|(_, s)| read_sockets(s).1.len())
        .sum()
}

#[derive(Clone)]
struct Socketed {
    id: String,
    opt: i32,
    pow: f32,
}

use horizon_world::nbt::{Compound, JavaString, Tag};

fn tag_num(t: &Tag) -> Option<f64> {
    Some(match t {
        Tag::Byte(v) => *v as f64,
        Tag::Short(v) => *v as f64,
        Tag::Int(v) => *v as f64,
        Tag::Long(v) => *v as f64,
        Tag::Float(b) => f32::from_bits(*b) as f64,
        Tag::Double(b) => f64::from_bits(*b),
        _ => return None,
    })
}

/// The sockets an item was given (None: none yet, its material's count then) and the gems in them.
fn read_sockets(stack: &ItemStack) -> (Option<u32>, Vec<Socketed>) {
    let key = |k: &str| JavaString::from(k);
    let Some(Tag::Compound(c)) = stack.custom_data().and_then(|d| d.get(&key(GEM_NS))) else {
        return (None, Vec::new());
    };
    let cap = c.get(&key("sockets")).and_then(tag_num).map(|n| n.clamp(0.0, 9.0) as u32);
    let mut list = Vec::new();
    if let Some(Tag::List(items)) = c.get(&key("gems")) {
        for t in items {
            let Tag::Compound(g) = t else { continue };
            let Some(Tag::String(id)) = g.get(&key("id")) else { continue };
            list.push(Socketed {
                id: id.to_string_lossy(),
                opt: g.get(&key("opt")).and_then(tag_num).unwrap_or(-1.0) as i32,
                pow: g.get(&key("pow")).and_then(tag_num).unwrap_or(1.0) as f32,
            });
        }
    }
    (cap, list)
}

fn write_sockets(stack: &mut ItemStack, cap: u32, list: &[Socketed]) {
    let key = |k: &str| JavaString::from(k);
    let mut data: Compound = stack.custom_data().cloned().unwrap_or_default();
    let gems = list
        .iter()
        .map(|s| {
            Tag::Compound(Compound::from([
                (key("id"), Tag::String(key(&s.id))),
                (key("opt"), Tag::Int(s.opt)),
                (key("pow"), Tag::Float(s.pow.to_bits())),
            ]))
        })
        .collect();
    data.insert(
        key(GEM_NS),
        Tag::Compound(Compound::from([(key("sockets"), Tag::Int(cap as i32)), (key("gems"), Tag::List(gems))])),
    );
    stack.data_mut().set_other("minecraft:custom_data", Some(Tag::Compound(data)));
}

/// The bonus a socketed gem gives an item of `class`.
fn socket_line(s: &Socketed, class: &str) -> Option<&'static GemLine> {
    let g = gem(&s.id)?;
    if g.random.is_empty() {
        g.by_class.get(class)
    } else {
        g.random.get(s.opt.max(0) as usize)
    }
}

/// A player's learned skills: the server's record where this is the server (or single player), else the client's.
fn learned_of(uuid: u128) -> Vec<u16> {
    if let Some(d) = lock(&SERVER).as_ref().and_then(|s| s.players.get(&uuid)) {
        return d.learned.clone();
    }
    lock(&CLIENT)
        .as_ref()
        .map(|c| c.learned.iter().enumerate().filter(|(_, l)| **l).map(|(i, _)| i as u16).collect())
        .unwrap_or_default()
}

/// The tree's gem power (an added share) and extra sockets for an item.
fn gem_skills(uuid: u128, item: &str) -> (f64, u32) {
    let t = tree();
    let (mut power, mut sockets) = (0.0, 0.0);
    for i in learned_of(uuid) {
        let Some(s) = t.skills.get(i as usize) else { continue };
        for fx in &s.fx {
            let fits = fx.e == "any" || is_type(item, &fx.e);
            match fx.kind {
                Kind::GemPower if fits => power += fx.v,
                Kind::Sockets if fits => sockets += fx.v,
                _ => {}
            }
        }
    }
    (power, sockets.max(0.0) as u32)
}

fn mix(parts: &[&[u8]]) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for p in parts {
        for &b in *p {
            h = (h ^ b as u64).wrapping_mul(0x100000001b3);
        }
    }
    h
}

/// `native_anvil_update` (the server's menu and the window's alike): an item with sockets on the
/// left, a gem on the right. The result has the gem in its next free socket (a vacucite empties
/// them), for 1 to 3 levels and one gem. Asked again for the same inputs, it answers the same.
pub fn anvil_update(
    player: u128,
    left: &ItemStack,
    right: Option<&ItemStack>,
    output: &mut Option<ItemStack>,
    xp_cost: &mut i32,
    material_cost: &mut i32,
) {
    let Some(g) = right.and_then(|r| gem_of(&r.item)) else { return };
    let mut result = None;
    if gem_class(&left.item).is_some() && left.count == 1 {
        let (cap, mut list) = read_sockets(left);
        let (power, extra) = gem_skills(player, &left.item);
        let cap = cap.unwrap_or(0).max((base_sockets(&left.item) + extra).min(MAX_SOCKETS));
        let changed = if g.removal {
            let any = !list.is_empty();
            list.clear();
            any
        } else if (list.len() as u32) < cap {
            let mut opt = -1;
            if !g.random.is_empty() {
                // Iriscite: one of its bonuses the server runs, the same each time it is asked.
                let usable: Vec<usize> =
                    g.random.iter().enumerate().filter(|(_, l)| l.fx.is_some()).map(|(i, _)| i).collect();
                let (who, n, dmg) = (player.to_le_bytes(), (list.len() as u32).to_le_bytes(), left.damage.to_le_bytes());
                let h = mix(&[left.item.as_bytes(), who.as_slice(), n.as_slice(), dmg.as_slice(), g.id.as_bytes()]);
                opt = usable.get((h % usable.len().max(1) as u64) as usize).copied().unwrap_or(0) as i32;
            }
            list.push(Socketed { id: g.id.clone(), opt, pow: (1.0 + power) as f32 });
            true
        } else {
            false
        };
        if changed {
            let mut out = left.clone();
            write_sockets(&mut out, cap, &list);
            result = Some(out);
        }
    }
    match result {
        Some(out) => {
            *output = Some(out);
            *xp_cost = 1 + g.tier as i32 / 2;
            *material_cost = 1;
        }
        None => {
            *output = None;
            *xp_cost = 0;
            *material_cost = 0;
        }
    }
}

/// `native_loot_drops` (server): now and then a gem from an ore a player broke, never with Silk Touch.
/// The dearer the ore, the likelier and the better (config: `gem_<ore>`).
pub fn loot_drops(context: &NativeLootDrops<'_>, drops: &mut Vec<ItemStack>) {
    if context.block.is_none() {
        return;
    }
    let Some(uuid) = context.player else { return };
    let Some(block) = context.table.strip_prefix("minecraft:blocks/") else { return };
    let Some(ore) = block.trim_start_matches("deepslate_").trim_start_matches("nether_").strip_suffix("_ore") else {
        return;
    };
    if context.tool.is_some_and(|t| t.enchantment_level("minecraft:silk_touch") > 0)
        || drops.iter().any(|d| d.item.ends_with("_ore"))
    {
        return;
    }
    let (chance, lo, hi, vacucite) = {
        let guard = lock(&SERVER);
        let Some(s) = guard.as_ref() else { return };
        let Some(&(c, lo, hi)) = s.cfg.gems.get(ore) else { return };
        (c, lo, hi, s.cfg.vacucite)
    };
    let mut found = Vec::new();
    if rand01() < chance {
        // Each tier up is half as likely as the one below.
        let weights: Vec<f64> = (lo..=hi).map(|t| 0.5f64.powi((t - lo) as i32)).collect();
        let mut r = rand01() * weights.iter().sum::<f64>();
        let mut tier = hi;
        for (k, w) in weights.iter().enumerate() {
            if r < *w {
                tier = lo + k as u8;
                break;
            }
            r -= w;
        }
        let kind = GEM_TYPES[((rand01() * GEM_TYPES.len() as f64) as usize).min(GEM_TYPES.len() - 1)];
        found.push(format!("{GEM_NS}:{kind}_{tier}"));
    }
    if matches!(ore, "emerald" | "diamond") && rand01() < vacucite {
        found.push(format!("{GEM_NS}:vacucite_3"));
    }
    if found.is_empty() {
        return;
    }
    let more: Vec<(f64, f64)> =
        lock(&SNAPS).as_ref().and_then(|(s, _)| s.get(&uuid)).map(|s| s.gem_loot.clone()).unwrap_or_default();
    for id in found {
        let mut count = 1;
        for &(c, x) in &more {
            if rand01() < c {
                count += x.round().max(0.0) as u32;
            }
        }
        drops.push(ItemStack { item: id.as_str().into(), count, damage: 0, extra: None });
    }
}

/// `native_tooltip` (client): a gem's bonus for each kind of item; an item's sockets and its gems.
/// Grey: a bonus this version doesn't run yet.
pub fn item_tooltip(game: &Game<'_>, stack: &ItemStack, lines: &mut Vec<(Text, u32)>) {
    const GRAY: u32 = 0xAAAAAA;
    const DARK: u32 = 0x666666;
    let l = if game.settings.language.to_lowercase().starts_with("ru") { 0 } else { 1 };
    let d = gems();
    let ui = |k: &str| d.ui.get(k).map(|p| p[l].clone()).unwrap_or_default();
    let mut add = |s: String, color: u32| lines.push((Text::literal(s.as_str()).with_color(color), u32::MAX));
    if let Some(g) = gem_of(&stack.item) {
        add(ui("info"), GRAY);
        if g.removal {
            add(format!("• {}", ui("removal")), g.color);
        } else if !g.random.is_empty() {
            add(format!("• {}", ui("random")), g.color);
        } else {
            for class in CLASS_ORDER {
                let (Some(line), Some(name)) = (g.by_class.get(class), d.classes.get(class)) else { continue };
                add(format!("• {}: {}", name[l], line.text[l]), if line.fx.is_some() { g.color } else { DARK });
            }
        }
        return;
    }
    let Some(class) = gem_class(&stack.item) else { return };
    let (cap, list) = read_sockets(stack);
    let cap = cap.unwrap_or(0).max(base_sockets(&stack.item)).max(list.len() as u32);
    add(format!("{} {}/{}:", ui("sockets"), list.len(), cap), GRAY);
    for s in &list {
        let Some(g) = gem(&s.id) else { continue };
        let line = socket_line(s, class);
        let text = line.map(|x| x.text[l].clone()).unwrap_or_default();
        let boost = if s.pow > 1.001 { format!(" (+{}%)", ((s.pow - 1.0) * 100.0).round()) } else { String::new() };
        let color = if line.is_some_and(|x| x.fx.is_some()) { g.color } else { DARK };
        add(format!(" ◆ {}: {}{}", g.name[l], text, boost), color);
    }
    for _ in list.len() as u32..cap {
        add(format!(" ◇ {}", ui("socket")), DARK);
    }
}
