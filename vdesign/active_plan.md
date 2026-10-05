# Active: Phase 1, walking skeleton (dry-run only)

Read `docs/architecture_design.md` first; module boundaries and types are defined there.
Build in this order: each step leaves the binary runnable.

- [ ] **1. `detector`: run one executable to an `Outcome`.** Spawn in its own process
  group, poll with timeout, kill the group on timeout, map exit status (0 healthy, 77
  unknown, other/signal/timeout unhealthy, spawn failure broken). Add `libc`.
- [ ] **2. `detector`: discovery.** Regular files in the directory, sorted by name.
- [ ] **3. `main`: `--once`.** Parse `--detectors-dir`, `--dry-run`, `--once` by hand. Run
  every detector once, print `name outcome`, exit 0 only if none unhealthy or broken.
  First end-to-end path; everything after widens it.
- [ ] **4. `monitor`: counters and trip.** Pure logic with time passed in: the outcome
  table, trip at threshold, reset after the action is attempted. Unit tests.
- [ ] **5. `main`: the loop.** Fixed interval for all detectors (built-in default), feed
  outcomes to `monitor`, on trip log `would reboot: detector X, cause, N consecutive
  failures`.
- [ ] **6. Refuse real reboots.** Without `--dry-run`, exit non-zero at startup with
  "reboot is not implemented; run with --dry-run".
- [ ] **7. Logging.** stderr with `<N>` priority prefixes.
- [ ] **8. Integration tests** in `tests/`, shell-script detectors in a temp dir: healthy,
  unhealthy, exit 77, timeout whose child process must be gone afterwards,
  non-executable file, empty directory.

## Built-in defaults for this phase

interval 60 s, timeout 30 s, threshold 3. Tests must be able to shorten them: put them in
one `Policy::default()` and give the test build a way in (hidden flags or env are both
acceptable; they are replaced by the config file in Phase 3).

## Out of scope for this phase

State file, guard, real reboot, TOML, per-detector policy, ownership check, sd_notify.
Do not stub them; leave them absent.

## Verify

`make test` passes; `make` still builds both targets; `qemu-aarch64
dist/failsafe-aarch64-unknown-linux-musl --once --detectors-dir <dir>` behaves like the
host binary.
