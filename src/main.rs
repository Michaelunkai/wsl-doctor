//! wsl-doctor: one-shot WSL2 + Docker repair for Windows. Pure std Rust, live progress.
//! No args  : interactive full repair (elevates itself, asks before rebooting).
//! fix --quiet : silent self-heal (used by the scheduled tasks at boot/logon).
//! diag | watch-remove : status / remove scheduled tasks.
use std::env;
use std::io::{self, Write};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

const TASK: &str = "WslDoctorHeal";
static QUIET: AtomicBool = AtomicBool::new(false);

macro_rules! say {
    ($($a:tt)*) => { if !QUIET.load(Ordering::Relaxed) { println!($($a)*); let _ = io::stdout().flush(); } };
}

/// Captured run: (exit code, stdout+stderr with UTF-16 NULs removed).
fn sh(prog: &str, args: &[&str]) -> (i32, String) {
    match Command::new(prog).args(args).stdin(Stdio::null()).output() {
        Ok(o) => {
            let mut s = String::from_utf8_lossy(&o.stdout).into_owned();
            s.push_str(&String::from_utf8_lossy(&o.stderr));
            (o.status.code().unwrap_or(-1), s.replace('\0', "").replace('\r', ""))
        }
        Err(e) => (-1, format!("spawn {prog}: {e}")),
    }
}

/// Live run: child output streams straight to this console.
fn live(prog: &str, args: &[&str]) -> i32 {
    if QUIET.load(Ordering::Relaxed) {
        return sh(prog, args).0;
    }
    Command::new(prog).args(args).status().map(|s| s.code().unwrap_or(-1)).unwrap_or(-1)
}

fn ps(script: &str) -> String {
    sh("powershell", &["-NoProfile", "-NonInteractive", "-Command", script]).1.trim().to_string()
}

/// `fltmc` succeeds only in an elevated token.
fn is_admin() -> bool {
    sh("fltmc", &[]).0 == 0
}

fn is_system_account() -> bool {
    env::var("USERNAME").map(|u| u.ends_with('$')).unwrap_or(false)
}

struct Steps {
    n: usize,
    total: usize,
    t: Instant,
}
impl Steps {
    fn begin(&mut self, label: &str) {
        self.n += 1;
        self.t = Instant::now();
        say!("\n[{}/{}] {} ...", self.n, self.total, label);
    }
    fn done(&self, msg: &str) {
        say!("      -> {} ({:.1}s)", msg, self.t.elapsed().as_secs_f32());
    }
}

struct Status {
    hypervisor: bool,
    firmware: bool,
    vmp: String,
    wsl_feat: String,
}

fn status() -> Status {
    let r = ps("$c=Get-CimInstance Win32_ComputerSystem; $p=Get-CimInstance Win32_Processor|select -First 1; \
        $a=(Get-WindowsOptionalFeature -Online -FeatureName VirtualMachinePlatform).State; \
        $b=(Get-WindowsOptionalFeature -Online -FeatureName Microsoft-Windows-Subsystem-Linux).State; \
        \"$($c.HypervisorPresent)|$($p.VirtualizationFirmwareEnabled)|$a|$b\"");
    let f: Vec<&str> = r.split('|').collect();
    let g = |i: usize| f.get(i).copied().unwrap_or("?").to_string();
    Status { hypervisor: g(0) == "True", firmware: g(1) == "True", vmp: g(2), wsl_feat: g(3) }
}

fn launch_type() -> String {
    sh("bcdedit", &["/enum", "{current}"])
        .1
        .lines()
        .find(|l| l.starts_with("hypervisorlaunchtype"))
        .map(|l| l.split_whitespace().nth(1).unwrap_or("?").to_string())
        .unwrap_or_else(|| "unset(Auto)".into())
}

fn wsl_version() -> String {
    sh("wsl", &["--version"]).1.lines().next().unwrap_or("?").trim().to_string()
}

/// Installed WSL MSI lost its cached package -> update fails with 1603/1612. Restore it.
fn repair_msi_cache(st: &Steps) {
    let info = ps(r#"
$wi = New-Object -ComObject WindowsInstaller.Installer
Get-ItemProperty HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\*,HKLM:\SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall\* -ErrorAction SilentlyContinue |
 ? { $_.DisplayName -eq 'Windows Subsystem for Linux' -and $_.PSChildName -like '{*' } | % {
  $c=$_.PSChildName; $v=$wi.ProductInfo($c,'VersionString'); $lp=$wi.ProductInfo($c,'LocalPackage')
  if (-not $lp) { $lp="C:\Windows\Installer\wsldoctor-$v.msi" }
  "$v|$lp|$(Test-Path $lp)" } | select -First 1"#);
    let f: Vec<&str> = info.split('|').collect();
    if f.len() < 3 {
        st.done("no MSI-based WSL install (nothing to repair)");
        return;
    }
    if f[2] == "True" {
        st.done("MSI cache present");
        return;
    }
    let tag = f[0].trim_end_matches(".0");
    let url = format!("https://github.com/microsoft/WSL/releases/download/{tag}/wsl.{}.x64.msi", f[0]);
    say!("      cache file missing: {}\n      downloading {url}", f[1]);
    let c = live("curl.exe", &["-L", "--fail", "--progress-bar", "-A", "Mozilla/5.0", "-o", f[1], &url]);
    st.done(&format!("download exit {c}"));
}

fn docker_step(st: &Steps, hypervisor: bool) {
    for svc in ["vmcompute", "hns", "vmms", "HvHost"] {
        let (c, _) = sh("sc", &["start", svc]);
        say!("      service {svc}: {}", if c == 0 { "started" } else if c == 1056 { "running" } else { "n/a" });
    }
    let path = format!("{}\\Docker\\settings-store.json", env::var("APPDATA").unwrap_or_default());
    match std::fs::read_to_string(&path) {
        Ok(txt) if txt.contains("\"WslEngineEnabled\": false") => {
            sh("taskkill", &["/f", "/im", "Docker Desktop.exe"]);
            sh("taskkill", &["/f", "/im", "com.docker.backend.exe"]);
            let _ = std::fs::copy(&path, format!("{path}.wsl-doctor.bak"));
            let r = std::fs::write(&path, txt.replace("\"WslEngineEnabled\": false", "\"WslEngineEnabled\": true"));
            say!("      WSL2 engine enabled next to VMM engine: {}", if r.is_ok() { "ok (backup saved)" } else { "WRITE FAILED" });
        }
        Ok(_) => say!("      Docker WSL2 engine already enabled"),
        Err(_) => say!("      Docker settings not found (Docker not set up for this user)"),
    }
    if !hypervisor {
        st.done("Docker will start after the hypervisor loads (reboot)");
        return;
    }
    if is_system_account() {
        st.done("running as SYSTEM: Docker Desktop is started by the logon task");
        return;
    }
    sh("wsl", &["--shutdown"]);
    let _ = Command::new("C:\\Program Files\\Docker\\Docker\\Docker Desktop.exe").spawn();
    for i in 1..=36 {
        std::thread::sleep(Duration::from_secs(5));
        if sh("docker", &["info", "--format", "{{.ServerVersion}}"]).0 == 0 {
            st.done(&format!("docker engine up after {}s", i * 5));
            return;
        }
        say!("      waiting for docker engine... {}s", i * 5);
    }
    st.done("docker engine did not answer within 180s");
}

fn install_tasks() {
    let exe = env::current_exe().unwrap();
    let cmd = format!("\"{}\" fix --quiet", exe.display());
    let a = sh("schtasks", &["/create", "/f", "/tn", TASK, "/sc", "onstart", "/ru", "SYSTEM", "/rl", "highest", "/tr", &cmd]).0;
    let b = sh("schtasks", &["/create", "/f", "/tn", &format!("{TASK}Logon"), "/sc", "onlogon", "/rl", "highest", "/tr", &cmd]).0;
    say!("      boot task exit {a}, logon task exit {b} (0 = ok)");
}

fn elevate(args: &str) -> ! {
    let exe = env::current_exe().unwrap();
    println!("Administrator rights needed: approve the UAC prompt (a new window opens)...");
    let (c, o) = sh("powershell", &["-NoProfile", "-Command", &format!(
        "Start-Process -FilePath '{}' -ArgumentList '{}' -Verb RunAs", exe.display(), args)]);
    if c != 0 {
        eprintln!("elevation failed/declined: {o}");
        pause();
    }
    std::process::exit(c);
}

fn pause() {
    if QUIET.load(Ordering::Relaxed) {
        return;
    }
    print!("\nPress Enter to close...");
    let _ = io::stdout().flush();
    let _ = io::stdin().read_line(&mut String::new());
}

fn full_repair(interactive: bool) {
    let mut st = Steps { n: 0, total: 8, t: Instant::now() };
    say!("=== wsl-doctor: WSL2 + Docker repair ===");

    st.begin("Reading firmware / hypervisor / Windows feature state (about 10s)");
    let s = status();
    st.done(&format!("firmware virt={}, hypervisor running={}, VMP={}, WSL feature={}", s.firmware, s.hypervisor, s.vmp, s.wsl_feat));
    if !s.firmware {
        say!("      WARNING: virtualization is off in BIOS/UEFI; only the firmware menu can fix that.");
    }

    st.begin("Boot configuration (hypervisorlaunchtype)");
    let lt = launch_type();
    if lt.eq_ignore_ascii_case("off") {
        let (c, o) = sh("bcdedit", &["/set", "hypervisorlaunchtype", "auto"]);
        st.done(&format!("was Off -> set Auto (exit {c}) {}", o.trim()));
    } else {
        st.done(&format!("already {lt}"));
    }

    st.begin("Windows features (VirtualMachinePlatform, WSL)");
    for (n, v) in [("VirtualMachinePlatform", &s.vmp), ("Microsoft-Windows-Subsystem-Linux", &s.wsl_feat)] {
        if v != "Enabled" {
            let c = live("dism", &["/online", "/enable-feature", &format!("/featurename:{n}"), "/all", "/norestart"]);
            say!("      enable {n}: exit {c}");
        }
    }
    st.done("features ok");

    st.begin("WSL installer cache repair (fixes update error 1603)");
    repair_msi_cache(&st);

    st.begin("Updating WSL (live output)");
    let c = live("wsl", &["--update"]);
    st.done(&format!("wsl --update exit {c}; {}", wsl_version()));

    st.begin("WSL default version -> 2, stop stale WSL VM");
    live("wsl", &["--set-default-version", "2"]);
    sh("wsl", &["--shutdown"]);
    st.done("ok");

    st.begin("Docker Desktop (services, WSL2 + VMM engine, restart)");
    docker_step(&st, s.hypervisor);

    st.begin("Self-heal scheduled tasks (boot + logon)");
    install_tasks();
    st.done("installed");

    if s.hypervisor {
        let (c, o) = sh("wsl", &["-d", "Ubuntu", "--", "uname", "-sr"]);
        say!("\nUbuntu check: exit {c}: {}", o.trim());
        say!("\nALL FIXED. Nothing left to do.");
        return;
    }
    say!("\nEverything is fixed except loading the hypervisor, which Windows only does at boot.");
    say!("After the reboot the boot/logon tasks finish Docker + WSL2 automatically.");
    if !interactive {
        return;
    }
    print!("Reboot now? Press Y to reboot, anything else to stay: ");
    let _ = io::stdout().flush();
    let mut a = String::new();
    let _ = io::stdin().read_line(&mut a);
    if a.trim().eq_ignore_ascii_case("y") {
        say!("Rebooting in 10 seconds (run `shutdown /a` to cancel)...");
        sh("shutdown", &["/r", "/t", "10", "/c", "wsl-doctor: loading the hypervisor"]);
    } else {
        say!("Not rebooting. Reboot later; everything else is already fixed.");
    }
}

fn main() {
    let a: Vec<String> = env::args().skip(1).collect();
    let quiet = a.iter().any(|x| x == "--quiet");
    QUIET.store(quiet, Ordering::Relaxed);
    match a.first().map(String::as_str) {
        Some("diag") => {
            let s = status();
            println!("admin={} firmware={} hypervisor={} launchtype={} VMP={} WSLfeat={} {}",
                is_admin(), s.firmware, s.hypervisor, launch_type(), s.vmp, s.wsl_feat, wsl_version());
        }
        Some("watch-remove") => {
            for t in [TASK.to_string(), format!("{TASK}Logon")] {
                println!("{}", sh("schtasks", &["/delete", "/f", "/tn", &t]).1.trim());
            }
        }
        Some("fix") | None | Some("--elevated") => {
            if !is_admin() {
                if quiet || a.iter().any(|x| x == "--elevated") {
                    eprintln!("admin required");
                    std::process::exit(2);
                }
                elevate("--elevated");
            }
            full_repair(!quiet);
            pause();
        }
        Some(_) => println!("usage: wsl-doctor [fix [--quiet] | diag | watch-remove]  (no args = interactive repair)"),
    }
}
