# Checkpoint (2026-10-11)

Phase 2 host implementation and architect review findings (P1–P9) are complete:
- P1: `--test-uptime-secs` rejected at startup without `--dry-run`.
- P2: `reboot_deadline` bounds `systemctl reboot` poll/kill; `sync()` bounded by timeout; boot ID stored in `state.toml` to detect un-rebooted restarts at startup; failed forced reboot leaves streak unincremented and sets `force_next = true`.
- P3: `load_state` treats only `io::ErrorKind::NotFound` as streak 0; unreadable/stat errors assume worst-case streak.
- P4: Directory fsync errors in `save_state` propagated to abort reboots.
- P5: Unreadable `/proc/uptime` logs error before returning zero.
- P6: Errors logged on `systemctl reboot` failure; `--state-dir` not created in dry-run mode.
- P7: Sound `bounded_sync` implementation via `Rebooter::start_sync(&mut self) -> Receiver<()>` channel communication; detached thread only executes `libc::sync()` without aliasing or holding references/pointers to `Rebooter`.
- P8: Eliminated flaky `ETXTBUSY` via `TEST_LOCK` mutex serialization across script-writing and process-spawning tests.
- P9: Replaced direct `eprintln!("<3>failsafe: ...")` in `action.rs` with `log_error!`.
- 52 tests pass (35 unit, 17 integration); `cargo clippy --all-targets` has 0 warnings; `make` produces static binaries for `x86_64` and `aarch64`.

Phase 2 is ready for the physical board test (~90 minutes) before being marked done.

Phase 2A (Debian/Ubuntu Packaging) host implementation and architect review (Q1–Q5) are complete:
- License: Mono MIT license chosen by user; `LICENSE` added, `Cargo.toml` updated, decision recorded in `docs/architecture_design.md`.
- Q1 & Q2: Integrated `cargo-deb` `#DEBHELPER#` (`deb-systemd-helper`) to track operator enable/disable state; implemented robust state-aware start/restart without `>/dev/null || true` so upgrade restarts only currently active units (keeping operator-disabled/stopped services off) and start/restart failures are visible in `apt`.
- Q3: Updated `README.md` status line ("Phase 2 and 2A built, awaiting board verification"), added file creation command to "Adding a detector", clarified systemd restarts on exit until Phase 4 watchdog integration.
- Q4: Made package filename and control version agree: `Makefile` defines `DEB_VERSION := $(VERSION)-$(DEB_REVISION)`, producing `dist/failsafe_0.1.0-1_arm64.deb`, `dist/failsafe_0.1.0-1_armhf.deb`, and `dist/failsafe_0.1.0-1_amd64.deb` matching control field `Version: 0.1.0-1`.
- Q5: Deleted duplicate unit file `failsafe.service` at repository root; packaged unit is maintained in `debian/failsafe.service`.
- Multi-arch expansion: Added `armv7-unknown-linux-musleabihf` (`armhf`) to default `TARGETS` in `Makefile` and `README.md`; compiles static musl binary `dist/failsafe-armv7-unknown-linux-musleabihf` and Debian package `dist/failsafe_0.1.0-1_armhf.deb` without external C cross-compilers; verified running under `qemu-arm`.
- Verified on host: 52 tests pass, clippy clean, `make deb` builds all three packages (`arm64`, `armhf`, `amd64`). Full lifecycle (`install`, `remove`, `reinstall`, `purge`) passed in Ubuntu container.

Phase 3 (Configuration) host implementation and architect review (C1–C6) are complete:
- Strict TOML configuration parsing and validation in `src/config.rs`: `*_secs` keys are whole integers (`u64`) between 1 and 31536000 (365 days; `boot_grace_secs >= 0`), decimals/floats and values exceeding the ceiling rejected; zero threshold, invalid action, inverted backoff (`max < base`), `stable_after_secs` below `boot_grace_secs + backoff_base_secs` (evaluated after defaults), or relative paths fail startup immediately.
- File and CLI handling: deduplicated `load_file` opens file handle once and checks handle metadata; missing default `/etc/failsafe/failsafe.toml` uses built-in defaults; missing `--config` path is a hard error; `--dry-run`, `--once`, `-h`/`--help`.
- Root security ownership checks: pure function `check_permissions(euid, uid, mode)` in `src/detector.rs` enforced on config file and detectors (directory and files) when effective UID is 0 (must be root-owned and not group/world writable `0o022`). Permission error mode masked with `0o7777` to display clean octal permissions (`0775`).
- Wire into daemon & discovery: removed all temporary CLI flags; each detector receives its own resolved policy; unmatched `[detector.NAME]` tables halt startup; clean key-value format logging for detector overrides.
- Packaging & parity: shipped `/etc/failsafe/failsafe.toml` template conffile with explanatory header (documenting whole seconds rule), per-key comments (accurate continuous healthy reset semantics for `stable_after_secs` and floor requirement), and commented detector example; unit parity tests verify commented-out and uncommented template match `Config::default()` and detector example parses validly; `debian/postrm` cleans conffile on purge.
- Verification: 93 tests pass (75 unit, 18 integration); `cargo clippy --all-targets` 0 warnings; `make deb` builds all 3 packages (`arm64`, `armhf`, `amd64`); armv7 unit tests pass under `qemu-arm`; Ubuntu 24.04 container lifecycle verified.

## Next Steps

1. Board run on the aarch64 Ubuntu board:
   - Install `dist/failsafe_0.1.0-1_arm64.deb` (`sudo apt install ./failsafe_0.1.0-1_arm64.deb`).
   - Verify `systemctl status failsafe` is active and enabled, journal shows "using configuration from /etc/failsafe/failsafe.toml" (or built-in defaults) and "no detectors found" warning.
   - Verify upgrade path: reinstall same package, service restarts and stays active; modified config file survives.
   - Verify operator override persistence: after `systemctl disable --now failsafe`, reinstalling package leaves service disabled and stopped.
   - Verify remove/purge behavior: `apt remove failsafe`, then `apt purge failsafe`.
   - Reinstall package, place failing detector in `/etc/failsafe/detectors.d/` and observe reboots at ~5, 15, 25, 45 minutes of uptime.
   - Replace detector with exit 0 and verify streak reset after 30 minutes.
   - Record observed uptimes in this file.
2. Exit code 77 is implemented but still listed as unconfirmed in `docs/product_specification.md`.
3. Run the probe on the board before Phase 4 (kernel, systemd, watchdog details).
