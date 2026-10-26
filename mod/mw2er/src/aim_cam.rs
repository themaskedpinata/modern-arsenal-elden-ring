//! C6 round 6 (work/c6/aim6.md, docs/C6_HOLD_DESIGN.md 14.8): third-person ADS keeps ER's crossbow aim BODY
//! animation, but not ER's aim camera and not its crossbow reticle (the round-5 report).
//!
//! One MinHook on ER's precision-shooting getter (`mw2er_core::aim_cam::AIM_CAM_2720`: 0x1403F4A90, 2.7.2.0 only),
//! installed by the task the first time `[coop] tp_aim_cam = "keep"` is in effect with `tp_ads_pose = "er_aim"`
//! (`CoopCfg::aim_cam_keep`), never removed (disarmed = a passthrough). The detour [`getter_detour`] is a naked
//! function: atomics and compares only, no stack frame, no call, no write of ER memory. It returns 0 only when
//! - [`ARMED`] is set (the task: our own third-person aim episode, `mw2er_core::aim_cam::armed`),
//! - the getter's `ChrIns*` (rcx) is our main PlayerIns ([`PLAYER`]; peers / NPCs never match),
//! - the caller's return address is `ChrCam::Update`'s getter call, the FeMan reticle update's getter call or (round 7)
//!   the snipe-fade writer's getter call in the PlayerIns update (ER's aim-time body fade, `PlayerIns+0x5D8`);
//!
//! every other call (HKS `env()`, the input handler's body turn, FE, draw) jumps to the trampoline and gets ER's real
//! bit. ER's camera thus stays in camera type 0 (its follow camera: our look route, the camera-step / follow-cam
//! detours unchanged), the reticle never takes mode 2 and the body never fades out, while the body turn still sees the bit.
//!
//! Install-time checks (non-faulting reads, `mw2er_core::aim_cam::validate`): the getter's 12 bytes, both functions'
//! prologues, both call sites `E8 rel32` onto the getter, the fade writer's 33 bytes + its two timer stores; any
//! mismatch = nothing hooked (`HOLD aim cam refused`) and, with `tp_aim_cam = "keep"`, no aim episode at all
//! ([`refused`]: an invisible body or ER's aim camera would follow). The DLL is pinned before the hook is enabled (ER calls the getter from many threads all
//! frame; a FreeLibrary must never unmap the detour).

use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

use hudhook::mh::{MH_EnableHook, MH_Initialize, MH_STATUS, MhHook};
use mw2er_core::aim_cam::{AimCamAddrs, ArmIn, armed, kept_line, ret_addrs, validate};

use crate::log::{log_i, log_w};

/// The detour's inputs (read by the naked detour through `sym`): the arm, our PlayerIns, the two return addresses,
/// the trampoline, and its hit counters.
static ARMED: AtomicBool = AtomicBool::new(false);
static PLAYER: AtomicUsize = AtomicUsize::new(0);
static RET_CAM: AtomicUsize = AtomicUsize::new(0);
static RET_RET: AtomicUsize = AtomicUsize::new(0);
static RET_FADE: AtomicUsize = AtomicUsize::new(0);
static TRAMP: AtomicUsize = AtomicUsize::new(0);
static HITS_CAM: AtomicU64 = AtomicU64::new(0);
static HITS_RET: AtomicU64 = AtomicU64::new(0);
static HITS_FADE: AtomicU64 = AtomicU64::new(0);
static INSTALLED: AtomicBool = AtomicBool::new(false);
static TRIED: AtomicBool = AtomicBool::new(false);
/// C6 round 7: the install was refused / unavailable for the session (with "keep" the aim pose then stays off).
static REFUSED: AtomicBool = AtomicBool::new(false);

/// ER's `IsPrecisionShooting(ChrIns*) -> bool` with the camera / reticle filter (see the module docs). Naked: rcx is
/// untouched, `[rsp]` is the caller's return address at entry; the zero path is `xor eax,eax; ret` (the getter is a
/// leaf returning al), every other path is a tail jump to the trampoline (the original's own frame and result).
#[unsafe(naked)]
unsafe extern "C" fn getter_detour() {
    std::arch::naked_asm!(
        "cmp byte ptr [rip + {armed}], 0",
        "je 2f",
        "cmp rcx, qword ptr [rip + {player}]",
        "jne 2f",
        "mov rax, qword ptr [rsp]",
        "cmp rax, qword ptr [rip + {ret_cam}]",
        "je 3f",
        "cmp rax, qword ptr [rip + {ret_ret}]",
        "je 4f",
        "cmp rax, qword ptr [rip + {ret_fade}]",
        "je 5f",
        "2:",
        "jmp qword ptr [rip + {tramp}]",
        "3:",
        "lock inc qword ptr [rip + {hits_cam}]",
        "xor eax, eax",
        "ret",
        "4:",
        "lock inc qword ptr [rip + {hits_ret}]",
        "xor eax, eax",
        "ret",
        "5:",
        "lock inc qword ptr [rip + {hits_fade}]",
        "xor eax, eax",
        "ret",
        armed = sym ARMED,
        player = sym PLAYER,
        ret_cam = sym RET_CAM,
        ret_ret = sym RET_RET,
        ret_fade = sym RET_FADE,
        tramp = sym TRAMP,
        hits_cam = sym HITS_CAM,
        hits_ret = sym HITS_RET,
        hits_fade = sym HITS_FADE,
    )
}

/// The detour's arm (task thread). Every aim restore point calls [`disarm`] FIRST.
pub fn disarm() {
    ARMED.store(false, Ordering::SeqCst);
}

fn rd(addr: usize, out: &mut [u8]) -> bool {
    for (k, b) in out.iter_mut().enumerate() {
        // SAFETY: non-faulting read of one code byte of the exe image.
        match unsafe { crate::game::read::<u8>(addr + k) } {
            Some(v) => *b = v,
            None => return false,
        }
    }
    true
}

/// Pins the module holding the detour (GET_MODULE_HANDLE_EX_FLAG_PIN | FROM_ADDRESS): it stays loaded until process
/// exit whatever FreeLibrary calls follow.
fn pin_self() -> bool {
    use windows::Win32::System::LibraryLoader::{GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, GET_MODULE_HANDLE_EX_FLAG_PIN, GetModuleHandleExW};
    let addr = getter_detour as unsafe extern "C" fn() as usize;
    let mut h = windows::Win32::Foundation::HMODULE::default();
    // SAFETY: FROM_ADDRESS: the address of our own detour is passed as the "name" pointer; PIN: never unloaded.
    unsafe { GetModuleHandleExW(GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_PIN, windows::core::PCWSTR(addr as *const u16), &mut h) }.is_ok()
        && !h.is_invalid()
}

fn install(a: &AimCamAddrs, base: usize) -> Result<usize, &'static str> {
    validate(a, base, &rd)?;
    let target = base + a.getter;
    let (rc, rr, rf) = ret_addrs(a, base);
    RET_CAM.store(rc, Ordering::SeqCst);
    RET_RET.store(rr, Ordering::SeqCst);
    RET_FADE.store(rf, Ordering::SeqCst);
    // SAFETY: idempotent.
    match unsafe { MH_Initialize() } {
        MH_STATUS::MH_OK | MH_STATUS::MH_ERROR_ALREADY_INITIALIZED => {}
        _ => return Err("minhook"),
    }
    // SAFETY: the getter's 12 bytes were checked above (its first instruction is 7 bytes, no RIP-relative operand);
    // the detour keeps the getter's contract (rcx in, al out, a leaf) and tail-jumps to the trampoline otherwise.
    let h = unsafe { MhHook::new(target as *mut c_void, getter_detour as unsafe extern "C" fn() as *mut c_void) }.map_err(|_| "minhook")?;
    TRAMP.store(h.trampoline() as usize, Ordering::SeqCst);
    std::mem::forget(h); // lives for the process lifetime; disarmed = passthrough
    if !pin_self() {
        return Err("pin");
    }
    // SAFETY: created above, the trampoline and the return addresses stored before enabling (ARMED is false).
    match unsafe { MH_EnableHook(target as *mut c_void) } {
        MH_STATUS::MH_OK => Ok(target),
        _ => Err("minhook"),
    }
}

/// Task thread: installs the hook once (when `keep` is first wanted) and logs it.
pub fn ensure_installed() -> bool {
    if INSTALLED.load(Ordering::Relaxed) {
        return true;
    }
    if TRIED.swap(true, Ordering::SeqCst) {
        return false;
    }
    let Some(a) = crate::version_gate::own().ok().and_then(|o| o.aim_cam) else {
        REFUSED.store(true, Ordering::SeqCst);
        log_w!("HOLD", "aim cam unavailable reason=build (no verified getter sites on this exe; tp_aim_cam = \"keep\": the aim pose stays off)");
        return false;
    };
    let base = crate::game::c3_read::exe_base();
    if base == 0 {
        TRIED.store(false, Ordering::SeqCst);
        return false;
    }
    match install(&a, base) {
        Ok(t) => {
            INSTALLED.store(true, Ordering::SeqCst);
            log_i!(
                "HOLD",
                "aim cam keep installed getter=0x{t:x} sites=3 ret_cam=0x{:x} ret_ret=0x{:x} ret_fade=0x{:x} (read-only: ER's aim camera / crossbow reticle / snipe body fade masked for our own aim episode; the body turn keeps the bit)",
                RET_CAM.load(Ordering::Relaxed),
                RET_RET.load(Ordering::Relaxed),
                RET_FADE.load(Ordering::Relaxed)
            );
            true
        }
        Err(why) => {
            REFUSED.store(true, Ordering::SeqCst);
            log_w!(
                "HOLD",
                "aim cam refused reason={why} (nothing hooked; tp_aim_cam = \"keep\": the aim pose stays off - ER's aim camera / an invisible body would follow)"
            );
            false
        }
    }
}

/// C6 round 7 (aim7.md 4 A): with `tp_aim_cam = "keep"` an aim episode may start only once the filter (camera, reticle
/// AND the snipe fade) is installed; a refused / unavailable install keeps the aim pose off for the session. Installs
/// on the first call (task thread).
pub fn keep_ready() -> bool {
    ensure_installed() && !REFUSED.load(Ordering::Relaxed)
}

/// The task's per-tick arm and the per-episode line (owned by the stance).
#[derive(Default)]
pub struct AimCam {
    /// The armed span: (start, HITS_CAM, HITS_RET, HITS_FADE at its start).
    span: Option<(f64, u64, u64, u64)>,
    /// The last time an aim episode of ours ran (the release grace).
    last_episode_t: Option<f64>,
    er_logged: bool,
}

impl AimCam {
    /// One stance tick: the arm from the rule (disarmed first when not wanted), the kept line at the end of a span.
    pub fn tick(&mut self, now: f64, keep_cfg: bool, player: usize, i: ArmIn) {
        if i.episode {
            self.last_episode_t = Some(now);
        }
        let keep = keep_cfg && ensure_installed();
        let since_end_s = match (i.episode, self.last_episode_t) {
            (true, _) => 0.0,
            (false, Some(t)) => now - t,
            (false, None) => f64::INFINITY,
        };
        let want = armed(&ArmIn { keep, since_end_s, ..i });
        if !want {
            disarm();
        }
        if keep {
            // the compare value before the arm (a player change re-read every tick; 0 = nothing matches)
            PLAYER.store(player, Ordering::SeqCst);
        }
        if want {
            ARMED.store(true, Ordering::SeqCst);
        }
        match (want, self.span) {
            (true, None) => self.span = Some((now, HITS_CAM.load(Ordering::Relaxed), HITS_RET.load(Ordering::Relaxed), HITS_FADE.load(Ordering::Relaxed))),
            (false, Some((t0, c0, r0, f0))) => {
                self.span = None;
                let (c, r, f) = (HITS_CAM.load(Ordering::Relaxed) - c0, HITS_RET.load(Ordering::Relaxed) - r0, HITS_FADE.load(Ordering::Relaxed) - f0);
                log_i!("HOLD", "{}", kept_line(((now - t0) * 1000.0).round().max(0.0) as u64, c, r, f));
            }
            _ => {}
        }
        // the escape hatch in effect for an episode: once per switch to "er"
        if i.episode && !keep_cfg && i.tp {
            if !self.er_logged {
                self.er_logged = true;
                log_i!("HOLD", "aim cam er (tp_aim_cam = \"er\": ER's over-the-shoulder aim camera and crossbow reticle for the aim episodes)");
            }
        } else if keep_cfg {
            self.er_logged = false;
        }
    }

    /// A hard stop (stance exit / restore_all / disable / panic): disarmed at once; the span line follows next tick.
    pub fn stop(&mut self) {
        disarm();
        // no release grace after a hard stop (F5, AC-130, a weapon switch, a disable, panic): ER's camera as it is
        self.last_episode_t = None;
    }
}
