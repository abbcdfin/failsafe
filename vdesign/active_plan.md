# Active: Phase 2A, Ubuntu package

Read first in `docs/architecture_design.md`: "Packaging" and the decision "Debian package
built with `cargo-deb`". Paths, modes and the install, upgrade, remove and purge behaviour
are specified there and not repeated here.

**Safety:** the package must never contain a detector. Never install it on a machine you
are not prepared to have rebooted once detectors are added.

## Steps

- [x] **1. Prove the tool.** Install `cargo-deb`. Build a package from the existing
  aarch64 binary without rebuilding it and confirm the control file says
  `Architecture: arm64` (and `amd64` for x86_64) and `Depends` is empty. If `cargo-deb`
  cannot do this for the musl targets without `dpkg` on the host, stop and report; do not
  switch tools on your own.

- [x] **2. The unit.** `failsafe.service` in the source tree, as specified. No
  `WatchdogSec`, no `Type=notify` (Phase 4).

- [x] **3. Package metadata.** In `Cargo.toml`: maintainer, description, licence, section,
  the binary and unit as assets. Version comes from `Cargo.toml`.

- [x] **4. Directories.** Both directories exist after install with the specified owner
  and mode. failsafe exits if the detector directory is missing, so it must exist before
  the service starts. Do not put a placeholder file in `detectors.d`: every file there is
  treated as a detector.

- [x] **5. Maintainer scripts.** Enable and start on install, restart on upgrade, stop and
  disable on remove, purge as specified. Scripts are POSIX `sh` with `set -e` and must
  succeed where systemd is not running (a container or image build).

- [x] **6. `make deb`.** Depends on `all`; writes `dist/failsafe_<version>_<arch>.deb` for
  every entry in `TARGETS`. The target-to-Debian-architecture mapping lives in one place.
  `make clean` removes the packages.

- [x] **7. README and `vdesign/env.md`.** Installation section in the README (install,
  add a detector, read the log); correct the stale "not functional yet" status line.

## Out of scope

Config file in the package (Phase 3), watchdog settings in the unit (Phase 4), an apt
repository, package signing, a Yocto recipe.

## Verify on the host

- `make deb` produces both packages; `make test` and clippy stay clean.
- Without `dpkg`: list the archive (`ar t`, then the two tarballs) and check the control
  fields, the file list with owner root and the modes, and that no file lies under
  `detectors.d`.
- If Docker or Podman is available: in an `ubuntu` container for each architecture that
  can run, `apt install ./failsafe_*.deb`, then remove, reinstall, purge, and check the
  directories after each. Report which of these were actually run.

## Architect review (2026-10-11): fix before the board run

Host criteria met: both packages build, control fields and contents are as specified, the
amd64 lifecycle (install, upgrade, remove, reinstall, purge) passes in an Ubuntu 26.04
container, 52 tests, clippy clean. No container has systemd, so every `systemctl` line in
the scripts is still unexecuted.

- [x] **Q1. An upgrade re-enables and starts a service the operator turned off.**
  `postinst` runs `systemctl enable` on every `configure` and `systemctl restart` on
  upgrade. An operator who ran `systemctl disable --now failsafe` to stop reboots gets it
  back on the next upgrade. Required behaviour: fresh install, and reinstall after remove,
  enable and start; upgrade keeps the operator's enabled/disabled state and restarts only
  a running service. `$2` alone cannot tell these apart: it is set after a remove too
  (state `rc`). Use `cargo-deb`'s generated systemd snippets (`systemd-units` plus
  `#DEBHELPER#` in the scripts, which use `deb-systemd-helper`) instead of hand-written
  calls; confirm they give the behaviour above, otherwise report.
- [x] **Q2. Script failures are invisible.** Every `systemctl` call discards its output
  and its status, so an install whose service fails to start looks successful. A failed
  start or restart must be visible in the `apt` output.
- [x] **Q3. README.** The status line says Phases 2 and 2A are complete; both still need
  their board run. "Adding a detector" shows `chown`/`chmod` on a file it never creates.
  "systemd restarts it if it hangs" is true only from Phase 4; until then it restarts on
  exit.
- [x] **Q4. Version mismatch.** The file is named `failsafe_0.1.0_<arch>.deb`, the control
  field says `0.1.0-1`. Make them agree.

Add to "Verify on the board": after `systemctl disable --now failsafe`, installing the
package again leaves the service disabled and stopped.

## Architect re-review (2026-10-11)

Q1–Q4 verified closed. In an Ubuntu 26.04 container with `systemctl` installed but systemd
not running: fresh install enables; an upgrade keeps a disabled service disabled; remove
then reinstall restores the previous state; purge leaves nothing behind. Start, stop and
restart are still unexecuted anywhere and are covered by "Verify on the board".

- [x] **Q5. Two copies of the unit.** `failsafe.service` exists at the repository root and
  in `debian/`; only `debian/failsafe.service` is packaged. Delete the root copy.

## Verify on the board

1. `apt install ./failsafe_<version>_arm64.deb`. `systemctl status failsafe` shows it
   active and enabled; the journal shows the "no detectors found" warning.
2. Install the same package again (upgrade path): the service restarts and stays active.
3. Reboot the board: the service is running after boot.
4. `apt remove failsafe`: service gone, both directories still there. Reinstall, then
   `apt purge failsafe`: `/var/lib/failsafe` gone.

## Phase 2 board run (after the steps above; about 90 minutes, mostly unattended)

Phase 2 is not done without it. It is the first time the real reboot path executes.

1. Install the package. Put one detector that always exits 1 in
   `/etc/failsafe/detectors.d/` and restart the service.
2. Expect reboots at about 5, 15, 25 and 45 minutes of uptime (plus up to three detector
   intervals each). After each boot the journal shows the previous reason and the streak.
3. Replace the detector with one that exits 0. After 30 minutes the journal shows the
   streak cleared, and `/var/lib/failsafe/state.toml` has streak 0.
4. Record the observed uptimes in `vdesign/checkpoint.md`.
