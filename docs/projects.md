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
- `kind`: `tool`, `app`, `tweak`, `library`, `bundle`, `framework` or `appex`, and `rust` for products, see below
- `archs`: armv6, armv7, arm64 or arm64e. rootless holds arm64 and arm64e only. both slices when left out
- `min_ios`: 3.0 when the archs hold armv6, 5.0 when left out
- `sdk`: the sdk to compile against, `9.3` for `iPhoneOS9.3.sdk`. `meow sdk 9.3` installs it. the newest one a slice can use when left out
- `scheme`: `rootful`, or `rootless`: files under `var/jb`, Architecture `iphoneos-arm64`
- `src`: every c, m, mm, cpp, cc, x, xm and swift file under `src/` when left out. `.x` and `.xm` files go through [logos](logos.md) first, `.swift` files are built by [swift](swift.md)
- `swiftflags`, `swift_module`, `bridging_header`: swift, see [swift](swift.md)
- `logos`: backend for `.x` and `.xm` files, `MobileSubstrate` when left out, or `libhooker`, `internal`
- `cflags`, `ldflags`, `frameworks`, `libs`, `arc`: objective-c is manual reference counting unless `arc` is set
- `strip`: `strip -x` on the release binary, on when left out
- `entitlements`: the binary is signed with ldid either way, this adds the entitlements
- `info_plist`, `resources`, `bundle`: app, bundle, framework and appex. the plist is required, `CFBundleExecutable` must be the name, `resources` is copied into the bundle, `bundle` is the directory name and defaults to the name
- `plist`: `binary` or `xml`, the format `Info.plist` has in the package, see [plist](plist.md)
- `bundle_extension`: bundle only, `bundle` when left out, `prefs` for example
- `install`: tool, library, bundle, framework and appex, see below
- `filter_bundles`, `filter_executables`: tweak only, one of them is required
- `file`, `tree`: extra deb files (`from`, `to`, `mode`) and directories

names it makes: target `hello`, fat output `hello-fat` (two slices or more), deb `hello-deb`, ipa `hello-ipa` (apps) and step `sdk-armv6` (armv6 only). `build` with no name builds the deb, or the ipa of an app without a package. a slice you define yourself is used as it is.

## what each kind installs

rootless adds the `var/jb` prefix.

- tool: `usr/bin/<name>`
- app: `Applications/<bundle>.app/<name>`, `Info.plist` and the resources
- tweak: `Library/MobileSubstrate/DynamicLibraries/<name>.dylib` and a generated `<name>.plist` filter
- library: `usr/lib/lib<name>.dylib`
- bundle: `<install>/<bundle>.<extension>/<name>`, `Info.plist` and the resources next to it. `install` is required, `Library/PreferenceBundles` for a preference bundle. the binary is a dylib
- framework: `<install>/<bundle>.framework/<name>`, `install` is `Library/Frameworks` when left out. the binary is a dylib with that install name
- appex: `<install>/<bundle>.appex/<name>`, `install` is required, `Applications/Host.app/PlugIns` for an extension of an app. the binary is an executable that starts at `_NSExtensionMain`

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
- `kind`: `tool`, `app`, `tweak`, `library`, `bundle`, `framework`, `appex`, `objects` (compiled once, linked into others, installed nowhere) or `rust` (a cargo crate for arm64 and armv7, linked into others, see [rust](rust.md)) or `go` (a go package for arm64 and armv7, see [go](go.md))
- `src` is required with several products
- `install`: defaults to `usr/bin/NAME` and `usr/lib/libNAME.dylib` for tools and libraries, see above for the rest
- `uses`: link the objects of an `objects` product, the static library of a `rust` product, or the archive of a `go` product
- `needs`: steps, deps and products that finish first

every product that links gets a `[verify.verify-NAME]`: its slices, a deployment target no slice exceeds and a signature. the deb and ipa wait for them. an app product also gets `NAME-ipa`.

a tweak with a preference bundle and a daemon is one package of three products, this is what theos calls subprojects:

```toml
[product.tweak]
kind = "tweak"
filter_bundles = ["com.apple.springboard"]

[product.prefs]
kind = "bundle"
src = ["prefs/*.m"]
install = "Library/PreferenceBundles"
info_plist = "prefs/Info.plist"
resources = "prefs/Resources"
```

an ipa holds the app only, bundles, frameworks and extensions go into the deb.
