# hosts

meow runs on the machine that builds, not on the phone. one source tree, no c compiler needed to build it (blake3 is the pure rust one).

## what was checked

- linux: built, `cargo test` passes, `meow build` run on host projects and on a real ios project
- `x86_64-pc-windows-gnu`, `aarch64-apple-darwin`, `x86_64-apple-darwin`, `aarch64-linux-android`: `cargo check` and `cargo check --tests` with an official rust release, no errors and no warnings. nothing was linked and nothing was run on those systems

so for these four the code type-checks against the real standard library of each system. no one has run meow on windows, macos or android yet.

check it again:

```bash
rustc --version                       # official release, not a distro build
cargo check --tests --target x86_64-pc-windows-gnu
cargo check --tests --target aarch64-apple-darwin
cargo check --tests --target aarch64-linux-android
```

a distro rustc that carries its own version string refuses the official `rust-std` of other targets (`crate core compiled by an incompatible version of rustc`). use a release from static.rust-lang.org for this.

## linux

every command works. the 16 KiB page note for armv6 is in [toolchain](toolchain.md).

## macos

builds and should run the same as linux.

- the toolchain directory is looked up under `toolchain/darwin/iphone/bin`
- deb and ipa are written by meow itself, no host tar or gzip, so the bytes equal the linux ones

## android

builds in termux with `cargo build --release`, the target os is `android` and it takes the unix paths. the terminal width uses the linux ioctl number, which android shares.

- termux has no `/bin/sh`, so the unit tests that run it as `/bin/sh` cannot pass there. meow itself runs commands by argument vector and never through a shell
- the iOS toolchain has to exist on the device, meow does not bring one

## windows

builds with the gnu target. compile, link, `lipo`, steps, host tests, `verify`, pictures, the object cache and `plan` use no unix file modes and type-check for the target.

what differs:

- deb and ipa are packed from a tree kept in memory, the modes come from the package spec and not from the filesystem. the data tar is byte equal to `tar --format=gnu --sort=name` on linux (checked with a tree of links, long names and `a`, `a-b`, `a.d`)
- a file taken from a `tree` has no mode bits to read, so a mach-o file or a `#!` script is packed as 755 and everything else as 644. set `mode` on the `file` or `tree` entry to override it
- symlinks inside a tree need developer mode or admin rights to exist on windows, a link read back with backslashes is stored with slashes
- `write` applies no mode, there is no execute bit to set
- `sdk-thin` copies the target of a file symlink instead of making a link, a link to a directory is an error
- tools are found without their `.exe` suffix, `Command` adds it
- the cache directory is `%LOCALAPPDATA%\meow` when `MEOW_CACHE` and `XDG_CACHE_HOME` are not set
- the status line needs the console to accept escape sequences. meow turns that mode on, and falls back to plain lines when the console refuses
- host tools that meow calls must be on PATH: `gzip` (pictures), `curl`, `tar`, `unzip`, `patch`, `git` (dependencies), `sh` for `sh_check`. Git for Windows brings most of them
- `install` needs `ssh` and `scp`, and `sshpass` for `--password-env`, which has no windows build. use a key
