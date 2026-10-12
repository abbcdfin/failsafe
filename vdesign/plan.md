# failsafe plan

`[ ]` todo, `[/]` in progress, `[x]` done

Design reference: `docs/product_specification.md` (what), `docs/architecture_design.md` (how).
Each phase ends with something runnable end to end; later phases widen it.

## Phases

- [x] **Phase 0: scaffold and multi-architecture static build.** `make` produces
  `dist/failsafe-<target>` for aarch64 and x86_64.

- [x] **Phase 1: walking skeleton, dry-run only.** Detectors are discovered (symlinks
  followed), run under a timeout in their own process group, counted, and a trip logs
  "would reboot". `--once` tests a detector. 19 tests. Architect review R1–R6 closed.
  Left over: `--detectors-dir` and the `--test-*` flags are temporary until Phase 3.

- [/] **Phase 2: real reboot behind the loop guard.** Persisted streak, boot grace,
  backoff (confirmed by the user 2026-10-10), stable reset, boot ID check, reason logged
  at next startup, bounded reboot sequence with forced fallback. Host part done: 52 tests,
  architect review P1–P9 closed, commit `b368db8`.
  *Still open:* the board run, a permanently failing detector producing reboots at the
  uptimes the backoff formula predicts. It is done with the Phase 2A package; procedure in
  `active_plan.md`.

- [/] **Phase 2A: Ubuntu package.** `make deb` produces one `.deb` per architecture with
  the binary, the detector and state directories and the systemd unit; the service is
  enabled and started on install. Accepted by the user 2026-10-11; rationale in the
  architecture document, "Packaging". Board in `active_plan.md`.
  *Done when:* on the Ubuntu board, `apt install ./failsafe_*.deb` leaves the service
  running, and remove, purge and upgrade behave as specified.

- [ ] **Phase 3: configuration.** TOML file, per-detector policy and intervals, startup
  validation, root-ownership check on detectors. The package gains the config file as a
  conffile.
  *Done when:* every key in the architecture document's example is honoured, and each
  invalid-input case exits non-zero with a message naming the problem.

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
