# failsafe

A watchdog for unattended Linux devices. It runs a set of small health checks, and when
one keeps failing it resets the system to a proper state, without a person on site.

> **Status: Phase 2 and 2A built, awaiting board verification.** Failsafe runs detectors,
> counts failures, and executes bounded reboots guarded against reboot loops. Built as
> static musl binaries and packaged as `.deb` packages for Ubuntu. Progress is tracked
> in [vdesign/plan.md](vdesign/plan.md).

## Installation

Prebuilt Debian packages are produced by `make deb` for `arm64`, `armhf`, and `amd64`:

```sh
sudo apt install ./dist/failsafe_<version>-<revision>_<arch>.deb
```

Installing the package automatically enables and starts `failsafe.service`. The package
ships with an empty detector directory, so failsafe starts safely without rebooting.

### Adding a detector

Drop executable checks into `/etc/failsafe/detectors.d/`:

```sh
cat <<'EOF' | sudo tee /etc/failsafe/detectors.d/root-writable
#!/bin/sh
touch /var/lib/.failsafe-probe 2>/dev/null
EOF
sudo chown root:root /etc/failsafe/detectors.d/root-writable
sudo chmod 0755 /etc/failsafe/detectors.d/root-writable
```

Test detectors manually without running the daemon:

```sh
failsafe --once
```

### Viewing logs

```sh
journalctl -u failsafe -f
```

## Why

A device in the field can end up in a state it cannot leave by itself: a modem that
stopped responding, a service that hangs without crashing, a full disk. Each of these
needs its own test, but the response is the same handful of actions. failsafe separates
the two:

- **Detectors** decide what "unhealthy" means. A detector is any executable, a shell
  script or a binary, that tests one condition and reports it through its exit status.
  Adding a check means dropping a file in a directory.
- **failsafe** decides what to do about it: how often to run each detector, how many
  failures in a row count, and which action to take. The first action is a reboot; the
  design leaves room for milder ones (repair script, service restart) ahead of it.

## How it behaves

1. Runs every detector on its interval, each under a hard timeout.
2. Counts consecutive failures per detector. One bad result never triggers anything.
3. When a detector reaches its threshold, takes that detector's action.
4. Protects against reboot loops. A reboot cannot fix a carrier outage or a bad config,
   so failsafe waits a grace period after boot, and each reboot that did not restore
   health doubles the wait before the next, up to one attempt per day. Sustained health
   resets it.
5. Records why it rebooted, and reports it after the next boot.

failsafe is itself watched: systemd restarts it if it exits, and the hardware watchdog
resets the board if systemd hangs (systemd watchdog pinging during hangs arrives in Phase 4).

## Writing a detector

```sh
#!/bin/sh
# /etc/failsafe/detectors.d/root-writable
touch /var/lib/.failsafe-probe 2>/dev/null
```

| Exit status | Meaning |
|---|---|
| 0 | healthy |
| 77 | cannot tell; counted as neither |
| anything else, a timeout, or death by signal | unhealthy |

A detector only reports a condition; it never reboots anything. The full contract is in
[docs/product_specification.md](docs/product_specification.md).

## Build

Requires Rust (via rustup) and GNU make. No C cross-toolchain is needed for any target.

```sh
make          # static binaries for every architecture in the Makefile's TARGETS
make deb      # Debian packages (.deb) in dist/
make test
```

Binaries land in `dist/failsafe-<target>`, each a single static file with no runtime
dependencies. The default targets are `aarch64-unknown-linux-musl`, `armv7-unknown-linux-musleabihf`
(`armhf`), and `x86_64-unknown-linux-musl`. To build for another architecture, add its
triple to `TARGETS` in the `Makefile`, or override once:

```sh
make TARGETS=riscv64gc-unknown-linux-musl
```

Any musl target that Rust ships a standard library for works
(`rustup target list | grep musl`).

## Documentation

| File | Contents |
|---|---|
| [docs/product_specification.md](docs/product_specification.md) | What failsafe does and why: concepts, detector contract, safety requirements |
| [docs/architecture_design.md](docs/architecture_design.md) | How: modules, reboot-loop guard, configuration format, decisions and their reasons |
| [vdesign/plan.md](vdesign/plan.md) | Roadmap and current phase |
