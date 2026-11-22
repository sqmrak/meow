# swift

`.swift` files in a product are compiled by `swiftc` and linked into it, next to c, objective-c and logos sources. one `swiftc` run per slice builds the whole module into one object and an objective-c header, a step `swift-NAME-ARCH` does it.

```toml
[project]
name = "mytool"
kind = "tool"
archs = ["arm64"]
min_ios = "12.2"
frameworks = ["Foundation"]
bridging_header = "src/Bridge.h"     # objective-c the swift code uses, optional
swiftflags = ["-D", "FEATURE"]       # more flags for swiftc, optional
# swift_module = "MyTool"            # the module name, the product name when left out
```

- `min_ios` must be 12.2 or later. the swift runtime is part of ios from 12.2, older releases need it bundled and this toolchain does not do that
- arm64 and arm64e only. swift for armv7 stopped with Xcode 11
- the objective-c side imports the swift side with `#import "NAME-Swift.h"`, the header is in `build/swift/NAME/ARCH/` and that directory is on the include path. the c and objective-c files of the product wait for the swift step
- a file called `main.swift` is the entry point, without one the module is compiled with `-parse-as-library`
- the swift code is built against the sdk of the swift toolchain and the product is linked against that sdk too, the c objects only take system libraries from it. the swift runtime is linked from `/usr/lib/swift` with an rpath, the libraries the newer sdk adds are linked weak
- a change to a `.swift` file or to something the bridging header includes runs the swift step again. the step records every file `swiftc` read, so an unchanged module is fresh

## toolchain

swift for ios on a linux host needs the host `swiftc`, an ios sdk and the swift resources of an Xcode. `meow setup swift --xcode Xcode.xip` builds the sdk with [xtool](https://github.com/xtool-org/xtool) into the tools directory. by hand:

```bash
xtool sdk build Xcode_26.xip ~/dev/tools/swift-sdk
export MEOW_TOOLS=~/dev/tools     # meow looks for $MEOW_TOOLS/swift-sdk
meow doctor                       # swiftc, swift sdk, swift resources, swift clang
```

`MEOW_SWIFT_ROOT` names another directory, `MEOW_SWIFTC`, `MEOW_SWIFT_SDK` and `MEOW_SWIFT_RESOURCES` name single pieces. `swift clang` is the clang resource directory of the host swift. the resource directory of the Xcode has no clang builtin headers, and without them the clang importer of `swiftc` builds the modules of the sdk wrongly (`_c_standard_library_obsolete requires feature found_incompatible_headers`), so `meow setup swift` links the headers of the host swift into the sdk as `usr/lib/clang/<n>/include`. with a sdk made by hand, link them yourself.

## not done

- ios before 12.2, there is no way to bundle the runtime
- swift packages and dependencies, only the files named in `src`
- a swift tweak with logos in the same module: logos turns `.x` into objective-c, the swift code is a separate module that the objective-c code calls
