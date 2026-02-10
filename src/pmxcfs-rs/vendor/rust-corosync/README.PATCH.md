# Temporary Vendored rust-corosync v0.1.0

This is a temporary vendored copy of `rust-corosync` v0.1.0 with a critical bug fix.

## Why Vendored?

The published `rust-corosync` v0.1.0 on crates.io has a bug that prevents Rust and C applications from joining the same CPG groups. This bug has been fixed in corosync upstream but not yet released.

## Upstream Fix

The fix has been committed to the corosync repository:
- Repository: https://github.com/corosync/corosync
- Local commit: `~/dev/corosync` commit 71d6d93c
- File: `bindings/rust/src/cpg.rs`
- Lines changed: 209-220

## The Bug

CPG group name length calculation was excluding the null terminator:
- C code: `length = strlen(name) + 1` (includes \0)
- Rust (before): `length = name.len()` (excludes \0)
- Rust (after): `length = name.len() + 1` (includes \0)

This caused Rust and C nodes to be isolated in separate CPG groups even when using identical group names.

## Removal Plan

Once `rust-corosync` v0.1.1+ is published with this fix:

1. Remove this `vendor/rust-corosync` directory
2. Remove the `[patch.crates-io]` section from `../Cargo.toml`
3. Update workspace dependency to `rust-corosync = "0.1.1"`

## Testing

The fix has been tested with mixed C/Rust pmxcfs clusters and verified that all nodes successfully join the same CPG group and communicate properly.
