//! C6 part 16 (docs/C6_HOLD_DESIGN.md 16.2-16.7, work/c6/xbow8.md): the REAL crossbow carrier of MW2 mode.
//!
//! With `[coop] er_carrier = "crossbow"` (code default "off"; needs `er_stance = "crossbow"`), in third person with MW2
//! mode on, the active right-hand weapon is swapped for an owned vanilla crossbow through ER's OWN
//! `EquipGameData::EquipItem` (the inventory menu's call with the menu's arguments `(egd, slot, &handle, inv_index, 1, 1,
//! 0)`), called here from the mod's `ChrIns_PostPhysics` task (the game thread; never Present, a detour or the render
//! thread). ER's PlayerIns update then copies the equipment, raises its weapon-changed notify (HKS gets real crossbow
//! values: the genuine carry and aim) and syncs it to peers like a menu swap. The marker's hide row hides the crossbow as
//! it hid the sword; the MW2 gun is drawn on its weapon model.
//!
//! Safety:
//! - 2.7.2.0 only (`OwnAddrs::carrier`); the four functions' first bytes are pinned on the loaded image before the
//!   first call (`mw2er_core::carrier::validate`); any mismatch = `HOLD xbow off why=prologue fn=..` for the session.
//! - The journal `<root>\state\carrier.json` (the mod's own folder; never the game folder or %APPDATA%\EldenRing) is
//!   written and flushed BEFORE every EquipItem of a swap ([`write_journal`]); it is deleted only after a verified
//!   restore. A crash / quit / world leave with the carrier on = the next world enter restores (`why=journal`).
//! - Never a byte write into ChrAsm / EquipGameData for the swap, never `ChrAsm::SetSlot`; the grip probe's
//!   `arm_style` (er_stance.rs, P-X2) and, with `[coop] carrier_load` (16.16, code default false), the carrier slot's
//!   `bolt_loaded_states` byte ([`write_bolt_loaded`]: ER's reload TAE event 35 end state, 1 only on a settled
//!   two-handed carrier with no chord; back to 0 first at every exit / restore / grip change / chord) are the writes.
//! - No ER call while dead, loading, in a menu or while ER spawns the player (it waits). A slot that no longer holds our
//!   carrier (the user changed gear) is never touched (`HOLD xbow foreign`).
//! - ER never fires a bolt: RB never reaches ER in MW2 mode; the R3 melee pulse is off while the carrier is equipped
//!   (er_stance.rs); the chord's RB only while the crossbow is unloaded; any drop of the equipped bolt quantity =
//!   `HOLD xbow trip why=bolt_used`, restore at once, off for the session.

use std::path::PathBuf;
use std::sync::OnceLock;

use mw2er_core::carrier::{
    BoltTrip, CARRIER_2720, CarrierAddrs, DiskVerdict, GIVE_ID, GripBack, InvItem, JOURNAL_FILE, JOURNAL_TMP, JOURNAL_VERSION, Journal, LOAD_MAX_PER_SWAP, LOAD_VERIFY_S,
    LoadIn, LoadStep, Mode, PlanIn, RestoreKind, SaveId, Step, VERIFY_S, disk_verdict, dry_line, load_ok, load_step, pick, plan, restore_kind, right_slot, row_of, validate,
};

use crate::game::equip::{self, EquipRead};
use crate::log::{log_i, log_w};

/// `<root>\state` (set by the boot thread when the root is valid; None = `HOLD xbow off why=no_root`).
static STATE_DIR: OnceLock<PathBuf> = OnceLock::new();

/// Boot thread: the folder of the journal (the mod's own root only).
pub fn set_root(root: &std::path::Path) {
    let _ = STATE_DIR.set(root.join("state"));
}

fn journal_path() -> Option<PathBuf> {
    STATE_DIR.get().map(|d| d.join(JOURNAL_FILE))
}

/// Writes the journal atomically (tmp, flush, rename). Returns false on any error (the swap is then refused).
fn write_journal(j: &Journal) -> bool {
    let Some(dir) = STATE_DIR.get() else { return false };
    let tmp = dir.join(JOURNAL_TMP);
    let res = (|| -> std::io::Result<()> {
        use std::io::Write;
        std::fs::create_dir_all(dir)?;
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(j.to_json().as_bytes())?;
        f.sync_all()?;
        drop(f);
        std::fs::rename(&tmp, dir.join(JOURNAL_FILE))
    })();
    res.is_ok()
}

fn read_journal() -> Option<String> {
    std::fs::read_to_string(journal_path()?).ok()
}

fn delete_journal() -> bool {
    match journal_path() {
        Some(p) => std::fs::remove_file(&p).is_ok() || !p.exists(),
        None => false,
    }
}

/// Seamless Co-op's DLL is loaded: the co-op save (ER0000.mw2co2), else the solo one (ER0000.mw2er.sl2) [I: the
/// Seamless module name `ersc.dll`; a wrong answer only ever keeps a journal, never applies one to another save].
fn seamless_loaded() -> bool {
    // SAFETY: GetModuleHandleW only looks a loaded module up by name (no load, no reference taken).
    unsafe { windows::Win32::System::LibraryLoader::GetModuleHandleW(windows::core::w!("ersc.dll")) }.is_ok()
}

/// The loaded character's identity (review C6 part 16): save flavour, `GameMan.save_slot` (-1 = unreadable: such a
/// journal only matches another unreadable read) and the name. None = no name yet (no carrier step this tick).
fn save_id(base: usize, pgd: usize) -> Option<SaveId> {
    let character = equip::character_name(pgd).filter(|n| !n.is_empty())?;
    // SAFETY: non-faulting reads of the exe's GameMan pointer global and one integer field of it.
    let save_slot = unsafe {
        let gm = crate::game::read_ptr(base + CARRIER_2720.game_man);
        if crate::game::plausible(gm) { crate::game::read::<i32>(gm + std::mem::offset_of!(eldenring::cs::GameMan, save_slot)).unwrap_or(-1) } else { -1 }
    };
    Some(SaveId { save: if seamless_loaded() { "coop" } else { "solo" }.to_string(), save_slot, character })
}

type EquipItemFn = unsafe extern "C" fn(usize, i32, *const u32, i32, bool, bool, bool);
type AddItemFn = unsafe extern "C" fn(usize, *const u32, i32, bool, bool) -> i32;
type CreateFn = unsafe extern "C" fn(usize, *mut u32, *const u32) -> usize;
type FindFn = unsafe extern "C" fn(usize, *const u32) -> i32;

/// What the stance hands in every tick.
pub struct CarrierIn {
    pub now: f64,
    pub mode: Mode,
    pub give: bool,
    pub mode_on: bool,
    pub in_world: bool,
    pub fp: bool,
    pub ac130: bool,
    pub nuke: bool,
    pub dead: bool,
    pub settling: bool,
    pub menu: bool,
    pub busy: Option<&'static str>,
    /// Our PlayerIns (0 = none).
    pub player: usize,
    /// The stance is disabled for the session (stray bullet, refresh fail, ...): the carrier restores and stays off.
    pub stance_off: bool,
    /// The active right weapon is already a real ranged weapon (the stance's `skip reason=ranged`): nothing to swap.
    pub orig_ranged: bool,
    /// `probe_er_aim`: the P-X3 / P-X5 lines.
    pub probe: bool,
    /// 16.16 `[coop] carrier_load`: put the swapped crossbow in ER's loaded state once the grip is two-handed.
    pub load: bool,
}

/// The carrier as the stance sees it this tick.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CarrierView {
    /// Equipped and verified: the stance carries the real crossbow (no row patch, the aim without writes).
    pub on: bool,
    pub row: Option<u32>,
    /// Our crossbow is in the slot (verified or not): the chord's RB only while it is unloaded.
    pub equipped: bool,
    /// A swap / restore call is being verified: no chord.
    pub busy: bool,
    /// The grip before the swap: the stance's exit target.
    pub orig_arm: Option<u32>,
    /// `bolt_loaded_states[slot]` of the equipped crossbow (None = unreadable / not equipped).
    pub loaded: Option<bool>,
    /// The stance must not patch the sword now (`HOLD stance skip reason=carrier`).
    pub holds: bool,
    /// Review C6 part 16: (seq, grip) after a restore the stance's exit chords did not cover (a journal restore after a
    /// world leave / crash, a death): the stance chords back to it once per seq when idle.
    pub grip_back: Option<(u32, u32)>,
}

#[derive(Clone, Copy, Debug)]
struct Verify {
    t0: f64,
    slot: usize,
    /// The param row the PlayerIns ChrAsm must show in `slot`.
    row: u32,
    /// None = a swap, Some(why) = a restore.
    restore: Option<&'static str>,
}

#[derive(Default)]
pub struct Carrier {
    /// The pinned functions of this build (None = not checked yet).
    addrs: Option<Result<(usize, CarrierAddrs), &'static str>>,
    off_logged: Option<&'static str>,
    disabled: Option<&'static str>,
    /// Our live swap (journal on disk).
    live: Option<Journal>,
    /// A journal of this character found on disk at this world enter (crash / quit / the last world): restored first.
    /// Only ever set from the file for the loaded character ([`Carrier::check_disk`]).
    disk: Option<Journal>,
    /// Another character's / save's journal on disk (kept; no swap; the stance keeps the patched sword).
    foreign_disk: Option<String>,
    /// A restore found the original gone (`RestoreKind::Missing`): our crossbow stays in the slot (equipped: no R3 melee,
    /// no chord RB, the bolt tripwire), the journal stays on disk (retried at the next world enter), no rescan per tick.
    missing: Option<Journal>,
    /// The grip to bring back after a restore the stance's exit did not cover.
    grip: GripBack,
    id_logged: bool,
    disk_checked: bool,
    verify: Option<Verify>,
    last_change: f64,
    bolt: BoltTrip,
    dry_done: bool,
    skip_logged: Option<&'static str>,
    wait_logged: Option<&'static str>,
    missing_logged: bool,
    /// A restore call did not take: no further call this world (the journal stays; the next world enter retries).
    stuck: bool,
    holds: bool,
    in_world: bool,
    frame_logged: u32,
    /// 16.16: the loaded state we set (`carrier_load`).
    load: Load,
}

/// 16.16 (`[coop] carrier_load`): our write of the carrier slot's `bolt_loaded_states` byte.
#[derive(Clone, Copy, Debug, Default)]
struct Load {
    /// (slot, carrier gaitem handle) of a load of ours not undone yet.
    ours: Option<(usize, u32)>,
    /// The write being verified: (time, slot, loaded before on PlayerIns, before on EquipGameData, bolts before).
    verify: Option<(f64, usize, bool, bool, Option<u32>)>,
    /// Loads this swap (capped: `LOAD_MAX_PER_SWAP`).
    tries: u32,
    /// Since when EquipGameData's grip reads 3.
    arm3_since: Option<f64>,
    wait_logged: Option<&'static str>,
    cap_logged: bool,
}

impl Carrier {
    /// The carrier is equipped and verified (the stance then carries the real crossbow: no patch, aim without writes).
    pub fn on(&self) -> bool {
        self.live.is_some() && self.verify.is_none()
    }

    /// The carrier's param row while it is on.
    pub fn row(&self) -> Option<u32> {
        self.on().then(|| self.live.as_ref().map(|j| row_of(j.carrier_item_id))).flatten()
    }

    /// Our crossbow is (or may be) in the slot: a live swap (verified or being verified / restored), this character's
    /// journal not restored yet (the settling / menu wait after a world enter), or a restore that found the original
    /// gone. No R3 melee, no chord RB on a loaded crossbow, the bolt tripwire runs (review C6 part 16).
    pub fn equipped(&self) -> bool {
        self.live.is_some() || self.disk.is_some() || self.missing.is_some()
    }

    /// A swap / restore call is being verified: the stance sends no chord meanwhile.
    pub fn busy(&self) -> bool {
        self.verify.is_some()
    }

    /// The grip before the swap (the stance's exit target while the carrier is on).
    pub fn orig_arm(&self) -> Option<u32> {
        self.live.as_ref().map(|j| j.orig_arm)
    }

    /// The ChrAsm slot our crossbow is (or may be) in: the live swap's, else the pending / missing journal's.
    pub fn slot(&self) -> Option<usize> {
        self.live.as_ref().or(self.disk.as_ref()).or(self.missing.as_ref()).map(|j| j.slot as usize)
    }

    /// The stance must not patch the sword now: the carrier is wanted (crossbow mode, not refused / disabled / dry) and
    /// not on yet, or a restore is in flight (`HOLD stance skip reason=carrier`).
    pub fn holds_stance(&self, i: &CarrierIn) -> bool {
        let wanted = i.mode == Mode::Crossbow
            && self.disabled.is_none()
            && self.off_logged.is_none()
            && self.addrs.is_none_or(|a| a.is_ok())
            && i.mode_on
            && !i.fp
            && !i.orig_ranged;
        // a refused / skipped swap (none owned, layout) = the stance's patched sword as the fallback; so is another
        // character's journal on disk (no swap until that character restores it: review C6 part 16) and a restore that
        // found the original gone (`missing`: not held, never rescanned)
        self.verify.is_some() || self.disk.is_some() || (wanted && self.foreign_disk.is_none() && !self.on() && self.skip_logged.is_none())
    }

    fn off(&mut self, why: &'static str) {
        if self.off_logged != Some(why) {
            self.off_logged = Some(why);
            log_w!("HOLD", "xbow off why={why} (the carrier stays off for the session; the stance keeps the patched sword)");
        }
    }

    /// The pinned functions (checked once per session, on the first wanted call).
    fn addrs(&mut self) -> Result<(usize, CarrierAddrs), &'static str> {
        if let Some(a) = self.addrs {
            return a;
        }
        let r = (|| {
            let a = crate::version_gate::own().ok().and_then(|o| o.carrier).ok_or("version")?;
            let base = crate::game::c3_read::exe_base();
            if base == 0 {
                return Err("version");
            }
            let rd = |addr: usize, out: &mut [u8]| crate::game::read_bytes(addr, out);
            validate(&a, base, &rd).map_err(|f| match f {
                "equip_item" => "prologue fn=equip_item",
                "add_item" => "prologue fn=add_item",
                "create" => "prologue fn=create",
                _ => "prologue fn=find",
            })?;
            Ok((base, a))
        })();
        match r {
            Ok((base, a)) => log_i!(
                "HOLD",
                "xbow pins ok equip_item=0x{:x} add_item=0x{:x} create=0x{:x} find=0x{:x} (2.7.2.0, first bytes checked)",
                base + a.equip_item,
                base + a.add_item,
                base + a.create,
                base + a.find
            ),
            Err(w) => self.off(w),
        }
        self.addrs = Some(r);
        r
    }

    fn disable(&mut self, why: &'static str) {
        if self.disabled.is_none() {
            self.disabled = Some(why);
            log_w!("HOLD", "xbow disabled reason={why} (the original weapon is restored as soon as ER may be called; off for this session)");
        }
    }

    /// The world was left (loading / title): a live swap's journal stays on disk and is restored at the next enter of
    /// ITS character. Nothing is carried in memory (review C6 part 16: the next world may load another character or the
    /// other save; [`Carrier::check_disk`] re-derives everything from the file and the loaded character).
    fn world_left(&mut self) {
        if self.live.take().is_some() {
            log_i!("HOLD", "xbow restore why=world_leave ok=0 pending=journal (ER cannot be called while loading; the next world enter of this character restores)");
        }
        self.disk = None;
        self.missing = None;
        self.foreign_disk = None;
        self.grip.clear();
        self.verify = None;
        self.disk_checked = false;
        self.dry_done = false;
        self.stuck = false;
        self.bolt.reset();
    }

    /// Once per world enter: a journal on disk of this character (save flavour, save slot, name) is restored first;
    /// another's is kept and never applied. Only a journal parsed from the file for the loaded character is ever set.
    fn check_disk(&mut self, me: &SaveId) {
        if self.disk_checked {
            return;
        }
        self.disk_checked = true;
        self.disk = None;
        self.foreign_disk = None;
        match disk_verdict(read_journal().as_deref(), me) {
            DiskVerdict::Absent => {}
            DiskVerdict::Unreadable => log_w!("HOLD", "xbow journal unreadable (ignored, kept on disk for the log; never applied)"),
            DiskVerdict::Other(who) => {
                log_w!(
                    "HOLD",
                    "xbow journal other owner={who} me={} (kept; restored when that character loads; no swap meanwhile, the stance keeps the patched sword)",
                    me.text()
                );
                self.foreign_disk = Some(who);
            }
            DiskVerdict::Own(j) => {
                if self.live.as_ref().is_some_and(|l| *l == j) {
                    return;
                }
                log_i!("HOLD", "xbow journal found slot={} orig={} carrier={} arm={} (a swap of an earlier world / session: restore first)", j.slot, j.orig_item_id, j.carrier_item_id, j.orig_arm);
                self.disk = Some(j);
            }
        }
    }

    /// One tick on the game thread (the task, before the stance: the stance then reads [`Carrier::view`]). Also sets the
    /// carrier's own R3-melee block while the crossbow is in the slot.
    pub fn tick(&mut self, i: &CarrierIn) {
        self.tick_inner(i);
        self.holds = self.holds_stance(i);
        crate::pad::set_carrier_melee_off(self.equipped());
    }

    /// What the stance reads this tick.
    pub fn view(&self, player: usize) -> CarrierView {
        CarrierView {
            on: self.on(),
            row: self.row(),
            equipped: self.equipped(),
            busy: self.busy(),
            orig_arm: self.orig_arm(),
            loaded: if self.equipped() { self.loaded(player) } else { None },
            holds: self.holds,
            grip_back: self.grip.pending(),
        }
    }

    fn tick_inner(&mut self, i: &CarrierIn) {
        if i.in_world != self.in_world {
            self.in_world = i.in_world;
            if !i.in_world {
                // 16.16: our load back to 0 first (EquipGameData lives on; best effort: the journal restore's EquipItem
                // clears the slot's byte anyway)
                if self.load.ours.is_some()
                    && let Some(pgd) = equip::main_pgd(crate::game::c3_read::exe_base(), CARRIER_2720.game_data_man)
                    && let Some(r) = equip::read_equip(pgd)
                {
                    self.load_undo(i, &r, "world_leave");
                }
                self.load = Load::default();
                self.world_left();
            }
        }
        let flag = i.mode != Mode::Off;
        // inert: off, no journal on disk / live (one file probe per world enter only while a journal may exist)
        if !flag && self.live.is_none() && self.disk.is_none() && self.disk_checked {
            return;
        }
        if STATE_DIR.get().is_none() {
            if flag {
                self.off("no_root");
            }
            return;
        }
        if !i.in_world {
            return;
        }
        let base = crate::game::c3_read::exe_base();
        let Some(pgd) = equip::main_pgd(base, CARRIER_2720.game_data_man) else { return };
        // review C6 part 16: the journal belongs to a save flavour + save slot + name; no step without it
        let Some(me) = save_id(base, pgd) else { return };
        if !self.id_logged {
            self.id_logged = true;
            log_i!("HOLD", "xbow identity {} (the journal is applied only to this save / slot / character)", me.text());
        }
        self.check_disk(&me);
        if !flag && !self.equipped() {
            return;
        }
        // review C6 part 16: the stance is off for the session (stray bullet, refresh fail, panic): no new swap at all
        // (its hide row and guards are gone), a live one is restored
        if i.stance_off {
            self.disable("stance_off");
        }
        let Some(r) = equip::read_equip(pgd) else { return };
        // 16.16: the loaded state first (undone before any restore's EquipItem below and before the stance's chord)
        self.load_tick(i, &r);
        if self.stuck {
            return;
        }
        // a swap / restore in flight: verify on the PlayerIns ChrAsm
        if let Some(v) = self.verify {
            self.verify_step(i, &r, v);
            return;
        }
        // the grip came back (or our crossbow is in the slot again): nothing pending for the stance / the next swap
        if self.live.is_none() {
            self.grip.note_arm(r.arm_style);
        }
        // the bolt tripwire (16.4 step 5), also while our crossbow may be in the slot from a journal not yet restored
        if self.equipped()
            && let Some((a, b)) = self.bolt.note(equip::bolt_qty(&r))
        {
            log_w!("HOLD", "xbow trip why=bolt_used qty={a}->{b} (a bolt was used: restore now, the carrier off for the session)");
            self.disable("bolt_used");
        }
        let step = plan(&PlanIn {
            flag,
            disabled: self.disabled.is_some() || self.off_logged.is_some(),
            mode_on: i.mode_on,
            in_world: i.in_world,
            fp: i.fp,
            ac130: i.ac130,
            nuke: i.nuke,
            dead: i.dead,
            settling: i.settling,
            menu: i.menu,
            busy: i.busy,
            swapped: self.live.is_some(),
            journal: self.disk.is_some(),
            foreign_journal: self.foreign_disk.is_some(),
            since_change_s: i.now - self.last_change,
        });
        match step {
            Step::Idle => {
                self.wait_logged = None;
                if self.live.is_none() && (!i.mode_on || i.fp) {
                    // a skip (none owned / layout / ranged) is decided anew at the next mode on (one inventory scan)
                    self.dry_done = false;
                    self.skip_logged = None;
                }
            }
            Step::Wait(w) => {
                let wanted = i.mode == Mode::Crossbow && i.mode_on && self.skip_logged.is_none();
                if self.wait_logged != Some(w) && (self.live.is_some() || self.disk.is_some() || wanted) {
                    self.wait_logged = Some(w);
                    log_i!("HOLD", "xbow wait reason={w} (no ER call now; retried every tick)");
                }
            }
            Step::Swap => {
                self.wait_logged = None;
                // a skip (none owned / layout / ranged) holds until the next mode on: no inventory scan per tick
                if self.skip_logged.is_none() {
                    self.swap(i, &r, &me);
                }
            }
            Step::Restore(why) => {
                self.wait_logged = None;
                self.restore(i, &r, why);
            }
        }
    }

    fn skip(&mut self, why: &'static str) {
        if self.skip_logged != Some(why) {
            self.skip_logged = Some(why);
            log_i!("HOLD", "xbow skip reason={why} (the stance keeps the patched sword)");
        }
    }

    /// 16.3 steps 2-5.
    fn swap(&mut self, i: &CarrierIn, r: &EquipRead, me: &SaveId) {
        if i.mode == Mode::Dry && self.dry_done {
            return;
        }
        let Some(s) = right_slot(r.right_sel) else { return };
        // the slot's own entry must be the equipped item (layout / index pin before anything is called)
        let Some(orig) = equip::entry(r, r.idx[s]).filter(|e| e.handle == r.handles[s] && e.item_id as i32 == r.ids[s]) else {
            self.skip("layout");
            if i.mode == Mode::Dry {
                self.dry_done = true;
            }
            return;
        };
        // a real ranged weapon carries as it is (the stance skips it as ranged: nothing to do)
        if i.orig_ranged {
            self.skip("ranged");
            self.dry_done = true;
            return;
        }
        let inv = equip::scan_normal(r).unwrap_or_default();
        let mut got = pick(&inv, &r.handles);
        let bolts = [r.ids[mw2er_core::carrier::SLOT_BOLT1], r.ids[mw2er_core::carrier::SLOT_BOLT2]];
        let jpath = journal_path().map_or("?".to_string(), |p| p.display().to_string());
        if i.mode == Mode::Dry {
            self.dry_done = true;
            log_i!("HOLD", "{} n_inv={} give={}", dry_line(s, &orig, r.arm_style, got.as_ref(), bolts, &jpath), inv.len(), i.give as u8);
            // the pins are checked in dry mode too (no equip call); 2026-10-22 (user): with carrier_give the dry run also
            // adds the Light Crossbow to the mod save, so probe 0 (a vanilla crossbow aim, MW2 off) can be run; no swap
            if let Ok((base, a)) = self.addrs()
                && got.is_none()
                && i.give
            {
                let _ = give(base, &a, r);
            }
            return;
        }
        let Ok((base, a)) = self.addrs() else { return };
        let mut given = Vec::new();
        if got.is_none() {
            if !i.give {
                self.skip("none_owned");
                return;
            }
            match give(base, &a, r) {
                Some(e) => {
                    given.push(GIVE_ID);
                    got = Some(e);
                }
                None => {
                    self.disable("give");
                    return;
                }
            }
        }
        let Some(c) = got else { return };
        // review C6 part 16: a grip not back yet after a restore (a journal restore leaves 1) is the player's, not 1
        let orig_arm = self.grip.orig_arm_for_swap(r.arm_style);
        let j = Journal {
            version: JOURNAL_VERSION,
            character: me.character.clone(),
            save: me.save.clone(),
            save_slot: me.save_slot,
            slot: s as u32,
            orig_item_id: orig.item_id,
            orig_handle: orig.handle,
            orig_index: orig.index,
            orig_arm,
            bolts,
            carrier_item_id: c.item_id,
            carrier_handle: c.handle,
            carrier_index: c.index,
            given,
            t_unix: std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs()),
        };
        // 16.3 step 4: the journal is on disk BEFORE the call
        if !write_journal(&j) {
            log_w!("HOLD", "xbow refused reason=journal path={jpath} (no swap without the journal)");
            self.disable("journal");
            return;
        }
        if i.probe {
            self.parts_line(i.player, "before");
            self.frame_line(i.player, "sword");
        }
        // 16.3 step 5: ER's own menu equip, on the game thread
        // SAFETY: the pinned EquipItem (first bytes checked this session, 2.7.2.0) on the main player's live EquipGameData,
        // called from the ChrIns_PostPhysics task (the game thread, as the inventory menu does), with the menu's arguments:
        // the carrier's own gaitem handle and inventory index read from that inventory this tick.
        unsafe { call_equip(base, &a, r.egd, s, c.handle, c.index) };
        log_i!(
            "HOLD",
            "xbow call slot={s} orig={} carrier={} handle=0x{:08x} idx={} arm={} orig_arm={orig_arm} (EquipItem, menu args 1,1,0; verifying)",
            orig.item_id,
            c.item_id,
            c.handle,
            c.index,
            r.arm_style
        );
        self.live = Some(j);
        self.verify = Some(Verify { t0: i.now, slot: s, row: row_of(c.item_id), restore: None });
        self.last_change = i.now;
        self.bolt.reset();
        self.skip_logged = None;
        // 16.16: a new swap may load again
        self.load = Load::default();
    }

    /// 16.5: the original back (or the journal's verdict).
    fn restore(&mut self, i: &CarrierIn, r: &EquipRead, why: &'static str) {
        let from_disk = self.live.is_none();
        let Some(j) = self.live.clone().or_else(|| self.disk.clone()) else { return };
        let s = j.slot as usize;
        if s >= 22 {
            return;
        }
        let slot_handle = r.handles[s];
        // the original's inventory index (re-found by its handle when the index moved)
        let mut idx = j.orig_index;
        let mut present = equip::entry(r, idx).is_some_and(|e| e.handle == j.orig_handle);
        if !present
            && let Some(e) = equip::scan_normal(r).unwrap_or_default().into_iter().find(|e| e.handle == j.orig_handle)
        {
            idx = e.index;
            present = true;
        }
        match restore_kind(&j, slot_handle, present) {
            RestoreKind::Done => {
                log_i!("HOLD", "xbow restore why={why} ok=1 ms=0 already=1 (the original is in slot {s})");
                self.grip.restored(j.orig_arm);
                self.finish_restore();
            }
            RestoreKind::Foreign => {
                log_w!("HOLD", "xbow foreign slot={s} now=0x{slot_handle:08x} carrier=0x{:08x} (the gear changed meanwhile: nothing touched, journal dropped)", j.carrier_handle);
                self.finish_restore();
            }
            RestoreKind::Missing => {
                if !self.missing_logged {
                    self.missing_logged = true;
                    log_w!("HOLD", "xbow restore_missing orig={} handle=0x{:08x} (the original is no longer in the inventory: the crossbow stays, journal kept)", j.orig_item_id, j.orig_handle);
                    crate::shared::toast("mw2er: your original weapon was not found - the crossbow stays equipped (see logs\\mw2er.log)", 10.0, true);
                }
                self.disable("restore_missing");
                // review C6 part 16: the crossbow stays in the slot (equipped: guards on), the journal stays on disk (the
                // next world enter retries once); not live / pending any more: no restore per tick, the stance not held
                self.missing = Some(j);
                self.live = None;
                self.disk = None;
            }
            RestoreKind::Equip => {
                let Ok((base, a)) = self.addrs() else { return };
                // 16.16: unloaded before the original comes back (EquipItem clears the slot's byte too [V 0x1403BF47A])
                self.load_undo(i, r, why);
                // SAFETY: as in `swap`: the pinned EquipItem on the main player's EquipGameData, on the game thread, with the
                // original's own handle and its inventory index read this tick.
                unsafe { call_equip(base, &a, r.egd, s, j.orig_handle, idx) };
                log_i!("HOLD", "xbow restore call why={why} slot={s} orig={} idx={idx} from_disk={} (verifying)", j.orig_item_id, from_disk as u8);
                if self.live.is_none() {
                    self.live = Some(j.clone());
                    self.disk = None;
                }
                self.verify = Some(Verify { t0: i.now, slot: s, row: row_of(j.orig_item_id), restore: Some(why) });
                self.last_change = i.now;
            }
        }
    }

    fn finish_restore(&mut self) {
        if !delete_journal() {
            log_w!("HOLD", "xbow journal delete failed (it is ignored once the slot holds the original: `already=1`)");
        }
        self.live = None;
        self.disk = None;
        self.verify = None;
        self.bolt.reset();
    }

    fn verify_step(&mut self, i: &CarrierIn, r: &EquipRead, v: Verify) {
        // SAFETY: our PlayerIns this tick (or 0); non-faulting reads.
        let now_row = unsafe { equip::player_slot(i.player, v.slot) }.map(|x| row_of(x.0.max(0) as u32));
        let ms = ((i.now - v.t0) * 1000.0) as u32;
        let ok = now_row == Some(v.row) && r.ids[v.slot] > 0 && row_of(r.ids[v.slot] as u32) == v.row;
        if !ok && i.now - v.t0 < VERIFY_S {
            return;
        }
        self.verify = None;
        match (v.restore, ok) {
            (None, true) => {
                let j = self.live.as_ref();
                log_i!(
                    "HOLD",
                    "xbow on slot={} orig={} carrier={} ms={ms} (ER's real crossbow: its carry and aim; hidden by the marker row)",
                    v.slot,
                    j.map_or(0, |j| j.orig_item_id),
                    j.map_or(0, |j| j.carrier_item_id)
                );
                self.bolt.reset();
                let _ = self.bolt.note(equip::bolt_qty(r));
                if i.probe {
                    self.parts_line(i.player, "after");
                    self.frame_line(i.player, "xbow");
                }
            }
            (None, false) => {
                log_w!("HOLD", "xbow refused reason=verify ms={ms} row_now={} (ER did not take the crossbow: restored, off for the session)", now_row.map_or(-1, |x| x as i64));
                self.disable("verify");
            }
            (Some(why), true) => {
                log_i!("HOLD", "xbow restore why={why} ok=1 ms={ms}");
                // review C6 part 16: the grip before the swap back (the stance chords to it when its own exit did not)
                if let Some(a) = self.live.as_ref().map(|j| j.orig_arm) {
                    self.grip.restored(a);
                }
                self.finish_restore();
            }
            (Some(why), false) => {
                log_w!("HOLD", "xbow restore why={why} ok=0 ms={ms} row_now={} (journal kept: retried at the next world enter)", now_row.map_or(-1, |x| x as i64));
                // the journal stays on disk and live stays (the crossbow may still be in the slot: no melee, no chord RB);
                // no further call this world
                self.stuck = true;
                self.disable("restore");
            }
        }
    }

    /// 16.16 (`[coop] carrier_load`): one tick of the loaded state. ER's own load is its reload animation's TAE event 35
    /// (0x14042C280: `ChrAsm::SetBoltLoaded(PlayerIns asm, right hand, 1)` 0x1403BF4B0, then PlayerIns -> EquipGameData
    /// 0x140659FC0); it needs an R1 that never reaches ER in MW2 mode, so the same end state is written instead
    /// ([`write_bolt_loaded`]): EquipGameData's byte (the master: the PlayerIns update copies it every frame,
    /// 0x140661543) and PlayerIns's (this tick's readers: the stance's chord gate sees it at once). No bolt is used.
    fn load_tick(&mut self, i: &CarrierIn, r: &EquipRead) {
        let s = self.slot().filter(|s| *s < 12);
        if r.arm_style == 3 {
            self.load.arm3_since.get_or_insert(i.now);
        } else {
            self.load.arm3_since = None;
        }
        let egd = s.map(|s| r.bolt_loaded[s] != 0);
        let pl = s.and_then(|_| self.loaded(i.player));
        let loaded = match (egd, pl) {
            (Some(true), _) | (_, Some(true)) => Some(true),
            (Some(false), Some(false)) => Some(false),
            _ => None,
        };
        let bolts = equip::bolt_qty(r);
        let txt = |x: Option<bool>| x.map_or("?".to_string(), |x| (x as u8).to_string());
        let num = |x: Option<u32>| x.map_or("?".to_string(), |x| x.to_string());
        let li = LoadIn {
            flag: i.load && i.mode == Mode::Crossbow,
            disabled: self.disabled.is_some() || self.off_logged.is_some(),
            on: self.on(),
            in_world: i.in_world,
            mode_on: i.mode_on,
            fp: i.fp,
            ac130: i.ac130,
            nuke: i.nuke,
            dead: i.dead,
            arm: r.arm_style,
            arm3_s: self.load.arm3_since.map_or(0.0, |t| i.now - t),
            busy: i.busy,
            chord_keys: crate::shared::KB_CHORD_T0.load(std::sync::atomic::Ordering::Relaxed) != 0,
            loaded,
            bolts,
            ours: self.load.ours.is_some(),
            tries: self.load.tries,
        };
        let step = load_step(&li);
        if let LoadStep::Undo(why) = step {
            if let Some((t0, vs, pb, eb, bb)) = self.load.verify.take() {
                log_i!(
                    "HOLD",
                    "xbow load method=write slot={vs} ok=- loaded={}->{} egd={}->{} bolts={}->{} ms={} why={why} (undone before its verdict)",
                    pb as u8,
                    txt(pl),
                    eb as u8,
                    txt(egd),
                    num(bb),
                    num(bolts),
                    ((i.now - t0) * 1000.0) as u32
                );
            }
            self.load_undo(i, r, why);
            return;
        }
        if let Some((t0, vs, pb, eb, bb)) = self.load.verify {
            if i.now - t0 < LOAD_VERIFY_S {
                return;
            }
            self.load.verify = None;
            let ok = load_ok(egd, pl);
            log_i!(
                "HOLD",
                "xbow load method=write slot={vs} ok={} loaded={}->{} egd={}->{} bolts={}->{} ms={} arm={} ({})",
                ok as u8,
                pb as u8,
                txt(pl),
                eb as u8,
                txt(egd),
                num(bb),
                num(bolts),
                ((i.now - t0) * 1000.0) as u32,
                r.arm_style,
                if ok { "ER keeps the crossbow loaded: the next aims log loaded=1" } else { "ER did not keep it: undone, no more loads this swap" }
            );
            if !ok {
                self.load.tries = LOAD_MAX_PER_SWAP;
                self.load_undo(i, r, "verify");
            }
            return;
        }
        // ours, but ER reads it unloaded now (a shot's TAE event 34 or an equip): forget it (the bolt tripwire judges)
        if self.load.ours.is_some() && loaded == Some(false) {
            log_w!("HOLD", "xbow load lost loaded=1->0 bolts={} (ER unloaded the crossbow itself; not loaded again this swap)", num(bolts));
            self.load.ours = None;
            self.load.tries = LOAD_MAX_PER_SWAP;
            return;
        }
        match step {
            LoadStep::Wait(w) => {
                if self.load.wait_logged != Some(w) {
                    self.load.wait_logged = Some(w);
                    log_i!("HOLD", "xbow load wait reason={w} arm={} loaded={} bolts={} (16.16: no load now; retried every tick)", r.arm_style, txt(loaded), num(bolts));
                }
            }
            LoadStep::Load => self.load_write(i, r, pl.unwrap_or(false), egd.unwrap_or(false), bolts),
            LoadStep::Idle | LoadStep::Undo(_) => {
                if li.flag && li.on && !li.ours && self.load.tries >= LOAD_MAX_PER_SWAP && !self.load.cap_logged {
                    self.load.cap_logged = true;
                    log_i!("HOLD", "xbow load cap tries={} (no more loads this swap)", self.load.tries);
                }
            }
        }
    }

    /// 16.16: the load (1) on the live carrier's slot, only while the slot (both ChrAsms) still holds our crossbow.
    fn load_write(&mut self, i: &CarrierIn, r: &EquipRead, pl_before: bool, egd_before: bool, bolts: Option<u32>) {
        let Some(j) = self.live.as_ref() else { return };
        let s = j.slot as usize;
        let h = j.carrier_handle;
        // SAFETY: our PlayerIns this tick (or 0); non-faulting reads.
        let pl_slot = unsafe { equip::player_slot(i.player, s) };
        if s >= 12 || r.handles[s] != h || pl_slot.is_none_or(|x| x.1 != h) {
            if self.load.wait_logged != Some("slot") {
                self.load.wait_logged = Some("slot");
                log_w!(
                    "HOLD",
                    "xbow load wait reason=slot slot={s} egd=0x{:08x} carrier=0x{h:08x} (the slot does not hold our crossbow on both ChrAsms: no write)",
                    r.handles.get(s).copied().unwrap_or(0)
                );
            }
            return;
        }
        self.load.tries += 1;
        let (e, p) = write_bolt_loaded(r.egd, i.player, s, 1);
        log_i!(
            "HOLD",
            "xbow load write slot={s} handle=0x{h:08x} arm={} loaded={} egd={} bolts={} try={}/{LOAD_MAX_PER_SWAP} wrote_egd={} wrote_pl={} (ER's reload TAE event 35 end state: bolt_loaded_states[{s}]=1; no bolt used; verifying)",
            r.arm_style,
            pl_before as u8,
            egd_before as u8,
            bolts.map_or("?".to_string(), |x| x.to_string()),
            self.load.tries,
            e as u8,
            p as u8
        );
        if e || p {
            self.load.ours = Some((s, h));
            self.load.verify = Some((i.now, s, pl_before, egd_before, bolts));
        } else {
            log_w!("HOLD", "xbow load method=write slot={s} ok=0 why=write_refused (game::write refused both bytes; no more loads this swap)");
            self.load.tries = LOAD_MAX_PER_SWAP;
        }
    }

    /// 16.16: our load back to 0 (only while the slot still holds the crossbow we loaded; an equip has cleared it anyway).
    fn load_undo(&mut self, i: &CarrierIn, r: &EquipRead, why: &'static str) {
        let Some((s, h)) = self.load.ours.take() else { return };
        self.load.verify = None;
        if s >= 12 || r.handles[s] != h {
            log_i!("HOLD", "xbow load undo why={why} slot={s} ok=1 already=1 (the slot holds another item: ER's equip cleared the byte)");
            return;
        }
        let before = r.bolt_loaded[s];
        let (e, p) = write_bolt_loaded(r.egd, i.player, s, 0);
        log_i!("HOLD", "xbow load undo why={why} slot={s} ok={} egd={before}->0 wrote_egd={} wrote_pl={} (our load restored: unloaded)", e as u8, e as u8, p as u8);
    }

    /// P-X3 (`probe_er_aim`): the non-null parts-model slots of our CSChrAsmModelIns (which slot draws the loaded bolt).
    fn parts_line(&self, player: usize, phase: &str) {
        let mut v = Vec::new();
        for k in 0..crate::game::chars::ASM_MODEL_SLOTS {
            // SAFETY: our PlayerIns this tick; non-faulting reads only (`Err` = empty slot / unreadable).
            if let Ok(w) = unsafe { crate::game::chars::read_weapon_model(player, k) } {
                v.push(format!("{k}:0x{:x}", w.parts));
            }
        }
        log_i!("HOLD", "xbow parts phase={phase} slots=[{}] hidden=? (P-X3: the loaded-bolt model's parts slot)", v.join(","));
    }

    /// P-X5 (`probe_er_aim`): ER's right weapon model frame (FLVER bone 0 entry and the location matrix), once per phase
    /// per swap, so the crossbow's weapon-frame axes / grip can be derived offline (`weapon_axes_xbow`).
    fn frame_line(&mut self, player: usize, phase: &str) {
        if self.frame_logged >= 16 {
            return;
        }
        // SAFETY: our PlayerIns this tick; non-faulting reads only.
        if let Ok(w) = unsafe { crate::game::chars::read_weapon_model(player, crate::game::chars::ASM_SLOT_RIGHT0) } {
            self.frame_logged += 1;
            let f = |v: &[f32]| v.iter().map(|x| format!("{x:.4}")).collect::<Vec<_>>().join(",");
            log_i!(
                "HOLD",
                "xbow frame phase={phase} e0=[{}] root=[{}] n={} (P-X5: the weapon frame for weapon_axes_xbow)",
                w.e0.map_or("?".to_string(), |e| f(&e)),
                w.root.map_or("?".to_string(), |m| f(&m)),
                w.n
            );
        }
    }

    /// The bolt slot state of the carrier's slot (`bolt_loaded_states[s]`), None = not on / unreadable.
    pub fn loaded(&self, player: usize) -> Option<bool> {
        let s = self.slot()?;
        // SAFETY: our PlayerIns this tick; non-faulting reads.
        unsafe { equip::player_slot(player, s) }.map(|x| x.2)
    }
}

/// 16.16 (`[coop] carrier_load`): the carrier slot's `bolt_loaded_states[s]` = `v` in EquipGameData's ChrAsm (ER's
/// master copy; the PlayerIns update copies it into PlayerIns's every frame [V 0x140661543 `ChrAsm::Copy`]) and in
/// PlayerIns's ChrAsm (what ER's TAE event 35 writes first and this tick's readers see). The only bytes the carrier
/// writes; 1 = ER's loaded state (the next R1 would shoot instead of reload: R1 never reaches ER in MW2 mode, the
/// chord refuses a loaded crossbow), 0 = ER's state after an equip. Returns (EquipGameData written, PlayerIns written).
fn write_bolt_loaded(egd: usize, player: usize, s: usize, v: u8) -> (bool, bool) {
    if s >= 12 || v > 1 {
        return (false, false);
    }
    // SAFETY: one byte of the main player's live EquipGameData ChrAsm `bolt_loaded_states` (heap; offsets pinned by the
    // const asserts in equip.rs); game::write checks the page, never faults and is version-gated; the game thread.
    let e = unsafe { crate::game::write::<u8>(egd + equip::EGD_CHR_ASM + equip::ASM_BOLT_LOADED + s, v) };
    // SAFETY: our PlayerIns this tick (or 0); the same byte of its ChrAsm (heap), through game::write as above.
    let p = unsafe { equip::player_asm(player) }.is_some_and(|asm| unsafe { crate::game::write::<u8>(asm + equip::ASM_BOLT_LOADED + s, v) });
    (e, p)
}

/// ER's EquipItem with the inventory menu's arguments.
///
/// # Safety
/// `a` must be validated on this image (base) this session; `egd` the main player's live EquipGameData; the game thread.
unsafe fn call_equip(base: usize, a: &CarrierAddrs, egd: usize, slot: usize, handle: u32, idx: i32) {
    // SAFETY: the caller's contract: the address holds EquipItem's pinned first bytes (2.7.2.0).
    let f: EquipItemFn = unsafe { std::mem::transmute::<usize, EquipItemFn>(base + a.equip_item) };
    let h = handle;
    // SAFETY: as above; `h` lives across the call.
    unsafe { f(egd, slot as i32, &h, idx, true, true, false) };
}

/// X4 (`carrier_give`): ER's own starting-gear recipe in the MOD save: Find, else CSGaitemImp::Create + AddItem (the
/// result at `+0x3FC` checked). A vanilla Light Crossbow only. Returns its inventory entry.
fn give(base: usize, a: &CarrierAddrs, r: &EquipRead) -> Option<InvItem> {
    let id = GIVE_ID;
    // SAFETY: the pinned Find (validated with the other three) on the main player's EquipInventoryData.
    let found = unsafe {
        let f: FindFn = std::mem::transmute::<usize, FindFn>(base + a.find);
        f(r.egd + equip::EGD_INVENTORY, &id)
    };
    if found >= 0 {
        log_i!("HOLD", "xbow give id={id} idx={found} result=owned (already in the inventory: equipped elsewhere?)");
        return None;
    }
    // SAFETY: a non-faulting read of the CSGaitemImp pointer global.
    let imp = unsafe { crate::game::read_ptr(base + a.gaitem_imp) };
    if !crate::game::plausible(imp) {
        return None;
    }
    let mut h: u32 = 0;
    // SAFETY: the pinned Create on ER's own CSGaitemImp (the starting-gear path), on the game thread; `h` is ours.
    unsafe {
        let c: CreateFn = std::mem::transmute::<usize, CreateFn>(base + a.create);
        c(imp, &mut h, &id);
    }
    if h == 0 || h == u32::MAX {
        log_w!("HOLD", "xbow give id={id} idx=-1 result=create_failed");
        return None;
    }
    // SAFETY: the pinned AddItem on the main player's EquipGameData (qty 1, the starting-gear flags).
    let idx = unsafe {
        let f: AddItemFn = std::mem::transmute::<usize, AddItemFn>(base + a.add_item);
        f(r.egd, &h, 1, true, true)
    };
    // SAFETY: a non-faulting read of the add result.
    let res = unsafe { crate::game::read::<u32>(r.egd + equip::EGD_LAST_ADD) }.unwrap_or(u32::MAX);
    log_i!("HOLD", "xbow give id={id} idx={idx} result={res} handle=0x{h:08x} (a vanilla Light Crossbow into the mod save; it stays there)");
    (res == 0 && idx >= 0).then_some(InvItem { index: idx, handle: h, item_id: id, qty: 1 })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// C6 part 16.5: the journal file is written atomically (tmp, flush, rename), read back exactly, a torn tmp file left
    /// by a crash is never read, and the delete removes it (a temp folder: never the game folder).
    /// The tests share one journal file (the root is set once per process): one at a time.
    static FILE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn test_root() -> PathBuf {
        let root = std::env::temp_dir().join(format!("mw2er_carrier_test_{}", std::process::id()));
        set_root(&root);
        root
    }

    fn journal_a() -> Journal {
        Journal {
            version: JOURNAL_VERSION,
            character: "Tarnished".into(),
            save: "solo".into(),
            save_slot: 0,
            slot: 3,
            orig_item_id: 2_000_000,
            orig_handle: 0x8080_0001,
            orig_index: 410,
            orig_arm: 3,
            bolts: [52_000_000, -1],
            carrier_item_id: 43_020_000,
            carrier_handle: 0x8080_0004,
            carrier_index: 413,
            given: vec![],
            t_unix: 1,
        }
    }

    /// Review C6 part 16: quit to the title with a live swap of A, enter as B (another name, or A's copy in the other
    /// save): nothing of A's journal is carried in memory, no restore runs, the file is kept, the carrier is equipped
    /// for nobody and does not hold the stance (the patched sword is the fallback). Entering as A again restores.
    #[test]
    fn c6_carrier_world_leave_other_character() {
        let _g = FILE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let root = test_root();
        let j = journal_a();
        assert!(write_journal(&j));
        let mut c = Carrier { live: Some(j.clone()), in_world: true, disk_checked: true, ..Default::default() };
        c.world_left();
        assert!(c.disk.is_none() && c.live.is_none(), "nothing carried across the world leave");
        let ci = CarrierIn {
            now: 10.0,
            mode: Mode::Crossbow,
            give: false,
            mode_on: true,
            in_world: true,
            fp: false,
            ac130: false,
            nuke: false,
            dead: false,
            settling: false,
            menu: false,
            busy: None,
            player: 0,
            stance_off: false,
            orig_ranged: false,
            probe: false,
            load: false,
        };
        for b in [
            SaveId { save: "solo".into(), save_slot: 0, character: "Other".into() },
            SaveId { save: "coop".into(), save_slot: 0, character: "Tarnished".into() },
        ] {
            c.disk_checked = false;
            c.check_disk(&b);
            assert!(c.disk.is_none() && c.foreign_disk.is_some(), "{b:?}");
            assert!(!c.equipped() && !c.holds_stance(&ci), "{b:?}: no guard of another's crossbow, the stance not held");
            let p = PlanIn { flag: true, mode_on: true, in_world: true, since_change_s: 10.0, journal: c.disk.is_some(), foreign_journal: true, ..Default::default() };
            assert_eq!(plan(&p), Step::Wait("journal_other"), "no restore of A's journal against B, no swap");
            assert_eq!(read_journal().and_then(|t| Journal::parse(&t)), Some(j.clone()), "the file is kept");
        }
        // A again: restored first, guarded meanwhile
        c.disk_checked = false;
        c.check_disk(&SaveId { save: "solo".into(), save_slot: 0, character: "Tarnished".into() });
        assert_eq!(c.disk, Some(j.clone()));
        assert!(c.equipped() && c.holds_stance(&ci) && c.slot() == Some(3));
        assert!(delete_journal());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn c6_carrier_journal_file_atomic() {
        let _g = FILE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let root = test_root();
        let dir = root.join("state");
        let j = Journal {
            version: JOURNAL_VERSION,
            character: "Tarnished".into(),
            save: "solo".into(),
            save_slot: 0,
            slot: 3,
            orig_item_id: 2_000_000,
            orig_handle: 0x8080_0001,
            orig_index: 410,
            orig_arm: 3,
            bolts: [52_000_000, -1],
            carrier_item_id: 43_020_000,
            carrier_handle: 0x8080_0004,
            carrier_index: 413,
            given: vec![],
            t_unix: 1,
        };
        assert!(write_journal(&j));
        assert_eq!(read_journal().and_then(|t| Journal::parse(&t)), Some(j.clone()));
        // a crash between the tmp write and the rename leaves a torn tmp: the committed journal is what is read
        std::fs::write(dir.join(JOURNAL_TMP), "{\"version\": 1, \"char").unwrap();
        assert_eq!(read_journal().and_then(|t| Journal::parse(&t)), Some(j.clone()));
        // a rewrite replaces it whole
        let j2 = Journal { slot: 5, ..j };
        assert!(write_journal(&j2));
        assert_eq!(read_journal().and_then(|t| Journal::parse(&t)).map(|x| x.slot), Some(5));
        assert!(delete_journal() && read_journal().is_none() && delete_journal(), "deleted; a second delete is fine");
        let _ = std::fs::remove_dir_all(&root);
    }
}
