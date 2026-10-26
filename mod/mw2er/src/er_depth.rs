//! C6 depth probe P-D, the read-only part (work/c6/depth3.md 5 lines 1-2 + the GX wrapper dump; docs/C6_HOLD_DESIGN.md
//! 13.3). `[coop] probe_er_depth` (cold, default false).
//!
//! - Boot: the chain's sites (`exe_build::er_depth_sites`: 4 RTTI vtables, the count function, the `.data` global) are
//!   checked on the loaded image; a miss logs `DEPTH probe off why=sites` and nothing else runs.
//! - Present callback: only our own back buffer's vtable and size are published (atomics).
//! - Task thread, every 5 s: the pinned chain (`mw2er_core::er_depth::scene_chain`, non-faulting reads; ER's getter is
//!   never called), candidate B (`S+0xE8` DefDepthStencil), the resource's module (same D3D12 runtime module as our back
//!   buffer, else no `GetDesc`). The task never calls `GetDesc`: it publishes each candidate's vtable and a due flag.
//! - UiBegin's detour (ER's render thread; review C6 depth, the C4.1 rule: ER cannot release / recreate the resource
//!   while its own frame uses it): on a due frame it re-reads both pinned chains for that frame and calls `GetDesc` (a
//!   pure query, `ui_probe::desc_if_vtable`) only through the vtable the task found in the D3D12 module; the packed
//!   desc goes into atomics, the task logs it and the desc rule. Without the UiBegin hook the desc stays `unmeasured`.
//! - At 4 Hz (task) the GX wrapper `[obj+8]` +0..+0x60 is read and logged whenever it changes (at most WRAP_LINES
//!   lines): a word taking D3D12 state values between frames would be ER's own tracked resource state, which the live
//!   copy pass would need.
//! - Nothing is bound, barriered, sampled or written; no hook of its own; no GPU work. ER's depth stays as ER uses it.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

use mw2er_core::er_depth::{self, DsbChain, ErDepthAddrs, WRAP_QWORDS};
use mw2er_core::exe_build::ScaleformUi;

use crate::game::read;
use crate::log::{log_i, log_w};

static ON: AtomicBool = AtomicBool::new(false);
static BB_VT: AtomicUsize = AtomicUsize::new(0);
static BB_SIZE: AtomicU64 = AtomicU64::new(0);
/// The chain's addresses for UiBegin's detour (set once at boot; the render thread never calls `version_gate`).
static ADDRS: OnceLock<ErDepthAddrs> = OnceLock::new();
/// Review C6 depth: the 5 s `GetDesc` sample of candidates A / B, taken by UiBegin's detour (as `P41_DESC*`): the task
/// publishes the vtable it found in the D3D12 module (0 = none) and the due flag; the detour stores the measured
/// resource (0 = no desc) and the packed desc, then bumps the generation.
static DESC_DUE: AtomicBool = AtomicBool::new(false);
static DESC_VT: [AtomicUsize; 2] = [const { AtomicUsize::new(0) }; 2];
static DESC_RES: [AtomicUsize; 2] = [const { AtomicUsize::new(0) }; 2];
static DESC: [[AtomicU64; 4]; 2] = [const { [const { AtomicU64::new(0) }; 4] }; 2];
static DESC_GEN: AtomicU64 = AtomicU64::new(0);

/// Lines of the 4 Hz wrapper dump at most (a change every frame would otherwise flood the log).
const WRAP_LINES: u32 = 40;
const CHAIN_EVERY_S: f64 = 5.0;
const WRAP_EVERY_S: f64 = 0.25;
const VERDICT_S: f64 = 60.0;

/// Boot: the sites on the loaded image. Returns whether the probe runs.
pub fn boot(requested: bool) -> bool {
    if !requested {
        return false;
    }
    let Some(build) = crate::version_gate::build() else {
        log_w!("DEPTH", "probe off why=build (no accepted exe build)");
        return false;
    };
    let sites = mw2er_core::exe_build::er_depth_sites(build);
    if sites.is_empty() {
        log_w!("DEPTH", "probe off why=build ({} has no verified depth chain)", build.table_name());
        return false;
    }
    match crate::version_gate::check_sites(&sites) {
        Ok(pf) if pf.passed() => {
            let Some(a) = crate::version_gate::own().ok().and_then(|o| o.er_depth) else {
                log_w!("DEPTH", "probe off why=build (no depth chain addresses)");
                return false;
            };
            let _ = ADDRS.set(a);
            ON.store(true, Ordering::SeqCst);
            log_i!(
                "DEPTH",
                "probe on sites {} ({}; read-only: chain / wrapper words on the task thread, desc on ER's render thread in UiBegin hook={}, nothing bound or sampled)",
                pf.summary(),
                build.table_name(),
                crate::ui_probe::INSTALLED_UI_BEGIN.load(Ordering::Relaxed) as u8
            );
            true
        }
        Ok(pf) => {
            log_w!("DEPTH", "probe off why=sites {}", pf.failures.join("; "));
            false
        }
        Err(e) => {
            log_w!("DEPTH", "probe off why=sites {e}");
            false
        }
    }
}

/// Present callback (hudhook's thread; atomics only): our back buffer's vtable (the D3D12 runtime module) and size.
pub fn on_present(rt_vtable: usize, w: u32, h: u32) {
    if ON.load(Ordering::Relaxed) {
        BB_VT.store(rt_vtable, Ordering::Relaxed);
        BB_SIZE.store(w as u64 | (h as u64) << 32, Ordering::Relaxed);
    }
}

pub fn on() -> bool {
    ON.load(Ordering::Relaxed)
}

/// UiBegin's detour, after the original (ER's render thread; atomics and non-faulting reads only, no allocation, no
/// log, no lock): on a due frame, both candidates' chains re-read for this frame and one `GetDesc` each, only through
/// the vtable the task found in the D3D12 runtime module (`desc_if_vtable` re-checks the object's vtable first).
/// C6 depth 5: with the barrier probe on, every frame's chain A goes to `er_depth_state::on_ui_begin` (the watched
/// resource, the copy / sample gate), with the vtable the task proved in the D3D12 module. C6 round 6: candidate B's
/// pinned dsb chain and its proven vtable too (both are watched).
pub fn on_ui_begin(base: usize, sf: &ScaleformUi, f: &crate::ui_probe::UiFacts) {
    if !ON.load(Ordering::Relaxed) {
        return;
    }
    let state = crate::er_depth_state::on();
    let due = DESC_DUE.swap(false, Ordering::Relaxed);
    if !due && !state {
        return;
    }
    let Some(a) = ADDRS.get() else { return };
    let rdf: mw2er_core::c4::Rd = &rd;
    let rs = rd(base + a.rs_global, 8).unwrap_or(0) as usize;
    let c = er_depth::scene_chain(rs, base, a, rdf);
    // candidate B (`S[0]+0xE8` DefDepthStencil): C6 round 6 watches it too (the scene context's frame-buffer depth and
    // the barrier probe's positive control)
    let g = rd(base + sf.g_render, 8).unwrap_or(0) as usize;
    let s = if g != 0 { rd(g + sf.layout.s_off, 8).unwrap_or(0) as usize } else { 0 };
    let dsb = if s != 0 { rd(s + a.def_ds, 8).unwrap_or(0) as usize } else { 0 };
    let b = er_depth::dsb_chain(dsb, base, a, rdf);
    if due {
        for (i, (ok, res)) in [(c.ok(), c.a.res), (b.ok(), b.res)].into_iter().enumerate() {
            let vt = DESC_VT[i].load(Ordering::Relaxed);
            let d = if ok && vt != 0 { crate::ui_probe::desc_if_vtable(res, vt) } else { None };
            DESC_RES[i].store(if d.is_some() { res } else { 0 }, Ordering::Relaxed);
            let v = d.as_ref().map_or([0; 4], crate::ui_probe::pack_desc);
            for (k, x) in v.into_iter().enumerate() {
                DESC[i][k].store(x, Ordering::Relaxed);
            }
        }
        DESC_GEN.fetch_add(1, Ordering::Release);
    }
    if state {
        crate::er_depth_state::on_ui_begin(f, &c, &b, [DESC_VT[0].load(Ordering::Relaxed), DESC_VT[1].load(Ordering::Relaxed)], base, sf);
    }
}

fn rd(addr: usize, len: usize) -> Option<u64> {
    // SAFETY: non-faulting reads of plain integers at any address.
    unsafe {
        match len {
            1 => read::<u8>(addr).map(u64::from),
            4 => read::<u32>(addr).map(u64::from),
            _ => read::<u64>(addr),
        }
    }
}

/// Task thread: the probe's state and lines.
#[derive(Default)]
pub struct DepthProbe {
    next_chain: f64,
    next_wrap: f64,
    t0: Option<f64>,
    last_res: usize,
    changes: u32,
    last_wrap: Option<[u64; WRAP_QWORDS]>,
    wrap_lines: u32,
    /// `[obj+8]` qword offsets seen with a state-like value, and how many distinct values each took.
    state_words: Vec<(usize, Vec<u32>)>,
    chains: u32,
    chain_ok: u32,
    desc_ok: u32,
    desc_why: Option<&'static str>,
    verdict_done: bool,
    mod_cache: (usize, bool),
    /// The last `DESC_GEN` the task logged (review C6 depth: the detour's sample).
    desc_gen: u64,
}

impl DepthProbe {
    pub fn tick(&mut self, now: f64, in_world: bool) {
        if !ON.load(Ordering::Relaxed) || !in_world {
            return;
        }
        let t0 = *self.t0.get_or_insert(now);
        let Ok(own) = crate::version_gate::own() else { return };
        let Some(a) = own.er_depth else { return };
        let base = crate::game::c3_read::exe_base();
        if base == 0 {
            return;
        }
        let rdf: mw2er_core::c4::Rd = &rd;
        let rs = rd(base + a.rs_global, 8).unwrap_or(0) as usize;
        let c = er_depth::scene_chain(rs, base, &a, rdf);
        if now >= self.next_wrap {
            self.next_wrap = now + WRAP_EVERY_S;
            self.wrap(now - t0, &c.a);
        }
        if now < self.next_chain {
            return;
        }
        self.next_chain = now + CHAIN_EVERY_S;
        self.chains += 1;
        if c.a.res != 0 && c.a.res != self.last_res {
            if self.last_res != 0 {
                self.changes += 1;
            }
            self.last_res = c.a.res;
        }
        // candidate B: S + 0xE8 (S = [[base + g_render] + 0x128], as UiBegin reads it)
        let b = own.scaleform_ui.map_or(DsbChain::default(), |sf| {
            let g = rd(base + sf.g_render, 8).unwrap_or(0) as usize;
            let s = if g != 0 { rd(g + sf.layout.s_off, 8).unwrap_or(0) as usize } else { 0 };
            let dsb = if s != 0 { rd(s + a.def_ds, 8).unwrap_or(0) as usize } else { 0 };
            er_depth::dsb_chain(dsb, base, &a, rdf)
        });
        let bb_vt = BB_VT.load(Ordering::Relaxed);
        let bbs = BB_SIZE.load(Ordering::Relaxed);
        let bb = (bbs as u32, (bbs >> 32) as u32);
        let res_vt = if c.a.res != 0 { rd(c.a.res, 8).unwrap_or(0) as usize } else { 0 };
        let res_mod = self.in_d3d12(res_vt, bb_vt);
        log_i!("DEPTH", "probe {}", er_depth::chain_text(&c, &b, res_mod, self.changes));
        if c.ok() {
            self.chain_ok += 1;
        }
        let b_vt = if b.ok() { rd(b.res, 8).unwrap_or(0) as usize } else { 0 };
        let b_mod = b.ok() && self.in_d3d12(b_vt, bb_vt);
        // review C6 depth: GetDesc runs only in UiBegin's detour (ER's render thread); the task logs its last sample
        if !crate::ui_probe::INSTALLED_UI_BEGIN.load(Ordering::Relaxed) {
            for tag in ["A", "B"] {
                log_i!("DEPTH", "probe {}", er_depth::desc_unmeasured(tag, "no UiBegin hook: GetDesc only on ER's render thread"));
            }
        } else {
            let gen_now = DESC_GEN.load(Ordering::Acquire);
            if gen_now != self.desc_gen {
                self.desc_gen = gen_now;
                let [da, db] = [0, 1].map(|i| {
                    (DESC_RES[i].load(Ordering::Relaxed) != 0).then(|| crate::ui_probe::unpack_desc([0, 1, 2, 3].map(|k| DESC[i][k].load(Ordering::Relaxed))))
                });
                log_i!("DEPTH", "probe {}", er_depth::desc_text("A", da.as_ref(), bb));
                match da.as_ref().map(|d| er_depth::desc_rule(d, bb)) {
                    Some(Ok(())) => self.desc_ok += 1,
                    Some(Err(w)) => self.desc_why = Some(w),
                    None => {}
                }
                log_i!("DEPTH", "probe {}", er_depth::desc_text("B", db.as_ref(), bb));
            } else if self.chains > 1 {
                // a sample was due since the last read and no UiBegin frame took it (loading screen, minimised)
                for tag in ["A", "B"] {
                    log_i!("DEPTH", "probe {}", er_depth::desc_unmeasured(tag, "no UiBegin frame since the last request"));
                }
            }
            // the next sample: the vtables proven in the D3D12 module (0 = none: the detour calls nothing)
            DESC_VT[0].store(if c.ok() && res_mod { res_vt } else { 0 }, Ordering::Relaxed);
            DESC_VT[1].store(if b_mod { b_vt } else { 0 }, Ordering::Relaxed);
            DESC_DUE.store(true, Ordering::Relaxed);
        }
        if !self.verdict_done && now - t0 >= VERDICT_S {
            self.verdict_done = true;
            let words: Vec<String> = self.state_words.iter().filter(|(_, v)| v.len() >= 2).map(|(o, v)| format!("+{o:02x}:{v:x?}")).collect();
            log_i!(
                "DEPTH",
                "probe verdict chain={}/{} desc={}/{}{} res_changes={} wrap_state_words={} state=unmeasured (a live copy needs the resource state at UiBegin: the barrier probe, design 13.3)",
                self.chain_ok,
                self.chains,
                self.desc_ok,
                self.chains,
                self.desc_why.map_or(String::new(), |w| format!(" desc_fail={w}")),
                self.changes,
                if words.is_empty() { "none".to_string() } else { words.join(",") }
            );
        }
    }

    /// The vtable lies in the D3D12 runtime module of our back buffer (cached per vtable; task thread only).
    fn in_d3d12(&mut self, vt: usize, bb_vt: usize) -> bool {
        if vt == 0 || bb_vt == 0 {
            return false;
        }
        if self.mod_cache.0 != vt {
            let m = crate::ui_probe::module_of(vt);
            self.mod_cache = (vt, m != 0 && m == crate::ui_probe::module_of(bb_vt));
        }
        self.mod_cache.1
    }

    fn wrap(&mut self, t: f64, a: &DsbChain) {
        if a.wrap == 0 || !a.ok() {
            return;
        }
        let mut w = [0u64; WRAP_QWORDS];
        for (k, v) in w.iter_mut().enumerate() {
            *v = rd(a.wrap + k * 8, 8).unwrap_or(0);
        }
        for (o, st) in er_depth::state_like(&w) {
            match self.state_words.iter().position(|(x, _)| *x == o) {
                Some(k) => {
                    let v = &mut self.state_words[k].1;
                    if !v.contains(&st) && v.len() < 8 {
                        v.push(st);
                    }
                }
                None if self.state_words.len() < 16 => self.state_words.push((o, vec![st])),
                None => {}
            }
        }
        if self.last_wrap != Some(w) {
            self.last_wrap = Some(w);
            if self.wrap_lines < WRAP_LINES {
                self.wrap_lines += 1;
                log_i!("DEPTH", "probe wrap t={t:.2} wrap=0x{:x} {}", a.wrap, er_depth::wrap_text(&w));
            }
        }
    }
}
