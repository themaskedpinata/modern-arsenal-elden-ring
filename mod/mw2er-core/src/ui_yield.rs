//! C3 (docs/C3_DESIGN.md 2.3): the "UI yield". While ER shows a message (popup, system announcement, status banner,
//! summon message, blinking / area text, configured `ui_states` indices) our viewmodel slides down and our HUD text
//! dims, so ER's message (drawn under everything we draw at Present) can be read. Render-only: it never blocks or
//! delays a shot; firing or ADS cancels the yield at once and it comes back only after `reyield_s` without a shot.
//!
//! C5 `mode = "cutout"`: the gun keeps its place and is not drawn inside the screen regions of the messages that
//! show ([`cutout_rects`]; the renderer masks them with feathered edges at the same frac).
//!
//! Pure: the task passes the signal bits it read (non-faulting reads of ER's UI managers) and the input state.

/// Signal bits (`UI yield on reason=` names them).
pub const SIG_POPUP: u32 = 1 << 0;
pub const SIG_ANNOUNCE: u32 = 1 << 1;
pub const SIG_STATUS: u32 = 1 << 2;
pub const SIG_SUMMON: u32 = 1 << 3;
pub const SIG_BLINK: u32 = 1 << 4;
/// One of the configured `[ui_yield] indices` (`ui_states[i]` visible).
pub const SIG_INDEX: u32 = 1 << 5;
pub const SIG_NAMES: [(u32, &str); 6] =
    [(SIG_POPUP, "popup"), (SIG_ANNOUNCE, "announce"), (SIG_STATUS, "status"), (SIG_SUMMON, "summon"), (SIG_BLINK, "blink"), (SIG_INDEX, "idx")];

/// `popup+status` style label of a bit mask ("none" for 0).
pub fn why_label(mask: u32) -> String {
    let v: Vec<&str> = SIG_NAMES.iter().filter(|(b, _)| mask & b != 0).map(|(_, n)| *n).collect();
    if v.is_empty() { "none".into() } else { v.join("+") }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct YieldParams {
    /// 0 -> 1 time (s).
    pub ease_in_s: f32,
    /// 1 -> 0 time (s).
    pub ease_out_s: f32,
    /// The yield holds at least this long after the last signal (s).
    pub hold_s: f64,
    /// Fire / ADS: 1 -> 0 in this long (s).
    pub cancel_s: f32,
    /// After a shot / ADS the yield may come back only after this long without one (s).
    pub reyield_s: f64,
    /// A signal bit that stays on longer than this is ignored until it goes off (a stuck / misread signal can never
    /// keep the gun down; logged by the task) (s).
    pub stuck_s: f64,
}

impl Default for YieldParams {
    fn default() -> Self {
        YieldParams { ease_in_s: 0.12, ease_out_s: 0.25, hold_s: 0.4, cancel_s: 0.08, reyield_s: 0.6, stuck_s: 20.0 }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum YieldEvent {
    /// The yield started (frac left 0) with these signal bits.
    On { why: u32 },
    /// The yield ended (frac back at 0) after `dur` s; `cancelled` = a shot / ADS ended it.
    Off { dur: f64, cancelled: bool },
    /// A signal bit was on for `stuck_s` and is ignored from now on until it clears.
    Stuck { bit: u32 },
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct YieldOut {
    pub frac: f32,
    /// The effective signal bits this tick (stuck bits removed).
    pub why: u32,
    pub event: Option<YieldEvent>,
    /// C5 cutout: each signal bit's own 0..1 envelope (eases in / holds / eases out like `frac`, per bit), so a
    /// message's region fades on its own when another message keeps the yield on.
    pub bit_frac: [f32; 8],
}

#[derive(Clone, Debug, PartialEq)]
pub struct UiYield {
    frac: f32,
    last_t: Option<f64>,
    last_signal_t: f64,
    last_fire_t: f64,
    on_since: [Option<f64>; 8],
    stuck: u32,
    on_t: Option<f64>,
    cancelled: bool,
    bit_frac: [f32; 8],
    bit_last: [f64; 8],
}

impl Default for UiYield {
    fn default() -> Self {
        UiYield {
            frac: 0.0,
            last_t: None,
            last_signal_t: f64::NEG_INFINITY,
            last_fire_t: f64::NEG_INFINITY,
            on_since: [None; 8],
            stuck: 0,
            on_t: None,
            cancelled: false,
            bit_frac: [0.0; 8],
            bit_last: [f64::NEG_INFINITY; 8],
        }
    }
}

impl UiYield {
    pub fn frac(&self) -> f32 {
        self.frac
    }

    pub fn bit_frac(&self) -> [f32; 8] {
        self.bit_frac
    }

    /// Back to no yield at once (world left, mode off, config off).
    pub fn reset(&mut self) {
        *self = UiYield::default();
    }

    /// One tick: `signals` (SIG_* bits read this tick), `fire_or_ads` (the trigger or the ADS button is held).
    pub fn update(&mut self, now: f64, signals: u32, fire_or_ads: bool, p: &YieldParams) -> YieldOut {
        let dt = self.last_t.map_or(0.0, |t| (now - t).clamp(0.0, 0.25)) as f32;
        self.last_t = Some(now);
        let mut event = None;
        let mut eff = 0u32;
        for (k, since) in self.on_since.iter_mut().enumerate() {
            let bit = 1u32 << k;
            if signals & bit != 0 {
                let s = *since.get_or_insert(now);
                if now - s < p.stuck_s {
                    eff |= bit;
                } else if self.stuck & bit == 0 {
                    self.stuck |= bit;
                    event = Some(YieldEvent::Stuck { bit });
                }
            } else {
                *since = None;
                self.stuck &= !bit;
            }
        }
        if eff != 0 {
            self.last_signal_t = now;
        }
        if fire_or_ads {
            self.last_fire_t = now;
        }
        let want = eff != 0 || now - self.last_signal_t < p.hold_s;
        let blocked = now - self.last_fire_t < p.reyield_s;
        let was = self.frac;
        if blocked {
            if self.frac > 0.0 {
                self.cancelled = true;
            }
            self.frac = (self.frac - dt / p.cancel_s.max(1e-3)).max(0.0);
        } else if want {
            self.frac = (self.frac + dt / p.ease_in_s.max(1e-3)).min(1.0);
        } else {
            self.frac = (self.frac - dt / p.ease_out_s.max(1e-3)).max(0.0);
        }
        // C5: the per-bit envelopes (never above the global frac where they are used: see `cutout_rects`)
        for k in 0..8 {
            if eff & (1u32 << k) != 0 {
                self.bit_last[k] = now;
            }
            let b = &mut self.bit_frac[k];
            *b = if now - self.bit_last[k] < p.hold_s || eff & (1u32 << k) != 0 {
                (*b + dt / p.ease_in_s.max(1e-3)).min(1.0)
            } else {
                (*b - dt / p.ease_out_s.max(1e-3)).max(0.0)
            };
        }
        if was == 0.0 && self.frac > 0.0 {
            self.on_t = Some(now);
            self.cancelled = false;
            event = Some(YieldEvent::On { why: eff });
        } else if was > 0.0 && self.frac == 0.0 {
            let dur = self.on_t.map_or(0.0, |t| now - t);
            self.on_t = None;
            event = Some(YieldEvent::Off { dur, cancelled: self.cancelled });
        }
        YieldOut { frac: self.frac, why: eff, event, bit_frac: self.bit_frac }
    }
}

/// C5 `mode = "cutout"`: at most this many regions are cut out of our draws at once (the shader's constant array).
pub const MAX_CUT_RECTS: usize = 4;

/// One cut-out region: `rect` = [x0, y0, x1, y1] as fractions of the screen (0,0 top-left), `strength` 0..1 (1 = our
/// draws are not drawn inside it at all), `bit` = the signal it belongs to.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CutRect {
    pub rect: [f32; 4],
    pub strength: f32,
    pub bit: u32,
}

/// The regions cut out this tick: every configured region of every signal whose envelope is up, at strength
/// `min(frac, bit_frac)` (so a shot / ADS that cancels the yield brings the whole gun back with the yield's own
/// timing), strongest first, duplicates merged, at most [`MAX_CUT_RECTS`]. `regions` = (signal bit, its rects).
pub fn cutout_rects(frac: f32, bit_frac: &[f32; 8], regions: &[(u32, &[[f32; 4]])]) -> ([CutRect; MAX_CUT_RECTS], usize) {
    let mut all: Vec<CutRect> = Vec::new();
    let f = frac.clamp(0.0, 1.0);
    if f > 0.0 {
        for &(bit, rects) in regions {
            if bit == 0 || bit.trailing_zeros() >= 8 {
                continue;
            }
            let s = f.min(bit_frac[bit.trailing_zeros() as usize].clamp(0.0, 1.0));
            if s <= 0.0 {
                continue;
            }
            for r in rects {
                if !(r[2] > r[0] && r[3] > r[1]) || !r.iter().all(|v| v.is_finite()) {
                    continue;
                }
                match all.iter_mut().find(|c| c.rect == *r) {
                    Some(c) if c.strength >= s => {}
                    Some(c) => {
                        c.strength = s;
                        c.bit = bit;
                    }
                    None => all.push(CutRect { rect: *r, strength: s, bit }),
                }
            }
        }
    }
    // stable: equal strengths keep the order of `regions`
    all.sort_by(|a, b| b.strength.total_cmp(&a.strength));
    let mut out = [CutRect::default(); MAX_CUT_RECTS];
    let n = all.len().min(MAX_CUT_RECTS);
    out[..n].copy_from_slice(&all[..n]);
    (out, n)
}

/// The signal bits that own at least one of the first `n` regions.
pub fn cut_bits(rects: &[CutRect], n: usize) -> u32 {
    rects.iter().take(n).fold(0, |m, c| m | c.bit)
}

/// `mode = "hide"`: the viewmodel is not drawn above this frac.
pub const HIDE_FRAC: f32 = 0.95;

pub fn hide(frac: f32) -> bool {
    frac > HIDE_FRAC
}

/// Review C3: the world guns (our own in third person, the peers') are drawn after ER's UI too, cannot slide away and
/// have no per-draw alpha: they are not drawn at all above this frac while the yield is on.
pub const WORLD_HIDE_FRAC: f32 = 0.5;

/// Review C3: the world-gun pass is skipped (ER's message stays readable). `active` = `[ui_yield] enabled` and its
/// mode is not "off".
pub fn world_hidden(active: bool, frac: f32) -> bool {
    active && frac >= WORLD_HIDE_FRAC
}

/// The viewmodel offset (renderer axes: x forward, y left, z up; inches) at `frac`.
pub fn vm_offset(frac: f32, drop_in: [f32; 3]) -> [f32; 3] {
    let f = frac.clamp(0.0, 1.0);
    [drop_in[0] * f, drop_in[1] * f, drop_in[2] * f]
}

/// Alpha multiplier of the dimmed HUD texts: 1 at frac 0, `dim` at frac 1.
pub fn hud_alpha(frac: f32, dim: f32) -> f32 {
    let f = frac.clamp(0.0, 1.0);
    1.0 - f * (1.0 - dim.clamp(0.0, 1.0))
}

/// The configured indices that are visible in `mask` (bit i = ui_states[i] visible).
pub fn index_bits(mask: u128, indices: &[u32]) -> bool {
    indices.iter().any(|&i| i < 128 && mask & (1u128 << i) != 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(y: &mut UiYield, p: &YieldParams, t0: f64, t1: f64, sig: u32, fire: bool) -> (f32, Vec<YieldEvent>) {
        let mut ev = Vec::new();
        let mut t = t0;
        let mut f = y.frac();
        while t <= t1 + 1e-9 {
            let o = y.update(t, sig, fire, p);
            f = o.frac;
            if let Some(e) = o.event {
                ev.push(e);
            }
            t += 1.0 / 60.0;
        }
        (f, ev)
    }

    #[test]
    fn c3_uiyield_eases_in_holds_and_out() {
        let p = YieldParams::default();
        let mut y = UiYield::default();
        let (f, ev) = run(&mut y, &p, 0.0, 0.06, SIG_POPUP, false);
        assert!(f > 0.3 && f < 0.7, "half way after ~0.06 s: {f}");
        assert_eq!(ev, vec![YieldEvent::On { why: SIG_POPUP }]);
        let (f, _) = run(&mut y, &p, 0.07, 0.2, SIG_POPUP, false);
        assert_eq!(f, 1.0);
        // signal gone: holds for 0.4 s, then eases out over 0.25 s
        let (f, ev) = run(&mut y, &p, 0.2 + 1.0 / 60.0, 0.55, 0, false);
        assert_eq!(f, 1.0, "hold");
        assert!(ev.is_empty());
        let (f, ev) = run(&mut y, &p, 0.56, 0.95, 0, false);
        assert_eq!(f, 0.0);
        assert!(matches!(ev[..], [YieldEvent::Off { cancelled: false, .. }]), "{ev:?}");
    }

    #[test]
    fn c3_uiyield_fire_cancels_and_reyields_after_quiet() {
        let p = YieldParams::default();
        let mut y = UiYield::default();
        run(&mut y, &p, 0.0, 0.5, SIG_STATUS, false);
        assert_eq!(y.frac(), 1.0);
        // one shot: down within ~0.08 s
        let (f, ev) = run(&mut y, &p, 0.5 + 1.0 / 60.0, 0.62, SIG_STATUS, true);
        assert_eq!(f, 0.0);
        assert!(matches!(ev[..], [YieldEvent::Off { cancelled: true, .. }]), "{ev:?}");
        // still signalled, no shot: stays down for 0.6 s after the last shot, then comes back
        let (f, _) = run(&mut y, &p, 0.64, 1.15, SIG_STATUS, false);
        assert_eq!(f, 0.0);
        let (f, _) = run(&mut y, &p, 1.25, 1.5, SIG_STATUS, false);
        assert_eq!(f, 1.0);
    }

    #[test]
    fn c3_uiyield_stuck_bit_is_ignored_until_cleared() {
        let p = YieldParams { stuck_s: 2.0, ..YieldParams::default() };
        let mut y = UiYield::default();
        let (_, ev) = run(&mut y, &p, 0.0, 3.0, SIG_ANNOUNCE, false);
        assert!(ev.contains(&YieldEvent::Stuck { bit: SIG_ANNOUNCE }), "{ev:?}");
        assert_eq!(y.frac(), 0.0, "a stuck signal releases the gun");
        // another bit still yields while the stuck one stays on
        let (f, _) = run(&mut y, &p, 3.02, 3.3, SIG_ANNOUNCE | SIG_POPUP, false);
        assert_eq!(f, 1.0);
        // cleared: the bit counts again
        run(&mut y, &p, 3.32, 4.5, 0, false);
        let o = y.update(4.6, SIG_ANNOUNCE, false, &p);
        assert_eq!(o.why, SIG_ANNOUNCE);
    }

    #[test]
    fn c3_uiyield_mask_index_bits_and_hide() {
        let mask = (1u128 << 15) | (1u128 << 66);
        assert!(index_bits(mask, &[15, 24, 66]));
        assert!(!index_bits(mask, &[24]));
        assert!(!index_bits(mask, &[200]));
        assert_eq!(why_label(SIG_POPUP | SIG_INDEX), "popup+idx");
        assert_eq!(why_label(0), "none");
        assert!(hide(0.96) && !hide(0.95));
        assert!(world_hidden(true, 0.5) && !world_hidden(true, 0.49) && !world_hidden(false, 1.0));
        assert_eq!(vm_offset(0.5, [-2.0, 0.0, -9.0]), [-1.0, 0.0, -4.5]);
        assert!((hud_alpha(1.0, 0.35) - 0.35).abs() < 1e-6 && hud_alpha(0.0, 0.35) == 1.0);
    }

    #[test]
    fn c5_cutout_bit_envelopes_fade_per_signal() {
        let p = YieldParams::default();
        let mut y = UiYield::default();
        run(&mut y, &p, 0.0, 0.5, SIG_BLINK | SIG_INDEX, false);
        let b = y.bit_frac();
        assert_eq!((b[4], b[5]), (1.0, 1.0));
        // the prompt (idx) closes, the blink stays: idx holds 0.4 s, then fades out over 0.25 s; blink stays at 1
        run(&mut y, &p, 0.5 + 1.0 / 60.0, 0.85, SIG_BLINK, false);
        assert_eq!(y.bit_frac()[5], 1.0, "hold");
        run(&mut y, &p, 0.86, 1.3, SIG_BLINK, false);
        let b = y.bit_frac();
        assert_eq!((b[4], b[5], y.frac()), (1.0, 0.0, 1.0));
    }

    #[test]
    fn c5_cutout_rects_strength_order_cap_and_cancel() {
        let mut bf = [0.0f32; 8];
        bf[0] = 1.0; // popup
        bf[4] = 0.5; // blink
        bf[5] = 1.0; // idx
        let popup = [[0.3, 0.3, 0.7, 0.7]];
        let blink = [[0.0, 0.4, 1.0, 0.6], [0.3, 0.3, 0.7, 0.7]];
        let idx = [[0.3, 0.7, 0.7, 0.85], [0.1, 0.86, 0.9, 0.97], [0.5, 0.5, 0.4, 0.6]];
        let regions: [(u32, &[[f32; 4]]); 3] = [(SIG_POPUP, &popup), (SIG_BLINK, &blink), (SIG_INDEX, &idx)];
        let (r, n) = cutout_rects(1.0, &bf, &regions);
        // the duplicate popup / blink rect is merged at the popup's 1.0; the empty idx rect is dropped; 4 kept
        assert_eq!(n, 4);
        assert_eq!(r[0], CutRect { rect: popup[0], strength: 1.0, bit: SIG_POPUP });
        assert_eq!((r[1].bit, r[2].bit), (SIG_INDEX, SIG_INDEX));
        assert_eq!(r[3], CutRect { rect: blink[0], strength: 0.5, bit: SIG_BLINK });
        assert_eq!(cut_bits(&r, n), SIG_POPUP | SIG_INDEX | SIG_BLINK);
        // the global frac caps every region (a shot cancelling the yield brings the gun back everywhere)
        let (r, n) = cutout_rects(0.25, &bf, &regions);
        assert!(n == 4 && r.iter().all(|c| c.strength <= 0.25));
        assert_eq!(cutout_rects(0.0, &bf, &regions).1, 0, "no yield: no region (the zero-cost path)");
        assert_eq!(cutout_rects(1.0, &[0.0; 8], &regions).1, 0);
        // more than MAX regions: capped
        let many = [[0.0, 0.0, 0.1, 0.1], [0.1, 0.1, 0.2, 0.2], [0.2, 0.2, 0.3, 0.3], [0.3, 0.3, 0.4, 0.4], [0.4, 0.4, 0.5, 0.5]];
        assert_eq!(cutout_rects(1.0, &bf, &[(SIG_POPUP, &many)]).1, MAX_CUT_RECTS);
    }

    /// Mutation guard: a yield that ignores fire (render-only rule broken the other way: the gun stays down while
    /// shooting) must be caught.
    #[test]
    fn c3_uiyield_never_stays_down_while_firing() {
        let p = YieldParams::default();
        let mut y = UiYield::default();
        let (f, _) = run(&mut y, &p, 0.0, 2.0, SIG_POPUP | SIG_STATUS, true);
        assert_eq!(f, 0.0);
    }
}
