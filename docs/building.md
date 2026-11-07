# Building the tool

The tool is written in Rust. It has no runtime dependencies: the Windows build uses only Windows'
own USB and driver APIs, and the Linux build talks to the device through `/dev/bus/usb`. The USB
code is pure Rust (`nusb`), so there is no libusb to install. The window is drawn with `egui`.

The fixed firmware (`tool/firmware/mtrack2x2-upgrade-v1-signed.bin`) is embedded into the program
at build time, so a single file is all you need to ship.

## Windows

1. Install **Visual Studio Build Tools** (free) from
   <https://visualstudio.microsoft.com/visual-cpp-build-tools/> and select the
   **"Desktop development with C++"** workload. Rust needs its linker and Windows SDK.
2. Install Rust from <https://rustup.rs> (the default `stable-x86_64-pc-windows-msvc` toolchain).
3. Build:

   ```
   cd tool
   cargo build --release
   ```

The program is `tool\target\release\mtrack-fix.exe`.

The release build carries a manifest that asks for administrator rights when started (needed for
attaching drivers and resetting USB ports). Debug builds (`cargo build` without `--release`) run as
the normal user so that `cargo test` works.

## Ubuntu / Debian

```
sudo apt install build-essential libxkbcommon-dev libwayland-dev libxcb1-dev libxrandr-dev libxi-dev libgl1-mesa-dev libudev-dev
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh     # if Rust is not installed yet
cd tool
cargo build --release
```

The program is `tool/target/release/mtrack-fix`.

The `-dev` packages are only needed at build time (window system and OpenGL headers for the GUI).
The finished binary loads X11, Wayland and OpenGL at run time and runs on either display server.

## Fedora

```
sudo dnf install gcc libxkbcommon-devel wayland-devel libxcb-devel libXrandr-devel libXi-devel mesa-libGL-devel systemd-devel
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh     # if Rust is not installed yet
cd tool
cargo build --release
```

The program is `tool/target/release/mtrack-fix`.

## Other Linux distributions

You need a C compiler, pkg-config and the development headers for: xkbcommon, Wayland, XCB, Xrandr,
Xi, OpenGL (Mesa) and libudev. Then `cargo build --release` as above.

## Checks

```
cd tool
cargo fmt --check
cargo clippy -- -D warnings
cargo test
```

The tests cover the image parser and CRC rule, the descriptor check and the bit reversal. None of
them touch hardware.

## Continuous integration

`.github/workflows/build.yml` runs exactly the steps above on `windows-latest` and `ubuntu-22.04`
for every push and pull request: format check, clippy, tests, release build. It uploads the two
binaries and the firmware image as build artifacts with a `SHA256SUMS.txt`.

Pushing a tag that starts with `v` (for example `v1.0.0`) additionally creates a GitHub release with
`mtrack-fix-windows-x64.exe`, `mtrack-fix-linux-x64` and `mtrack2x2-upgrade-v1-signed.bin`.

## Cross-compiling

Not set up. Build each target on its own OS, or let CI do it.
