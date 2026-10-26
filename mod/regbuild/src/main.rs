//! regbuild CLI (dev / equivalence tool; the installer calls the library in-process).
//!
//!   regbuild build     [--root DIR] [--vanilla FILE] [--out FILE] [--rows FILE]   build + verify + publish
//!   regbuild unpack    <regulation.bin> <DIR>              write the BND4 and the 194 .param files (game-derived:
//!                                                           keep them under the gitignored work\)
//!   regbuild diff      <A.bin> <B.bin> [--rows A.json B.json]   param / row / field equivalence report
//!   regbuild roundtrip [--vanilla FILE] [--out FILE]        vanilla -> unpack -> repack (no edits) -> re-read
//!   regbuild plan      [--root DIR] --log build_*.json      the plan vs a build_regulation.py log (row by row)
//!   regbuild self-test [--root DIR] [--vanilla FILE]        the mutation cases must all be caught
//! Exit 0 = PASS / equivalent, 1 = FAIL / differences, 2 = usage.

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Instant;

use regbuild::{bnd4, diff, plan, pyfmt, rows_json, selftest, verify};

const USAGE: &str = "usage: regbuild build|unpack|diff|roundtrip|plan|self-test ... (see the source header)";

fn find_root() -> Option<PathBuf> {
    let starts = [std::env::current_dir().ok(), std::env::current_exe().ok()];
    for s in starts.into_iter().flatten() {
        for a in s.ancestors() {
            if a.join("mw2er.toml").is_file() && a.join("converted").is_dir() {
                return Some(a.to_path_buf());
            }
        }
    }
    None
}

fn default_vanilla(root: &Path) -> PathBuf {
    if let Some(v) = std::env::var_os("MW2ER_GAME_REGULATION").filter(|v| !v.is_empty()) {
        return PathBuf::from(v);
    }
    let staged = root.join(r"work\regulation\vanilla\regulation.bin");
    if staged.is_file() {
        return staged;
    }
    PathBuf::from(r"D:\SteamLibrary\steamapps\common\ELDEN RING\Game\regulation.bin")
}

struct Opts {
    pos: Vec<String>,
    root: Option<PathBuf>,
    vanilla: Option<PathBuf>,
    out: Option<PathBuf>,
    rows: Option<PathBuf>,
    rows_ab: Option<(PathBuf, PathBuf)>,
    log: Option<PathBuf>,
}

fn parse(args: &[String]) -> Result<Opts, String> {
    let mut o = Opts { pos: vec![], root: None, vanilla: None, out: None, rows: None, rows_ab: None, log: None };
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut val = || it.next().map(PathBuf::from).ok_or(format!("{a} needs a value"));
        match a.as_str() {
            "--root" => o.root = Some(val()?),
            "--vanilla" => o.vanilla = Some(val()?),
            "--out" => o.out = Some(val()?),
            "--log" => o.log = Some(val()?),
            "--rows" => {
                let x = val()?;
                match it.next() {
                    Some(y) if !y.starts_with("--") => o.rows_ab = Some((x, PathBuf::from(y))),
                    _ => o.rows = Some(x),
                }
            }
            x if x.starts_with("--") => return Err(format!("unknown option {x}")),
            x => o.pos.push(x.to_string()),
        }
    }
    Ok(o)
}

fn read(p: &Path) -> Result<Vec<u8>, String> {
    std::fs::read(p).map_err(|e| format!("{}: {e}", p.display()))
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(cmd) = args.first().cloned() else {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    };
    let o = match parse(&args[1..]) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("{e}\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    let r = match cmd.as_str() {
        "build" => cmd_build(&o),
        "unpack" => cmd_unpack(&o),
        "diff" => cmd_diff(&o),
        "roundtrip" => cmd_roundtrip(&o),
        "plan" => cmd_plan(&o),
        "self-test" => cmd_selftest(&o),
        _ => {
            eprintln!("{USAGE}");
            return ExitCode::from(2);
        }
    };
    match r {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::from(1),
        Err(e) => {
            println!("REGULATION FAIL {e}");
            ExitCode::from(1)
        }
    }
}

fn root_of(o: &Opts) -> Result<PathBuf, String> {
    o.root.clone().or_else(find_root).ok_or_else(|| "MW2inER root not found (--root DIR)".into())
}

fn cmd_build(o: &Opts) -> Result<bool, String> {
    let root = root_of(o)?;
    let vanilla = o.vanilla.clone().unwrap_or_else(|| default_vanilla(&root));
    let package = o.out.clone().unwrap_or_else(|| root.join(r"build\package\regulation.bin"));
    let rows = o.rows.clone().unwrap_or_else(|| root.join(r"build\rows.json"));
    let t0 = Instant::now();
    let opts = regbuild::BuildOptions { root, vanilla: vanilla.clone(), package: package.clone(), rows_json: rows.clone(), built_utc: None, cancel: None };
    println!("vanilla {} (read only)", vanilla.display());
    let r = regbuild::build(&opts, &mut |s| println!("... {s}"))?;
    for l in &r.lines {
        println!("{l}");
    }
    println!("timings: {} (total {:.1} s)", r.timings.iter().map(|(n, s)| format!("{n} {s:.2} s")).collect::<Vec<_>>().join(", "), t0.elapsed().as_secs_f64());
    println!("wrote {} ({} B) sha256 {}", package.display(), r.package_len, r.package_sha256);
    println!("wrote {}", rows.display());
    println!(
        "REGULATION PASS bullet+{} atk+{} speffect+{} package={} builder=\"{}\"",
        r.added[0].1,
        r.added[1].1,
        r.added[2].1,
        &r.package_sha256[..12],
        regbuild::BUILDER_ID
    );
    Ok(true)
}

fn cmd_unpack(o: &Opts) -> Result<bool, String> {
    let [src, dir] = o.pos.as_slice() else { return Err("unpack <regulation.bin> <DIR>".into()) };
    let u = regbuild::unpack(&read(Path::new(src))?)?;
    let b = u.bnd4()?;
    let dir = PathBuf::from(dir);
    std::fs::create_dir_all(dir.join("params")).map_err(|e| e.to_string())?;
    std::fs::write(dir.join("regulation.bnd4"), &u.bnd).map_err(|e| e.to_string())?;
    for (k, e) in b.entries.iter().enumerate() {
        std::fs::write(dir.join("params").join(e.file_name()), b.data(k)).map_err(|e| e.to_string())?;
    }
    println!("{}: sha256 {}, BND4 {} B version {} with {} files -> {}", src, u.file_sha256, u.bnd.len(), b.version, b.entries.len(), dir.display());
    Ok(true)
}

fn cmd_diff(o: &Opts) -> Result<bool, String> {
    let [a, b] = o.pos.as_slice() else { return Err("diff <A.bin> <B.bin> [--rows A.json B.json]".into()) };
    let t0 = Instant::now();
    let r = diff::diff(&read(Path::new(a))?, &read(Path::new(b))?)?;
    println!("A = {a}\nB = {b}");
    for l in &r.lines {
        println!("{l}");
    }
    let mut ok = r.equivalent();
    if let Some((ra, rb)) = &o.rows_ab {
        let d = rows_json::compare(&String::from_utf8_lossy(&read(ra)?), &String::from_utf8_lossy(&read(rb)?))?;
        println!("rows.json (without {:?}): {}", rows_json::VOLATILE, if d.is_empty() { "identical values and types".to_string() } else { format!("{} differences", d.len()) });
        for x in d.iter().take(40) {
            println!("  {x}");
        }
        ok &= d.is_empty();
    }
    println!("{} ({:.1} s)", if ok { "EQUIVALENT: 0 param / row / field differences" } else { "DIFFERENT" }, t0.elapsed().as_secs_f64());
    Ok(ok)
}

fn cmd_roundtrip(o: &Opts) -> Result<bool, String> {
    let root = root_of(o).ok();
    let vanilla = o.vanilla.clone().or_else(|| root.as_deref().map(default_vanilla)).ok_or("--vanilla FILE")?;
    let raw = read(&vanilla)?;
    let t0 = Instant::now();
    let u = regbuild::unpack(&raw)?;
    let b = u.bnd4()?;
    let files: Vec<&[u8]> = (0..b.entries.len()).map(|k| b.data(k)).collect();
    let bnd = b.write(&files)?;
    println!("vanilla {} sha256 {}: BND4 {} B, canonical layout {}", vanilla.display(), u.file_sha256, u.bnd.len(), b.canonical_layout());
    println!("no-edit BND4 rebuild byte-identical to vanilla: {}", bnd == u.bnd);
    // every edited param through the PARAM writer with no rows added
    let mut writer_ok = true;
    for stem in plan::EDITED {
        let k = b.find(&format!("{stem}.param")).ok_or("missing param")?;
        let p = regbuild::param::Param::parse(b.data(k))?;
        let w = regbuild::param::insert_rows(&p, &[])?;
        println!("  {stem}: PARAM writer with no rows == vanilla bytes: {}", w == b.data(k));
        writer_ok &= w == b.data(k);
    }
    let t1 = Instant::now();
    let out = regbuild::pack(&bnd, &u.dcx_header)?;
    let pack_s = t1.elapsed().as_secs_f64();
    if let Some(p) = &o.out {
        regbuild::publish_file(p, &out)?;
        println!("wrote {} ({} B) sha256 {}", p.display(), out.len(), regbuild::sha256_hex(&out));
    }
    let back = regbuild::unpack(&out)?;
    println!("re-read: BND4 byte-identical to vanilla: {}", back.bnd == u.bnd);
    let r = diff::diff(&raw, &out)?;
    for l in &r.lines {
        println!("{l}");
    }
    let ok = bnd == u.bnd && back.bnd == u.bnd && r.equivalent() && r.bnd_identical && writer_ok;
    println!("ROUNDTRIP {} (pack {pack_s:.1} s, total {:.1} s)", if ok { "PASS" } else { "FAIL" }, t0.elapsed().as_secs_f64());
    Ok(ok)
}

fn json_v(v: &serde_json::Value) -> Option<regbuild::fields::V> {
    let n = v.as_number()?;
    Some(match n.as_i64() {
        Some(i) => regbuild::fields::V::I(i),
        None => regbuild::fields::V::F(n.as_f64()?),
    })
}

fn cmd_plan(o: &Opts) -> Result<bool, String> {
    let root = root_of(o)?;
    let log = o.log.clone().ok_or("plan --log build_*.json (work\\regulation\\logs)")?;
    let p = plan::make_plan(&plan::load_inputs(&root)?)?;
    let j: serde_json::Value = serde_json::from_slice(&read(&log)?).map_err(|e| e.to_string())?;
    let mut diffs = Vec::new();
    let mut n = 0;
    for stem in plan::EDITED {
        let want = j["rows"][stem].as_array().cloned().unwrap_or_default();
        let got = p.rows(stem);
        if want.len() != got.len() {
            diffs.push(format!("{stem}: {} rows planned, the log has {}", got.len(), want.len()));
        }
        for (w, g) in want.iter().zip(got) {
            n += 1;
            let (wid, wname, wov, wclone) = (w[0].as_i64(), w[1].as_str(), w[2].as_object(), w[3].as_i64());
            if wid != Some(g.id) || wname != Some(g.name.as_str()) || wclone != Some(g.clone) {
                diffs.push(format!("{stem}: row {:?} {:?} clone {:?} vs ours {} {:?} clone {}", wid, wname, wclone, g.id, g.name, g.clone));
                continue;
            }
            let wov = wov.cloned().unwrap_or_default();
            let mut keys: Vec<&str> = wov.keys().map(|k| k.as_str()).collect();
            let mut gk: Vec<&str> = g.ov.iter().map(|x| x.0).collect();
            keys.sort();
            gk.sort();
            if keys != gk {
                diffs.push(format!("{stem}[{}]: override keys differ: log {keys:?} ours {gk:?}", g.id));
                continue;
            }
            for (k, v) in &g.ov {
                let w = json_v(&wov[*k]);
                // exact value and Python type (int vs float)
                if w != Some(*v) {
                    diffs.push(format!("{stem}[{}].{k}: log {:?} ours {v:?}", g.id, wov[*k]));
                }
            }
        }
    }
    // the rows.json blocks the log carries
    for (k, ours) in [("weapons", &p.weapons), ("ac130", &p.ac130), ("arsenal", &p.arsenal), ("nuke", &p.nuke), ("coop", &p.coop)] {
        let theirs = pyfmt::Json::from_serde(&j["plan"][k]).sorted();
        if theirs != ours.sorted() {
            diffs.push(format!("plan block {k} differs"));
        }
    }
    println!("plan vs {}: {n} rows compared (id, name, clone, override keys, values and int/float types), {} differences", log.display(), diffs.len());
    for d in diffs.iter().take(40) {
        println!("  {d}");
    }
    let known = plan::arsenal_known_answers(&p);
    println!("arsenal known answers: {}", if known.is_empty() { "OK".to_string() } else { known.join("; ") });
    Ok(diffs.is_empty() && known.is_empty())
}

fn cmd_selftest(o: &Opts) -> Result<bool, String> {
    let root = root_of(o)?;
    let vanilla = o.vanilla.clone().unwrap_or_else(|| default_vanilla(&root));
    let raw = read(&vanilla)?;
    if regbuild::sha256_hex(&raw) != regbuild::VANILLA_SHA256 {
        return Err("vanilla sha256 differs from the pin".into());
    }
    let u = regbuild::unpack(&raw)?;
    let vb = u.bnd4()?;
    let inputs = plan::load_inputs(&root)?;
    let p = plan::make_plan(&inputs)?;
    let files = regbuild::apply(&vb, &p)?;
    let bnd = vb.write(&files.iter().map(|f| f.as_slice()).collect::<Vec<_>>())?;
    let bb = bnd4::Bnd4::parse(&bnd)?;
    let st = selftest::SelfTest { vanilla: &vb, built: &bb, plan: &p, inputs: &inputs };
    let res = st.run()?;
    println!("control: the unmutated build passes verify");
    let mut caught = 0;
    for (label, ok, first) in &res {
        if *ok {
            caught += 1;
            println!("caught: {label} -> {}", first.chars().take(140).collect::<String>());
        } else {
            println!("MISSED: {label} passed the check {first}");
        }
    }
    let ok = caught == res.len();
    println!("REGULATION SELF-TEST {} {caught}/{} mutations caught (control passed; {} verify problems on the control)", if ok { "PASS" } else { "FAIL" }, res.len(), verify::verify(&vb, &verify::Files::new(&bb), &p).0.len());
    Ok(ok)
}
