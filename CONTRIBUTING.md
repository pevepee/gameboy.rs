# Contributing

Thanks for looking. This project has one unusual rule and a handful of ordinary
ones.

## The unusual rule

**A change that affects emulation behaviour needs a test that fails without it.**

Emulator bugs are invisible in review. A wrong half-carry flag looks exactly like
a right one, and the symptom shows up three thousand instructions later in a game
nobody has loaded. So:

- Instruction semantics: add a unit test in `crates/gb-core/src/cpu/mod.rs`.
- Mapper behaviour: add a unit test in `crates/gb-core/src/cartridge/mod.rs`.
- Anything broad: run the Blargg suite and say in the PR which ROMs changed.

If you cannot write a failing test, say so in the PR and explain what you did
instead. That is fine and much better than a silent change.

## Getting set up

```sh
rustup toolchain install stable --component rustfmt clippy
rustup target add wasm32-unknown-unknown

cargo test                    # no ROMs needed
bash scripts/fetch-test-roms.sh
cargo test --release -p gb-core --test blargg -- --nocapture
```

On Windows use `pwsh -File scripts/fetch-test-roms.ps1`.

There is a devcontainer at `.devcontainer/devcontainer.json` that does all of
this, including fetching the ROMs.

## Before opening a PR

```sh
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --release
cargo doc --workspace --no-deps
```

CI runs all four on Linux, macOS and Windows, plus the Blargg suite and a
`wasm32-unknown-unknown` build. Run what is relevant to your change rather than
all of it — but do not push a PR you have not compiled.

## Style

- `snake_case` functions and variables, `PascalCase` types.
- Comments explain **why**, not what. A comment restating the code is noise.
  A comment naming the hardware quirk that forced an odd-looking branch is the
  most valuable line in the file.
- Public items need a doc comment. If a function can panic or return an error,
  the doc comment says when.
- Prefer a named constant over a magic number. `0xFF40` in a match arm is fine;
  `if value & 0x80 != 0` is not — that is `LCDC_ENABLE`.
- Errors as values, not panics. The one place a panic is correct is an
  unimplemented opcode, because on hardware that is a fault.

## Adding a test ROM

Do not commit ROM binaries. If a new suite is worth running, add it to
`scripts/fetch-test-roms.sh` and to the table in `docs/COMPATIBILITY.md`.

## Commits

Conventional-commit prefixes: `feat`, `fix`, `perf`, `refactor`, `test`, `docs`,
`chore`. One logical change per commit. The body says what was wrong, not what
the diff does.
