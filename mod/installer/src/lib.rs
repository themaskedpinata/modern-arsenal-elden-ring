//! MW2 in Elden Ring: the installer, launcher (shortcut target), updater and uninstaller
//! (docs/INSTALLER_DESIGN.md phase P1). One program, no admin rights, default root `%LOCALAPPDATA%\MW2inER`.
//!
//! All logic lives in this library with no GUI types; the front ends (`cli` now, the eframe wizard next) consume
//! it. Module map:
//! - discovery: [`vdf`] (KeyValues parser), [`steam`] (registry + libraryfolders.vdf + appmanifest), [`detect`]
//!   (facts about ER / MW2 / me3 / Seamless / Python / WitchyBND / VC++ / disk, evaluated into [`detect::Check`] rows)
//! - release: [`release`] (pins, host manifest, release.json, co-op id, Rust re-hash of the plain manifest entries)
//! - safety: [`guard`] (every write; forbidden game / Steam / save folders; install-root rules), [`seamless::Secret`]
//!   (the co-op password: no Debug / Display / Serialize; written only into the mod's own ersc_settings.ini)
//! - install: [`plan`] (steps, inputs, outputs, skip keys), [`run`] (the session + executor + [`run::Progress`]),
//!   [`state`] (`state\install.json`, never the password), [`regulation`] (the in-process Rust regulation builder,
//!   default; WitchyBND as the fallback), [`seamless`], [`witchy`], [`download`] (pinned
//!   sources only), [`proc`] (external tools with timeout + job object + cancel), [`payload`] (release folder or
//!   zip, the bundled CPython embeddable package), [`friend_toml`] (the release toml policy, friend_toml.ps1 ported)
//! - update (P3): [`update`] (kind, what changes, co-op id before / after, the opt-in GitHub release check),
//!   [`toml_merge`] (the player's hot settings into the release's mw2er.toml), [`rollback`] (the store of what an
//!   update replaced, and Roll back)
//! - play / remove: [`launch`] (preflight, save backup rule, me3 command), [`uninstall`]
//! - Windows glue: [`win`] (registry, known folders, processes, free space, PE versions, masked input, .lnk)
//! - front end: [`cli`] (also [`cli::front_end`], which picks the binary's front end, and the launcher's
//!   [`cli::play_prepare`] / [`cli::play_start`]); the eframe wizard and the shortcut launcher live in the binary
//!   (`src/main.rs`, `src/gui`, `src/launcher.rs`), so this library has no GUI dependency
//!
//! Safety (MODDING_PLAN / installer decisions): this program never starts Elden Ring, MW2, Steam or Seamless; me3
//! is started only by `play`. It never writes a game folder, a Steam library or `%APPDATA%\EldenRing` (the save
//! backup only reads it), never redistributes Seamless / game files / converted data / saves, and downloads only
//! from [`download::ALLOWED`] with a sha256 pin.

/// P4: the mod's public, user-visible name (window title / header, shortcuts, Apps & Features, messages). Display
/// only: every internal id stays "MW2inER" (install folder, [`run::APP_KEY`] / [`run::UNINSTALL_KEY`], state files,
/// mw2er.dll, the me3 profile names, the co-op id format). Keep in step with build.rs (the exe's version resource).
pub const DISPLAY_NAME: &str = "Modern Arsenal for Elden Ring";
/// P4: who publishes the mod (Apps & Features "Publisher").
pub const PUBLISHER: &str = "maskedpinata";
/// The names releases before 0.3.0 used for the shortcuts and the Start-menu folder: an update replaces those
/// shortcuts with the new ones, an uninstall removes either (see `run::Session::finish`, `uninstall::execute`).
pub const LEGACY_DISPLAY_NAMES: &[&str] = &["MW2 in Elden Ring"];

pub mod cli;
pub mod detect;
pub mod download;
pub mod friend_toml;
pub mod guard;
pub mod launch;
pub mod payload;
pub mod plan;
pub mod proc;
pub mod regulation;
pub mod release;
pub mod rollback;
pub mod run;
pub mod seamless;
pub mod state;
pub mod steam;
pub mod toml_merge;
pub mod uninstall;
pub mod update;
pub mod util;
pub mod vdf;
pub mod win;
pub mod witchy;
