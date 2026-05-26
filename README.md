# armup

`armup` is a Windows-focused Rust CLI for bootstrapping a Cortex-M development
toolchain. It resolves the latest supported tool releases, downloads the
matching Windows zip assets, extracts them into a single install root, and can
update the user `Path` in `HKCU\Environment`.

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
- Installs selected tools in parallel
- Uses bounded multi-connection downloads for large archives when the server supports ranged requests
- Can show recent upstream versions interactively before installing
- Verifies downloaded GitHub archives when an upstream SHA-256 digest is advertised
- Downloads release archives to a temporary file for extraction
- Extracts each tool into a versioned install directory
- Removes stale staging directories from interrupted installs
- Shows a preview before updating managed `Path` entries
- Rebuilds managed `Path` entries from the install root itself
- Adds the installed executable directories to the user `Path`

## Requirements

- Windows
- Network access to `api.github.com`, GitHub release assets, and
  `developer.arm.com`
- A new terminal session after the user `Path` is updated

This project is currently Windows-only. The code depends on `winreg`, updates
the user `Path` in `HKCU\Environment`, and resolves Windows-specific release
assets.

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

Install without prompts:

```powershell
armup install --all --root D:\Embedded_Toolchain --path --yes
```

Install selected tools:

```powershell
armup install --tool ninja --tool cmake --no-path
```

Choose from recent upstream versions instead of always installing latest:

```powershell
armup install --select-versions
```

Check installed tools and managed `Path` entries:

```powershell
armup status
```

Run diagnostics:

```powershell
armup doctor
```

### Interactive flow

The current CLI is prompt-driven. During `install`, `armup` will:

1. Ask whether to install all supported tools or let you choose a subset
2. Ask for an install root, defaulting to `D:\Embedded_Toolchain`
3. Ask whether to update the user `Path`

If `--select-versions` is passed, `armup` resolves recent upstream releases and
shows a short version list for each selected tool. The first option is the
latest release. Arm GNU Toolchain currently exposes only the discovered latest
version through `armup`.

### Non-interactive behavior

If standard input or output is not attached to a terminal, `armup` currently
uses these defaults:

- Install all supported tools
- Use the default install root
- Apply user `Path` changes

You can make those choices explicit with `--all`, `--tool`, `--root`, `--path`,
`--no-path`, and `--yes`.

`--select-versions` requires an interactive terminal because versions are chosen
from a list.

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
The default is intentionally on `D:`. If the selected drive does not exist,
`armup` stops before downloading and prints a clear error.

## PATH updates

When `Path` updates are enabled, `armup` adds each installed tool's executable
directory to the user `Path`.

`armup` does not write `ARMUP_HOME` or tool-specific environment variables. If
older versions previously created `ARMUP_HOME`, `ARM_GNU_TOOLCHAIN_ROOT`,
`CLANGD_ROOT`, `CMAKE_ROOT`, `NINJA_ROOT`, `OPENOCD_ROOT`, or
`OPENOCD_SCRIPTS`, the installer removes those managed variables during the next
`Path` update.

Before writing to `HKCU\Environment`, `armup` prints the managed `Path` entries
it will remove, the entries it will add, and any legacy `ARMUP_HOME` or
tool-specific variables it will clean up.

## Proxy Behavior

`armup` uses standard proxy environment variables when present:

- `HTTPS_PROXY`
- `HTTP_PROXY`
- `ALL_PROXY`
- Lowercase variants of the same names

If no proxy environment variable is set, it probes common local proxy ports on
`127.0.0.1` and uses the first SOCKS5 or HTTP proxy it can identify.

## Checksums

For GitHub release assets, `armup` verifies the archive when the GitHub API
advertises a SHA-256 digest for the asset. For Arm GNU Toolchain, `armup`
downloads the companion `.sha256asc` file for the selected Windows zip and
verifies the archive against the SHA-256 digest in that file.

Some upstreams do not publish a digest through the API; in that case `armup`
reports that no checksum was available and continues.

## Scope

`armup` is Windows-only by design. It depends on Windows registry access,
chooses Windows release assets, and targets users who want an opinionated
Windows embedded toolchain bootstrapper.

## License

MIT. See [LICENSE](LICENSE).
