//! Save/restore models of every field first person changes (docs/M3B_DESIGN.md section 5.2). Pure: the DLL
//! reads the current value, asks the model what to write, writes it and logs it.
//!
//! | Field | Model | Restore |
//! |---|---|---|
//! | `pers_cam_1` row 3 / rows 0..2 / fov | [`PersistWatch`] | nothing if the engine was seen rewriting it, else ER's last value, once |
//! | `ChrIns.chr_flags1c5` bit 3 | [`RenderBit`] | only bit 3, from the save, only on the same character |
//! | `ChrIns.base_transparency_modifier` | [`FadeHide`] / [`FadeRestoring`] | +1 until visible (max 2 s), then the saved value |
//! | `CSChrPhysicsModule.orientation` (+ interpolated) | [`YawRelease`] | release: nothing is written back |
//! | `ChrCam.pers_cam` row 3 (hook, after run 5) | [`hook_row_restore`] | ER's pre-write value, only if the row still holds our eye |
//! | `ChrExFollowCam.camera_auto_rotation`, exe global `CAM_WALL_RECOVERY` | [`ClearedByte`] | the saved byte, only if the field still holds our 0 |

/// What `PersistWatch::observe` saw at the start of a tick.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Observation {
    /// We wrote nothing last tick.
    NoWrite,
    /// The value differs from our last write: the engine rewrote the field (the free restore holds).
    Rewritten,
    /// The value is still exactly ours: the write persists (it would poison the next read).
    Persisted,
}

/// A field the engine is expected to rebuild every frame (`CSCamera.pers_cam_1` matrix rows and fov).
///
/// Per tick: `observe(current)` BEFORE writing, then `wrote(value)`. On handback, `stop()` says whether a value
/// must be written back: `None` once the engine was seen rewriting the field, else ER's last value.
#[derive(Clone, Copy, Debug)]
pub struct PersistWatch<T> {
    last_written: Option<T>,
    last_er: Option<T>,
    rewrite_seen: bool,
    persist_seen: bool,
    /// Consecutive `Persisted` observations (0 after any other observation).
    persist_streak: u32,
}

/// Consecutive persisted samples before the DLL concludes ER does not rebuild the field (review M3b minor 6: one
/// frame where ER's camera step did not run must not disable first person for the session).
pub const PERSIST_STREAK_DISABLE: u32 = 3;

impl<T> Default for PersistWatch<T> {
    fn default() -> Self {
        PersistWatch { last_written: None, last_er: None, rewrite_seen: false, persist_seen: false, persist_streak: 0 }
    }
}

impl<T: Copy + PartialEq> PersistWatch<T> {
    pub fn observe(&mut self, current: T) -> Observation {
        match self.last_written {
            Some(w) if current == w => {
                self.persist_seen = true;
                self.persist_streak += 1;
                Observation::Persisted
            }
            Some(_) => {
                self.rewrite_seen = true;
                self.persist_streak = 0;
                self.last_er = Some(current);
                Observation::Rewritten
            }
            None => {
                self.persist_streak = 0;
                self.last_er = Some(current);
                Observation::NoWrite
            }
        }
    }

    pub fn wrote(&mut self, value: T) {
        self.last_written = Some(value);
    }

    /// ER's last pre-write value.
    pub fn last_er(&self) -> Option<T> {
        self.last_er
    }

    pub fn last_written(&self) -> Option<T> {
        self.last_written
    }

    pub fn rewrite_seen(&self) -> bool {
        self.rewrite_seen
    }

    pub fn persist_seen(&self) -> bool {
        self.persist_seen
    }

    /// Consecutive `Persisted` observations up to now.
    pub fn persist_streak(&self) -> u32 {
        self.persist_streak
    }

    /// True once [`PERSIST_STREAK_DISABLE`] persisted samples came in a row.
    pub fn persists(&self) -> bool {
        self.persist_streak >= PERSIST_STREAK_DISABLE
    }

    /// Ends an episode. Returns the value to write back (ER's last value) when our last write may still be in the
    /// field: we wrote, and the engine was never seen rewriting it (or it was seen persisting).
    pub fn stop(&mut self) -> Option<T> {
        let wrote = self.last_written.take().is_some();
        if wrote && (!self.rewrite_seen || self.persist_seen) { self.last_er } else { None }
    }
}

/// `ChrIns.chr_flags1c5` bit 3 `enable_render` (the byte also holds precision_shooting b1, is_invincible b4 and
/// death_flag b7, which are never touched).
pub const RENDER_BIT: u8 = 0x08;

/// What to do with the render bit this tick.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BitTick {
    /// Still hidden.
    Ok,
    /// The engine set the bit again: write this value (bit cleared, other bits as they are).
    Reassert(u8),
    /// A different character now sits in `main_player`: hand back, write nothing.
    OwnerChanged,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RenderBit {
    pub owner: usize,
    pub saved: u8,
    pub reasserts: u32,
}

impl RenderBit {
    /// Saves the byte and returns the model plus the value to write (bit 3 cleared).
    pub fn enter(owner: usize, current: u8) -> (RenderBit, u8) {
        (RenderBit { owner, saved: current, reasserts: 0 }, current & !RENDER_BIT)
    }

    pub fn tick(&mut self, owner_now: usize, current: u8) -> BitTick {
        if owner_now != self.owner {
            return BitTick::OwnerChanged;
        }
        if current & RENDER_BIT != 0 {
            self.reasserts += 1;
            return BitTick::Reassert(current & !RENDER_BIT);
        }
        BitTick::Ok
    }

    /// The value to write on handback: only bit 3 comes from the save, every other bit stays as the engine has it
    /// now. `None` when the character changed (never write to an address whose value we did not save).
    pub fn restore(&self, owner_now: usize, current: u8) -> Option<u8> {
        (owner_now == self.owner).then_some((current & !RENDER_BIT) | (self.saved & RENDER_BIT))
    }
}

/// Fade alternative: `base_transparency_modifier` = -1 makes the engine fade `base_transparency` to 0.
pub const FADE_HIDE: f32 = -1.0;
pub const FADE_SHOW: f32 = 1.0;
pub const FADE_VISIBLE_AT: f32 = 0.999;
pub const FADE_RESTORE_MAX_S: f64 = 2.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FadeHide {
    pub owner: usize,
    pub saved_modifier: f32,
    pub saved_base: f32,
    pub reasserts: u32,
}

impl FadeHide {
    /// Saves both values; the value to write to the modifier is [`FADE_HIDE`].
    pub fn enter(owner: usize, base: f32, modifier: f32) -> (FadeHide, f32) {
        (FadeHide { owner, saved_modifier: modifier, saved_base: base, reasserts: 0 }, FADE_HIDE)
    }

    /// As [`FadeHide::enter`], but when a fade-in restore of the same character is still running the field holds
    /// our own +1.0, not the game's value: carry that restore's saved modifier over (review M3b minor 7).
    pub fn enter_after(owner: usize, base: f32, modifier: f32, restoring: Option<FadeRestoring>) -> (FadeHide, f32) {
        let saved = match restoring {
            Some(r) if r.owner == owner => r.saved_modifier,
            _ => modifier,
        };
        FadeHide::enter(owner, base, saved)
    }

    /// `Some(FADE_HIDE)` when the engine changed the modifier (write it again); `Err(())` when the owner changed.
    pub fn tick(&mut self, owner_now: usize, modifier: f32) -> Result<Option<f32>, ()> {
        if owner_now != self.owner {
            return Err(());
        }
        if modifier != FADE_HIDE {
            self.reasserts += 1;
            return Ok(Some(FADE_HIDE));
        }
        Ok(None)
    }

    /// Starts the restore: write [`FADE_SHOW`] now (only on the same character).
    pub fn begin_restore(&self, owner_now: usize, now: f64) -> Option<(FadeRestoring, f32)> {
        (owner_now == self.owner)
            .then_some((FadeRestoring { owner: self.owner, saved_modifier: self.saved_modifier, t0: now }, FADE_SHOW))
    }
}

/// The multi-tick tail of a fade restore.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FadeRestoring {
    pub owner: usize,
    pub saved_modifier: f32,
    pub t0: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum FadeStep {
    /// Still fading in.
    Wait,
    /// Visible (or timed out): write the saved modifier and stop.
    Done(f32),
    /// The character changed: stop without writing.
    Drop,
}

impl FadeRestoring {
    pub fn step(&self, owner_now: usize, base: f32, now: f64) -> FadeStep {
        if owner_now != self.owner {
            FadeStep::Drop
        } else if base >= FADE_VISIBLE_AT || now - self.t0 >= FADE_RESTORE_MAX_S || !base.is_finite() {
            FadeStep::Done(self.saved_modifier)
        } else {
            FadeStep::Wait
        }
    }
}

/// Facing writes use the release policy: the facing is a dynamic game state that ER updates every frame, so
/// writing an old facing back on handback would snap the character. The model only records the original values
/// of the episode (logged at the first write) and counts writes; `restore()` is always `None`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct YawRelease {
    pub first_old: Option<([f32; 4], [f32; 4])>,
    pub writes: u64,
}

impl YawRelease {
    /// Returns true on the first write of the episode (log it).
    pub fn record(&mut self, old_orientation: [f32; 4], old_interpolated: [f32; 4]) -> bool {
        self.writes += 1;
        if self.first_old.is_none() {
            self.first_old = Some((old_orientation, old_interpolated));
            return true;
        }
        false
    }

    pub fn restore(&self) -> Option<([f32; 4], [f32; 4])> {
        None
    }

    pub fn reset(&mut self) {
        *self = YawRelease::default();
    }
}

/// Hook-mode restore of a camera position row (`CSCamera.pers_cam_1` and `ChrCam.pers_cam` row 3): the detour saved
/// ER's pre-write row and wrote `eye` this frame. On handback, ER's row goes back only when the row still holds
/// exactly our eye (bitwise); if ER already rebuilt it, nothing is written. `None` also when no write happened.
pub fn hook_row_restore(current: [f32; 4], eye: [f32; 4], er_saved: [f32; 4], wrote: bool) -> Option<[f32; 4]> {
    let ours = current.map(f32::to_bits) == eye.map(f32::to_bits);
    (wrote && ours && er_saved.iter().all(|x| x.is_finite())).then_some(er_saved)
}

/// A byte that first person keeps at 0 while it is on (erfps2's camera handling, after run 5):
/// `ChrExFollowCam.camera_auto_rotation` and the exe's `CAM_WALL_RECOVERY` bool. The first value seen in an episode
/// is saved; every later non-zero value is cleared again. On handback the saved value goes back only while the
/// field still holds our 0 (when ER already put its own value there, that newer value stays).
///
/// The detours keep this state in atomics (`encode`/`decode`): no lock, no allocation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct ClearedByte {
    /// The value before our first clear of the episode.
    pub saved: Option<u8>,
}

/// What the detour does with the field this call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClearStep {
    /// Save this as the episode's original (first call only).
    pub save: Option<u8>,
    /// Write 0 now.
    pub clear: bool,
}

impl ClearedByte {
    /// One detour call while first person is on, with the field's current value.
    pub fn step(&mut self, current: u8) -> ClearStep {
        let save = if self.saved.is_none() {
            self.saved = Some(current);
            Some(current)
        } else {
            None
        };
        ClearStep { save, clear: current != 0 }
    }

    /// Handback: the value to write back (and forget the save). `same_owner` = the field still belongs to the object
    /// whose value was saved (always true for the exe global).
    pub fn restore(&mut self, current: u8, same_owner: bool) -> Option<u8> {
        let saved = self.saved.take()?;
        (same_owner && current == 0 && saved != 0).then_some(saved)
    }

    /// Atomic form: 0 = nothing saved, else 0x100 | value.
    pub fn encode(self) -> u32 {
        self.saved.map_or(0, |v| 0x100 | v as u32)
    }

    pub fn decode(bits: u32) -> ClearedByte {
        ClearedByte { saved: (bits & 0x100 != 0).then_some((bits & 0xff) as u8) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fp_restore_persist_watch_engine_rewrites() {
        // ER rebuilds the field every frame: handback writes nothing (the next frame is ER's).
        let mut w = PersistWatch::<[f32; 4]>::default();
        let er = [[1.0, 2.0, 3.0, 1.0], [1.1, 2.0, 3.0, 1.0], [1.2, 2.0, 3.0, 1.0]];
        let eye = [[9.0, 9.0, 9.0, 1.0], [9.1, 9.0, 9.0, 1.0], [9.2, 9.0, 9.0, 1.0]];
        assert_eq!(w.observe(er[0]), Observation::NoWrite);
        w.wrote(eye[0]);
        assert_eq!(w.observe(er[1]), Observation::Rewritten);
        w.wrote(eye[1]);
        assert_eq!(w.observe(er[2]), Observation::Rewritten);
        w.wrote(eye[2]);
        assert!(w.rewrite_seen() && !w.persist_seen());
        assert_eq!(w.last_er(), Some(er[2]));
        assert_eq!(w.stop(), None, "free restore");
        assert_eq!(w.stop(), None, "idempotent");
    }

    #[test]
    fn fp_restore_persist_watch_write_persists() {
        // The engine never rewrites: the round trip gives back ER's exact last value, once.
        let mut w = PersistWatch::<f32>::default();
        assert_eq!(w.observe(0.8378), Observation::NoWrite);
        w.wrote(1.13);
        assert_eq!(w.observe(1.13), Observation::Persisted);
        assert_eq!(w.last_er(), Some(0.8378), "a persisted read never replaces ER's value");
        assert_eq!(w.stop(), Some(0.8378));
        assert_eq!(w.stop(), None);
        // a stop without any write writes nothing
        let mut w = PersistWatch::<f32>::default();
        w.observe(0.5);
        assert_eq!(w.stop(), None);
        // rewritten once, then persisted later: still restore (the last write may be in the field)
        let mut w = PersistWatch::<f32>::default();
        w.observe(0.5);
        w.wrote(1.0);
        w.observe(0.6);
        w.wrote(1.0);
        w.observe(1.0);
        assert_eq!(w.stop(), Some(0.6));
        // the streak: one stale sample is not a verdict, three in a row are; a rewrite resets it
        let mut w = PersistWatch::<f32>::default();
        w.observe(0.5);
        w.wrote(1.0);
        assert_eq!(w.observe(1.0), Observation::Persisted);
        assert!(!w.persists() && w.persist_streak() == 1);
        w.wrote(1.0);
        assert_eq!(w.observe(0.7), Observation::Rewritten);
        assert_eq!(w.persist_streak(), 0);
        for k in 1..=PERSIST_STREAK_DISABLE {
            w.wrote(1.0);
            w.observe(1.0);
            assert_eq!(w.persists(), k >= PERSIST_STREAK_DISABLE, "{k}");
        }
        assert_eq!(w.last_er(), Some(0.7), "the gate gets ER's last value while ours persists");
    }

    #[test]
    fn fp_restore_render_bit_all_256() {
        // For every saved byte and every value the engine may have put in the other bits meanwhile, the restore
        // gives back exactly the saved bit 3 and leaves every other bit as the engine has it.
        for saved in 0u8..=255 {
            let (mut m, hidden) = RenderBit::enter(0x1000, saved);
            assert_eq!(hidden, saved & !RENDER_BIT);
            assert_eq!(m.tick(0x1000, hidden), BitTick::Ok);
            assert_eq!(m.restore(0x1000, hidden), Some(saved), "plain round trip of {saved:#04x}");
            for engine in [0x00u8, 0x80, 0x10, 0x02, 0xF7, 0x91] {
                let cur = engine & !RENDER_BIT;
                let r = m.restore(0x1000, cur).unwrap();
                assert_eq!(r & RENDER_BIT, saved & RENDER_BIT);
                assert_eq!(r & !RENDER_BIT, cur & !RENDER_BIT);
            }
        }
        // reassert: the engine set the bit again -> clear it again, count it
        let (mut m, _) = RenderBit::enter(0x1000, 0x98);
        assert_eq!(m.tick(0x1000, 0x98), BitTick::Reassert(0x90));
        assert_eq!(m.tick(0x1000, 0x90), BitTick::Ok);
        assert_eq!(m.reasserts, 1);
        // another character in main_player: no write at all
        assert_eq!(m.tick(0x2000, 0x90), BitTick::OwnerChanged);
        assert_eq!(m.restore(0x2000, 0x90), None);
    }

    #[test]
    fn fp_restore_fade_stages() {
        let (mut h, w) = FadeHide::enter(0x10, 1.0, 0.0);
        assert_eq!(w, FADE_HIDE);
        assert_eq!(h.tick(0x10, FADE_HIDE), Ok(None));
        assert_eq!(h.tick(0x10, 1.0), Ok(Some(FADE_HIDE)));
        assert_eq!(h.reasserts, 1);
        assert_eq!(h.tick(0x20, FADE_HIDE), Err(()));
        let (r, show) = h.begin_restore(0x10, 100.0).unwrap();
        assert_eq!(show, FADE_SHOW);
        assert_eq!(r.step(0x10, 0.4, 100.1), FadeStep::Wait);
        assert_eq!(r.step(0x10, 0.9995, 100.3), FadeStep::Done(0.0), "visible: saved modifier back");
        assert_eq!(r.step(0x10, 0.2, 102.0), FadeStep::Done(0.0), "timeout");
        assert_eq!(r.step(0x99, 1.0, 100.3), FadeStep::Drop);
        assert!(h.begin_restore(0x20, 0.0).is_none());
        // re-enter while the fade-in of the same character still runs: the original 0.0 survives, not our +1.0
        let (h2, w2) = FadeHide::enter_after(0x10, 0.4, FADE_SHOW, Some(r));
        assert_eq!((h2.saved_modifier, w2), (0.0, FADE_HIDE));
        let (r2, _) = h2.begin_restore(0x10, 200.0).unwrap();
        assert_eq!(r2.step(0x10, 1.0, 200.1), FadeStep::Done(0.0), "round trip through a re-enter is exact");
        // another character, or no restore running: the current value is the original
        assert_eq!(FadeHide::enter_after(0x20, 1.0, 0.25, Some(r)).0.saved_modifier, 0.25);
        assert_eq!(FadeHide::enter_after(0x10, 1.0, 0.25, None).0.saved_modifier, 0.25);
    }

    #[test]
    fn fp_restore_yaw_release_writes_nothing() {
        let mut y = YawRelease::default();
        assert!(y.record([0.0, 0.3, 0.0, 0.95], [0.0, 0.31, 0.0, 0.95]));
        assert!(!y.record([0.0, 0.5, 0.0, 0.86], [0.0, 0.5, 0.0, 0.86]));
        assert_eq!(y.writes, 2);
        assert_eq!(y.first_old.unwrap().0, [0.0, 0.3, 0.0, 0.95]);
        assert_eq!(y.restore(), None, "release policy");
        y.reset();
        assert_eq!(y, YawRelease::default());
    }

    #[test]
    fn fp_restore_hook_row_restore() {
        let er = [-1.67, 8.91, -0.18, 1.0];
        let eye = [1.22, 7.58, 1.34, 1.0];
        // our eye still in the row: ER's saved row goes back
        assert_eq!(hook_row_restore(eye, eye, er, true), Some(er));
        // ER rebuilt the row already: nothing
        assert_eq!(hook_row_restore([-1.6, 8.9, -0.2, 1.0], eye, er, true), None);
        // no write happened this episode, or a non-finite save: nothing
        assert_eq!(hook_row_restore(eye, eye, er, false), None);
        assert_eq!(hook_row_restore(eye, eye, [f32::NAN, 0.0, 0.0, 1.0], true), None);
        // bitwise: -0.0 vs 0.0 is a different value (ER wrote it)
        assert_eq!(hook_row_restore([0.0, 1.0, 2.0, 1.0], [-0.0, 1.0, 2.0, 1.0], er, true), None);
    }

    #[test]
    fn fp_restore_cleared_byte_round_trip() {
        // auto rotation on (1): saved, cleared every call ER sets it again, restored exactly
        let mut b = ClearedByte::default();
        assert_eq!(b.step(1), ClearStep { save: Some(1), clear: true });
        assert_eq!(b.step(0), ClearStep { save: None, clear: false });
        assert_eq!(b.step(1), ClearStep { save: None, clear: true }, "ER re-set it: cleared again");
        assert_eq!(ClearedByte::decode(b.encode()), b);
        assert_eq!(b.restore(0, true), Some(1));
        assert_eq!(b.saved, None, "save consumed");
        assert_eq!(b.restore(0, true), None, "idempotent");
        // option already off (0): nothing is ever written
        let mut b = ClearedByte::default();
        assert_eq!(b.step(0), ClearStep { save: Some(0), clear: false });
        assert_eq!(b.restore(0, true), None);
        // ER put its own value back meanwhile: that newer value stays
        let mut b = ClearedByte::default();
        b.step(1);
        assert_eq!(b.restore(1, true), None);
        // a different follow-cam object now: never write to it
        let mut b = ClearedByte::default();
        b.step(1);
        assert_eq!(b.restore(0, false), None);
        // the atomic encoding of "nothing saved" is 0, and every byte round-trips
        assert_eq!(ClearedByte::default().encode(), 0);
        for v in 0..=255u8 {
            assert_eq!(ClearedByte::decode(ClearedByte { saved: Some(v) }.encode()).saved, Some(v));
        }
    }
}
