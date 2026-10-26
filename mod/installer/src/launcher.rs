//! The launcher as the shortcuts run it (`MW2inER.exe play solo|coop` with a console Windows made for it, which
//! main.rs has already hidden): the same preflight and start as `--cli play` ([`installer::cli::play_prepare`] /
//! [`installer::cli::play_start`]), but problems are shown in a message box (Retry re-checks, e.g. after starting
//! Steam) and a good start shows nothing at all. It never starts Steam or the game itself; only me3 is started, and
//! only after every check passed.

use installer::cli::{self, Args};
use installer::detect::{Env, Level};
use installer::win::{self, BoxIcon, Buttons};

pub const TITLE: &str = installer::DISPLAY_NAME;

/// The message for the failing rows (pure; tested).
pub fn problem_text(mode_coop: bool, rows: &[installer::detect::Check]) -> String {
    let mut s = format!("{}{} cannot start yet:\n\n", installer::DISPLAY_NAME, if mode_coop { " (co-op)" } else { "" });
    for r in rows.iter().filter(|r| r.level == Level::Fail) {
        s.push_str("- ");
        s.push_str(&r.msg);
        s.push('\n');
    }
    s.push_str(&format!("\nRetry checks again (for example after starting Steam). The setup (shortcut \"{} - Setup\") repairs a changed install.", installer::DISPLAY_NAME));
    s
}

pub fn run(a: &Args) -> i32 {
    let env = Env::from_system();
    let root = cli::resolve_root(a, &env);
    let mode = a.mode.unwrap_or_default();
    loop {
        let p = match cli::play_prepare(&env, &root, mode) {
            Ok(p) => p,
            Err(e) => {
                win::message_box(TITLE, &format!("{e}\n\nInstall folder: {}", root.display()), Buttons::Ok, BoxIcon::Error);
                return 1;
            }
        };
        if !p.can_start() {
            if win::message_box(TITLE, &problem_text(mode.coop(), &p.rows), Buttons::RetryCancel, BoxIcon::Warning) {
                continue;
            }
            return 1;
        }
        return match cli::play_start(&env, &root, &p) {
            Ok(_) => 0,
            Err(e) => {
                win::message_box(TITLE, &e, Buttons::Ok, BoxIcon::Error);
                1
            }
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use installer::detect::Check;

    #[test]
    fn problem_text_lists_only_the_failures() {
        let rows = vec![
            Check { id: "profile", level: Level::Ok, msg: "profile ok".into(), fix: None },
            Check { id: "steam", level: Level::Fail, msg: "Steam is not running".into(), fix: None },
            Check { id: "files", level: Level::Warn, msg: "no recorded hash".into(), fix: None },
        ];
        let t = problem_text(true, &rows);
        assert!(t.contains("(co-op)") && t.contains("- Steam is not running") && t.contains("Retry"));
        assert!(!t.contains("profile ok") && !t.contains("no recorded hash"));
    }
}
