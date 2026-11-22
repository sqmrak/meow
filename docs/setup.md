# setup

`meow setup` installs what meow builds with, in one place meow looks without any environment variable.

```bash
meow setup                       # the components and what is installed
meow setup all                   # everything that can be installed here
meow setup toolchain headers     # some of them
meow setup sdks --sdk 9.3        # other sdks than the default set
meow setup swift --xcode ~/Downloads/Xcode_26.xip
meow doctor                      # what meow found
```

## where

the tools directory is `--to DIR`, else `$MEOW_TOOLS`, else `$XDG_DATA_HOME/meow/tools` or `~/.local/share/meow/tools`. meow uses `$MEOW_TOOLS` when it is set and that default directory when it exists, so after `meow setup` there is nothing to export. with `--to` or another `MEOW_TOOLS` the command says what to set. downloads are cached in the meow cache by hash.

```
theos/toolchain/linux/iphone/   clang, ldid, lipo, strip, ld
theos/vendor/include            substrate.h, libhooker, private framework headers
theos/vendor/lib                stub libraries and frameworks of the common tweak libraries
theos/sdks/iPhoneOS*.sdk        the ios sdks
sdks-armv7/                     iPhoneOS6.1.sdk cut to one slice per file, for the armv7 link
rust-official/                  rustc, cargo, std for aarch64-apple-ios, rust-src
go/ and go-1.14/                go for arm64 and go for armv7
swift-sdk/ and xtool/           the ios sdk and swift resources made from an Xcode
```

## components

| component | what | needs |
|---|---|---|
| toolchain | the clang 11.1.0, ldid, lipo, strip and ld of the theos toolchain, release `test-210562a` of L1ghtmann/llvm-project, the one meow is checked against | linux, aarch64 or x86_64 |
| headers | [theos/headers](https://github.com/theos/headers) and [theos/lib](https://github.com/theos/lib) at pinned commits. `${MEOW_HEADERS}` and `${MEOW_VENDOR_LIB}` in a project name them | git |
| sdks | ios sdks 5.1, 6.1 and 16.5: armv6 takes 5.1, armv7 6.1, arm64 the newest. the sdks of the theos repository between 7 and 10.3 are stubs without the armv7 system libraries, so 6.1 is the one armv7 links against. old sdks lay the slices of a fat library out on 4 KiB boundaries and ld64 cannot map them on a host with 16 KiB pages, so `sdks` also writes `sdks-armv7`, a copy of 6.1 with one slice per file, which meow prefers for armv7. `--sdk V` replaces the set, see [sdk](sdk.md) for all versions | |
| rust | rustc, cargo and the std of 1.99.1 for the host, the std for aarch64-apple-ios and rust-src, see [rust](rust.md) | sh |
| go | go 1.27.3 for arm64 and go 1.14.15 for armv7, see [go](go.md) | |
| swift | the ios sdk and swift resources that xtool builds from an Xcode, see [swift](swift.md) | linux, `--xcode Xcode.xip` or `Xcode.app`, from Apple |

- every download is checked against the sha256 in `sdk/setup.toml`, every git checkout against its commit. a file that does not match is an error and nothing is installed
- a component that is installed is left alone, `--force` installs it again. `all` skips what this host cannot do (swift without `--xcode`, the toolchain and swift on macos) and says so
- on macos the toolchain is the Xcode clang and `ldid` from `brew install ldid`, meow installs neither. rust and go are installed for macos, go 1.14 has no apple silicon build and the intel one runs under rosetta
- the host `swiftc` is not installed, install a swift toolchain from swift.org or your package manager. the swift component links its clang headers into the sdk, so install the swift toolchain first or run `meow setup swift` again after it
- a component that fails does not stop the others, the command lists what failed and exits 1

`meow setup` writes only inside the tools directory and the cache.
