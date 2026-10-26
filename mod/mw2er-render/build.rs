//! Compiles the HLSL shaders with fxc (SM 5.1, DXBC) into OUT_DIR; the crate embeds them with `include_bytes!`, so
//! mw2er.dll and vmtest.exe carry exactly the same bytes:
//! - shaders/viewmodel.hlsl (+ cutout.hlsli, C5) -> viewmodel_vs.cso, viewmodel_ps.cso (M3);
//! - shaders/thermal.hlsl -> thermal_vs.cso, thermal_ps.cso (M4);
//! - shaders/tracer.hlsl -> tracer_vs.cso, tracer_ps.cso (run 8: AC-130 round tracers);
//! - shaders/overlay.hlsl -> overlay_vs.cso, overlay_ps.cso (M6: the sniper scope overlay);
//! - shaders/nuke.hlsl -> nuke_vs.cso, nuke_bloom_ps.cso, nuke_ps.cso (M7: the nuke flash / white-out / aftermath);
//! - shaders/layer_sample.hlsl -> layer_sample_cs.cso (C4.1 probe P2: 16 texels of ER's UI layer, compute);
//! - shaders/solid.hlsl -> solid_vs.cso, solid_ps.cso (C4.1 PASS I41: the canary texel, one constant colour);
//! - shaders/occluder.hlsl -> occluder_vs.cso (C6 depth fallback: depth-only body capsules before the world guns);
//! - shaders/er_depth.hlsl -> er_depth_copy_cs.cso, er_depth_sample_cs.cso, er_depth_vs.cso, er_depth_prime_ps.cso (C6
//!   depth 5: ER's scene depth copied / sampled through our SRV, primed into our world-gun depth).
//! fxc: `MW2ER_FXC`, else the Windows SDK 10.0.26100.0 path. A missing fxc or a shader error fails the build;
//! there is no silent fallback (docs/M3_DESIGN.md section 4.3).

#[path = "../mw2er-core/src/sha256.rs"]
#[allow(dead_code)]
mod sha256;

use std::path::PathBuf;
use std::process::Command;

const DEFAULT_FXC: &str = r"C:\Program Files (x86)\Windows Kits\10\bin\10.0.26100.0\x64\fxc.exe";

fn main() {
    println!("cargo:rerun-if-changed=shaders/viewmodel.hlsl");
    println!("cargo:rerun-if-changed=shaders/cutout.hlsli");
    println!("cargo:rerun-if-changed=shaders/thermal.hlsl");
    println!("cargo:rerun-if-changed=shaders/tracer.hlsl");
    println!("cargo:rerun-if-changed=shaders/overlay.hlsl");
    println!("cargo:rerun-if-changed=shaders/nuke.hlsl");
    println!("cargo:rerun-if-changed=shaders/layer_sample.hlsl");
    println!("cargo:rerun-if-changed=shaders/solid.hlsl");
    println!("cargo:rerun-if-changed=shaders/occluder.hlsl");
    println!("cargo:rerun-if-changed=shaders/er_depth.hlsl");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=MW2ER_FXC");
    let fxc = std::env::var("MW2ER_FXC").unwrap_or_else(|_| DEFAULT_FXC.to_string());
    if !std::path::Path::new(&fxc).is_file() {
        panic!("fxc not found at {fxc} (set MW2ER_FXC); the shaders cannot be built");
    }
    let out = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR"));
    let dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("manifest dir")).join("shaders");
    let jobs = [
        ("viewmodel.hlsl", "vs_5_1", "vs_main", "viewmodel_vs.cso", "MW2ER_VS_SHA256"),
        ("viewmodel.hlsl", "ps_5_1", "ps_main", "viewmodel_ps.cso", "MW2ER_PS_SHA256"),
        ("thermal.hlsl", "vs_5_1", "vs_fullscreen", "thermal_vs.cso", "MW2ER_THERMAL_VS_SHA256"),
        ("thermal.hlsl", "ps_5_1", "ps_thermal", "thermal_ps.cso", "MW2ER_THERMAL_PS_SHA256"),
        ("tracer.hlsl", "vs_5_1", "vs_tracer", "tracer_vs.cso", "MW2ER_TRACER_VS_SHA256"),
        ("tracer.hlsl", "ps_5_1", "ps_tracer", "tracer_ps.cso", "MW2ER_TRACER_PS_SHA256"),
        ("overlay.hlsl", "vs_5_1", "vs_overlay", "overlay_vs.cso", "MW2ER_OVERLAY_VS_SHA256"),
        ("overlay.hlsl", "ps_5_1", "ps_overlay", "overlay_ps.cso", "MW2ER_OVERLAY_PS_SHA256"),
        ("nuke.hlsl", "vs_5_1", "vs_fullscreen", "nuke_vs.cso", "MW2ER_NUKE_VS_SHA256"),
        ("nuke.hlsl", "ps_5_1", "ps_bloom", "nuke_bloom_ps.cso", "MW2ER_NUKE_BLOOM_PS_SHA256"),
        ("nuke.hlsl", "ps_5_1", "ps_nuke", "nuke_ps.cso", "MW2ER_NUKE_PS_SHA256"),
        ("layer_sample.hlsl", "cs_5_1", "cs_sample", "layer_sample_cs.cso", "MW2ER_LAYER_SAMPLE_CS_SHA256"),
        ("solid.hlsl", "vs_5_1", "vs_solid", "solid_vs.cso", "MW2ER_SOLID_VS_SHA256"),
        ("solid.hlsl", "ps_5_1", "ps_solid", "solid_ps.cso", "MW2ER_SOLID_PS_SHA256"),
        ("occluder.hlsl", "vs_5_1", "vs_capsule", "occluder_vs.cso", "MW2ER_OCCLUDER_VS_SHA256"),
        ("er_depth.hlsl", "cs_5_1", "cs_copy", "er_depth_copy_cs.cso", "MW2ER_ER_DEPTH_COPY_CS_SHA256"),
        ("er_depth.hlsl", "cs_5_1", "cs_sample", "er_depth_sample_cs.cso", "MW2ER_ER_DEPTH_SAMPLE_CS_SHA256"),
        ("er_depth.hlsl", "vs_5_1", "vs_full", "er_depth_vs.cso", "MW2ER_ER_DEPTH_VS_SHA256"),
        ("er_depth.hlsl", "ps_5_1", "ps_prime", "er_depth_prime_ps.cso", "MW2ER_ER_DEPTH_PRIME_PS_SHA256"),
    ];
    for (srcname, profile, entry, file, env) in jobs {
        let src = dir.join(srcname);
        let dst = out.join(file);
        let o = Command::new(&fxc)
            .args(["/nologo", "/T", profile, "/E", entry, "/O3", "/WX", "/Qstrip_debug", "/Fo"])
            .arg(&dst)
            .arg(&src)
            .output()
            .unwrap_or_else(|e| panic!("running {fxc}: {e}"));
        if !o.status.success() {
            panic!(
                "fxc {srcname} {profile} {entry} failed:\n{}\n{}",
                String::from_utf8_lossy(&o.stdout),
                String::from_utf8_lossy(&o.stderr)
            );
        }
        let bytes = std::fs::read(&dst).expect("fxc output");
        assert!(bytes.starts_with(b"DXBC"), "{file} is not DXBC");
        println!("cargo:rustc-env={env}={}", sha256::sha256_hex(&bytes));
    }
}
