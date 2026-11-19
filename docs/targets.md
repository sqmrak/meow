# targets

a target is the objects of its sources per slice, then a link.

```toml
[target.app]
slices = ["armv7", "arm64"]
output = "build/${slice}/app"
cflags = ["-O2", "-Wall"]
ldflags = ["-framework", "UIKit"]
objects_from = ["zbar"]
needs = ["lwip"]
post = [["ldid", "-S", "${out}"]]
[[target.app.src]]
files = ["app/*.m"]
exclude = ["app/legacy_*.m"]
cflags = ["-fno-objc-arc"]
[target.app.per_slice.arm64]
cflags = ["-DROOTLESS=1"]
ldflags = []
```

## fields

- `slices`: the slices to build
- `src`: groups of `files`. a file may be a glob, a pattern that matches nothing is an error, `exclude` globs remove matches and an exclude that removes every source is an error. `cflags` apply to the group only
- `cflags`, `ldflags`: for every slice. `per_slice.<slice>` adds flags and variables for one slice
- `link`: `exe` (default), `archive` (needs `ar` in the slice) or `none` (objects only)
- `output`: required unless `link = "none"`. `${slice}` keeps the slices apart
- `objects_from`: link the objects of another target. it must have every slice of this one
- `needs`: finish another target, a fat output, a step or a package first. a name may use `${slice}`
- `post`: commands that run after a successful link. `${out}` is the output
- `strip`, `sign`, `entitlements`: see [verify](verify.md). only for `exe`

## layout

objects live in `build/obj/<slice>/<target>/<source path>.o`. a `..` in a source path becomes `_`, so a source outside the project cannot write outside `build/`. each source is compiled with `-MMD`, the depfile is next to the object.

the link command is the slice compiler, the slice flags, the objects, `-o`, then `ldflags` and the `ldflags` of the slice. libraries therefore come after the objects, where a static link needs them.

## link inputs

a rebuilt prebuilt archive relinks the target. meow records these as inputs of the link:

- every file named on the command line
- `-lname` found through `-L` (`libname.a`, `.dylib`, `.tbd`)
- each piece of `-Wl,a,b`, for example `-force_load` and `-exported_symbols_list`
- `@response` files

## one slice

```bash
meow build app --slice arm64
```

builds one slice of a target. `--slice` can repeat. an unknown slice is an error, and so is a target that has none of the requested slices. a fat output, a step and a package span every slice, `--slice` is refused for them. a package of one slice is written by hand: a target with that one slice and a `[deb.*]` or `[ipa.*]` that needs it.

## fat

```toml
[fat.app-fat]
from = "app"
lipo = "${TC}/lipo"
output = "build/app"
post = [["..."]]
```

`lipo -create` of the output of every slice of `from`. `from` links an executable or an archive and has two slices or more. an archive takes no `post`, `strip` or `sign`. `lipo` defaults to `${MEOW_LIPO}`. `strip`, `sign` and `entitlements` work as on a target.
