# Environment

- Rust via rustup (stable; built with 1.94.1). `make` adds the rustup targets it needs.
- GNU make.
- Optional: `qemu-user` (`qemu-aarch64`, `qemu-arm`, ...) to run foreign binaries on the host.
- First board: aarch64, exposes `/dev/watchdog`, systemd.
