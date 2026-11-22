# migrate

`meow migrate` reads a theos project and writes the `meow.toml` that builds the same thing. nothing is run: the Makefile is read, not executed.

```bash
meow migrate                      # print to standard output, notes to standard error
meow migrate ~/dev/mytweak --write
meow migrate --set THEOS_PACKAGE_SCHEME=rootless --set FINALPACKAGE=1
```

`--write` writes `DIR/meow.toml` and keeps an existing one unless `--force` is given. `--set VAR=VALUE` gives the variables that `ifeq` and `ifdef` blocks read from the environment, they count as unset otherwise.

## what is read

- `Makefile`: `TARGET` (the sdk and the deployment target), `ARCHS`, `THEOS_PACKAGE_SCHEME`, `TWEAK_NAME`, `APPLICATION_NAME`, `TOOL_NAME`, `BUNDLE_NAME`, `FRAMEWORK_NAME`, `LIBRARY_NAME`, and for each of them `_FILES`, `_FRAMEWORKS`, `_PRIVATE_FRAMEWORKS`, `_LIBRARIES`, `_CFLAGS`, `_OBJCFLAGS`, `_CCFLAGS`, `_LDFLAGS`, `_CODESIGN_FLAGS`, `_INSTALL_PATH`, `_BUNDLE_EXTENSION`, `_LOGOS_DEFAULT_GENERATOR`, `_ARCHS`, `_SWIFTFLAGS`, `_SWIFT_BRIDGING_HEADER`. `ADDITIONAL_CFLAGS`, `ADDITIONAL_OBJCFLAGS` and `ADDITIONAL_LDFLAGS` count for every product
- `SUBPROJECTS`: each subproject is read from its own directory and becomes a `[product.NAME]`
- `control`: the `[package]` table, `Depends` as written, other fields into `fields`. `Architecture: iphoneos-arm64` makes the scheme rootless
- `NAME.plist`: the filter of a tweak, in any plist format
- `layout/`: copied into the package. `layout/DEBIAN/preinst`, `postinst`, `prerm` and `postrm` become the package scripts
- `Info.plist` or `Resources/Info.plist`, `Resources/`, `entitlements.plist`

make is read as far as theos makefiles need it: `=`, `:=`, `?=`, `+=`, line continuations, `include`, `ifeq`, `ifneq`, `ifdef`, `ifndef`, `else`, and `wildcard`, `filter`, `filter-out`, `addprefix`, `addsuffix`, `patsubst`, `subst`, `strip`, `sort`, `notdir`, `dir`, `abspath` and `$(shell find DIR -name PATTERN -type f|d)`. any other `$(shell ...)` is not run, its value is empty and a note says so.

## what changes

- `-fobjc-arc` and `-fno-objc-arc` in the flags become `arc`. `-lname` and `-framework name` in `LDFLAGS` become `libs` and `frameworks`. flags that name a directory, `-I`, `-F` and `-L`, are made relative to the project
- `TARGET = iphone:clang:9.3:6.0` becomes `sdk = "9.3"` and `min_ios = "6.0"`. `meow sdk 9.3` installs that sdk
- `_PRIVATE_FRAMEWORKS` become frameworks plus `-F${SDK}/System/Library/PrivateFrameworks`
- tweaks and bundles get `-I${MEOW_HEADERS}` and `-F${MEOW_VENDOR_LIB}`, theos added them without being asked. `-lsubstrate` is dropped when a `.x` file is built, the code logos writes links CydiaSubstrate by itself
- `.x` and `.xm` files in the file list stay as they are, meow runs logos on them, see [logos](logos.md)
- `_INSTALL_PATH` is a directory in theos. for a tool or library meow wants the file, so the name is added

## what is reported and not carried over

every one of these is a line `note: ...` on standard error:

- rules (`after-stage::`, `before-package::`, `after-install::` and your own), write a `[step]` or a script for them
- `INSTALL_TARGET_PROCESSES`, codesign flags other than `-S`, `_EXTRA_FRAMEWORKS`, filter keys other than `Bundles` and `Executables`
- swift files below ios 12.2 or on armv7 (the project is carried over and meow refuses it with the reason), static libraries, products with `_INSTALL = 0`, `roothide`, `NULL_NAME`, `AGGREGATE_NAME`, `XPC_SERVICE_NAME` and the other instance types meow has no kind for
- a `$(shell ...)`, a function that is not supported, an include that was not found, and the result of every `ifeq` that was evaluated
- if the written file does not load or does not expand against the files in the directory, the reason is a note as well

a Makefile that gives no product, such as a static library or a project that only copies prebuilt files, is an error and nothing is written.
