//! C6 part 16 (docs/C6_HOLD_DESIGN.md 16.2-16.7, research work/c6/xbow8.md): the REAL crossbow carrier of MW2 mode.
//!
//! In third person with MW2 mode on, the active right-hand ER weapon is swapped for a genuine vanilla crossbow (hidden
//! by the Unseen-Blade hide row as the sword was; the MW2 gun drawn on its weapon model), so ER's own weapon-changed
//! notify gives HKS real crossbow values: the genuine crossbow carry and aim. The original weapon (grip, bolts) comes
//! back at every restore point; a journal written BEFORE each swap makes a crash / quit harmless (the next world enter
//! restores). ER never fires a bolt: RB never reaches ER in MW2 mode, the R3 melee pulse is off, the two-hand chord's RB
//! only runs while the crossbow is unloaded, and a bolt-count tripwire restores and turns the carrier off.
//!
//! The swap is ER's own `EquipGameData::EquipItem` (the inventory menu's call, the menu's arguments), on the game thread
//! (the mod's `ChrIns_PostPhysics` task), 2.7.2.0 only, its first bytes pinned ([`CARRIER_2720`], [`validate`]). Never a
//! byte write into ChrAsm / EquipGameData for the swap, never `ChrAsm::SetSlot`. 16.16 (`[coop] carrier_load`, default
//! false): the slot's `bolt_loaded_states` byte may be set to ER's loaded state on a settled two-handed carrier
//! ([`load_step`]) and is set back first at every exit.
//!
//! Pure: the DLL (`mw2er/src/carrier.rs`) reads the inventory / ChrAsm, calls ER, writes the journal file and asks this
//! module what to do ([`plan`]), which crossbow ([`pick`]), what the journal says ([`Journal`]) and whether a bolt was
//! used ([`BoltTrip`]).

use serde::{Deserialize, Serialize};

/// `[coop] er_carrier`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Off,
    /// Probe P-X1: everything of a swap except the call (`HOLD xbow dry ...`), no game write.
    Dry,
    Crossbow,
}

impl Mode {
    pub fn of(s: &str) -> Mode {
        match s {
            "dry" => Mode::Dry,
            "crossbow" => Mode::Crossbow,
            _ => Mode::Off,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Mode::Off => "off",
            Mode::Dry => "dry",
            Mode::Crossbow => "crossbow",
        }
    }
}

/// The crossbows the carrier may use, in order of preference [V vanilla EquipParamWeapon: wepType 55, spAtkcategory 0,
/// wepmotionCategory 46, wepmotionBothHandId 16]: Light (closest to a rifle carry), Soldier's, Heavy, Arbalest,
/// Crepus's Black-Key. Never Pulley / Full Moon / Repeating / Spread (their own special-attack banks).
pub const CARRIER_ROWS: [u32; 5] = [43_020_000, 43_000_000, 43_030_000, 43_080_000, 43_110_000];
/// `carrier_give`: the vanilla Light Crossbow (a vanilla row: a peer without the friend package resolves it).
pub const GIVE_ID: u32 = 43_020_000;
/// Bolt (only if probe 0 shows bolts are required; not given by this build).
pub const BOLT_ID: u32 = 52_000_000;
/// At most one swap / restore per this many seconds (F5 / F6 toggles do not churn the inventory).
pub const SWAP_HYST_S: f64 = 1.5;
/// A swap or restore is verified on the PlayerIns ChrAsm within this long, else restored / refused.
pub const VERIFY_S: f64 = 1.0;
/// The journal's format version (2: the save identity `save` / `save_slot`, review C6 part 16; a version-1 file is
/// ignored, never applied: no version-1 journal was ever written outside the tests).
pub const JOURNAL_VERSION: u32 = 2;
/// ChrAsm slots: WeaponRight1..3 = 1 / 3 / 5, Bolt1 / Bolt2 = 7 / 9.
pub const SLOT_BOLT1: usize = 7;
pub const SLOT_BOLT2: usize = 9;
/// An item id's type nibble (`type << 28 | param id`; weapon = 0).
pub const ITEM_TYPE_MASK: u32 = 0xF000_0000;

/// The active right weapon's ChrAsm slot of `ChrAsmEquipmentSlots.right_weapon_slot` (0..2): 1 / 3 / 5.
pub fn right_slot(sel: u32) -> Option<usize> {
    (sel <= 2).then_some(2 * sel as usize + 1)
}

/// The EquipParamWeapon row of a weapon param id (upgrade level / affinity in the last two digits).
pub fn row_of(param_id: u32) -> u32 {
    (param_id / 100) * 100
}

/// A weapon item id (type 0) whose row is one of [`CARRIER_ROWS`]: its preference rank.
pub fn carrier_rank(item_id: u32) -> Option<usize> {
    if item_id & ITEM_TYPE_MASK != 0 || item_id == 0 || item_id == u32::MAX {
        return None;
    }
    let row = row_of(item_id);
    CARRIER_ROWS.iter().position(|r| *r == row)
}

/// One inventory entry as the DLL read it: the inventory index (what EquipItem takes: key items first, then
/// `key_items_capacity + k` for normal item k), the gaitem handle, the item id and the quantity.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct InvItem {
    pub index: i32,
    pub handle: u32,
    pub item_id: u32,
    pub qty: u32,
}

/// The carrier to equip: the most preferred OWNED crossbow not equipped in any slot (`equipped` = the 22 ChrAsm gaitem
/// handles; a crossbow in the left hand or another right slot is the player's and stays there), lowest index first.
pub fn pick(inv: &[InvItem], equipped: &[u32]) -> Option<InvItem> {
    inv.iter()
        .filter(|e| e.handle != 0 && e.handle != u32::MAX && e.index >= 0 && !equipped.contains(&e.handle))
        .filter_map(|e| carrier_rank(e.item_id).map(|r| (r, e.index, *e)))
        .min_by_key(|(r, i, _)| (*r, *i))
        .map(|(_, _, e)| e)
}

/// The inventory index -> (key list?, position) as ER's `entry_at_slot` reads it.
pub fn inv_location(index: i32, key_cap: u32) -> Option<(bool, usize)> {
    let i = u32::try_from(index).ok()?;
    Some(if i < key_cap { (true, i as usize) } else { (false, (i - key_cap) as usize) })
}

// ---- the plan ------------------------------------------------------------------------------------------------------

/// Everything that decides this tick's step (docs 16.3 step 1, 16.5).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PlanIn {
    /// `er_carrier` is dry or crossbow (the flag), and the carrier is not off for the session (trip / refused / exe).
    pub flag: bool,
    pub disabled: bool,
    pub mode_on: bool,
    pub in_world: bool,
    /// First person (a scope peek is third person here: no restore per scope-in).
    pub fp: bool,
    pub ac130: bool,
    pub nuke: bool,
    pub dead: bool,
    /// ER still spawns the player after a world enter (`er_stance::spawn_settling`).
    pub settling: bool,
    /// A menu / talk / loading latch / the game window in the background.
    pub menu: bool,
    /// ER is not idle (`er_stance::er_busy`), mid-air, on a ladder, mounted, an ER prompt, our aim episode: no swap now.
    pub busy: Option<&'static str>,
    /// Our carrier is equipped (journal written, the swap verified or in verification).
    pub swapped: bool,
    /// A journal of OURS (this character) is on disk without a live swap: restore it first (a crash / quit / the world
    /// left with the carrier on).
    pub journal: bool,
    /// A journal of ANOTHER character is on disk: never swap (it is restored when that character loads).
    pub foreign_journal: bool,
    /// Seconds since the last swap / restore.
    pub since_change_s: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    /// Nothing to do.
    Idle,
    /// Equip the carrier now (the gate passed).
    Swap,
    /// Equip the original back now (why).
    Restore(&'static str),
    /// A swap / restore is wanted but ER cannot be called now (why: dead / loading / menu / spawn / busy / hyst).
    Wait(&'static str),
}

/// Why ER's equip may not be called now (16.5 rules: never while dead, loading or with menus open; it waits).
pub fn call_block(i: &PlanIn) -> Option<&'static str> {
    if !i.in_world {
        Some("loading")
    } else if i.dead {
        Some("dead")
    } else if i.menu {
        Some("menu")
    } else if i.settling {
        Some("spawn")
    } else {
        None
    }
}

/// Why the carrier is not wanted (None = wanted): the restore reason when it is equipped.
pub fn off_reason(i: &PlanIn) -> Option<&'static str> {
    if !i.flag {
        Some("flag")
    } else if i.disabled {
        Some("disabled")
    } else if !i.in_world {
        Some("world_leave")
    } else if !i.mode_on {
        Some("mode_off")
    } else if i.fp {
        Some("fp")
    } else if i.ac130 {
        Some("ac130")
    } else if i.nuke {
        Some("nuke")
    } else {
        None
    }
}

/// The step (docs 16.3 / 16.5): a pending journal is restored first; an equipped carrier is restored at every off
/// reason as soon as ER may be called (restores never wait the hysteresis); a swap needs the full gate.
pub fn plan(i: &PlanIn) -> Step {
    if i.journal && !i.swapped {
        return match call_block(i) {
            Some(w) => Step::Wait(w),
            None => Step::Restore("journal"),
        };
    }
    if i.swapped {
        return match off_reason(i) {
            None => Step::Idle,
            Some(why) => match call_block(i) {
                Some(w) => Step::Wait(w),
                None => Step::Restore(why),
            },
        };
    }
    if off_reason(i).is_some() {
        return Step::Idle;
    }
    if i.foreign_journal {
        return Step::Wait("journal_other");
    }
    if let Some(w) = call_block(i).or(i.busy) {
        return Step::Wait(w);
    }
    if i.since_change_s < SWAP_HYST_S {
        return Step::Wait("hyst");
    }
    Step::Swap
}

/// What a restore finds in slot `s` (16.5 rules).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RestoreKind {
    /// Our carrier is there: equip the original back.
    Equip,
    /// The original is already there (a restore that went through before a crash): drop the journal.
    Done,
    /// Something else is there (the user changed gear meanwhile): change nothing, drop the journal.
    Foreign,
    /// Our carrier is there but the original item is no longer in the inventory: leave the carrier, keep the journal.
    Missing,
}

/// `slot_handle` = the gaitem handle in slot `s` now; `orig_present` = the original's handle is still in the inventory.
pub fn restore_kind(j: &Journal, slot_handle: u32, orig_present: bool) -> RestoreKind {
    if slot_handle == j.orig_handle {
        RestoreKind::Done
    } else if slot_handle != j.carrier_handle {
        RestoreKind::Foreign
    } else if !orig_present {
        RestoreKind::Missing
    } else {
        RestoreKind::Equip
    }
}

// ---- the journal (16.5) --------------------------------------------------------------------------------------------

/// `<root>\state\carrier.json`: written (tmp, flush, rename) BEFORE every EquipItem of a swap, deleted after a verified
/// restore. Only the mod's own folder; never the game folder or `%APPDATA%\EldenRing`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Journal {
    pub version: u32,
    /// The character (PlayerGameData name), so a journal is only ever applied to its own character.
    pub character: String,
    /// The save flavour: "solo" (ER0000.mw2er.sl2) or "coop" (Seamless, ER0000.mw2co2). Both share this file and
    /// usually hold copies of the same character (review C6 part 16: a solo crash is never judged on the co-op copy).
    pub save: String,
    /// `GameMan.save_slot` at the swap (the character's slot in that save).
    pub save_slot: i32,
    /// The ChrAsm slot (1 / 3 / 5).
    pub slot: u32,
    pub orig_item_id: u32,
    pub orig_handle: u32,
    pub orig_index: i32,
    /// The grip before the swap (`arm_style`).
    pub orig_arm: u32,
    /// The bolt slots' param ids before the swap (7 / 9).
    pub bolts: [i32; 2],
    pub carrier_item_id: u32,
    pub carrier_handle: u32,
    pub carrier_index: i32,
    /// Item ids this mod gave into the MOD save for the carrier (they stay in the inventory, documented).
    pub given: Vec<u32>,
    /// Unix seconds of the swap.
    pub t_unix: u64,
}

impl Journal {
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_default()
    }

    /// A journal read back (a torn / foreign / newer file = None: ignored, never applied).
    pub fn parse(text: &str) -> Option<Journal> {
        let j: Journal = serde_json::from_str(text).ok()?;
        let slot_ok = right_slot((j.slot.saturating_sub(1)) / 2) == Some(j.slot as usize);
        (j.version == JOURNAL_VERSION && slot_ok && j.orig_handle != 0 && j.carrier_handle != 0 && j.orig_handle != j.carrier_handle).then_some(j)
    }

    /// The journal belongs to the loaded character: the same save flavour, save slot and name.
    pub fn owned_by(&self, me: &SaveId) -> bool {
        self.save == me.save && self.save_slot == me.save_slot && self.character == me.character
    }

    /// The owner as the log names it.
    pub fn owner_text(&self) -> String {
        format!("{} save={} slot={}", self.character, self.save, self.save_slot)
    }
}

/// The loaded character's identity (review C6 part 16): the save flavour ("solo" / "coop"), `GameMan.save_slot` and the
/// PlayerGameData name. A journal is applied (restored or dropped) only when all three match.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SaveId {
    pub save: String,
    pub save_slot: i32,
    pub character: String,
}

impl SaveId {
    pub fn text(&self) -> String {
        format!("{} save={} slot={}", self.character, self.save, self.save_slot)
    }
}

/// What the journal file says at a world enter (review C6 part 16: only a journal parsed from the file for THIS
/// character is ever applied; a journal kept in memory across a world leave never is).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DiskVerdict {
    /// No file.
    Absent,
    /// A torn / foreign / old-version file: ignored, kept for the log, never applied.
    Unreadable,
    /// This character's journal: restored first.
    Own(Journal),
    /// Another character's (or another save's) journal: kept, never applied or deleted; no swap meanwhile.
    Other(String),
}

pub fn disk_verdict(text: Option<&str>, me: &SaveId) -> DiskVerdict {
    let Some(t) = text else { return DiskVerdict::Absent };
    match Journal::parse(t) {
        None => DiskVerdict::Unreadable,
        Some(j) if j.owned_by(me) => DiskVerdict::Own(j),
        Some(j) => DiskVerdict::Other(j.owner_text()),
    }
}

// ---- the grip back (review C6 part 16) -----------------------------------------------------------------------------

/// The player's grip before the swap, after a restore the stance's exit chords did not cover (they are dropped at a
/// death / world leave, and a journal restore after a crash has no stance at all): handed to the stance as an exit
/// target (`seq` = once per restore) and, while the grip is not back yet, used as the next swap's `orig_arm` instead of
/// the one-handed grip EquipItem left (so a re-swap never records 1 over the player's 3).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GripBack {
    arm: Option<u32>,
    seq: u32,
}

impl GripBack {
    /// A verified restore of a journal whose grip was `orig_arm`.
    pub fn restored(&mut self, orig_arm: u32) {
        self.arm = Some(orig_arm);
        self.seq = self.seq.wrapping_add(1);
    }

    /// Every tick without our carrier in the slot: the grip reached its target = nothing pending.
    pub fn note_arm(&mut self, arm_now: u32) {
        if self.arm == Some(arm_now) {
            self.arm = None;
        }
    }

    /// The `orig_arm` of a new swap (consumes a pending grip).
    pub fn orig_arm_for_swap(&mut self, arm_now: u32) -> u32 {
        self.arm.take().unwrap_or(arm_now)
    }

    /// (seq, target) for the stance while a grip is pending.
    pub fn pending(&self) -> Option<(u32, u32)> {
        self.arm.map(|a| (self.seq, a))
    }

    pub fn clear(&mut self) {
        self.arm = None;
    }
}

/// The journal file's name inside `<root>\state` and the temporary name it is written under first.
pub const JOURNAL_FILE: &str = "carrier.json";
pub const JOURNAL_TMP: &str = "carrier.json.tmp";

// ---- the bolt tripwire (16.4 step 5) -------------------------------------------------------------------------------

/// The equipped bolt quantity while the carrier is on: any decrease = a bolt was used (`HOLD xbow trip why=bolt_used`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BoltTrip {
    last: Option<u32>,
}

impl BoltTrip {
    /// `qty` = the summed quantity of the equipped bolt slots (None = unreadable: no verdict this tick). Returns the
    /// (before, after) of a decrease.
    pub fn note(&mut self, qty: Option<u32>) -> Option<(u32, u32)> {
        let q = qty?;
        let prev = self.last.replace(q);
        prev.filter(|p| q < *p).map(|p| (p, q))
    }

    pub fn reset(&mut self) {
        self.last = None;
    }
}

// ---- the grip (16.2 X6) --------------------------------------------------------------------------------------------

/// The two-hand chord's RB / LMB is allowed with the carrier only while the slot's crossbow is NOT loaded (a stray RB
/// is then ER's reload animation, never a shot) [V EquipItem clears `bolt_loaded_states[slot]`].
pub fn chord_rb_allowed(bolt_loaded: Option<bool>) -> bool {
    bolt_loaded == Some(false)
}

/// The arm_style write's verdict `CHORD_CHECK_S` after it: the grip stayed two-handed (ER kept it) = ok.
pub fn grip_write_ok(target: u32, after: u32) -> bool {
    after == target
}

// ---- the loaded crossbow (16.16, `[coop] carrier_load`) ------------------------------------------------------------

/// ChrAsm `bolt_loaded_states` (12 bytes) inside the ChrAsm [V 2.7.2.0: `ChrAsm::Copy` 0x1403BF300 copies 12 bytes at
/// +0xDC; `ChrAsm::SetSlot` 0x1403BF47A clears `[asm + slot + 0xDC]`; vendor `ChrAsm.bolt_loaded_states`].
pub const ASM_BOLT_LOADED_OFF: usize = 0xDC;
/// The grip must have been two-handed (3) this long, with no chord in flight, before the load (the chord's verdict and
/// any grip retry are over: the chord's LMB / RB never reaches a loaded crossbow).
pub const LOAD_SETTLE_S: f64 = 1.0;
/// The load is verified on both ChrAsms (EquipGameData's and PlayerIns's) this long after the write.
pub const LOAD_VERIFY_S: f64 = 0.5;
/// At most this many loads per swap (an undo for a grip change may be followed by one more).
pub const LOAD_MAX_PER_SWAP: u32 = 3;

/// One tick's inputs of the load ([`load_step`]).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct LoadIn {
    /// `[coop] carrier_load` (and the carrier mode is crossbow).
    pub flag: bool,
    /// The carrier is off for the session (trip / refused / restore failure / stance off).
    pub disabled: bool,
    /// The carrier is equipped and verified (`xbow on`), no swap / restore in flight.
    pub on: bool,
    pub in_world: bool,
    pub mode_on: bool,
    pub fp: bool,
    pub ac130: bool,
    pub nuke: bool,
    pub dead: bool,
    /// EquipGameData's grip (3 = two-handed right).
    pub arm: u32,
    /// Seconds the grip has been 3 (0 when not).
    pub arm3_s: f64,
    /// Why the stance says ER is not idle now (er_busy / air / mount / prompt / aim / chord in flight).
    pub busy: Option<&'static str>,
    /// The keyboard chord's E + LMB window is open (`KB_CHORD_T0 != 0`): never load inside it.
    pub chord_keys: bool,
    /// `bolt_loaded_states[slot]` (None = unreadable).
    pub loaded: Option<bool>,
    /// The equipped bolt quantity (None = unreadable).
    pub bolts: Option<u32>,
    /// We loaded it (and have not undone it).
    pub ours: bool,
    pub tries: u32,
}

/// The load's step this tick.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LoadStep {
    Idle,
    /// Wanted, not now (why).
    Wait(&'static str),
    /// Write `bolt_loaded_states[slot] = 1` (ER's TAE event 35 end state) now.
    Load,
    /// Our load back to 0 now (why): unloaded is the safe state for every exit, restore and chord.
    Undo(&'static str),
}

/// 16.16: our load is kept only while the carrier is on in third-person MW2 mode with the two-handed grip; any other
/// state undoes it first (before a restore's EquipItem and before any chord). A load needs the grip settled at 3, no
/// chord (in flight or its keys), ER idle, a readable unloaded slot and bolts equipped.
pub fn load_step(i: &LoadIn) -> LoadStep {
    let why_not = if !i.flag {
        Some("flag_off")
    } else if i.disabled {
        Some("disabled")
    } else if !i.in_world {
        Some("world_leave")
    } else if !i.mode_on {
        Some("mode_off")
    } else if i.fp {
        Some("fp")
    } else if i.ac130 {
        Some("ac130")
    } else if i.nuke {
        Some("nuke")
    } else if i.dead {
        Some("dead")
    } else if !i.on {
        Some("carrier_off")
    } else {
        None
    };
    if i.ours {
        return match why_not {
            Some(w) => LoadStep::Undo(w),
            // the chord may follow a grip change: unloaded first
            None if i.arm != 3 => LoadStep::Undo("grip"),
            None if i.chord_keys => LoadStep::Undo("chord"),
            None => LoadStep::Idle,
        };
    }
    if why_not.is_some() || i.tries >= LOAD_MAX_PER_SWAP {
        return LoadStep::Idle;
    }
    let wait = match i.loaded {
        None => Some("unreadable"),
        Some(true) => Some("already"),
        Some(false) if i.bolts.unwrap_or(0) == 0 => Some("no_bolts"),
        Some(false) if i.arm != 3 => Some("grip"),
        Some(false) if i.chord_keys => Some("chord"),
        Some(false) => i.busy.or((i.arm3_s < LOAD_SETTLE_S).then_some("settle")),
    };
    match wait {
        Some(w) => LoadStep::Wait(w),
        None => LoadStep::Load,
    }
}

/// The load's verdict [`LOAD_VERIFY_S`] after the write: both ChrAsms read loaded (ER's per-frame copy kept it).
pub fn load_ok(egd_after: Option<bool>, player_after: Option<bool>) -> bool {
    egd_after == Some(true) && player_after == Some(true)
}

// ---- 2.7.2.0 addresses (16.3) --------------------------------------------------------------------------------------

/// One exe build's carrier functions (RVAs) and their first bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CarrierAddrs {
    /// `EquipGameData::EquipItem(egd, i32 slot, *const u32 handle, i32 inv_index, bool, bool, bool)`.
    pub equip_item: usize,
    pub equip_item_prologue: &'static [u8],
    /// `EquipGameData::AddItem(egd, *const u32 handle, i32 qty, bool, bool) -> i32` (give only).
    pub add_item: usize,
    pub add_item_prologue: &'static [u8],
    /// `CSGaitemImp::Create(imp, *mut u32 out, *const u32 item_id)` (give only).
    pub create: usize,
    pub create_prologue: &'static [u8],
    /// `EquipInventoryData::Find(egd + 0x158, *const u32 item_id) -> i32 / -1`.
    pub find: usize,
    pub find_prologue: &'static [u8],
    /// The `GameDataMan` / `CSGaitemImp` pointer globals.
    pub game_data_man: usize,
    pub gaitem_imp: usize,
    /// The `GameMan` pointer global (its `save_slot`: the journal's save identity) [V exe_sites `game_man`, 2.7.2.0].
    pub game_man: usize,
}

/// `push rbx; push rbp; push rsi; push rdi; push r14; sub rsp,90h` [V work/c6/xbow8/equip_249160.txt].
pub const EQUIP_ITEM_PROLOGUE_2720: [u8; 14] = [0x40, 0x53, 0x55, 0x56, 0x57, 0x41, 0x56, 0x48, 0x81, 0xEC, 0x90, 0x00, 0x00, 0x00];
/// `mov rax,rsp; push rbp; push r12; push r13; push r14; push r15; lea rbp,[rax-57h]` [V xbow8.md 2.2].
pub const ADD_ITEM_PROLOGUE_2720: [u8; 16] = [0x48, 0x8B, 0xC4, 0x55, 0x41, 0x54, 0x41, 0x55, 0x41, 0x56, 0x41, 0x57, 0x48, 0x8D, 0x68, 0xA9];
/// `mov [rsp+10h],rdx; push rbx; sub rsp,30h` [V xbow8.md 2.2].
pub const CREATE_PROLOGUE_2720: [u8; 10] = [0x48, 0x89, 0x54, 0x24, 0x10, 0x53, 0x48, 0x83, 0xEC, 0x30];
/// `push rsi; sub rsp,20h; or esi,-1; cmp dword [rdx],-1` [V xbow8.md 2.2].
pub const FIND_PROLOGUE_2720: [u8; 12] = [0x40, 0x56, 0x48, 0x83, 0xEC, 0x20, 0x83, 0xCE, 0xFF, 0x83, 0x3A, 0xFF];

/// 2.7.2.0 [V eldenring.exe 2.7.2.0 read as a file, work/c6/xbow8.md 2.1-2.2]; 2.7.1.0 has none (`HOLD xbow off
/// why=version`).
pub const CARRIER_2720: CarrierAddrs = CarrierAddrs {
    equip_item: 0x24_9160,
    equip_item_prologue: &EQUIP_ITEM_PROLOGUE_2720,
    add_item: 0x24_6480,
    add_item_prologue: &ADD_ITEM_PROLOGUE_2720,
    create: 0x67_35c0,
    create_prologue: &CREATE_PROLOGUE_2720,
    find: 0x24_c560,
    find_prologue: &FIND_PROLOGUE_2720,
    game_data_man: 0x3d6_4fb8,
    gaitem_imp: 0x3d7_0920,
    game_man: 0x3d7_09a8,
};

/// The VA (image base 0x140000000) of `ChrAsm::SetSlot`, which the carrier must never call (static check rule 40).
pub const CHR_ASM_SET_SLOT_RVA: usize = 0x3b_f3d0;

/// The install-time pin (`rd(abs_addr, out)` = a non-faulting read of the loaded image): every function's first bytes.
/// Err = the function whose bytes differ (`HOLD xbow off why=prologue fn=..`).
pub fn validate(a: &CarrierAddrs, base: usize, rd: &dyn Fn(usize, &mut [u8]) -> bool) -> Result<(), &'static str> {
    let same = |rva: usize, want: &[u8]| {
        let mut b = [0u8; 32];
        let n = want.len();
        n <= b.len() && rd(base + rva, &mut b[..n]) && b[..n] == *want
    };
    for (name, rva, want) in [
        ("equip_item", a.equip_item, a.equip_item_prologue),
        ("add_item", a.add_item, a.add_item_prologue),
        ("create", a.create, a.create_prologue),
        ("find", a.find, a.find_prologue),
    ] {
        if !same(rva, want) {
            return Err(name);
        }
    }
    Ok(())
}

// ---- lines ---------------------------------------------------------------------------------------------------------

/// `HOLD xbow dry slot=.. orig=<id> idx=.. arm=.. pick=<id> idx=..|none bolts=[..] journal=<path>`.
pub fn dry_line(slot: usize, orig: &InvItem, arm: u32, pick: Option<&InvItem>, bolts: [i32; 2], journal: &str) -> String {
    format!(
        "xbow dry slot={slot} orig={} idx={} handle=0x{:08x} arm={arm} pick={} bolts=[{},{}] journal={journal} (P-X1: no call, no write)",
        orig.item_id,
        orig.index,
        orig.handle,
        pick.map_or("none".to_string(), |p| format!("{} idx={} handle=0x{:08x}", p.item_id, p.index, p.handle)),
        bolts[0],
        bolts[1]
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inv(index: i32, handle: u32, item_id: u32) -> InvItem {
        InvItem { index, handle, item_id, qty: 1 }
    }

    #[test]
    fn c6_carrier_pick_order_and_exclusions() {
        let items = [
            inv(10, 0x8080_0001, 2_000_000),         // a longsword
            inv(11, 0x8080_0002, 43_000_000 + 5),    // Soldier's +5
            inv(12, 0x8080_0003, 43_020_000),        // Light, equipped in the left hand
            inv(13, 0x8080_0004, 43_020_000 + 3),    // Light +3, free
            inv(14, 0x8080_0005, 43_050_000),        // Pulley: never
            inv(15, 0x8080_0006, 0x4000_0000 | 43_020_000), // not a weapon item id
        ];
        let equipped = [0x8080_0001, 0x8080_0003];
        assert_eq!(pick(&items, &equipped).map(|e| e.index), Some(13), "the free Light crossbow first");
        assert_eq!(pick(&items[..3], &equipped).map(|e| e.index), Some(11), "else Soldier's");
        assert_eq!(pick(&[items[0], items[2], items[4], items[5]], &equipped), None, "none owned / all equipped / excluded");
        assert_eq!((carrier_rank(43_050_000), carrier_rank(43_110_025), carrier_rank(0)), (None, Some(4), None));
        assert_eq!((right_slot(0), right_slot(2), right_slot(3)), (Some(1), Some(5), None));
        assert_eq!((inv_location(5, 384), inv_location(400, 384), inv_location(-1, 384)), (Some((true, 5)), Some((false, 16)), None));
    }

    fn base_in() -> PlanIn {
        PlanIn { flag: true, mode_on: true, in_world: true, since_change_s: 10.0, ..Default::default() }
    }

    #[test]
    fn c6_carrier_plan_gate_and_restore_points() {
        let i = base_in();
        assert_eq!(plan(&i), Step::Swap);
        // the gate rows: no swap while any of them holds
        for (m, why) in [
            (PlanIn { dead: true, ..i }, "dead"),
            (PlanIn { menu: true, ..i }, "menu"),
            (PlanIn { settling: true, ..i }, "spawn"),
            (PlanIn { busy: Some("er_busy"), ..i }, "er_busy"),
            (PlanIn { busy: Some("aim"), ..i }, "aim"),
            (PlanIn { since_change_s: 1.0, ..i }, "hyst"),
            (PlanIn { foreign_journal: true, ..i }, "journal_other"),
        ] {
            assert_eq!(plan(&m), Step::Wait(why), "{m:?}");
        }
        // not wanted at all: idle (nothing equipped)
        for m in [PlanIn { flag: false, ..i }, PlanIn { mode_on: false, ..i }, PlanIn { fp: true, ..i }, PlanIn { ac130: true, ..i }, PlanIn { nuke: true, ..i }, PlanIn { disabled: true, ..i }, PlanIn { in_world: false, ..i }] {
            assert_eq!(plan(&m), Step::Idle, "{m:?}");
        }
        // every restore point of an equipped carrier, at once (no hysteresis)
        let on = PlanIn { swapped: true, since_change_s: 0.1, ..i };
        assert_eq!(plan(&on), Step::Idle);
        for (m, why) in [
            (PlanIn { mode_on: false, ..on }, "mode_off"),
            (PlanIn { fp: true, ..on }, "fp"),
            (PlanIn { ac130: true, ..on }, "ac130"),
            (PlanIn { nuke: true, ..on }, "nuke"),
            (PlanIn { flag: false, ..on }, "flag"),
            (PlanIn { disabled: true, ..on }, "disabled"),
        ] {
            assert_eq!(plan(&m), Step::Restore(why), "{m:?}");
        }
        // no ER call while dead / loading / in a menu: it waits (the world leave restores at the next world enter)
        assert_eq!(plan(&PlanIn { mode_on: false, dead: true, ..on }), Step::Wait("dead"));
        assert_eq!(plan(&PlanIn { in_world: false, ..on }), Step::Wait("loading"));
        assert_eq!(plan(&PlanIn { fp: true, menu: true, ..on }), Step::Wait("menu"));
        // a busy ER never blocks a restore (only a swap)
        assert_eq!(plan(&PlanIn { fp: true, busy: Some("er_busy"), ..on }), Step::Restore("fp"));
        // the journal of a crash: restored first at the next world enter, once ER may be called
        let j = PlanIn { journal: true, ..i };
        assert_eq!(plan(&j), Step::Restore("journal"));
        assert_eq!(plan(&PlanIn { settling: true, ..j }), Step::Wait("spawn"));
        assert_eq!(plan(&PlanIn { mode_on: false, fp: true, ..j }), Step::Restore("journal"), "whatever the mode");
    }

    fn journal() -> Journal {
        Journal {
            version: JOURNAL_VERSION,
            character: "Tarnished".into(),
            save: "solo".into(),
            save_slot: 0,
            slot: 1,
            orig_item_id: 2_000_000,
            orig_handle: 0x8080_0001,
            orig_index: 410,
            orig_arm: 3,
            bolts: [52_000_000, -1],
            carrier_item_id: 43_020_000,
            carrier_handle: 0x8080_0004,
            carrier_index: 413,
            given: vec![],
            t_unix: 1_792_000_000,
        }
    }

    #[test]
    fn c6_carrier_journal_roundtrip() {
        let j = journal();
        assert_eq!(Journal::parse(&j.to_json()), Some(j.clone()));
        // a torn tmp file / garbage / another version / a bad slot / equal handles are ignored, never applied
        let t = j.to_json();
        assert_eq!(Journal::parse(&t[..t.len() / 2]), None);
        assert_eq!(Journal::parse("{}"), None);
        assert_eq!(Journal::parse(&Journal { version: 1, ..j.clone() }.to_json()), None);
        assert_eq!(Journal::parse(&Journal { version: 3, ..j.clone() }.to_json()), None);
        assert_eq!(Journal::parse(&Journal { slot: 2, ..j.clone() }.to_json()), None);
        assert_eq!(Journal::parse(&Journal { carrier_handle: j.orig_handle, ..j.clone() }.to_json()), None);
        assert_eq!(Journal::parse(&t.replace("\"t_unix\"", "\"extra\": 1, \"t_unix\"")), None, "unknown fields");
        for s in [1, 3, 5] {
            assert!(Journal::parse(&Journal { slot: s, ..j.clone() }.to_json()).is_some());
        }
    }

    fn me(save: &str, slot: i32, name: &str) -> SaveId {
        SaveId { save: save.into(), save_slot: slot, character: name.into() }
    }

    /// Review C6 part 16: only a journal of THIS character in THIS save is applied. Quit to the title with a live swap of
    /// A, enter as B (or as A's copy in the other save / another slot): no restore runs, the file is kept, no swap.
    #[test]
    fn c6_carrier_journal_identity() {
        let j = journal();
        let t = j.to_json();
        assert_eq!(disk_verdict(Some(&t), &me("solo", 0, "Tarnished")), DiskVerdict::Own(j.clone()));
        for other in [me("solo", 0, "Other"), me("coop", 0, "Tarnished"), me("solo", 3, "Tarnished")] {
            let v = disk_verdict(Some(&t), &other);
            assert!(matches!(v, DiskVerdict::Other(_)), "{other:?}: {v:?}");
            // with only the other journal on disk (none of ours): never a restore of A's journal against B, never a swap
            let p = PlanIn { foreign_journal: true, ..base_in() };
            assert_eq!(plan(&p), Step::Wait("journal_other"));
            assert_eq!(plan(&PlanIn { mode_on: false, ..p }), Step::Idle);
        }
        assert_eq!(disk_verdict(None, &me("solo", 0, "Tarnished")), DiskVerdict::Absent);
        assert_eq!(disk_verdict(Some(&t[..t.len() / 2]), &me("solo", 0, "Tarnished")), DiskVerdict::Unreadable);
        // a version-1 journal (no identity) is never applied
        let v1 = t.replace("\"version\": 2", "\"version\": 1");
        assert_eq!(disk_verdict(Some(&v1), &me("solo", 0, "Tarnished")), DiskVerdict::Unreadable);
    }

    /// Review C6 part 16: swap with arm 3, world leave, journal restore (EquipItem leaves 1), re-swap: the new journal
    /// still says orig_arm 3; the stance gets the grip as an exit target once per restore; reaching it clears it.
    #[test]
    fn c6_carrier_grip_back_survives_a_journal_restore() {
        let mut g = GripBack::default();
        let j = Journal { orig_arm: 3, ..journal() };
        // the world leave drops the stance's exit chords; the next world enter restores the journal
        g.restored(j.orig_arm);
        let (seq, target) = g.pending().unwrap();
        assert_eq!(target, 3);
        g.note_arm(1);
        assert_eq!(g.pending(), Some((seq, 3)), "still one-handed: pending");
        assert_eq!(g.orig_arm_for_swap(1), 3, "the re-swap records the player's grip");
        assert_eq!(g.pending(), None);
        assert_eq!(g.orig_arm_for_swap(1), 1, "nothing pending: the grip now");
        // the stance's chords reached the grip: nothing carried
        g.restored(3);
        assert!(g.pending().unwrap().0 != seq, "a new restore = a new seq");
        g.note_arm(3);
        assert_eq!(g.orig_arm_for_swap(1), 1);
    }

    #[test]
    fn c6_carrier_plan_restore_kind() {
        let j = journal();
        assert_eq!(restore_kind(&j, j.carrier_handle, true), RestoreKind::Equip);
        assert_eq!(restore_kind(&j, j.orig_handle, true), RestoreKind::Done);
        assert_eq!(restore_kind(&j, 0x8080_0099, true), RestoreKind::Foreign, "the user changed gear: never touched");
        assert_eq!(restore_kind(&j, j.carrier_handle, false), RestoreKind::Missing);
    }

    #[test]
    fn c6_carrier_bolt_trip() {
        let mut b = BoltTrip::default();
        assert_eq!(b.note(Some(20)), None);
        assert_eq!(b.note(None), None, "unreadable: no verdict");
        assert_eq!(b.note(Some(20)), None);
        assert_eq!(b.note(Some(21)), None, "picked up: fine");
        assert_eq!(b.note(Some(20)), Some((21, 20)), "one used: trip");
        b.reset();
        assert_eq!(b.note(Some(5)), None);
        // the chord's RB only on an unloaded crossbow
        assert!(chord_rb_allowed(Some(false)) && !chord_rb_allowed(Some(true)) && !chord_rb_allowed(None));
        assert!(grip_write_ok(3, 3) && !grip_write_ok(3, 1));
    }

    /// 16.16: the load only on a settled two-handed carrier with no chord / busy ER / no bolts; ours is undone at every
    /// exit (before the restore's EquipItem), at a grip change and inside a chord window; capped per swap.
    #[test]
    fn c6_carrier_load_step() {
        let ready = LoadIn {
            flag: true,
            on: true,
            in_world: true,
            mode_on: true,
            arm: 3,
            arm3_s: 2.0,
            loaded: Some(false),
            bolts: Some(4),
            ..Default::default()
        };
        assert_eq!(load_step(&ready), LoadStep::Load);
        assert_eq!(load_step(&LoadIn { flag: false, ..ready }), LoadStep::Idle, "flag off: nothing");
        assert_eq!(load_step(&LoadIn { on: false, ..ready }), LoadStep::Idle, "not swapped / verifying: nothing");
        for (k, w) in [
            (LoadIn { arm: 1, ..ready }, "grip"),
            (LoadIn { arm3_s: 0.5, ..ready }, "settle"),
            (LoadIn { chord_keys: true, ..ready }, "chord"),
            (LoadIn { busy: Some("chord"), ..ready }, "chord"),
            (LoadIn { busy: Some("aim"), ..ready }, "aim"),
            (LoadIn { bolts: Some(0), ..ready }, "no_bolts"),
            (LoadIn { bolts: None, ..ready }, "no_bolts"),
            (LoadIn { loaded: None, ..ready }, "unreadable"),
            (LoadIn { loaded: Some(true), ..ready }, "already"),
        ] {
            assert_eq!(load_step(&k), LoadStep::Wait(w), "{k:?}");
        }
        assert_eq!(load_step(&LoadIn { tries: LOAD_MAX_PER_SWAP, ..ready }), LoadStep::Idle, "capped");
        // ours: kept only in the ready state; every exit / grip change / chord window undoes it first
        let ours = LoadIn { ours: true, loaded: Some(true), ..ready };
        assert_eq!(load_step(&ours), LoadStep::Idle);
        assert_eq!(load_step(&LoadIn { busy: Some("aim"), ..ours }), LoadStep::Idle, "an aim keeps it (the point of the load)");
        for (k, w) in [
            (LoadIn { flag: false, ..ours }, "flag_off"),
            (LoadIn { disabled: true, ..ours }, "disabled"),
            (LoadIn { in_world: false, ..ours }, "world_leave"),
            (LoadIn { mode_on: false, ..ours }, "mode_off"),
            (LoadIn { fp: true, ..ours }, "fp"),
            (LoadIn { ac130: true, ..ours }, "ac130"),
            (LoadIn { nuke: true, ..ours }, "nuke"),
            (LoadIn { dead: true, ..ours }, "dead"),
            (LoadIn { on: false, ..ours }, "carrier_off"),
            (LoadIn { arm: 1, ..ours }, "grip"),
            (LoadIn { chord_keys: true, ..ours }, "chord"),
        ] {
            assert_eq!(load_step(&k), LoadStep::Undo(w), "{k:?}");
        }
        assert!(load_ok(Some(true), Some(true)) && !load_ok(Some(false), Some(true)) && !load_ok(Some(true), None));
        // the same byte the TAE event and SetSlot address (0x148 inside EquipGameData = 0x6C + 0xDC)
        assert_eq!(0x6C + ASM_BOLT_LOADED_OFF, 0x148);
    }

    #[test]
    fn c6_carrier_prologues_validate() {
        let mem = std::cell::RefCell::new(vec![0u8; 0x70_0000]);
        let a = CARRIER_2720;
        for (rva, b) in [(a.equip_item, a.equip_item_prologue), (a.add_item, a.add_item_prologue), (a.create, a.create_prologue), (a.find, a.find_prologue)] {
            mem.borrow_mut()[rva..rva + b.len()].copy_from_slice(b);
        }
        let rd = |addr: usize, out: &mut [u8]| {
            let m = mem.borrow();
            let a = addr - 0x1000;
            match m.get(a..a + out.len()) {
                Some(s) => {
                    out.copy_from_slice(s);
                    true
                }
                None => false,
            }
        };
        let rd_off = |addr: usize, out: &mut [u8]| rd(addr, out);
        assert_eq!(validate(&a, 0x1000, &rd_off), Ok(()));
        mem.borrow_mut()[a.find + 3] ^= 1;
        assert_eq!(validate(&a, 0x1000, &rd_off), Err("find"));
        mem.borrow_mut()[a.find + 3] ^= 1;
        mem.borrow_mut()[a.equip_item] = 0xE9;
        assert_eq!(validate(&a, 0x1000, &rd_off), Err("equip_item"), "a detour over EquipItem: refused");
        assert!(CHR_ASM_SET_SLOT_RVA != a.equip_item && a.game_data_man == 0x3d6_4fb8);
    }

    /// The EquipItem pin against the research dump (work/c6/xbow8/equip_249160.txt, the 2.7.2.0 exe read as a file):
    /// our 14 bytes decode to exactly its first six instructions.
    #[test]
    fn c6_carrier_prologue_matches_dump() {
        let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../work/c6/xbow8/equip_249160.txt");
        let Ok(text) = std::fs::read_to_string(&p) else {
            println!("SKIP: {} missing", p.display());
            return;
        };
        let text = text.trim_start_matches('\u{feff}');
        let want = ["140249160  push rbx", "140249162  push rbp", "140249163  push rsi", "140249164  push rdi", "140249165  push r14", "140249167  sub rsp,90h"];
        for (l, w) in text.lines().zip(want) {
            assert_eq!(l.trim_end(), w);
        }
        // 40 53 = push rbx (REX), 55 / 56 / 57, 41 56 = push r14, 48 81 EC 90 00 00 00 = sub rsp,90h: 2+1+1+1+2+7 bytes
        // and the next instruction starts at +0x0E (0x14024916E)
        assert_eq!(EQUIP_ITEM_PROLOGUE_2720.len(), 0x0E);
        assert!(text.lines().nth(6).is_some_and(|l| l.starts_with("14024916e")));
    }
}
