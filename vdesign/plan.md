# failsafe plan

`[ ]` todo, `[/]` in progress, `[x]` done

Design reference: `docs/product_specification.md` (what), `docs/architecture_design.md` (how).
Each phase ends with something runnable end to end; later phases widen it.

## Phases

- [x] **Phase 0: scaffold and multi-architecture static build.** `make` produces
  `dist/failsafe-<target>` for aarch64, armv7 (hard-float, added 2026-10-12) and x86_64.

- [x] **Phase 1: walking skeleton, dry-run only.** Detectors are discovered (symlinks
  followed), run under a timeout in their own process group, counted, and a trip logs
  "would reboot". `--once` tests a detector. 19 tests. Architect review R1–R6 closed.

- [/] **Phase 2: real reboot behind the loop guard.** Persisted streak, boot grace,
  backoff (confirmed by the user 2026-10-10), stable reset, boot ID check, reason logged
  at next startup, bounded reboot sequence with forced fallback. Host part done: 52 tests,
  architect review P1–P9 closed, commit `b368db8`.
  *Still open:* the board run, a permanently failing detector producing reboots at the
  uptimes the backoff formula predicts. It is done with the Phase 2A package; procedure in
  `active_plan.md`.

- [/] **Phase 2A: Ubuntu package.** `make deb` builds one `.deb` per architecture (`arm64`,
  `armhf`, `amd64`) with `cargo-deb`: binary, unit, directories; enabled and started on
  install; an operator's `disable` survives upgrades. Host part done: container lifecycle
  on amd64, architect review Q1–Q5 closed, commit `7292395`.
  *Still open:* the checks on the board, where service start, stop and restart run for
  the first time. Procedure in `active_plan.md`.

- [x] **Phase 3: configuration.** One TOML file read at startup, shipped in the package as
  a conffile with every key commented out at its default. Per-detector policy. Strict
  startup: an unknown key, a wrong type, an out-of-range value or a `[detector.NAME]`
  without a file stops failsafe with a message. Whole seconds, at most 365 days;
  `stable_after_secs` at least `boot_grace_secs + backoff_base_secs` (user decisions
  2026-10-13). As root, the config file, the detectors directory and each detector must be
  root-owned and not writable by others. Temporary flags removed. 75 unit and 18
  integration tests; unit tests also pass for armv7 under `qemu-arm`; architect review
  C1–C6 closed.

- [ ] **Phase 4: systemd watchdog integration.** `READY=1` / `WATCHDOG=1`, the unit becomes
  `Type=notify` with `WatchdogSec`, hardware watchdog via `RuntimeWatchdogSec` documented
  for the board. (The unit file and installation moved to Phase 2A.)
  Needs the board facts listed as unknown in the architecture document.
  *Done when:* on a board, `kill -STOP` of failsafe gets it restarted by systemd, and the
  service survives a reboot cycle enabled.

## Candidates

- Detector output tail recorded as the reboot reason (redirect to a file under `/run`,
  read after exit). Specified in the product spec; deferred because the journal already
  has the output.
- Actions below reboot (repair script, restart a service) and stepping through the
  escalation list. The `Action` enum and `actions` config key are the seam.
- Coordination with `network-utils/wan-failover` if a WAN detector is added.
- Reload detectors and config on `SIGHUP` instead of restart.
- Yocto recipe, if the image is built from `raft-oe`.
