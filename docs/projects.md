# projects

a `[project]` table is shorthand for the slices, target, fat output and deb of one product. meow expands it at load into the ordinary tables, so `meow list` and `meow plan` show what it made.

```toml
[project]
name = "hello"
kind = "tool"
archs = ["armv7", "arm64"]
min_ios = "5.0"
scheme = "rootful"
src = ["src/*.m"]

[package]
id = "com.example.hello"
version = "1.0"
description = "..."
maintainer = "you"
```

## project

- `name`: binary and bundle name
- `kind`: `tool`, `app`, `tweak` or `library`
- `archs`: armv6, armv7, arm64 or arm64e. rootless holds arm64 and arm64e only. both slices when left out
- `min_ios`: 3.0 when the archs hold armv6, 5.0 when left out
- `scheme`: `rootful`, or `rootless`: files under `var/jb`, Architecture `iphoneos-arm64`
- `src`: every c, m, mm, cpp and cc file under `src/` when left out
- `cflags`, `ldflags`, `frameworks`, `libs`, `arc`: objective-c is manual reference counting unless `arc` is set
- `strip`: `strip -x` on the release binary, on when left out
- `entitlements`: the binary is signed with ldid either way, this adds the entitlements
- `info_plist`, `resources`, `bundle`: app only. `CFBundleExecutable` must be the name, `resources` is copied into the bundle
- `filter_bundles`, `filter_executables`: tweak only, one of them is required
- `file`, `tree`: extra deb files (`from`, `to`, `mode`) and directories

names it makes: target `hello`, fat output `hello-fat` (two slices or more), deb `hello-deb`, ipa `hello-ipa` (apps) and step `sdk-armv6` (armv6 only). `build` with no name builds the deb, or the ipa of an app without a package. a slice you define yourself is used as it is.

## what each kind installs

rootless adds the `var/jb` prefix.

- tool: `usr/bin/<name>`
- app: `Applications/<bundle>.app/<name>`, `Info.plist` and the resources
- tweak: `Library/MobileSubstrate/DynamicLibraries/<name>.dylib` and a generated `<name>.plist` filter
- library: `usr/lib/lib<name>.dylib`

## package

without `[package]` no deb is made.

- `id`, `name`, `version`, `description`, `maintainer`, `author`, `section`
- `depends`: replaces the default dependencies, `firmware (>= min_ios)` plus `mobilesubstrate` for a rootful tweak
- `scripts`: maintainer scripts by name, `preinst`, `postinst`, `prerm`, `postrm`
- `fields`: extra control fields
- `version`, `version_checks`, `packages_stanza`: see [packages](packages.md)

## products

several products in one package: put what they share in `[project]` and each product in `[product.NAME]`.

```toml
[project]
name = "suite"
min_ios = "5.0"

[product.daemon]
kind = "tool"
src = ["daemon/*.c"]
install = "usr/libexec/daemon"
archs = ["armv7", "arm64"]
uses = ["core"]
needs = ["dep-openssl"]

[product.core]
kind = "objects"
src = ["core/*.c"]
```

- the project has no `kind` here. `archs`, `scheme`, `arc`, `strip`, `cflags`, `ldflags`, `frameworks` and `libs` are shared defaults, a product overrides them. a tweak may add arm64e
- `kind`: `tool`, `app`, `tweak`, `library` or `objects` (compiled once, linked into others, installed nowhere)
- `src` is required with several products
- `install`: tools and libraries only, defaults to `usr/bin/NAME` and `usr/lib/libNAME.dylib`
- `uses`: link the objects of an `objects` product
- `needs`: steps, deps and products that finish first

every product that links gets a `[verify.verify-NAME]`: its slices, a deployment target no slice exceeds and a signature. the deb and ipa wait for them. an app product also gets `NAME-ipa`.
