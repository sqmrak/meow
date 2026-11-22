# commands

```
meow [-C dir] [-f file] [-V] <command> [args]
```

`-V` or `--version` prints the version.

## build

- `build [name]... [-j N] [-v] [--no-cache] [--slice S]...`: build targets, fat outputs, steps and packages. no name builds `default` from the config. `-v` prints every command, `-j` sets the number of jobs, the cpu count when left out
- `plan [name]... [--slice S]...`: print the commands and what is stale, run nothing
- `clean [name]...`: remove the `build/` outputs and records of the names
- `test [name]...`: build and run the host tests, the same as `build test`
- `why [name]...`: say why each step of the names is stale
- `compdb [name]...`: write `compile_commands.json` for the compile steps

## project

- `init`: write a `meow.toml` for the c sources in `src/`
- `list`: show everything the config defines
- `doctor`: show the toolchain, sdks and host tools meow found, exit 1 when a required one is missing
- `ios [VERSION]`: which cpu slices run on which ios release
- `cache [stats|gc|clear]`: the shared object cache

## setup

- `setup [COMPONENT|all]... [--to DIR] [--force] [--xcode Xcode.xip] [--sdk VERSION]...`: list the components, or install the toolchain, headers, sdks, rust, go and swift, see [setup](setup.md)

## sdk

- `sdk [VERSION|all]... [--to DIR] [--cache DIR] [--force]`: list the ios sdks, or download the named ones, see [sdk](sdk.md)

## new and migrate

- `new [TEMPLATE NAME] [--id ID] [--maintainer WHO] [--dir DIR]`: list the templates, or write a project from one, see [new](new.md)
- `migrate [DIR] [--write [--force]] [--set VAR=VALUE]...`: write a `meow.toml` from a theos project, see [migrate](migrate.md)
- `plist [--to xml|binary|json] [-o OUT | --in-place] FILE...` and `plist --get PATH FILE`: convert property lists, see [plist](plist.md)

## logos

- `logos [-c key=value]... FILE`: run the logos preprocessor on a `.x` or `.xm` file
- `logos logify [-i LIST | -e LIST] [FILE]...`: make logging hooks from a header, see [logos](logos.md)

## release

- `install --host H [--user root] [--port 22] [--password-env VAR] [--legacy] F.deb`: copy a deb to a device and run `dpkg -i`
- `verify-sign FILE [--ldid L] [--entitlements P]`: check every slice of FILE is signed
- `verify-macho FILE [--slices a,b] [--min-ios V] [--signed] [--allow-dylib P]... [--forbid-dylib P]... [--forbid-symbol S]... [--forbid-symbols-file F] [--require-symbol S]... [--nm path]`: release checks on a binary
- `check-version --expect V (--plist F K | --field F K)...`: fail when a file carries another version
- `sdk-thin --arch A --from SDK --to DIR`: copy an sdk with one slice per fat file, for old sdks on 16 KiB page hosts

## internal

meow runs these itself for `[deb.*]`, `[ipa.*]` and `[dep.*]`, call them by hand only to debug a package.

- `pack --out F.deb ...`: build a deb
- `fetch --name N (--url U --sha256 H | --git U --rev R) --cache D --to D`: a pinned download

## exit status

0 on success, 1 on any failure. a failed command prints its label, the command line and its output, and removes the output it was writing.
