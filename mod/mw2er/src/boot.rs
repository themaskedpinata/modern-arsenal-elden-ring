//! Boot thread (docs/M2_DESIGN.md section 5.2). The order is fixed:
//! paths + log -> VERSION GATE -> config + data -> hudhook DX12 -> DirectInput hook -> audio ->
//! game window -> fresh reflection scan -> CSTask registration -> config watcher loop.
//!
//! Nothing touches fromsoftware-rs before the version gate passes, and no `FromStatic` instance is
//! looked up before the reflection scan contains every required name.

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant, UNIX_EPOCH};

use mw2er_core::config::Config;
use mw2er_core::logfmt::utc_from_unix;

use crate::config_watch::Watcher;
use crate::guard::guard;
use crate::log::{self, log_e, log_i, log_w, set_thread_role};
use crate::shared::{self, BOOTED, DATA_OK, HUD_APPLIED, INPUT_HOOK_OK, TASK_OK};
use crate::{audio, cam_hook, data, hud, input_hook, paths, singletons, task, version_gate};

/// Entry of the `mw2er-boot` thread.
pub fn run(hmodule_raw: usize) {
    shared::init_clock();
    set_thread_role("boot");
    let _ = guard("boot", || boot(hmodule_raw));
}

fn load_config(paths: &paths::Paths) -> (Config, Option<String>) {
    if !paths.root_valid {
        return (Config::default(), None);
    }
    match std::fs::read_to_string(paths.config_file()) {
        Ok(t) => match Config::parse(&t) {
            Ok(c) => (c, None),
            Err(e) => (Config::default(), Some(e.to_string())),
        },
        Err(e) => (Config::default(), Some(e.to_string())),
    }
}

fn boot(hmodule_raw: usize) {
    let started = Instant::now();

    // 1-2. Paths, config (it names the log folder), log file, panic hook, hudhook's tracing.
    // Without our own absolute path there is no safe place for the log: stay inert.
    let Some(paths) = paths::resolve(hmodule_raw) else {
        log::debug_out("mw2er: cannot resolve the DLL's own absolute path; staying inert (no log written)");
        return;
    };
    let (cfg, cfg_err) = load_config(&paths);
    let log_path = paths.log_file(&cfg.paths.log_dir);
    let log_ok = log::init(&log_path).is_ok(); // without a log we continue silently
    log::install_panic_hook();
    log::init_tracing(&cfg.log.hudhook_level);
    let meta = std::fs::metadata(&paths.dll).ok();
    let mtime = meta
        .as_ref()
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map_or("?".into(), |d| utc_from_unix(d.as_secs() as i64).date_time() + "Z");
    let built: i64 = option_env!("MW2ER_BUILD_UNIX").and_then(|s| s.parse().ok()).unwrap_or(0);
    log_i!(
        "BOOT",
        "mw2er {} dll={} size={} mtime={} built={}Z pid={} root={} root_valid={} log_ok={}",
        env!("CARGO_PKG_VERSION"),
        paths.dll.display(),
        meta.as_ref().map_or(0, |m| m.len()),
        mtime,
        utc_from_unix(built).date_time(),
        std::process::id(),
        paths.root.display(),
        paths.root_valid,
        log_ok
    );

    // 3. Version gate: inert on anything but eldenring.exe 2.7.1.0 / 2.7.2.0 whose every address passed the preflight.
    match version_gate::check() {
        Ok(info) => {
            log_i!("VERSION", "{}", info.pass_line());
            if !info.inferred.is_empty() {
                let why = match info.build {
                    mw2er_core::exe_build::ExeBuild::Ww2720 => "the mapper's first of several matches; none is used by mw2er",
                    mw2er_core::exe_build::ExeBuild::Ww2710 => "section kind only, no bytes pinned for that exe",
                };
                log_i!(
                    "VERSION",
                    "table {} inferred={} ({why}): {}",
                    info.build.table_name(),
                    info.inferred.len(),
                    info.inferred.iter().take(8).copied().collect::<Vec<_>>().join(",")
                );
            }
            // Tolerated differences on the loaded image (exe_build::Role): not inert, but worth a line each.
            for w in info.warnings.iter().take(16) {
                log_w!("VERSION", "WARN preflight {w}");
            }
            // M3b: game-memory writes (first person) are allowed only from here on.
            shared::WRITES_ALLOWED.store(true, Ordering::SeqCst);
        }
        Err(e) => {
            log_e!("VERSION", "FAIL {e}; staying inert");
            return;
        }
    }

    // 4. Config + data.
    if !paths.root_valid {
        log_w!("CONFIG", "WARN no mw2er.toml at {}; using defaults", paths.root.display());
    } else if let Some(e) = &cfg_err {
        log_e!("CONFIG", "ERROR {e}; using defaults");
    } else {
        log_i!(
            "CONFIG",
            "loaded {} weapon={} k={} toggle={} reload={} camera={}",
            paths.config_file().display(),
            cfg.gameplay.weapon,
            cfg.damage.k,
            cfg.keys.toggle,
            cfg.keys.reload,
            cfg.gameplay.camera_source.label()
        );
    }
    // C6 part 16: the carrier's journal lives in the mod's own root only (`<root>\state`; no valid root = no carrier)
    if paths.root_valid {
        crate::carrier::set_root(&paths.root);
    }
    let cfg = Arc::new(cfg);
    shared::set_config(cfg.clone());
    let game_data = data::load_logged(&paths, &cfg);
    // M6: the arsenal (converted\guns + rows.json's arsenal block); a failure keeps the M2-M5 single gun
    let arsenal = crate::arsenal_rt::load_logged(&paths, &cfg);
    // M5: converted\movement.json (a failure keeps the movement controller off: MOVE blocked reason=no_data)
    crate::movement::load_data(&paths, &cfg);
    DATA_OK.store(game_data.is_some(), Ordering::SeqCst);
    // M4: converted\ac130 (a failure only makes the AC-130 unavailable)
    let ac_data = if cfg.ac130.enabled {
        data::load_ac130_logged(&paths, &cfg, game_data.as_ref().map(|d| &d.rows))
    } else {
        log_i!("AC130", "disabled by config ([ac130] enabled = false)");
        None
    };
    // M7: converted\nuke + rows.json's nuke block (a failure only makes the nuke unavailable: NUKE blocked reason=data)
    let nuke_data = if cfg.nuke.enabled {
        data::load_nuke_logged(&paths, &cfg, game_data.as_ref().map(|d| &d.rows))
    } else {
        log_i!("NUKE", "disabled by config ([nuke] enabled = false)");
        None
    };

    // 5. hudhook now, before the game creates its swapchain (captures the command queue there).
    if cfg.hud.enabled {
        match hud::apply(hmodule_raw, paths.root.clone()) {
            Ok(()) => {
                HUD_APPLIED.store(true, Ordering::SeqCst);
                log_i!("HUD", "applied (DX12 hooks enabled; waiting for the first Present)");
            }
            Err(e) => log_e!("HUD", "FAIL {e}; no HUD, gameplay still allowed"),
        }
    } else {
        log_i!("HUD", "disabled by config");
    }

    // 6. DirectInput hook on hudhook's MinHook.
    match guard("input.install", input_hook::install) {
        Some(Ok(addrs)) => {
            INPUT_HOOK_OK.store(true, Ordering::SeqCst);
            let list: Vec<String> = addrs.iter().map(|a| format!("0x{a:x}")).collect();
            log_i!("INPUT", "PASS hooked={} addrs=[{}]", addrs.len(), list.join(","));
        }
        Some(Err(e)) => log_e!("INPUT", "FAIL {e}; MW2 mode will refuse to enable"),
        None => log_e!("INPUT", "FAIL panic; MW2 mode will refuse to enable"),
    }

    // 6b. C3 PASS B (docs/C3_DESIGN.md 4.2): the XInput gamepad detour (system DLL export; never fatal).
    if cfg.gamepad.enabled {
        match guard("pad.install", crate::pad::install) {
            Some(Ok(info)) => log_i!("PAD", "hook ok {info}"),
            Some(Err(e)) => log_e!("PAD", "hook FAIL {e}; no gamepad (keyboard and mouse unaffected)"),
            None => log_e!("PAD", "hook FAIL panic; no gamepad (keyboard and mouse unaffected)"),
        }
    } else {
        log_i!("PAD", "off ([gamepad] enabled = false at boot; nothing hooked)");
    }

    // 7. Audio (never fatal).
    let files = game_data.as_ref().map(|d| d.sound_files.clone()).unwrap_or_default();
    let ac_files = ac_data.as_ref().map(|d| d.sound_files.clone()).unwrap_or_default();
    if !cfg.audio.enabled {
        log_i!("AUDIO", "disabled by config");
    } else if files.is_empty() && ac_files.is_empty() && arsenal.is_none() && nuke_data.is_none() {
        log_w!("AUDIO", "WARN no sound files; silent");
    } else if audio::start(files, ac_files, cfg.audio.fire_max_ms, cfg.audio.fire_fade_ms) {
        log_i!("AUDIO", "thread started");
    }

    // 8-9. Boot gate: the game window, then a fresh reflection scan with every required name.
    log_i!("BOOTGATE", "waiting for the game window, then reflection names [{}]", singletons::required_names().join(","));
    singletons::wait_for_window();
    singletons::wait_for_reflection();

    // 10. The per-frame task.
    match guard("task.register", || task::register(game_data)) {
        Some(Ok(())) => {
            TASK_OK.store(true, Ordering::SeqCst);
            log_i!("TASK", "registered group=ChrIns_PostPhysics");
        }
        Some(Err(e)) => log_e!("TASK", "FAIL {e}; gameplay off"),
        None => log_e!("TASK", "FAIL panic; gameplay off"),
    }

    // 10b. M3b first person, Tier B only: the camera-step hook, then (after run 5) the follow-cam hook (both after
    // the version gate; only with first person enabled and camera_mode = "hook", or (M4) with [ac130] enabled: the
    // gunship camera is mode 2 of the same detour). On failure of the camera-step hook first person falls back to
    // the task-phase write and F7 answers `AC130 blocked reason=no_cam_hook`; on failure of the follow-cam hook only
    // the camera auto-rotation / wall-recovery handling is off.
    if cfg.first_person.enabled && cfg.first_person.camera_mode == mw2er_core::config::CameraMode::Hook || cfg.ac130.enabled {
        match guard("fp.hook", cam_hook::install) {
            Some(Ok(addr)) => {
                log_i!(
                    "FPHOOK",
                    "installed rva=0x{:x} addr=0x{addr:x} prologue=ok (camera_mode=hook; erfps2 RVA, see cam_hook.rs)",
                    cam_hook::camera_step_rva()
                );
                match guard("fp.follow_hook", cam_hook::install_follow) {
                    Some(Ok(f)) => {
                        log_i!(
                            "FPHOOK",
                            "follow installed rva=0x{:x} addr=0x{:x} prologue=ok (ChrExFollowCam update; erfps2 RVA)",
                            cam_hook::follow_cam_rva(),
                            f.addr
                        );
                        match f.wall {
                            Ok((a, v)) => log_i!(
                                "FPHOOK",
                                "CAM_WALL_RECOVERY rva=0x{:x} addr=0x{a:x} value={v} validated (.data, MEM_IMAGE, writable bool; cleared only while first person is on)",
                                cam_hook::cam_wall_recovery_rva()
                            ),
                            Err(e) => log_w!("FPHOOK", "WARN CAM_WALL_RECOVERY not validated ({e}); it is never touched"),
                        }
                    }
                    Some(Err(e)) => log_e!("FPHOOK", "FAIL follow {e}; camera auto rotation / wall recovery are left to ER"),
                    None => log_e!("FPHOOK", "FAIL follow panic; camera auto rotation / wall recovery are left to ER"),
                }
            }
            Some(Err(e)) => log_e!("FPHOOK", "FAIL {e}; first person uses camera_mode=task; the AC-130 is unavailable (no_cam_hook)"),
            None => log_e!("FPHOOK", "FAIL panic; first person uses camera_mode=task; the AC-130 is unavailable (no_cam_hook)"),
        }
    } else if cfg.first_person.camera_mode == mw2er_core::config::CameraMode::Hook {
        log_i!("FPHOOK", "not installed: [first_person] enabled = false at boot");
    }

    // 10c. C3 (docs/C3_DESIGN.md 2.4): the Scaleform UI-pass probe, only when asked for (cold, off by default).
    if cfg.ui.probe_scaleform {
        if guard("ui.probe", crate::ui_probe::install).is_none() {
            log_e!("UI", "FAIL probe panic; nothing hooked");
        }
    } else {
        log_i!("UI", "probe off ([ui] probe_scaleform = false)");
    }
    // 10d. C4 (docs/C4_UI_DESIGN.md 2 / 3): ER's UiBegin / UiEnd and hudhook's ECL callback, only when the probe or
    // the injection asks for them (both cold, both off by default: then nothing is installed and nothing is logged).
    for w in cfg.ui.warnings() {
        log_w!("CONFIG", "WARN {w}");
    }
    if cfg.ui.probe_scaleform || cfg.ui.inject_viewmodel {
        let ok = guard("ui.c4", || crate::ui_probe::install_ui(cfg.ui.probe_scaleform, cfg.ui.probe_layer_sample, cfg.ui.ui_thread_effective())).unwrap_or(false);
        if cfg.ui.inject_viewmodel {
            crate::ui_inject::boot(ok, cfg.ui.probe_scaleform, cfg.ui.ui_thread_effective());
        }
    }

    // 10e. C6 depth probe (docs/C6_HOLD_DESIGN.md 13.3; cold, off by default): the chain's sites on the loaded image
    if cfg.coop.probe_er_depth {
        let _ = guard("depth.boot", || crate::er_depth::boot(true));
    }
    // 10f. C6 depth 5 (docs/C6_HOLD_DESIGN.md 15; all cold, off by default): the barrier probe / sample / ER-depth prime
    // flags; the ResourceBarrier hook itself is installed by the task once a Present published our list's vtable
    if cfg.coop.probe_er_depth_state || cfg.coop.world_gun_er_depth {
        let _ = guard("depth.state_boot", || crate::er_depth_state::boot(cfg.coop.probe_er_depth_state, cfg.coop.probe_er_depth_sample, cfg.coop.world_gun_er_depth));
    }

    // 11. Done.
    BOOTED.store(true, Ordering::SeqCst);
    let failed: Vec<&str> = [
        ("data", DATA_OK.load(Ordering::SeqCst)),
        ("input", INPUT_HOOK_OK.load(Ordering::SeqCst)),
        ("task", TASK_OK.load(Ordering::SeqCst)),
    ]
    .into_iter()
    .filter(|(_, ok)| !ok)
    .map(|(n, _)| n)
    .collect();
    log_i!(
        "BOOT",
        "DONE in {} ms (hud={} input={} audio_thread={} data={} task={})",
        started.elapsed().as_millis(),
        HUD_APPLIED.load(Ordering::SeqCst) as u8,
        INPUT_HOOK_OK.load(Ordering::SeqCst) as u8,
        shared::SFX_TX.get().is_some() as u8,
        DATA_OK.load(Ordering::SeqCst) as u8,
        TASK_OK.load(Ordering::SeqCst) as u8
    );
    if !failed.is_empty() {
        shared::toast(format!("mw2er: {} failed - see logs\\mw2er.log", failed.join(", ")), 15.0, true);
    } else if cfg.hud.show_boot_toast {
        shared::toast(format!("mw2er ready - {}: MW2 mode", cfg.keys.toggle), 5.0, false);
    }

    // 12. Config hot reload + STATE lines, forever.
    let mut watcher = Watcher::new(paths.config_file());
    loop {
        std::thread::sleep(Duration::from_secs(1));
        let _ = guard("config_watch", || {
            watcher.poll();
            watcher.state_line();
        });
    }
}
