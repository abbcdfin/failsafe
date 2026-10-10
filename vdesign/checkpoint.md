# Checkpoint (2026-10-10)

Phase 1 (walking skeleton, dry-run only) is complete, and all architect review findings
(R1–R6) are addressed and verified:
- Symlinks (valid and dangling) are properly discovered and handled (R1).
- Configuration is strictly from flags/file, removing environment variables; test flags
  fail loudly on invalid/zero values (R2).
- Daemon logs startup state, unhealthy runs with progress counts, and returns to healthy (R3).
- Idle loop sleeps until the next due detector without polling (R4).
- Empty action lists are rejected and actions are handled via exhaustive matching (R5).
- Clean `cargo clippy --all-targets` with zero warnings (R6).
- All 19 tests pass (8 unit, 11 integration), and `make` builds static binaries for `aarch64`
  and `x86_64` which execute cleanly under `qemu-aarch64`.

## Next Steps

1. User verdict on the backoff vs hard reboot cap proposal before Phase 2 begins.
2. Build Phase 2: real reboot behind the loop guard (persisted streak, boot grace,
   backoff, stable reset, reason logged at next startup, reboot with forced fallback).
3. Run hardware/kernel probe on the target board before Phase 4.
