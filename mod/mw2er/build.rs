//! Records the build time (Unix seconds) for the BOOT log line. The project is not a git repo,
//! so there is no commit hash to embed. The script re-runs whenever a file in this package
//! changes (no rerun-if-changed is emitted on purpose).

fn main() {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    println!("cargo:rustc-env=MW2ER_BUILD_UNIX={secs}");
}
