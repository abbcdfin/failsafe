# failsafe: architecture design

What failsafe does and why is in `product_specification.md`. This document is how.

## Shape

One single-threaded daemon. Each loop iteration: run the detectors that are due, update
their failure counters, ask the guard whether an action is allowed, execute it, ping the
systemd watchdog.

```
main ── config ──► Config
  │
  └─► monitor (engine: schedule, counters, decisions)   pure logic, clock injected
        ├─► detector  (discover, spawn with timeout ─► Outcome)
        ├─► guard     (reboot streak, backoff, persisted state)
        ├─► action    (execute Action::Reboot)
        └─► notify    (READY=1 / WATCHDOG=1 to systemd)
```

| Module | Owns | Must not |
|---|---|---|
| `config` | parsing and validating all input (file, flags); defaults | be consulted for a default anywhere else |
| `detector` | discovery, process spawning, timeout, mapping exit status to `Outcome` | count failures or know about actions |
| `monitor` | per-detector schedule and consecutive-failure counter; the decision to request an action | spawn processes or read clocks directly |
| `guard` | whether a reboot is allowed now; the persisted state file | know which detector asked |
| `action` | carrying out an `Action` | decide whether to |
| `notify` | the sd_notify datagram protocol | anything else |

`monitor` and `guard` take time as a parameter and return decisions as values, so they are
unit-tested without processes, sleeping or root.

## Types

```rust
enum Outcome { Healthy, Unhealthy(Cause), Unknown, Broken(String) }
enum Cause   { Exit(i32), Signal(i32), Timeout }
enum Action  { Reboot }                    // the escalation seam; one variant today

struct Policy { interval: Duration, timeout: Duration, threshold: u32, actions: Vec<Action> }
struct Detector { name: String, path: PathBuf, policy: Policy }
```

`Outcome` is the detector contract from the product specification, and the only place it
is encoded. `Broken` covers "could not be run at all" (not executable, spawn error).

## Detector execution

- **Discovery.** Every regular file in the detectors directory is a detector, named by its
  file name, run in name order. Scanned once at startup; restart to pick up changes.
- **Spawn.** stdin `/dev/null`, cwd `/`, own process group (`CommandExt::process_group(0)`),
  stdout/stderr inherited so output reaches the journal.
- **Timeout.** Poll `try_wait` until the deadline, then `SIGKILL` the whole process group
  (`libc::kill(-pid, SIGKILL)`), so children of a shell-script detector do not linger.
- **Sequential.** One detector at a time. The watchdog is pinged between detectors, so the
  systemd `WatchdogSec` only has to exceed the largest single timeout, not their sum.

## Counting and deciding (`monitor`)

Per detector, one number: `consecutive_failures`.

| Outcome | Effect |
|---|---|
| `Healthy` | counter = 0 |
| `Unhealthy` | counter += 1 |
| `Unknown` | unchanged |
| `Broken` | unchanged, logged as error |

When the counter reaches `threshold` the detector is *tripped* and `monitor` requests its
action. After the action is attempted (executed, dry-run, or refused by the guard) the
counter resets to 0, so a persisting failure re-trips after another `threshold` runs
instead of every run.

The system is *stable* when every detector's counter has been 0 continuously for
`stable_after`.

## Reboot-loop guard

Goal: never reboot rapidly, never give up permanently, and do not depend on the wall
clock (boards without an RTC boot with the wrong time).

State is one number that survives reboots: `streak`, the count of failsafe-initiated
reboots since the system was last stable.

```
required_uptime(streak) = boot_grace                                         if streak == 0
                        = boot_grace + min(backoff_base * 2^(streak-1), backoff_max)   otherwise

reboot allowed  ⇔  system uptime ≥ required_uptime(streak)
```

Uptime comes from `/proc/uptime`, so restarting the daemon does not reset the grace
period. With the defaults a detector that never recovers produces reboots at 5 min, then
15, 25, 45, 85 min of uptime, doubling until one reboot per 24 h.

- **Before rebooting:** `streak += 1`, record the reason, write the state file (temp file,
  `fsync`, rename, `fsync` the directory).
- **On becoming stable:** `streak = 0`, reason cleared.
- **At startup:** if a reason is present, log it as a warning ("previous reboot was
  initiated by failsafe: detector X, cause, streak N").
- **State file missing:** `streak = 0` (first boot).
- **State file unreadable or corrupt:** assume the worst, `streak` = the value that yields
  `backoff_max`, and log an error. Treating it as 0 would disable the guard.
- **State directory not writable:** refuse to reboot and log an error each time. A reboot
  that cannot be counted is a potential loop.

## Reboot execution (`action`)

1. `sync()`.
2. `systemctl reboot`.
3. Keep pinging the watchdog. If the system is still up after `reboot_deadline`, call
   `libc::reboot(RB_AUTOBOOT)`, which does not depend on userspace.

In dry-run mode `action` logs what it would have done and `guard` leaves the state file
untouched.

## Configuration

One TOML file, default `/etc/failsafe/failsafe.toml`. Missing file: all defaults.
Malformed file, unknown key, or a `[detector.NAME]` with no matching file: exit non-zero
at startup with the reason.

```toml
dry_run        = false
detectors_dir  = "/etc/failsafe/detectors.d"
state_dir      = "/var/lib/failsafe"

[reboot]
boot_grace_secs      = 300
backoff_base_secs    = 600
backoff_max_secs     = 86400
stable_after_secs    = 1800
reboot_deadline_secs = 120

[defaults]                 # policy for any detector without its own table
interval_secs = 60
timeout_secs  = 30
threshold     = 3
actions       = ["reboot"]

[detector."lte-link"]      # key is the detector's file name; overrides defaults per field
threshold = 5
```

Command line: `failsafe [--config PATH] [--dry-run] [--once]`. `--once` runs every detector
one time, prints name and outcome, and exits 0 only if none was unhealthy or broken; it is
how a detector is tested on a board. Flags are parsed by hand; three flags do not justify
an argument-parsing crate.

When running as root, the detectors directory and each detector must be owned by root and
not writable by group or others; otherwise the detector is `Broken`.

## systemd and the liveness chain

Hardware watchdog watches systemd (`RuntimeWatchdogSec` in `system.conf`), systemd watches
failsafe (`WatchdogSec`), failsafe watches the system.

- `notify` sends `READY=1` once detectors are discovered and `WATCHDOG=1` from the main
  loop, as datagrams to `$NOTIFY_SOCKET` using `std::os::unix::net::UnixDatagram`. No
  `NOTIFY_SOCKET`: no-op, so the binary runs from a shell unchanged.
- The ping comes from the loop itself, never a helper thread: it must prove the loop turns.
- Unit: `Type=notify`, `Restart=always`, `WatchdogSec` > largest detector timeout.

## Logging

Lines on stderr with sd-daemon priority prefixes (`<3>` error, `<4>` warning, `<6>` info).
journald interprets them; no logging crate.

## Dependencies

`serde` + `toml` (configuration and state file), `libc` (process-group kill, `reboot`,
`sync`). All pure Rust. Nothing else without a recorded reason.

## Testing

- **Unit:** `monitor` (counter table, trip, stable) and `guard` (`required_uptime`, state
  transitions, corrupt file) with injected time.
- **Integration (`tests/`):** a temp directory of shell-script detectors run through the
  real binary with `--dry-run` / `--once`: healthy, unhealthy, exit 77, timeout with a
  lingering child, non-executable file.
- **Never in tests:** a real reboot. Reboot execution is verified on a board.
- All tests run on the host with `make test`; no root, no network.

## Decisions

### Rust, fully static musl binary (2026-10-05)

A failsafe must work when the system is sick: out of memory, full or corrupt disk, broken
userspace. A static binary depends only on the CPU architecture and the kernel (Rust
needs Linux 3.2 or newer). The sibling `wan-failover` is Python; consistency with it was
judged less important than not needing an interpreter.

### Multi-architecture build (2026-10-05)

The tool will be deployed to more than one architecture; the first board is aarch64.

- `TARGETS` in the `Makefile` is the one list of architectures. `make` installs the
  rustup targets, builds all of them in one cargo invocation and copies the results to
  `dist/failsafe-<target>`.
- `.cargo/config.toml` applies one rule to every musl target: link with the toolchain's
  bundled `rust-lld`, fully static. No per-target entry and no C cross-toolchain.
- Verified by building and running (under qemu-user) aarch64, armv7hf, riscv64gc and
  x86_64. Limit: the target must have a prebuilt `rust-std` (`rustup target list | grep
  musl`); MIPS, for example, does not.
- CMake was considered and rejected: Cargo is the build system, the Makefile is a wrapper,
  and there is no C/C++ to integrate.

**Constraint that follows: dependencies must be pure Rust.** A crate that compiles C
needs a C cross-compiler per architecture and breaks the one-rule build.

### Backoff instead of a reboot cap (2026-10-05)

A hard cap ("N reboots, then stop") leaves an unattended device permanently unprotected
after a transient fault. Exponential backoff with a ceiling gives both properties with
one mechanism: no rapid loop, and still one attempt per `backoff_max`. Counting a streak
against uptime, not reboots per wall-clock window, keeps it correct without an RTC.

### Single-threaded, sequential detectors (2026-10-05)

Detectors are few and cheap; concurrency would add shared state to a program whose value
is being predictable. The cost is that one slow detector delays the others by at most its
timeout. Revisit if detector count or timeouts make a full round exceed the shortest interval.

## Unknown about the first board

Kernel version, systemd version, watchdog driver timeout range and `nowayout`. The probe
in the session scratchpad reports them; it has not been run on the board. Needed before
Phase 4, not before.
