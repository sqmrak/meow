# toolchain

## slices

```toml
[slice.armv7]
cc = "${TC}/clang"
ar = "${TC}/ar"
flags = ["-arch", "armv7", "-isysroot", "${SDK}"]
[slice.armv7.vars]
SDK = "${MEOW_SDK_ARMV7}"
```

a slice is one cpu: the compiler, the archiver and the flags that compile and link share (triple, arch, deployment target, sysroot). `ar` is needed only by archive targets. a target lists the slices it builds, a slice has no meaning until a target uses it.

## lookup

`meow doctor` lists what meow found and where, and exits 1 when a required tool is missing. every found path is a variable in `meow.toml`: `MEOW_TC`, `MEOW_CLANG`, `MEOW_LDID`, `MEOW_LIPO`, `MEOW_AR`, `MEOW_STRIP`, `MEOW_NM`, `MEOW_SDK_ARMV6`, `MEOW_SDK_ARMV7`, `MEOW_SDK_ARM64`, `MEOW_CRT_ARMV7` and `MEOW_CACHE_DIR`. an environment variable of the same name wins.

the toolchain is looked up in this order:

- `MEOW_TC`
- `$THEOS/toolchain/<os>/iphone/bin`
- `$MEOW_TOOLS/theos/toolchain/...`
- a PATH directory that holds clang and ldid together

sdks come from `MEOW_SDK_*`, then `$MEOW_TOOLS/sdks-armv7`, `$MEOW_TOOLS/sdks`, `$THEOS/sdks`. armv7 takes the newest sdk up to 10.3, armv6 the newest up to 5.1, arm64 the newest. an sdk without `usr/include/stdio.h`, such as a dangling symlink, is skipped.

the cache is `MEOW_CACHE`, `$XDG_CACHE_HOME/meow` or `~/.cache/meow`.

host tests use `${MEOW_HOST_CC}` and `${MEOW_HOST_AR}`. they are searched on PATH with the cross toolchain directory skipped, so a theos bin directory first on PATH does not shadow the system assembler.

armv7 executables link without the default start files, the crt file comes from `MEOW_CRT_ARMV7`.

## ios versions

`meow ios` prints which slices run on which release, `meow ios 4.2` one release.

- armv6: 2.0 to 4.2.1
- armv7: 3.0 to 10.3.3 (ios 11 dropped 32-bit apps)
- arm64: 7.0 and later
- arm64e: 12.0 and later

a project is refused when `min_ios` leaves a slice no device, or when arc is on below ios 4.0. each slice gets the project minimum raised to the first release of its cpu (arm64 7.0, arm64e 12.0).

## armv6

armv6 needs an sdk up to 5.1, later sdks dropped the slice. old sdks lay the slices of a fat library out on 4 KiB boundaries and ld64 maps a slice at its offset, so on a host with 16 KiB pages the link fails with `can't re-map file, errno=22`.

the `sdk-armv6` step copies the sdk with every fat file cut to its armv6 slice (hard links for the rest) into `$MEOW_CACHE_DIR/sdk-armv6`. run it by hand with:

```bash
meow sdk-thin --arch armv6 --from SDK --to DIR
```

the step reruns when the sdk path changes, not when the sdk contents change.
