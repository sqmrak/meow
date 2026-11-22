# rust

a cargo crate is built as a static library and linked into other products. `kind = "rust"` makes the library, `uses` links it.

```toml
[project]
name = "suite"
archs = ["arm64"]
min_ios = "10.0"

[product.core]
kind = "rust"
crate_dir = "rust/core"        # the directory with Cargo.toml, the project directory when left out
features = ["fast"]            # cargo features, optional
rust_inputs = ["shared/**/*.rs"]  # more files that rebuild it, for path dependencies outside crate_dir

[product.app]
kind = "tool"
src = ["src/main.c"]
uses = ["core"]
```

the c side declares the `#[no_mangle] pub extern "C"` functions of the crate and calls them. a rust product makes no target and nothing is installed for it.

- a step `rust-NAME-ARCH` runs `cargo rustc --lib --release --target aarch64-apple-ios --crate-type staticlib`. the crate does not need `crate-type` in its `Cargo.toml`. the library is `build/rust/NAME/ARCH/cargo/aarch64-apple-ios/release/lib<crate>.a`
- the step reruns when `Cargo.toml`, `Cargo.lock`, `build.rs`, a file under `src/` or a `rust_inputs` file changes. cargo decides what to compile inside
- the environment of the step: `IPHONEOS_DEPLOYMENT_TARGET` is `min_ios`, `SDKROOT` is the sdk of the slice, `CC`, `AR` and `CFLAGS` for `aarch64-apple-ios` point at the toolchain of meow, so a `build.rs` that compiles c finds the cross compiler
- a product that uses a rust crate is linked with `-lresolv -liconv -framework Security -framework CoreFoundation`, what the std for ios asks for
- the product that uses it must build only slices the crate builds

## slices

- **arm64**: the `aarch64-apple-ios` target, rust ships its std. the arm64 slice is built for ios 8.0 or later in a project that has a rust product, the std uses thread local variables and the linker emits them from ios 8 on
- **armv7**: rust dropped the target, so meow builds the std for it. a step `rust-target-armv7` writes the spec (`rust/armv7-apple-ios.json`, the spec of armv7s-apple-ios with the cpu of a cortex-a8 and without native thread locals, which ios before 9 does not have) and the crate step runs `cargo rustc -Zbuild-std=std,panic_abort -Zjson-target-spec --target build/rust/armv7-apple-ios.json` with `RUSTC_BOOTSTRAP=1`, which lets the stable compiler take the nightly flags. the first build compiles the std, about ten seconds, and cargo keeps it in the target directory
- a product that builds armv7 links the armv7 library, it can go down to the deployment target of the project, 5.0 for example. the link against an old sdk is the check: a symbol the std needs that the system of that sdk does not have is a link error
- the crate follows the `archs` of the project, give it `archs = [...]` of its own to build fewer slices. arm64e and armv6 are errors

## limits

- armv7 needs `rust-src` and a network for the first build, the std has dependencies that cargo fetches (`hashbrown`, `gimli` and others). `meow doctor` shows `rust src`. to add it to an official install unpack `rust-src-<version>.tar.xz` from static.rust-lang.org and run its `install.sh --prefix=<the install>`
- rustc says that the deployment target is below its minimum of 10.0 when `min_ios` is lower, and passes it on. the armv7 std was built for a system older than rust supports and is only as good as the link check: it has not been run on a device
- arm64e has no std from rust and its spec would need the same build-std treatment, it is not done
- the deployment target is yours to choose, rust may use a symbol the system does not have
- `meow setup rust` installs rustc, cargo, the std for aarch64-apple-ios and rust-src. `cargo` and a `rustc` with the std for aarch64-apple-ios. meow looks for `$MEOW_TOOLS/rust-official/bin/cargo` and uses the `rustc` next to it, then `cargo` on PATH. `MEOW_CARGO` and `MEOW_RUSTC` name them. `meow doctor` shows both. to add the target to an official install, unpack `rust-std-<version>-aarch64-apple-ios.tar.xz` from static.rust-lang.org and run its `install.sh --prefix=<the install>`
- crates that are not vendored are downloaded by cargo, as always
