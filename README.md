<div align="center">

# 🩺 wsl-doctor

**One-click WSL2 + Docker Desktop repair for Windows — a single dependency-free Rust binary.**

![Rust](https://img.shields.io/badge/Rust-std%20only-orange?logo=rust)
![Dependencies](https://img.shields.io/badge/dependencies-0-brightgreen)
![Platform](https://img.shields.io/badge/platform-Windows%2010%2F11-0078D4?logo=windows)
![License](https://img.shields.io/badge/license-MIT-green)

**No installer. No admin prompt until the work starts. No ambiguity about what is broken.**

</div>

---

## The problem

```
WSL2 is unable to start since virtualization is not enabled on this machine.
Error code: Wsl/Service/CreateInstance/CreateVm/HCS/HCS_E_HYPERV_NOT_INSTALLED
...
Update failed (exit code: 1603).
Error code: Wsl/UpdatePackage/ERROR_INSTALL_FAILURE
```

`wsl-doctor` finds and fixes the causes behind these two errors, shows live progress for every step, and installs
scheduled tasks so the same problems repair themselves in the future.

## What it fixes

| # | Check | Fix |
|---|-------|-----|
| 1 | Firmware virtualization, hypervisor state, Windows features | Reports state; enables `VirtualMachinePlatform` / WSL feature if missing |
| 2 | `hypervisorlaunchtype` set to **Off** (cause of `HCS_E_HYPERV_NOT_INSTALLED`) | `bcdedit /set hypervisorlaunchtype auto` |
| 3 | WSL installer's cached MSI missing from `C:\Windows\Installer` (cause of **1603 / 1612**) | Downloads the matching MSI from the official WSL release and restores it |
| 4 | Outdated WSL | `wsl --update`, default version set to WSL 2 |
| 5 | Docker Desktop running VMM-only | Starts `vmcompute`/`hns`/`vmms`/`HvHost`, enables the WSL2 engine next to VMM (settings backed up), restarts Docker, waits for `docker info` |
| 6 | Problems coming back | Registers boot + logon scheduled tasks that run `wsl-doctor fix --quiet` |

## Usage

Run the prebuilt binary that ships next to the source — no Rust toolchain required:

```powershell
.\wsl-doctor.exe            # interactive repair (asks for admin via UAC)
```

Or build from source (pure `std`, ~390 KB release binary):

```powershell
cargo build --release
.\target\release\wsl-doctor.exe
```

| Command | Purpose |
|---------|---------|
| *(no args)* | Full interactive repair with numbered live progress |
| `fix --quiet` | Silent self-heal (used by the scheduled tasks) |
| `diag` | One-line status |
| `watch-remove` | Remove the scheduled tasks |

Sample output:

```
[1/8] Reading firmware / hypervisor / Windows feature state (about 10s) ...
      -> firmware virt=true, hypervisor running=true, VMP=Enabled, WSL feature=Enabled (9.8s)
[2/8] Boot configuration (hypervisorlaunchtype) ...
      -> already Auto
[4/8] WSL installer cache repair (fixes update error 1603) ...
      -> MSI cache present (0.5s)
[5/8] Updating WSL (live output) ...
      -> wsl --update exit 0; WSL version: 3.0.5.0 (0.3s)
...
Ubuntu check: exit 0: Linux 6.6.87

ALL FIXED. Nothing left to do.
```

The tool is **idempotent** — safe to run any number of times; it only acts on what is actually broken.

## ⚠️ Honest limitation: one reboot

Windows starts the hypervisor in its boot loader. If `hypervisorlaunchtype` was `Off`, no tool can start it while
Windows is running. `wsl-doctor` fixes everything else live, then offers a reboot (**Y**). After that, the boot/logon
tasks finish the Docker and WSL2 setup automatically.

## Design

- Pure Rust, standard library only — **zero crates**, ~390 KB release binary.
- Self-elevates once (guarded, no relaunch loops); every step prints *before* it runs, so nothing hangs silently.
- Isolated steps: one failure never blocks the others; the run always reaches the summary and never fails the shell.
- Docker Desktop settings are backed up to `settings-store.json.wsl-doctor.bak` before any change.

## License

MIT — see [LICENSE](LICENSE).
