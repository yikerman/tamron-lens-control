# Repository Guidelines

## Project Structure & Module Organization

This Rust 2024 project builds the `tlc` binary and reusable
`tamron_lens_control` library.

- `src/main.rs`, `src/cli.rs`: entry point, clap commands, help, and output.
- `src/lib.rs`: public library exports.
- `src/device.rs`: Linux USB serial discovery and device selection.
- `src/protocol.rs`: private framing, CRC, timing, and memory operations.
- `src/lens.rs`: typed lens settings, validation, and writes.
- `src/firmware.rs`: firmware metadata, container validation, and update control.
- `src/snapshot.rs`: versioned `.tlc` backup files.
- `PROTOCOL.md` `PROTOCOL_UPDATEFW.md`: normative wire and memory specification.
- `TODO.md`: functionality and verification status.

Tests live beside modules in `#[cfg(test)]` blocks. There are no assets.

## Build, Test, and Development Commands

```console
cargo build                         # Build the debug library and CLI
cargo run -- devices                # Run the CLI against local hardware
cargo test --all-targets            # Run library and CLI tests
cargo fmt --all -- --check          # Check rustfmt formatting
cargo clippy --all-targets --all-features -- -D warnings
cargo doc --no-deps                 # Check public API documentation
```

Run `cargo fmt --all` before submitting changes. Inspect generated help with
`cargo run -- <command> --help`.

## Coding Style & Naming Conventions

Use rustfmt with four-space indentation. Use `snake_case` for modules,
functions, and tests; `PascalCase` for types; and `SCREAMING_SNAKE_CASE` for
protocol constants. Keep wire bytes and memory offsets inside the protocol/lens
layers rather than exposing them to clap.
Return typed `Result` errors and avoid production panics. Write CLI text for
photographers, using Focus Set Button and Custom Switch rather than protocol
vocabulary.

## Testing Guidelines

Add focused tests for parser, validation, and frame changes. Use scripted fake
transports for writes, timeouts, ordering, and failures. Do not change a real
lens merely to run routine tests. Hardware write checks must be explicitly
authorized, reversible, read back, and restored immediately. Update `TODO.md`
only after the stated verification has passed. Firmware transfers require
separate explicit authorization because they are not reversible; never initiate
one as part of an automated test or routine verification command.

## Commit & Pull Request Guidelines

No commit convention exists yet. Use short imperative subjects, optionally
scoped, such as `cli clarify focus limiter help`. Keep unrelated changes
separate.

Pull requests should explain user-visible behavior, protocol assumptions, and
verification commands. Link issues and include terminal output for CLI changes.
Call out real-hardware setting changes and restoration results.

## Safety & Scope

Treat `PROTOCOL.md` `PROTOCOL_UPDATEFW.md` as the source of truth. Preserve the
stateless lifecycle, capability checks, no-retry behavior, and explicit
disconnect.
