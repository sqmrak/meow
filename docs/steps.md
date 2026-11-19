# steps

a step is an external command sequence with declared inputs and outputs, for work meow does not model itself: dependency builds, scripts, packaging helpers. commands run in order, no shell.

```toml
[step.deps]
run = [["bash", "scripts/deps.sh"], ["bash", "scripts/check.sh"]]
needs = ["app-fat"]
inputs = ["src/**/*.c"]
outputs = ["build/deps.a"]
env = { PATH = "/usr/bin:/bin" }
cwd = "scripts"
```

- `run`: commands, each an argument vector
- `needs`: targets, fat outputs or steps that finish first. `${slice}` is not available in a step
- `inputs`: globs the commands read without naming them. files named in `run` are inputs already
- `outputs`: must exist after the run, removed before it
- `stamp`: a file meow creates after success, for steps that make nothing
- `keep`: outputs stay when the step is stale, for scripts that rebuild only what is missing. a `keep` step runs when asked for by name, never because a package reads its output
- `bootstrap`: runs only while an output is missing and is never stale otherwise, for one-time dependency builds whose script wipes its output directory first
- `env`: added to the environment of the commands
- `cwd`: the directory of the commands, relative to the project

## dep

a pinned source, cached in `$MEOW_CACHE_DIR/src/NAME-<id>`.

```toml
[dep.openssl]
url = "https://..."
sha256 = "..."
strip = 1
patches = ["patches/a.patch"]

[dep.core]
git = "https://..."
rev = "..."
```

- `url`: https, http or file. `sha256` is required, 64 hex digits, a mismatch is an error and nothing is kept
- `git` instead of `url`: `rev` must be a full commit id
- `strip`: leading path parts removed, tar and zip
- `patches`: `patch -p1`, in order

a dep makes the step `dep-NAME` and the variable `${dep.NAME}`, the directory the source is in. use it with `needs = ["dep-NAME"]`. downloads are cached by hash in `$MEOW_CACHE_DIR/dl`, so a clean tree does not fetch again. curl, tar (or unzip), patch and git are host tools. `meow fetch` does the same by hand.

## write

a small generated file, a wrapper script for a configure step for example.

```toml
[write.wrap]
to = "build/wrap.sh"
content = "#!/bin/sh\nexec ${MEOW_CLANG} \"$@\"\n"
mode = "755"
needs = []
```

`${...}` is expanded in `content`, so a path of the build can be written into it. `mode` is octal, 644 when left out.

## test

a host program, built with the host compiler and run. `meow test` runs all of them.

```toml
[test.core]
src = ["core/*.c", "tests/t_core.c"]
sanitize = "address,undefined"
libs = ["m"]
cflags = []
ldflags = []
args = []
env = {}
inputs = ["tests/fixtures/*"]
needs = ["dep-zlib"]
```

- the program is `build/host/test-NAME`
- `sanitize`: `address`, `undefined`, `thread`, `leak`
- `inputs`: globs the program reads when it runs, such as fixtures
- `run` instead of `src`: a script or program that is not built here, run as it is. it excludes `src`. `cwd` sets its directory

host tests use `${MEOW_HOST_CC}` and `${MEOW_HOST_AR}`, see [toolchain](toolchain.md). they never touch a device.

## check

a syntax-only compile against another sdk, no objects. an old sdk has no declaration for a newer call, so this catches code that needs an api the target lacks.

```toml
[check.compat]
slice = "armv7"
sdk = "${MEOW_SDK_COMPAT}"
src = ["app/*.m"]
headers = ["app/*.h"]
cflags = []
needs = []
```

the flags of the slice are used, `sdk` replaces `${SDK}`. a change in `headers` reruns the check.
