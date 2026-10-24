//! FLX10FIX — diagnose & fix the DDJ-FLX10 "NO AUDIO DRIVER" problem on macOS.
//!
//! The DDJ-FLX10 jog display shows "NO AUDIO DRIVER" when the background helper
//! shipped with the DDJ-FLX10 Setting Utility cannot load. AlphaTheta's installer
//! has carried a `com.apple.quarantine` xattr on that helper's LaunchAgent since
//! 2022, and current macOS refuses to load quarantined agents — so the helper
//! silently stops working and the unit complains until the utility is opened by
//! hand. This tool checks for exactly that and repairs it.

use std::env;
use std::io::IsTerminal;
use std::path::Path;
use std::process::{Command, ExitCode};

const DISPLAY: &str = "FLX10FIX";
const VERSION: &str = env!("CARGO_PKG_VERSION");

const LABEL: &str = "com.pioneerdj.DDJ-FLX10AutoLauncherd";
const PLIST: &str = "/Library/LaunchAgents/com.pioneerdj.DDJ-FLX10AutoLauncherd.plist";
const APP_DIR: &str = "/Applications/Pioneer/DDJ-FLX10";
const SETUP_APP: &str = "/Applications/Pioneer/DDJ-FLX10/DDJ-FLX10 Setup.app";
const PIONEER_DIR: &str = "/Applications/Pioneer";
const DEVICE: &str = "DDJ-FLX10";
const QUARANTINE: &str = "com.apple.quarantine";

#[derive(Clone, Copy, PartialEq)]
enum Level {
    Ok,
    Warn,
    Fail,
}

struct Fact {
    name: &'static str,
    level: Level,
    detail: String,
    fixable: bool,
}

struct Ui {
    color: bool,
}

impl Ui {
    fn new(no_color: bool) -> Self {
        Self {
            color: std::io::stdout().is_terminal()
                && env::var_os("NO_COLOR").is_none()
                && !no_color,
        }
    }
    fn paint(&self, code: &str, s: &str) -> String {
        if self.color {
            format!("\x1b[{code}m{s}\x1b[0m")
        } else {
            s.to_string()
        }
    }
    fn ok(&self, s: &str) -> String {
        self.paint("32", s)
    }
    fn warn(&self, s: &str) -> String {
        self.paint("33", s)
    }
    fn fail(&self, s: &str) -> String {
        self.paint("31", s)
    }
    fn dim(&self, s: &str) -> String {
        self.paint("2", s)
    }
    fn bold(&self, s: &str) -> String {
        self.paint("1", s)
    }
    fn tag(&self, l: Level) -> String {
        match l {
            Level::Ok => self.ok("[ ok ]"),
            Level::Warn => self.warn("[warn]"),
            Level::Fail => self.fail("[fail]"),
        }
    }
}

/// Run a command, returning (success, stdout+stderr trimmed).
fn sh(program: &str, args: &[&str]) -> (bool, String) {
    match Command::new(program).args(args).output() {
        Ok(o) => {
            let mut s = String::from_utf8_lossy(&o.stdout).into_owned();
            let err = String::from_utf8_lossy(&o.stderr);
            if !err.trim().is_empty() {
                if !s.is_empty() && !s.ends_with('\n') {
                    s.push('\n');
                }
                s.push_str(err.trim());
            }
            (o.status.success(), s.trim().to_string())
        }
        Err(e) => (false, format!("could not run {program}: {e}")),
    }
}

fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// The uid whose launchd GUI domain owns the helper. Under sudo that is SUDO_UID;
/// otherwise our own uid; root without SUDO_UID falls back to the console user.
fn invoking_uid() -> u32 {
    if let Some(n) = env::var("SUDO_UID")
        .ok()
        .and_then(|v| v.trim().parse::<u32>().ok())
    {
        return n;
    }
    let me = sh("id", &["-u"]).1.trim().parse::<u32>().unwrap_or(501);
    if me != 0 {
        return me;
    }
    let console_user = sh("stat", &["-f", "%Su", "/dev/console"]).1;
    let cu = console_user.trim();
    if !cu.is_empty()
        && cu != "root"
        && let Ok(n) = sh("id", &["-u", cu]).1.trim().parse::<u32>()
    {
        return n;
    }
    501
}

fn is_root() -> bool {
    sh("id", &["-u"]).1.trim() == "0"
}

fn macos_version() -> String {
    let v = sh("sw_vers", &["-productVersion"]).1;
    if v.is_empty() { "unknown".into() } else { v }
}

/// Some(true) = quarantined, Some(false) = clean, None = cannot tell (missing file).
fn quarantine_of(path: &str) -> Option<bool> {
    let (ok, out) = sh("xattr", &[path]);
    if !ok {
        return None;
    }
    Some(out.lines().any(|l| l.trim() == QUARANTINE))
}

/// (loaded, running, human state) for the helper in the user's GUI domain.
fn agent_state(uid: u32) -> (bool, bool, String) {
    let target = format!("gui/{uid}/{LABEL}");
    let (ok, out) = sh("launchctl", &["print", target.as_str()]);
    if !ok {
        return (false, false, "not found in launchd".into());
    }
    let running = out.contains("state = running");
    let mut bits: Vec<String> = Vec::new();
    if let Some(l) = out.lines().find(|l| l.trim_start().starts_with("state = ")) {
        bits.push(l.trim().to_string());
    }
    if let Some(l) = out.lines().find(|l| l.trim_start().starts_with("pid = ")) {
        bits.push(l.trim().to_string());
    }
    let detail = if bits.is_empty() {
        "loaded".to_string()
    } else {
        bits.join(", ")
    };
    (true, running, detail)
}

fn gather_facts(uid: u32) -> Vec<Fact> {
    let mut f = Vec::new();

    let installed = Path::new(SETUP_APP).exists();
    f.push(Fact {
        name: "Setting Utility installed",
        level: if installed { Level::Ok } else { Level::Fail },
        detail: if installed {
            APP_DIR.into()
        } else {
            "missing — download the DDJ-FLX10 Setting Utility from AlphaTheta".into()
        },
        fixable: false,
    });

    let plist_exists = Path::new(PLIST).exists();
    f.push(Fact {
        name: "Helper LaunchAgent present",
        level: if plist_exists { Level::Ok } else { Level::Fail },
        detail: PLIST.into(),
        fixable: false,
    });

    let quar = if plist_exists {
        quarantine_of(PLIST)
    } else {
        None
    };
    f.push(Fact {
        name: "No quarantine flag on helper",
        level: match quar {
            Some(true) => Level::Fail,
            Some(false) => Level::Ok,
            None => Level::Warn,
        },
        detail: match quar {
            Some(true) => "quarantined — launchd refuses to load it on this macOS".into(),
            Some(false) => "clean".into(),
            None => "cannot tell".into(),
        },
        fixable: quar == Some(true),
    });

    let (loaded, running, state) = agent_state(uid);
    f.push(Fact {
        name: "Helper loaded and running",
        level: if running {
            Level::Ok
        } else if loaded {
            Level::Warn
        } else {
            Level::Fail
        },
        detail: state,
        fixable: plist_exists && !running,
    });

    let connected = sh("ioreg", &["-p", "IOUSB", "-w0"]).1.contains(DEVICE);
    f.push(Fact {
        name: "DDJ-FLX10 connected",
        level: if connected { Level::Ok } else { Level::Warn },
        detail: if connected {
            "on USB now".into()
        } else {
            "not attached right now — the fix still applies".into()
        },
        fixable: false,
    });

    let mut extra: Vec<String> = Vec::new();
    for p in [APP_DIR, PIONEER_DIR] {
        if quarantine_of(p) == Some(true) {
            extra.push(p.to_string());
        }
    }
    if !extra.is_empty() {
        f.push(Fact {
            name: "Other quarantined vendor files",
            level: Level::Warn,
            detail: extra.join(", "),
            fixable: true,
        });
    }

    f
}

fn print_facts(ui: &Ui, facts: &[Fact]) {
    let w = facts.iter().map(|f| f.name.len()).max().unwrap_or(0);
    for f in facts {
        println!(
            "  {} {:w$}  {}",
            ui.tag(f.level),
            f.name,
            ui.dim(&f.detail),
            w = w
        );
    }
}

fn cmd_check(ui: &Ui) -> ExitCode {
    println!(
        "{} {} — DDJ-FLX10 \"NO AUDIO DRIVER\" checker on macOS {}",
        ui.bold(DISPLAY),
        VERSION,
        macos_version()
    );
    println!();
    let uid = invoking_uid();
    let facts = gather_facts(uid);
    print_facts(ui, &facts);

    let fixable = facts.iter().any(|f| f.level != Level::Ok && f.fixable);
    let failing = facts.iter().any(|f| f.level == Level::Fail);

    if fixable {
        println!();
        println!("  Fix:  {}", ui.bold("sudo flx10fix fix"));
        println!(
            "  {}",
            ui.dim("(one admin prompt; safe to re-run any time)")
        );
        ExitCode::from(1u8)
    } else if failing {
        println!();
        println!(
            "  {}",
            ui.fail("Manual attention needed — see the failed checks above.")
        );
        ExitCode::from(1u8)
    } else {
        println!();
        println!("  {} All good — nothing to fix.", ui.ok("OK"));
        ExitCode::SUCCESS
    }
}

fn cmd_fix(ui: &Ui, dry: bool) -> ExitCode {
    let uid = invoking_uid();

    if !dry && !is_root() {
        // Nothing to do? Then don't bother the user for a password.
        let facts = gather_facts(uid);
        let need_fix = facts.iter().any(|f| f.level != Level::Ok && f.fixable);
        if !need_fix {
            println!("{} — nothing to fix automatically:", ui.bold(DISPLAY));
            println!();
            print_facts(ui, &facts);
            if facts.iter().any(|f| f.level == Level::Fail) {
                println!();
                println!("  A check still fails — see above.");
                return ExitCode::from(1u8);
            }
            return ExitCode::SUCCESS;
        }

        if env::var_os("SUDO_UID").is_some() {
            eprintln!("{DISPLAY}: running under sudo but not as root?");
            return ExitCode::from(2u8);
        }

        let exe = match env::current_exe() {
            Ok(p) => p,
            Err(e) => {
                eprintln!("cannot locate own executable: {e}");
                return ExitCode::from(2u8);
            }
        };
        let exe_s = exe.to_string_lossy();

        if std::io::stdin().is_terminal() {
            println!(
                "{} needs admin rights for the repair — re-running with sudo…",
                ui.bold(DISPLAY)
            );
            println!();
            return match Command::new("sudo").arg(&exe).arg("fix").status() {
                Ok(s) => ExitCode::from(s.code().unwrap_or(1).clamp(0, 255) as u8),
                Err(e) => {
                    eprintln!("could not run sudo: {e}");
                    ExitCode::from(2u8)
                }
            };
        }

        // No terminal (launched from a GUI/agent): use the macOS admin prompt.
        println!(
            "{} needs admin rights for the repair — asking macOS…",
            ui.bold(DISPLAY)
        );
        println!();
        let log_path = std::env::temp_dir().join(format!("flx10fix-{}.log", std::process::id()));
        let inner = format!(
            "{} fix > {} 2>&1",
            shell_quote(&exe_s),
            shell_quote(&log_path.to_string_lossy())
        );
        let apple = format!(
            "do shell script \"{}\" with administrator privileges",
            inner.replace('\\', "\\\\").replace('"', "\\\"")
        );
        let status = Command::new("osascript").arg("-e").arg(&apple).status();
        let log_text = std::fs::read_to_string(&log_path).unwrap_or_default();
        let _ = std::fs::remove_file(&log_path);
        if !log_text.trim().is_empty() {
            print!("{log_text}");
        }
        return match status {
            Ok(s) if s.success() => ExitCode::SUCCESS,
            Ok(s) => {
                eprintln!(
                    "{}: repair did not complete or was cancelled (osascript exit {}).",
                    DISPLAY,
                    s.code().unwrap_or(-1)
                );
                ExitCode::from(1u8)
            }
            Err(e) => {
                eprintln!("could not run osascript: {e}");
                ExitCode::from(2u8)
            }
        };
    }

    println!(
        "{} {} — applying fix{}",
        ui.bold(DISPLAY),
        VERSION,
        if dry { " (dry run)" } else { "" }
    );
    println!();

    let mut failures = 0usize;

    for t in [PLIST, APP_DIR, PIONEER_DIR] {
        match quarantine_of(t) {
            Some(true) if dry => {
                println!("  {} would remove quarantine from {}", ui.dim("plan"), t)
            }
            Some(true) => {
                let (ok, out) = sh("xattr", &["-d", QUARANTINE, t]);
                if ok {
                    println!("  {} removed quarantine from {}", ui.ok("fixed"), t);
                } else {
                    println!("  {} could not clear {} — {}", ui.fail("fail"), t, out);
                    failures += 1;
                }
            }
            Some(false) => println!("  {} {} already clean", ui.ok("ok"), t),
            None => {
                if t == PLIST {
                    println!(
                        "  {} {} missing — install the DDJ-FLX10 Setting Utility first",
                        ui.fail("fail"),
                        t
                    );
                    failures += 1;
                } else {
                    println!("  {} {} not present", ui.dim("skip"), t);
                }
            }
        }
    }

    let (loaded, running, state) = agent_state(uid);
    if dry {
        if !loaded {
            println!(
                "  {} would load the helper: launchctl bootstrap gui/{uid}",
                ui.dim("plan")
            );
        } else if !running {
            println!(
                "  {} would start the helper: launchctl kickstart gui/{uid}/{LABEL}",
                ui.dim("plan")
            );
        } else {
            println!(
                "  {} helper already loaded and running ({})",
                ui.ok("ok"),
                state
            );
        }
        println!();
        println!("Dry run complete — nothing was changed.");
        return ExitCode::SUCCESS;
    }

    let label_target = format!("gui/{uid}/{LABEL}");
    let domain = format!("gui/{uid}");
    let _ = sh("launchctl", &["enable", label_target.as_str()]);

    if !loaded {
        let (ok, out) = sh("launchctl", &["bootstrap", domain.as_str(), PLIST]);
        if ok {
            println!("  {} loaded helper into {}", ui.ok("fixed"), domain);
        } else {
            println!("  {} could not load helper — {}", ui.fail("fail"), out);
            failures += 1;
        }
    } else if !running {
        let (ok, out) = sh("launchctl", &["kickstart", label_target.as_str()]);
        if ok {
            println!("  {} started helper", ui.ok("fixed"));
        } else {
            println!("  {} could not start helper — {}", ui.fail("fail"), out);
            failures += 1;
        }
    } else {
        println!(
            "  {} helper already loaded and running ({})",
            ui.ok("ok"),
            state
        );
    }

    println!();
    println!("{} re-check:", ui.bold(DISPLAY));
    let facts = gather_facts(uid);
    print_facts(ui, &facts);

    let healthy = !facts.iter().any(|f| f.level == Level::Fail) && failures == 0;
    if healthy {
        println!();
        println!(
            "  {} Done. If the unit still shows \"NO AUDIO DRIVER\", unplug and replug its USB cable once.",
            ui.ok("OK")
        );
        ExitCode::SUCCESS
    } else {
        println!();
        println!(
            "  {} Some problems remain — details above.",
            ui.fail("fail")
        );
        ExitCode::from(1u8)
    }
}

fn help() {
    println!("{DISPLAY} {VERSION} — fix the DDJ-FLX10 \"NO AUDIO DRIVER\" problem on macOS");
    println!();
    println!("The DDJ-FLX10 Setting Utility ships a background helper that keeps the");
    println!("host <-> unit handshake alive. AlphaTheta's installer has carried a");
    println!("quarantine flag on that helper's LaunchAgent since 2022; current macOS");
    println!("refuses to load quarantined agents, so the helper dies silently and the");
    println!("jog display shows \"NO AUDIO DRIVER\" until the utility is opened by hand.");
    println!("{DISPLAY} repairs exactly that.");
    println!();
    println!("USAGE:");
    println!("  flx10fix            report status (same as `flx10fix check`)");
    println!(
        "  flx10fix check      show the state of the helper, its LaunchAgent and the controller"
    );
    println!("  flx10fix fix        repair: strip the quarantine flag and load the helper");
    println!("                      (asks for your password; --dry-run previews only)");
    println!();
    println!("OPTIONS:");
    println!("  --dry-run           with `fix`: show what would be done, change nothing");
    println!("  --no-color          disable colored output");
    println!("  -h, --help          show this help");
    println!("  -V, --version       show the version");
    println!();
    println!("EXIT CODES:  0 healthy / fixed   1 fixable or remaining problems   2 usage error");
}

fn main() -> ExitCode {
    let mut cmd: Option<String> = None;
    let mut dry = false;
    let mut no_color = false;

    for a in env::args().skip(1) {
        match a.as_str() {
            "check" => cmd = Some("check".into()),
            "fix" => cmd = Some("fix".into()),
            "help" | "-h" | "--help" => {
                help();
                return ExitCode::SUCCESS;
            }
            "version" | "-V" | "--version" => {
                println!("{DISPLAY} {VERSION}");
                return ExitCode::SUCCESS;
            }
            "--dry-run" => dry = true,
            "--no-color" => no_color = true,
            other => {
                eprintln!("{DISPLAY}: unknown argument `{other}`");
                eprintln!("Try `flx10fix --help`.");
                return ExitCode::from(2u8);
            }
        }
    }

    let ui = Ui::new(no_color);
    match cmd.as_deref() {
        Some("fix") => cmd_fix(&ui, dry),
        _ => cmd_check(&ui),
    }
}
