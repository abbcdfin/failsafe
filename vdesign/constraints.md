# Constraints

- Dependencies must be pure Rust (no crate that compiles C). Rationale in
  `docs/architecture_design.md`.
- The binary must be fully static and run as a single file with no runtime besides the kernel.
- Detectors run as root: the detector directory and files must be root-owned and not
  writable by others, and failsafe refuses to run them otherwise.
