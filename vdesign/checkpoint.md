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

## Next Steps

1. Board run (~90 minutes):
   - Install `dist/failsafe-aarch64-unknown-linux-musl` and a failing detector on the physical board. Run as root without `--dry-run`.
   - Observe reboots at ~5, 15, 25, 45 minutes of uptime and verify streak logging.
   - Switch detector to exit 0 and verify streak reset after 30 minutes.
   - Record observed uptimes in this file.
2. Exit code 77 is implemented but still listed as unconfirmed in `docs/product_specification.md`.
3. Run the probe on the board before Phase 4 (kernel, systemd, watchdog details).
