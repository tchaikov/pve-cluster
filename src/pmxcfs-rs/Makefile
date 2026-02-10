.PHONY: all test lint clippy fmt check build clean help

# Default target
all: check build

# Run all tests
test:
	cargo test --workspace

# Lint with clippy (using proxmox-backup style: only fail on correctness issues)
clippy:
	cargo clippy --workspace -- -A clippy::all -D clippy::correctness

# Check code formatting
fmt:
	cargo fmt --all --check

# Full quality check (format + lint + test)
check: fmt clippy test

# Build release version
build:
	cargo build --workspace --release

# Clean build artifacts
clean:
	cargo clean

# Show available targets
help:
	@echo "Available targets:"
	@echo "  all      - Run check and build (default)"
	@echo "  test     - Run all tests"
	@echo "  clippy   - Run clippy linter"
	@echo "  fmt      - Check code formatting"
	@echo "  check    - Run fmt + clippy + test"
	@echo "  build    - Build release version"
	@echo "  clean    - Clean build artifacts"
	@echo "  help     - Show this help message"
