# Active: Phase 2, real reboot behind the loop guard

Read first in `docs/architecture_design.md`: "Counting and deciding", "Reboot-loop guard",
"Reboot execution", "Testing". The formula, the state-file rules and the reboot sequence
are specified there and not repeated here.

**The rule of this phase:** no commit may contain a reachable real reboot without the
guard in front of it. Steps 1–5 build and test everything with a fake `Rebooter`; the real
one is wired in at step 6, last.

**Safety while developing:** never start the binary without `--dry-run` on a machine you
are not prepared to have rebooted, and never from a test (see step 8).

## Steps

- [ ] **1. `monitor`: the `failing` flag (closes R7).** Add the flag per the outcome
  table. "Returned to healthy" is emitted on the flag's true→false transition, so it is
  logged after a trip too. Expose `stable_since() -> Option<Instant>`: the time since
  which no detector has been `failing` (none if one is). Unit tests, including: trip, then
  healthy, yields `Recovered`; a trip does not start the stable clock.

- [ ] **2. `guard`: the pure part.** `GuardConfig` with the built-in defaults (boot grace
  300 s, backoff base 600 s, backoff max 86400 s, stable after 1800 s, reboot deadline
  120 s) in one `Default`. `required_uptime(streak)` and a verdict
  `check(uptime) -> Allowed | Refused { remaining }`. Unit tests: the documented sequence
  5, 15, 25, 45, 85 min; the ceiling; `streak = u32::MAX` does not overflow.

- [ ] **3. `guard`: the state file.** `state.toml` in the state directory, holding
  `streak` and the last reboot's reason (detector, cause, consecutive failures). Add
  `serde` (derive) and `toml`. Load: missing → streak 0; unreadable or corrupt → worst-case
  streak and an error. Save: atomic, as specified. Unit tests against a temp directory for
  each load case, a save/load round trip, and a save into a read-only directory.

- [ ] **4. `action`: the `Rebooter` trait and the sequence.** Trait with the three
  machine-touching operations (sync, request reboot, force reboot). The sequence function
  takes a `&mut dyn Rebooter` and the deadline. Unit tests with a fake that records calls:
  normal path; request cannot be spawned → force; force fails → error returned.

- [ ] **5. Orchestration in the library, not in `main`.** One function that handles a
  trip: take the uptime, ask the guard, then either refuse (log with time remaining),
  dry-run (log the verdict, touch nothing), or record the reboot in the state file and run
  the sequence. A failed state write means no reboot. Plus the stable path: when
  `stable_since` is older than `stable_after` and the streak is non-zero, clear it (one
  write, then no more). Unit tests with the fake `Rebooter`, a temp state directory and
  an injected uptime: refused in grace; allowed after; refused again at streak 1 until
  backoff passes; state write fails → fake never called; dry-run → state file unchanged;
  stable clears the streak exactly once.

- [ ] **6. Wire the real thing.** Real `Rebooter` (`libc::sync`, `systemctl reboot`,
  `libc::reboot(RB_AUTOBOOT)`); uptime from `/proc/uptime`; `--state-dir PATH` flag
  (default `/var/lib/failsafe`, created if missing; temporary until the config file, like
  `--detectors-dir`). Remove the "reboot is not implemented" refusal. In its place, without
  `--dry-run`: exit at startup if the effective uid is not 0.

- [ ] **7. Startup and logging.** Log the previous reboot's reason as a warning if the
  state file has one, and the current streak. Log every action requested and its result:
  executed, dry-run, or refused with the seconds remaining. If the state directory is not
  writable in real mode, log an error at startup and keep monitoring.

- [ ] **8. Integration tests.** Replace `test_refuse_real_reboot_without_dry_run`: it
  starts the binary without `--dry-run`, which after step 6 is a live daemon. New tests,
  all with `--dry-run` and a temp `--state-dir`: with no state file a trip logs "would
  reboot"; with a state file holding a high streak a trip logs "would be refused"; a
  corrupt state file logs the error and refuses; the state file is byte-identical after a
  dry run; a previous reboot reason is logged at startup. One test without `--dry-run`
  asserting the non-root startup exit, skipped when the test itself runs as root.

## Out of scope

Config file and any tuning of the guard values from outside (Phase 3), sd_notify and
watchdog pings during the reboot wait (Phase 4), detector output as the reason.

## Verify on the host

`make test`, `cargo clippy --all-targets` clean, `make` builds both targets.

## Verify on a board (needs someone with a board; about 90 minutes, mostly unattended)

Not automatable; the phase is not done without it.

1. Install the aarch64 binary and one detector that always exits 1. Start it as root at
   boot without `--dry-run` (a throwaway systemd unit is enough; the real one is Phase 4).
2. Expect reboots at about 5, 15, 25 and 45 minutes of uptime (plus up to three detector
   intervals each). After each boot the log shows the previous reason and the streak.
3. Replace the detector with one that exits 0. After 30 minutes the log shows the streak
   cleared, and `state.toml` has streak 0.
4. Record the observed uptimes in `vdesign/checkpoint.md`.
