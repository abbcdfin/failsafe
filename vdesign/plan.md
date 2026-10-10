# failsafe plan

`[ ]` todo, `[/]` in progress, `[x]` done

Design reference: `docs/product_specification.md` (what), `docs/architecture_design.md` (how).
Each phase ends with something runnable end to end; later phases widen it.

## Phases

- [x] **Phase 0: scaffold and multi-architecture static build.** `make` produces
  `dist/failsafe-<target>` for aarch64 and x86_64.

- [x] **Phase 1: walking skeleton, dry-run only.** Discover detectors in a directory, run
  them on one fixed interval with a timeout, count consecutive failures, log "would
  reboot" when a detector trips. Full detector contract (0 / 77 / other / timeout /
  broken). `--once` for testing a detector. Policy values are the built-in defaults; the
  only inputs are `--detectors-dir`, `--dry-run`, `--once`.
  *Done when:* the integration tests pass on the host, and running without `--dry-run`
  exits with "reboot is not implemented" instead of pretending.

- [ ] **Phase 2: real reboot behind the loop guard.** Persisted streak, boot grace,
  backoff, stable reset, reason logged at next startup, reboot with forced fallback. The
  guard and the reboot ship together: no build may contain one without the other.
  *Done when:* guard unit tests pass, and on a board a permanently failing detector
  produces reboots at the uptimes the backoff formula predicts.

- [ ] **Phase 3: configuration.** TOML file, per-detector policy and intervals, startup
  validation, root-ownership check on detectors.
  *Done when:* every key in the architecture document's example is honoured, and each
  invalid-input case exits non-zero with a message naming the problem.

- [ ] **Phase 4: systemd integration and deployment.** `READY=1` / `WATCHDOG=1`, unit file,
  `make install`, hardware watchdog via `RuntimeWatchdogSec` documented for the board.
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
