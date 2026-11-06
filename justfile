# Task runner for volee. Every recipe is a plain command: `just` is optional.
# Install just: https://github.com/casey/just

# List the recipes
default:
    @just --list

# Run what CI runs: formatting, lints, tests
check: fmt-check clippy test

# Format the code
fmt:
    cargo fmt --all

# Check formatting without changing files
fmt-check:
    cargo fmt --all --check

# Lint every crate and target, warnings are errors
clippy:
    cargo clippy --workspace --all-targets --all-features --locked -- -D warnings

# Run the test suite (unit, integration and doc tests)
test:
    cargo nextest run --workspace --all-features --locked
    cargo test --doc --workspace --locked

# Review pending insta snapshots
snapshots:
    cargo insta review

# Check licenses, advisories and duplicate dependencies
deny:
    cargo deny check

# Spell-check the repository
typos:
    typos

# Build the documentation site into docs/book
docs:
    mdbook build docs

# Serve the documentation site with live reload
docs-serve:
    mdbook serve docs --open

# Check the minimum supported Rust version
msrv:
    cargo +1.89 check --workspace --all-features --locked

# Regenerate CHANGELOG.md from the commit history (see RELEASING.md)
changelog:
    git cliff -o CHANGELOG.md

# Show what the release workflow would build for the current version
release-plan:
    dist plan
