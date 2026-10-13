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

- **Discovery.** Every entry in the detectors directory that is a regular file, or a
  symlink, is a detector, named by its file name, run in name order. Symlinks are
  followed; one that does not resolve to a regular file is a `Broken` detector, never
  silently skipped. Subdirectories are ignored. Scanned once at startup; restart to pick
  up changes.
- **Spawn.** stdin `/dev/null`, cwd `/`, own process group (`CommandExt::process_group(0)`),
  stdout/stderr inherited so output reaches the journal.
- **Timeout.** Poll `try_wait` until the deadline, then `SIGKILL` the whole process group
  (`libc::kill(-pid, SIGKILL)`), so children of a shell-script detector do not linger.
- **Idle.** Between runs the loop sleeps until the next detector is due (later bounded by
  the watchdog ping interval); it does not poll.
- **Sequential.** One detector at a time. The watchdog is pinged between detectors, so the
  systemd `WatchdogSec` only has to exceed the largest single timeout, not their sum.

## Counting and deciding (`monitor`)

Per detector, a counter `consecutive_failures` and a flag `failing`.

| Outcome | Counter | `failing` |
|---|---|---|
| `Healthy` | = 0 | false |
| `Unhealthy` | += 1 | true |
| `Unknown` | unchanged | unchanged |
| `Broken` | unchanged, logged as error | unchanged |

When the counter reaches `threshold` the detector is *tripped* and `monitor` requests its
action. After the action is attempted (executed, dry-run, or refused by the guard) the
counter resets to 0, so a persisting failure re-trips after another `threshold` runs
instead of every run.

The counter decides when to act; the flag records whether the detector's last verdict was
unhealthy. They differ after a trip: the counter is back at 0 while the detector is still
`failing`. Only a `Healthy` run clears the flag, and that transition is what is logged as
"returned to healthy".

The system is *stable* when no detector has been `failing` continuously for
`stable_after`. Stability is defined on the flag, not the counter, so a refused or dry-run
action (which zeroes the counter) can never make a sick system look stable.

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
  `fsync`, rename, `fsync` the directory). If the write fails, the reboot does not happen.
- **On becoming stable:** `streak = 0`, reason cleared.
- **Write only on change.** The state file is written before a reboot and when a non-zero
  streak is cleared, never periodically; the storage is flash.
- **Arithmetic saturates.** `required_uptime` must not overflow for any `streak`.
- **At startup:** if a reason is present, log it as a warning ("previous reboot was
  initiated by failsafe: detector X, cause, streak N").
- **State file missing:** `streak = 0` (first boot).
- **State file unreadable or corrupt:** assume the worst, `streak` = the value that yields
  `backoff_max`, and log an error. Treating it as 0 would disable the guard.
- **State directory not writable:** refuse to reboot and log an error each time. A reboot
  that cannot be counted is a potential loop.

## Reboot execution (`action`)

1. `sync()`.
2. `systemctl reboot`. If it cannot be spawned, go straight to step 3.
3. Keep pinging the watchdog. If the system is still up after `reboot_deadline`, `sync()`
   again and call `libc::reboot(RB_AUTOBOOT)`, which does not depend on userspace. If that
   fails too, log an error, do not count the attempt in the streak, and return to
   monitoring; the next allowed trip goes straight to this step.

There is no intermediate `systemctl reboot --force`: if failsafe is still alive at the
deadline, systemd never started the shutdown, and that command depends on the same systemd.

Nothing in this sequence may wait without a bound. `systemctl reboot` is spawned and
polled up to the deadline; `sync()` can block on dead storage, so the sequence moves on
after a fixed wait whether or not it has returned.

**Knowing whether the reboot happened.** There are three signals, of increasing strength:

| Signal | What it proves |
|---|---|
| `systemctl reboot` exits non-zero, or cannot be spawned | The request failed. Force at once. |
| `systemctl reboot` exits zero | Only that the request was queued; the command does not wait for the reboot. Keep waiting. |
| failsafe is still running at the deadline | The machine has not rebooted. Kill the `systemctl` child if it is still running, then force. |
| Boot ID at next startup | Definitive. The state file stores the kernel's boot ID (`/proc/sys/kernel/random/boot_id`) with the reason. Same ID at startup: failsafe was restarted and the machine never rebooted; the attempt is not counted in the streak and the next allowed reboot skips to the forced step. Different ID: it rebooted. |

failsafe is itself stopped early in the shutdown it requested, so the forced reboot only
covers a `systemctl reboot` that never gets that far. A shutdown that hangs later is
covered by systemd's `RebootWatchdogSec` and the hardware watchdog (Phase 4).

The steps that touch the machine sit behind one trait (`Rebooter`), so the sequence
around them (guard verdict, state write, deadline) is tested with a fake.

In dry-run mode the guard is still consulted, read-only: the log says what real mode would
have done ("would reboot" or "would be refused: N s of backoff remaining"), and the state
file is left untouched.

Real mode requires root: started without `--dry-run` and with a non-zero effective uid,
failsafe exits at startup. It could not reboot anyway, and the check keeps a developer
machine from ever being a reboot target by accident.

## Configuration

One TOML file, read once at startup. Changing it takes effect at the next restart.

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

Every key is optional; a missing key takes the value shown above.

**Which file.** Default `/etc/failsafe/failsafe.toml`. If the default file does not exist,
all defaults apply. A file named with `--config` must exist.

**Strictness.** Any of the following stops failsafe at startup, non-zero, with a message
naming the file and the key. An invalid value never falls back to its default: the operator
believes the file is in force, so running with something else is worse than not running.

- the file cannot be read or is not valid TOML
- an unknown key or table, at any level (a mistyped key must not be silently ignored)
- a value of the wrong type
- a `[detector.NAME]` with no matching file in the detectors directory
- a value outside its range (below)

| Key | Rule |
|---|---|
| `*_secs` | a whole number of seconds; a decimal is rejected; at most 31536000 (365 days), so no time arithmetic can overflow |
| `interval_secs`, `timeout_secs`, `backoff_base_secs`, `reboot_deadline_secs` | 1 or more |
| `boot_grace_secs` | 0 or more |
| `backoff_max_secs` | not below `backoff_base_secs` |
| `stable_after_secs` | not below `boot_grace_secs + backoff_base_secs` |
| `threshold` | integer, 1 or more |
| `actions` | non-empty list; only `"reboot"` exists |
| `detectors_dir`, `state_dir` | absolute path |

The rules are checked on the final values, after defaults are filled in, so a file that
sets only one side of a pair is still checked against the default of the other.

**Command line.** `failsafe [--config PATH] [--dry-run] [--once]`, parsed by hand.

- Dry-run is on if either `--dry-run` or `dry_run = true` says so. Neither can turn the
  other off, so the flag can only make a run safer.
- `--once` runs every detector one time with its configured timeout, prints name and
  outcome, and exits 0 only if none was unhealthy or broken. It is how a detector, and a
  configuration, is tested on a board. It needs no root.
- Paths, intervals and guard values have no flags. The temporary `--detectors-dir`,
  `--state-dir` and `--test-*` flags of Phases 1 and 2 are removed; tests write a config
  file.

**Ownership.** When running as root, failsafe executes whatever is in the detectors
directory as root, so nobody else may be able to change it:

- The config file must be owned by root and not writable by group or others; otherwise
  startup fails.
- The detectors directory and each detector (the file a symlink resolves to) must be owned
  by root and not writable by group or others. A detector that fails this is `Broken`:
  logged, never run, never counted. If the directory fails it, every detector is `Broken`.
  Checked before every run, not only at startup.
- Not root (dry-run or `--once` by a developer): no ownership check.

## systemd and the liveness chain

Hardware watchdog watches systemd (`RuntimeWatchdogSec` in `system.conf`), systemd watches
failsafe (`WatchdogSec`), failsafe watches the system.

- `notify` sends `READY=1` once detectors are discovered and `WATCHDOG=1` from the main
  loop, as datagrams to `$NOTIFY_SOCKET` using `std::os::unix::net::UnixDatagram`. No
  `NOTIFY_SOCKET`: no-op, so the binary runs from a shell unchanged.
- The ping comes from the loop itself, never a helper thread: it must prove the loop turns.
- Unit: `Type=notify`, `Restart=always`, `WatchdogSec` > largest detector timeout.

## Packaging

One Debian package per architecture, `failsafe_<version>_<arch>.deb` (`arm64`, `armhf`, `amd64`),
built by `make deb` from the static binaries. No dependencies: the binary needs only the
kernel, so one package serves every Ubuntu release.

| Path | Content | Owner, mode |
|---|---|---|
| `/usr/sbin/failsafe` | the binary | root, 0755 |
| `/usr/lib/systemd/system/failsafe.service` | the unit | root, 0644 |
| `/etc/failsafe/detectors.d/` | empty; detectors are installed by the operator | root, 0755 |
| `/var/lib/failsafe/` | state file | root, 0755 |

- **Unit:** `ExecStart=/usr/sbin/failsafe` with no flags (the built-in defaults are the
  paths above), root, `Restart=always` with a restart delay, `WantedBy=multi-user.target`.
  `Type=simple` until Phase 4 makes it `Type=notify` with `WatchdogSec`.
- **Install:** the service is enabled and started. This is safe because the package ships
  no detectors: with an empty directory failsafe only logs a warning and can never reboot.
  The package must never ship a detector.
- **Upgrade:** the operator's choice is kept. A running service is restarted; a stopped
  but enabled one is started; a disabled one stays disabled and stopped. The streak is on
  disk and survives; the in-memory failure counts start again.
- **Scripts:** enabling is tracked by `deb-systemd-helper` (from `init-system-helpers`,
  essential on Ubuntu, so not declared as a dependency). A failed start or restart fails
  the installation visibly instead of being ignored.
- **Remove:** stops the service; detectors and state stay. The enabled or disabled choice
  is remembered and restored by a reinstall; only a purge resets it to enabled.
- **Purge:** also deletes `/var/lib/failsafe`, and `/etc/failsafe/detectors.d` only if it
  is empty. Detectors belong to the operator, not to the package.

## Logging

Lines on stderr with sd-daemon priority prefixes (`<3>` error, `<4>` warning, `<6>` info).
journald interprets them; no logging crate.

The journal must show what failsafe is watching and how close it is to acting:

- at startup: the detectors directory and each detector found; a warning if there are none
- every unhealthy run, with the count against the threshold; the return to healthy
- every broken detector, as an error
- every action requested, and whether it was executed, dry-run or refused by the guard

## Dependencies

`serde` + `toml` (configuration and state file), `libc` (process-group kill, `reboot`,
`sync`). All pure Rust. Nothing else without a recorded reason.

## Testing

- **Unit:** `monitor` (counter table, trip, stable) and `guard` (`required_uptime`, state
  transitions, corrupt file) with injected time.
- **Integration (`tests/`):** a temp directory of shell-script detectors run through the
  real binary with `--dry-run` / `--once`: healthy, unhealthy, exit 77, timeout with a
  lingering child, non-executable file.
- **Never in tests:** a real reboot. No test may start the binary without `--dry-run`
  except to assert that it exits at startup. Reboot execution is verified on a board.
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

### Backoff instead of a reboot cap (proposed 2026-10-05, confirmed by the user 2026-10-10)

A hard cap ("N reboots, then stop") leaves an unattended device permanently unprotected
after a transient fault. Exponential backoff with a ceiling gives both properties with
one mechanism: no rapid loop, and still one attempt per `backoff_max`. Counting a streak
against uptime, not reboots per wall-clock window, keeps it correct without an RTC.

### Single-threaded, sequential detectors (2026-10-05)

Detectors are few and cheap; concurrency would add shared state to a program whose value
is being predictable. The cost is that one slow detector delays the others by at most its
timeout. Revisit if detector count or timeouts make a full round exceed the shortest interval.

### Configuration from file only, no environment variables (2026-10-06)

Reboot policy comes from the configuration file only, with no environment variables.

- One source means no precedence rules; the file on disk is the single source of truth.
- Environment variables are invisible, inherited across processes (systemd drop-ins, wrapper
  scripts, shells), and a silent change to a reboot threshold must be avoided.
- The file can be validated and reviewed; an invalid value stops the daemon at startup.

Outside the file remain CLI flags (`--config PATH`, `--dry-run`, `--once`) and `NOTIFY_SOCKET`
which carries no policy and is required by systemd's watchdog protocol.

### Debian package built with `cargo-deb` (2026-10-11)

The first board runs Ubuntu, so installation is a `.deb` (user request 2026-10-11), which
replaces the planned `make install`.

- `cargo-deb` writes the archive itself and needs no `dpkg` on the build host, which is
  not Debian-based. It is a build tool, not a dependency of the binary, so the pure-Rust
  constraint is unaffected.
- Start on install (user decision): the Debian convention, and harmless with no detectors.
- A Yocto recipe stays a candidate for images that are not Debian-based.

### MIT license (2026-10-11)

The MIT license was chosen for failsafe (user decision 2026-10-11).

### Whole seconds, and a floor on the stable period (user decision 2026-10-13)

- **Whole seconds only.** failsafe is a backup mechanism that returns a device to a usable
  state; it does not need sub-second response. Integer seconds remove rounding, NaN and
  the zero-duration cases that decimals allowed (a zero interval ran one detector in a
  tight loop; a zero timeout made every detector unhealthy). Tests use 1 s values and
  `threshold = 1` where they need speed.
- **`stable_after_secs` is at least `boot_grace_secs + backoff_base_secs`.** That sum is the
  uptime at which a repeat reboot first becomes possible. A shorter stable period would
  forget the streak before then and every reboot would wait only for the boot grace, which
  turns the backoff off. With this rule no configuration can do that. Other values keep
  no floor: an operator may still choose an aggressive guard.

## Unknown about the first board

Known: aarch64, Ubuntu. Unknown: kernel version, systemd version, watchdog driver timeout
range and `nowayout`. The probe
in the session scratchpad reports them; it has not been run on the board. Needed before
Phase 4, not before.
