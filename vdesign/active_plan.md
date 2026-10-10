# Active: Phase 1, walking skeleton (dry-run only)

Read `docs/architecture_design.md` first; module boundaries and types are defined there.
Build in this order: each step leaves the binary runnable.

- [x] **1. `detector`: run one executable to an `Outcome`.** Spawn in its own process
  group, poll with timeout, kill the group on timeout, map exit status (0 healthy, 77
  unknown, other/signal/timeout unhealthy, spawn failure broken). Add `libc`.
- [x] **2. `detector`: discovery.** Regular files in the directory, sorted by name.
- [x] **3. `main`: `--once`.** Parse `--detectors-dir`, `--dry-run`, `--once` by hand. Run
  every detector once, print `name outcome`, exit 0 only if none unhealthy or broken.
  First end-to-end path; everything after widens it.
- [x] **4. `monitor`: counters and trip.** Pure logic with time passed in: the outcome
  table, trip at threshold, reset after the action is attempted. Unit tests.
- [x] **5. `main`: the loop.** Fixed interval for all detectors (built-in default), feed
  outcomes to `monitor`, on trip log `would reboot: detector X, cause, N consecutive
  failures`.
- [x] **6. Refuse real reboots.** Without `--dry-run`, exit non-zero at startup with
  "reboot is not implemented; run with --dry-run".
- [x] **7. Logging.** stderr with `<N>` priority prefixes.
- [x] **8. Integration tests** in `tests/`, shell-script detectors in a temp dir: healthy,
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

## Architect review (2026-10-05): fix before Phase 2

Phase 1 meets its done-criteria. These must be closed before reboots become real.

- [x] **R1. Symlinked detectors are silently skipped.** `discover` uses
  `DirEntry::file_type`, which does not follow symlinks. Follow them (`fs::metadata`);
  a dangling symlink is a `Broken` detector, not an absent one. Add both to the tests.
- [x] **R2. One test hook, and it fails loudly.** Remove `Policy::from_env_or_default`
  and the `FAILSAFE_*` variables: an environment variable must not be able to change
  reboot policy. Keep the `--test-*` flags until Phase 3; an unparsable or zero value
  exits non-zero.
- [x] **R3. The daemon says what it is doing.** At startup log the detectors directory
  and each detector found (warning if none). Log every unhealthy run with its count
  (`detector X unhealthy (exit 1), 2/3`) and the return to healthy.
- [x] **R4. Sleep until the next detector is due.** The idle loop wakes every 50 ms.
- [x] **R5. No invented action.** `monitor` falls back to `Action::Reboot` when
  `actions` is empty; an empty list is invalid input and must never reach `monitor`.
  Handle the trip's action with an exhaustive `match`, not `if action == Reboot`.
- [x] **R6. Clean `cargo clippy --all-targets`** (5 collapsible-if warnings).

## Architect re-review (2026-10-10)

R1–R6 verified closed. One small item, to fold into Phase 2 (it touches the same code):

- [ ] **R7. Recovery after a trip is not logged.** `action_attempted` zeroes the counter,
  so the next healthy run is not seen as a recovery and "returned to healthy" is never
  logged for a detector that tripped. Once the guard can refuse a reboot this is the case
  an operator most needs to see. Track "was failing" separately from the counter.
