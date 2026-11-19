# verify

## strip and sign

```toml
[target.app]
strip = true
entitlements = "ents.plist"

[fat.app-fat]
from = "app"
output = "build/app"
sign = true
```

- `strip = true`: `strip -x` after the link
- `sign = true`: `ldid -S`, then the signature is read back
- `entitlements = "ents.plist"`: `ldid -Sents.plist`, implies `sign`

they run after `post`, on an executable target or a fat output of executables. meow reads the file after signing: every slice of the mach-o needs a code signature blob inside the file, and every key of the entitlements file has to be in `ldid -e`. ldid exits 0 on some broken inputs, so its exit status is not trusted.

```bash
meow verify-sign FILE [--ldid path] [--entitlements plist]
```

does the same check by hand.

## release check

a node like any other, so a deb or an ipa can wait for it.

```toml
[verify.release]
file = "build/app"
slices = ["armv7", "arm64"]
min_ios = "5.0"
signed = true
allow_dylibs = ["/usr/lib/", "/System/Library/"]
forbid_dylibs = ["/usr/lib/libc++"]
forbid_symbols = ["posix_spawn"]
forbid_symbols_file = "scripts/legacy-symbols.txt"
require_symbols = ["_main"]
needs = []
```

- `slices`: the file must hold exactly these cpus
- `min_ios`: the oldest release the binary is meant for. each cpu is raised to its own first release, so arm64 may need 7.0. no slice may ask for more
- `signed`: every slice has a code signature
- `allow_dylibs`: every linked dylib starts with one of these prefixes
- `forbid_dylibs`: no linked dylib starts with one of these
- `forbid_symbols`: undefined symbols that must not appear, read with `nm`. `forbid_symbols_file` has one per line, `#` starts a comment
- `require_symbols`: external symbols the binary has to define

by hand:

```bash
meow verify-macho FILE --slices armv7,arm64 --min-ios 5.0 --signed \
    --allow-dylib /usr/lib/ --forbid-dylib /usr/lib/libc++ \
    --forbid-symbol posix_spawn --require-symbol _main
```

`--nm`, `--forbid-symbols-file` also exist. a deployment target is read from every slice, so a build that quietly raised it fails here.

## after a build

passing tests do not make a binary valid. look at the artifact:

```bash
lipo -info build/app
meow verify-macho build/app --slices armv7,arm64 --min-ios 5.0 --signed
dpkg-deb -c build/app.deb
```
