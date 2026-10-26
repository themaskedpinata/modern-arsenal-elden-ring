//! mw2er.dll: Modern Warfare 2 (2009) guns inside Elden Ring 1.17.1 (exe 2.7.1.0), loaded by me3.
//!
//! Milestone M2 (docs/M2_DESIGN.md): F5 toggles "MW2 mode"; the UMP45 fires real engine bullets at
//! MW2 cadence with MW2 damage falloff, ammo and reload, an ImGui crosshair/hitmarker/ammo HUD, and
//! the converted MW2 sounds. Every rule that can be expressed on plain numbers lives in
//! `mw2er-core` (unit-tested); this crate reads the game, calls the core, and acts.
//!
//! Threads and boundaries (each one catches panics and logs them):
//! - loader thread: [`DllMain`] only spawns the boot thread.
//! - `mw2er-boot`: version gate, hooks, boot gate, task registration, then config hot reload.
//! - game main thread: the recurring `CSTask` closure ([`task`]) runs all gameplay.
//! - present thread: the ImGui render loop ([`hud`]) reads a snapshot only.
//! - input thread(s): the DirectInput `GetDeviceState` detour ([`input_hook`]) touches atomics only.
//! - `mw2er-audio`: rodio output ([`audio`]).
//! - M3: the pre-ImGui callback on the present thread and the `mw2er-vmload` loader ([`viewmodel`]).
//! - M3b: first person ([`first_person`], run by the task; optional camera-step hook [`cam_hook`]).
//! - M4: the AC-130 killstreak ([`ac130`], run by the task; camera mode 2 of [`cam_hook`], the thermal pass in
//!   [`viewmodel`]'s pre-ImGui callback).
//! - M7: the emitter watchdog every spawn path asks first ([`spawn_guard`]) and the MW2 Tactical Nuke ([`nuke`], run
//!   by the task; its post pass in the pre-ImGui callback, [`nuke_present`]).
//! - C4: ER's render thread runs our UiBegin / UiEnd detours ([`ui_probe`]) and the early record ([`ui_inject`]);
//!   hudhook's ExecuteCommandLists callback (patch 0002) splices our list ahead of ER's UI.

#![deny(unsafe_op_in_unsafe_fn)]

mod ac130;
mod aim_cam;
mod arsenal_rt;
mod arsenal_task;
mod audio;
mod boot;
mod cam_hook;
/// C6 part 16 (docs/C6_HOLD_DESIGN.md 16): the real crossbow carrier (ER's own EquipItem on the game thread, journal first).
mod carrier;
mod config_watch;
mod coop;
/// C3 PASS C: co-op visibility (twins out, peer effects in, markers, world guns).
mod coop_vis;
mod data;
/// C6 PASS H2 (docs/C6_HOLD_DESIGN.md part 3): the crossbow stance of the hidden ER weapon (runtime param patch + restore).
mod er_stance;
/// C6 depth probe P-D (read-only): the chain to ER's scene depth, its desc, the GX wrapper words.
mod er_depth;
/// C6 depth 5: the read-only ResourceBarrier probe, the fold, the ER-depth copy / sample and the prime (docs part 15).
mod er_depth_state;
mod first_person;
mod game;
mod guard;
mod hud;
mod input_hook;
mod log;
mod movement;
mod nuke;
mod nuke_present;
/// C3 PASS B: the XInput gamepad (`xinput1_4!XInputGetState` detour + the task half).
mod pad;
mod paths;
mod shared;
/// C6 part 18: the third-person over-the-shoulder camera (task side; the write is in cam_hook).
mod shoulder_cam;
mod singletons;
mod spawn;
mod spawn_guard;
mod task;
mod thermal_present;
/// C4: the viewmodel under ER's UI (off by default: `[ui] inject_viewmodel`).
mod ui_inject;
/// C3 + C4: the Scaleform UI-pass probe (off by default; read-only, logs only) and the UiBegin / UiEnd hooks.
mod ui_probe;
/// C4.1 probe P2: the layer sample (off by default: `[ui] probe_layer_sample`, with the probe).
mod ui_sample;
mod version_gate;
mod viewmodel;

use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, Ordering};

use windows::Win32::Foundation::HINSTANCE;

static BOOT_STARTED: AtomicBool = AtomicBool::new(false);

const DLL_PROCESS_ATTACH: u32 = 1;
const DLL_PROCESS_DETACH: u32 = 0;

/// DLL entry point. Runs under the loader lock, so on attach it only starts the boot thread (which begins
/// running after the lock is released) and returns TRUE. No hooks, no singletons, no file I/O, no
/// panics here. On detach (C4, docs/C4_UI_DESIGN.md S1): only for FreeLibrary (`reserved` is null), never at
/// process exit (the process and the device are going away, and MinHook under the loader lock at exit is unsafe),
/// the C4 hooks come out ([`detach`]). me3 never FreeLibrary's, so in practice the hot disable is the path used.
///
/// # Safety
/// Called by the Windows loader only.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn DllMain(hinst: HINSTANCE, reason: u32, reserved: *mut c_void) -> i32 {
    if reason == DLL_PROCESS_ATTACH && !BOOT_STARTED.swap(true, Ordering::SeqCst) {
        let raw = hinst.0 as usize;
        let _ = std::panic::catch_unwind(move || {
            // Builder::spawn reports failure as Err instead of panicking; ignore it (inert DLL).
            let _ = std::thread::Builder::new().name("mw2er-boot".into()).spawn(move || boot::run(raw));
        });
    } else if reason == DLL_PROCESS_DETACH && reserved.is_null() {
        let _ = std::panic::catch_unwind(detach);
    }
    1
}

/// FreeLibrary only: the C4 teardown (passthrough, MH_DisableHook of the UiBegin / UiEnd / C3 probe
/// hooks, fence wait, release or leak). No logging (loader lock). Hooks of other modules (pad, camera, input) are
/// follow-ups with the same pattern.
fn detach() {
    // C6 depth 5: the barrier hook's recording off, then disabled (passthrough; trampoline kept). Once that hook is
    // installed the DLL is pinned (er_depth_state::pin_self, review 2026-10-21), so a FreeLibrary reaching here means it
    // never was (or install failed at the pin): this is then a no-op for the barrier hook.
    er_depth_state::teardown();
    // C6 round 6: the getter filter disarmed (a passthrough; the DLL is pinned once it was installed)
    aim_cam::disarm();
    if ui_probe::any_installed() {
        let _ = ui_inject::teardown(true);
    }
}
