//! `MW2inER-<ver>-setup.exe` / installed as `<root>\MW2inER.exe`: the setup window, the launcher the shortcuts run,
//! and the console front end (docs/INSTALLER_DESIGN.md 2.1).
//!
//! [`installer::cli::front_end`] picks the front end: `--cli` (or any console command typed in a terminal) = the
//! console; no command = the setup window ([`gui`]); `play solo|coop` from a shortcut = [`launcher`]; `uninstall` from
//! Apps & Features = the window's uninstall page. The exe is a console program, so `--cli` keeps working in scripts
//! (output, exit code, the shell waits, masked password prompt); when Windows made a console just for this process
//! (Explorer, a shortcut) and a window front end runs, that console is hidden and released at once.
//! No window is ever created in `--cli` mode.

mod gui;
mod icon_art;
mod launcher;

use installer::cli::{self, FrontEnd};
use installer::win;

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let owns = win::owns_console();
    let fe = cli::front_end(&argv, owns);
    let code = if fe == FrontEnd::Cli {
        installer::cli::main_with(&argv, &mut std::io::stdout())
    } else {
        // front_end only returns a window front end for a command line that parses
        let a = cli::parse(&argv).unwrap_or_default();
        if owns {
            win::detach_own_console();
        }
        match fe {
            FrontEnd::Launcher => launcher::run(&a),
            FrontEnd::UninstallWindow => gui::run(a, gui::Start::Uninstall),
            _ => gui::run(a, gui::Start::Wizard),
        }
    };
    std::process::exit(code);
}
