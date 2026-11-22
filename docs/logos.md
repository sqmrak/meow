# logos

`meow logos` is the logos preprocessor of theos, written in rust inside meow. it reads a `.x` or `.xm` file and writes plain objective-c to stdout, warnings and errors go to stderr.

```bash
meow logos Tweak.x > Tweak.x.m
meow logos -c generator=libhooker Tweak.x
meow logos logify UIView.h > UIView.x
```

- `-c generator=[MobileSubstrate|libhooker|internal]`, default MobileSubstrate
- `-c warnings=[default|error|none]`
- `logify [-i LIST | -e LIST] [FILE]...` turns the `@interface` of a header into a file that logs every call, `-i` keeps only the listed methods and `-e` drops them. no file reads standard input

input must be utf-8. generators loaded from a path (`name@path`) are not supported.

## in a project

`.x` and `.xm` files in a project are generated before they are compiled: `.x` becomes objective-c, `.xm` objective-c++. each one has a step `logos-NAME-N` that runs `meow logos -o build/logos/NAME/<path>.m`, so a change to one file regenerates and recompiles only that file. line directives point back at the `.x`, so clang errors name your file. the directory of each source is added with `-I`, because a generated file in `build/` would otherwise not find its quoted includes.

```toml
[project]
name = "mytweak"
kind = "tweak"
filter_bundles = ["com.apple.springboard"]
logos = "MobileSubstrate"   # or libhooker, internal
cflags = ["-I${MEOW_HEADERS}", "-F${MEOW_VENDOR_LIB}"]
```

meow does not add the substrate or libhooker headers and frameworks, name them in `cflags` as above. `meow setup headers` installs them. the generated code links `CydiaSubstrate` or `libhooker` on its own.

`-o OUT` writes the result to a file instead of standard output.

## supported

`%hook`, `%subclass`, `%group`, `%new`, `%property`, `%orig`, `&%orig`, `%log`, `%init`, `%ctor`, `%dtor`, `%c`, `%class`, `%hookf`, `%config`, `%end`, `#if 0` blocks, comments and strings, preprocessed input with `# n "file"` markers.

## against perl logos

the golden files in `tests/logos/golden` are the output of the perl logos on `tests/logos/corpus` for all three generators, with the warnings modes, and `tests/logos/logify` holds the same for logify. `cargo test logos` compares them. `tests/logos/regen-golden.sh` rebuilds them, `LOGOS_ORACLE` points at `logos.pl`. use the revision before theos/logos#114: since then `[%orig foo]` loses the rest of the line.

perl walks hashes in random order, so the `@class` line and the static class lookups come out in first use order here, and the golden test sorts them before comparing.

## differences

- `%orig(...)` can continue on later lines, the call is joined into the first line and the later lines stay empty so line directives still match. text after the closing parenthesis is kept
- `%class` looks up the class it names, perl looks up an empty name
- `%log((Type)x)` with a type that has no format character no longer adds a stray `1` to the format
- `& %orig` with a space is replaced whole
- `%log` outside a method and a `%hookf` without a return type and name are errors, perl dies with a stack trace
- `logify` names the variable inside block return types, `void (^r)(int) = %orig;`, perl writes `void (^ r = %orig;`
