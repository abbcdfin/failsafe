# Active: board verification of Phases 2 and 2A

Nothing is being built. Phases 2 and 2A are finished on the host and stay open until the
two procedures below have been run on a board. Phase 4 is next and has no board yet.

Use the package for the board's architecture: `arm64`, `armhf` or `amd64`.

**Ownership.** Since Phase 3, failsafe as root runs a detector only if the file and
`/etc/failsafe/detectors.d` are owned by root and not writable by group or others.
A detector copied in as another user is reported as broken in the journal and never
counted, so the reboot run would do nothing. After copying one in:
`chown root:root FILE && chmod 0755 FILE`.

**To stop it at any time:** `systemctl disable --now failsafe`, or delete the detector.

## 1. Phase 2A: package (about 10 minutes)

The first time the service start, stop and restart lines of the install scripts run.

1. `apt install ./failsafe_<version>_<arch>.deb`. `systemctl status failsafe` shows it
   active and enabled; the journal shows "using configuration from
   /etc/failsafe/failsafe.toml" and the "no detectors found" warning.
2. Install the same package again (upgrade path): the service restarts and stays active.
3. Reboot the board: the service is running after boot.
4. `systemctl disable --now failsafe`, then install the package again: the service stays
   disabled and stopped. Re-enable it: `systemctl enable --now failsafe`.
5. `apt remove failsafe`: service gone; `/etc/failsafe` and `/var/lib/failsafe` still
   there. Reinstall, then `apt purge failsafe`: `/var/lib/failsafe` and the config file
   gone.

## 2. Phase 2: reboot run (about 35 minutes, mostly unattended)

The first time the real reboot path executes. The guard values are shortened through the
config file so the run takes 35 minutes; the formula and the code are the same as with the
defaults.

1. Install the package. Replace `/etc/failsafe/failsafe.toml` with:

   ```toml
   [reboot]
   boot_grace_secs   = 120
   backoff_base_secs = 60
   backoff_max_secs  = 480
   stable_after_secs = 180

   [defaults]
   interval_secs = 10
   ```

2. Put one detector that always exits 1 in `/etc/failsafe/detectors.d/` (root-owned,
   0755). Check with `failsafe --once`: it prints `unhealthy (exit 1)`. Restart the
   service.
3. Expect reboots at about 2, 3, 4, 6 and 10 minutes of uptime, each up to 30 seconds
   later (three detector intervals). After each boot the journal shows the previous
   reason and the streak. Between reboots the journal shows the refusals with the seconds
   remaining.
4. Replace the detector with one that exits 0. After 3 minutes the journal shows the
   streak cleared, and `/var/lib/failsafe/state.toml` has streak 0.
5. Restore the shipped config file (or purge and reinstall), and record the observed
   uptimes in `vdesign/checkpoint.md`.

To run with the default values instead, skip step 1: the reboots then come at about 5, 15,
25 and 45 minutes, and the streak clears after 30 minutes (about 90 minutes in total).

## Before Phase 4

Phase 4 needs facts about the board that are still unknown (kernel and systemd versions,
watchdog driver timeout range, `nowayout`); see "Unknown about the first board" in the
architecture document. Collect them during the board session above.
