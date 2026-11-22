# go

a go package is built as a c archive and linked into other products. `kind = "go"` makes the archive, `uses` links it.

```toml
[project]
name = "suite"
archs = ["armv7", "arm64"]
min_ios = "7.0"

[product.core]
kind = "go"
go_dir = "go/core"             # the directory with go.mod, the project directory when left out
go_pkg = "./cmd/lib"           # the package to build, . when left out
go_tags = ["fast"]             # build tags, optional
goflags = ["-ldflags=-s"]      # more flags for `go build`, optional
go_inputs = ["../shared/**/*.go"]  # more files that rebuild it, for replaced modules outside go_dir

[product.app]
kind = "tool"
src = ["src/main.m"]
uses = ["core"]
```

the package is `package main` with an empty `main` and `//export` functions, as `-buildmode=c-archive` wants. the c side includes `libNAME.h`, the header cgo writes, `NAME` being the product. the product that uses it gets `-Ibuild/go/NAME/ARCH` for it.

- a step `go-NAME-ARCH` runs `go build -buildmode=c-archive -trimpath` (and `-buildvcs=false` for the current go, a build inside a git checkout must not ask git) with `-extar` set to the `ar` of the toolchain, so the archive has the table of contents the darwin linker needs. the archive is `build/go/NAME/ARCH/libNAME.a`
- cgo gets the compiler, the flags and the sdk of the slice, and every slice has its own build cache, two go releases cannot share one. `GOTOOLCHAIN=local` stops a `toolchain` line in `go.mod` from fetching another go
- the step reruns when a `.go`, `.c`, `.h` or `.s` file under `go_dir`, `go.mod`, `go.sum` or a `go_inputs` file changes
- a product that uses a go package is linked with `-lresolv -framework Security -framework CoreFoundation`, what the go runtime for ios asks for

## slices

- **arm64**: `GOOS=ios GOARCH=arm64` with the current go, `MEOW_GO`, `$MEOW_TOOLS/go/bin/go` or `go` on PATH
- **armv7**: go 1.14, the last release with `ios/arm`, built as `GOOS=darwin GOARCH=arm GOARM=7`. it is `MEOW_GO_ARMV7` or `$MEOW_TOOLS/go-1.14/bin/go`. the go line of `go.mod` must not be newer than 1.14, meow says so before it runs. the code must compile with go 1.14: no generics, no `embed`, and the standard library of that release
- **arm64e and armv6**: errors. go has no arm64e and an arm64 archive does not link into an arm64e binary, armv6 never had go. a product that is built for arm64e gives the package its own `archs`

`meow setup go` installs both. `meow doctor` lists both go commands. a go 1.14 for linux comes from `https://go.dev/dl/go1.14.15.linux-arm64.tar.gz` or the build for your host, unpacked into `$MEOW_TOOLS/go-1.14`.

## limits

- the library links against the sdk of the slice, so the link finds every symbol. that does not mean the system has them: the go standard library imports calls that old ios does not export (`fstatat` and its relatives before ios 8, `clock_gettime` before 10, `SecTrustEvaluateWithError` before 12, `utimensat` before 13, `SecTrustCopyCertificateChain` before 15), and dyld stops the process that cannot bind one. below ios 15 a link with an older sdk or a set of shims that define them is the way, see the compat shims of ios/golang/toolchain
- armv7 and arm64 archives have been linked into binaries, nothing has been run on a device
- modules that are not vendored are downloaded by go, as always
