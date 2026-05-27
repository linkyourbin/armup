# armup

`armup` is a Windows-focused CLI for setting up an embedded Cortex-M toolchain.
It downloads and installs the supported tools into one root directory, then can
add them to the current user's `Path`.

It installs:

- Arm GNU Toolchain (`arm-none-eabi-gcc`)
- `clangd`
- `cmake`
- `ninja`
- `probe-rs`
- xPack OpenOCD

Ready to go:

```bash
armup install --all --root D:\Embedded_Toolchain --add-path --yes --download-connections 24
```

Update installed tools:

```bash
armup update --all --root D:\Embedded_Toolchain --add-path --download-connections 24
```
