## Installing & building

### System requirements

| Requirement                 | Details                                            |
| --------------------------- | -------------------------------------------------- |
| Operating systems           | macOS 12+, Ubuntu 20.04+/Debian 10+, or Windows 11 |
| Git (optional, recommended) | 2.23+ for built-in PR helpers                      |
| RAM                         | 4-GB minimum (8-GB recommended)                    |

### Script-managed CLI install

Install the latest `mcodex` CLI with the OSS installer:

```bash
# macOS/Linux
curl -fsSL https://downloads.mcodex.sota.wiki/install.sh | sh
```

```powershell
# Windows PowerShell
$installer = Join-Path $env:TEMP "mcodex-install.ps1"
iwr -UseBasicParsing https://downloads.mcodex.sota.wiki/install.ps1 -OutFile $installer
& $installer
```

Install an explicit version by passing it to the installer:

```bash
curl -fsSL https://downloads.mcodex.sota.wiki/install.sh | sh -s -- 0.96.0
```

```powershell
$installer = Join-Path $env:TEMP "mcodex-install.ps1"
iwr -UseBasicParsing https://downloads.mcodex.sota.wiki/install.ps1 -OutFile $installer
& $installer 0.96.0
```

Update by rerunning the same installer. Without a version argument it resolves the current stable version; with a version argument it switches the managed install to that version.

The default install root is `~/.mcodex` on macOS/Linux and `%LOCALAPPDATA%\Mcodex` on Windows. The installer stores versioned payloads in `<root>/install/<version>`, points `<root>/current` at the active version, writes metadata to `<root>/install.json`, and expects the native binaries under `<root>/current/bin`.

The default wrapper path is `~/.local/bin/mcodex` on macOS/Linux and `%LOCALAPPDATA%\Programs\Mcodex\bin\mcodex.ps1` on Windows. The wrapper adds `<root>/current/bin` to `PATH` for the launched process and exports:

- `MCODEX_INSTALL_MANAGED=1`
- `MCODEX_INSTALL_METHOD=script`
- `MCODEX_INSTALL_ROOT=<root>`

`MCODEX_INSTALL_ROOT` can also be set before running the installer to choose a different install root.

Package-manager channels such as npm, Bun, Homebrew, and WinGet are not advertised update paths for `mcodex`. Existing users who installed the CLI from npm should reinstall with the OSS installer to join the supported update channel.

DotSlash is no longer advertised for the `mcodex` CLI because lightweight GitHub Releases no longer carry native CLI assets. GitHub Releases remain release records; use the OSS installer and the `downloads.mcodex.sota.wiki` release repository as the primary binary delivery path.

### Build from source

```bash
# Clone the repository and navigate to the root of the Cargo workspace.
git clone https://github.com/vivym/mcodex.git
cd mcodex/codex-rs

# Install the Rust toolchain, if necessary.
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
source "$HOME/.cargo/env"
rustup component add rustfmt
rustup component add clippy
# Install helper tools used by the workspace justfile:
cargo install just
# Optional: install nextest for the `just test` helper
cargo install --locked cargo-nextest

# Build mcodex.
cargo build

# Launch the TUI with a sample prompt.
cargo run --bin mcodex -- "explain this codebase to me"

# After making changes, use the root justfile helpers (they default to codex-rs):
just fmt
just fix -p <crate-you-touched>

# Run the relevant tests (project-specific is fastest), for example:
cargo test -p codex-tui
# If you have cargo-nextest installed, `just test` runs the test suite via nextest:
just test
# Avoid `--all-features` for routine local runs because it increases build
# time and `target/` disk usage by compiling additional feature combinations.
# If you specifically want full feature coverage, use:
cargo test --all-features
```

### Home directory and migration

`mcodex` uses `MCODEX_HOME` as the active home override and defaults to
`~/.mcodex`.

On first launch, `mcodex` can import supported config and auth state from
legacy `CODEX_HOME` or `~/.codex` when present. That import is one-time; the
legacy home is not used as a live fallback after migration.

Account-pool startup selection remains installation-local state. After import,
it is re-established in the active `mcodex` home the next time you select or
join a pool locally.

## Tracing / verbose logging

`mcodex` is written in Rust, so it honors the `RUST_LOG` environment variable to configure its logging behavior.

The TUI defaults to `RUST_LOG=codex_core=info,codex_tui=info,codex_rmcp_client=info` and log messages are written to `~/.mcodex/log/codex-tui.log` by default. For a single run, you can override the log directory with `-c log_dir=...` (for example, `-c log_dir=./.mcodex-log`).

```bash
tail -F ~/.mcodex/log/codex-tui.log
```

By comparison, the non-interactive mode (`mcodex exec`) defaults to `RUST_LOG=error`, but messages are printed inline, so there is no need to monitor a separate file.

See the Rust documentation on [`RUST_LOG`](https://docs.rs/env_logger/latest/env_logger/#enabling-logging) for more information on the configuration options.
