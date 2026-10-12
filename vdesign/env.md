# Environment

- Rust via rustup (stable; built with 1.94.1). `make` adds the rustup targets it needs.
- GNU make.
- Optional: `qemu-user` (`qemu-aarch64`, `qemu-arm`, ...) to run foreign binaries on the host.
- `cargo-deb` (v3.8.0, installed via `cargo install cargo-deb`) for `make deb`. The build host has no `dpkg`.
- First board: aarch64, Ubuntu, exposes `/dev/watchdog`, systemd.
