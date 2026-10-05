# failsafe: product specification

## Purpose

Unattended devices get into states they cannot leave on their own. failsafe detects such
a state and resets the system to a proper one, without a person on site.

## Concepts

- **Detector**: an executable (binary or shell script) that tests one condition and
  reports the result through its exit status. Detectors define what "failure" means;
  failsafe knows nothing about networks, disks or services.
- **Action**: what failsafe does when a detector keeps failing. Actions form an ordered
  escalation list per detector. The prototype has one action: reboot.

## Detector contract

| Result | Meaning |
|---|---|
| exit 0 | healthy |
| exit 77 | cannot tell; counts as neither healthy nor unhealthy |
| any other exit, timeout, killed by signal | unhealthy |

- A detector reports a condition. It never decides to reboot.
- The last lines of its stdout/stderr are recorded as the reason.
- A missing or non-executable detector is a configuration error: logged, never counted
  as unhealthy. A bad deploy must not reboot the fleet.

## Policy

Set in failsafe's configuration, per detector, with defaults for detectors that have no
entry: run interval, timeout, consecutive-failure threshold, escalation list.

## Safety requirements

- **Reboot-loop protection.** A reboot cannot fix every failure (carrier outage, bad
  config). failsafe never reboots rapidly and never gives up permanently: it waits a grace
  period after boot, and each consecutive reboot that did not restore health doubles the
  wait before the next, up to one attempt per day. A period of sustained health resets it.
- **Reason survives the reboot**, so the cause can be reported afterwards.
- **Dry-run mode** logs the action it would have taken.

## Open

Proposed by the architect, not yet confirmed by the user:

- Exit code 77 for "cannot tell".
- Backoff with a one-per-day ceiling instead of a hard cap after which failsafe stops acting.
