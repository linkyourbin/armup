# armup

`armup` is a Windows-focused Rust CLI for bootstrapping a Cortex-M development
toolchain. It resolves the latest supported tool releases, downloads the
matching Windows zip assets, extracts them into a single install root, and can
update `HKCU\Environment` plus the user `Path`.

It is intended to be usable both as a local project binary and as an installed
crate from crates.io.

## Supported tools

- `arm-none-eabi-gcc` via the Arm GNU Toolchain
- `clangd`
- `cmake`
- `ninja`
- `xpack-openocd`

## What it does

- Resolves the latest tool versions in parallel from GitHub releases and Arm download pages
- Prioritizes large downloads so they do not compete with smaller archives on the same link
- Downloads smaller archives in parallel
- Uses bounded multi-connection downloads for large archives when the server supports ranged requests
- Downloads release archives to a temporary file for extraction
- Extracts each tool into a versioned install directory
- Rebuilds environment settings from the install root itself
- Sets per-user environment variables for the install root and tool roots
- Adds the installed executable directories to the user `Path`

## Requirements

- Windows
- Network access to `api.github.com`, GitHub release assets, and
  `developer.arm.com`
- A new terminal session after environment variables are updated

This project is currently Windows-only. The code depends on `winreg`, updates
`HKCU\Environment`, and resolves Windows-specific release assets.

## Install

Install from crates.io:

```powershell
cargo install armup
```

Then run:

```powershell
armup install
```

## Build From Source

```powershell
cargo build --release
```

The compiled binary will be available at:

```text
target\release\armup.exe
```

## Usage

Run the installer:

```powershell
cargo run -- install
```

Or run the built binary directly:

```powershell
.\target\release\armup.exe install
```

### Interactive flow

The current CLI is prompt-driven. During `install`, `armup` will:

1. Ask whether to install all supported tools or let you choose a subset
2. Ask for an install root, defaulting to `D:\Embedded_Toolchain`
3. Ask whether to update `HKCU\Environment` and the user `Path`

### Non-interactive behavior

If standard input or output is not attached to a terminal, `armup` currently
uses these defaults:

- Install all supported tools
- Use the default install root
- Apply environment changes for the current user

## Install layout

By default, files are placed under:

```text
D:\Embedded_Toolchain
```

Layout:

```text
D:\Embedded_Toolchain/
  arm-none-eabi-gcc/<version>/
  clangd/<version>/
  cmake/<version>/
  ninja/<version>/
  xpack-openocd/<version>/
```

The chosen install root only contains installed tools. `armup` does not keep a
persistent download cache, log directory, or install database.

If you prefer a different location, change the install root when prompted.

## Environment variables

When environment updates are enabled, `armup` writes:

- `ARMUP_HOME`
- `ARM_GNU_TOOLCHAIN_ROOT`
- `CLANGD_ROOT`
- `CMAKE_ROOT`
- `NINJA_ROOT`
- `OPENOCD_ROOT`
- `OPENOCD_SCRIPTS`

It also adds each installed tool's executable directory to the user `Path`.

## License

MIT. See [LICENSE](LICENSE).
