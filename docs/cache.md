# cache

## freshness

a step is fresh when the hash of its command line matches the record and every input still has the recorded content. records live in `build/.meow`.

the hash covers the commands, the environment and the set of glob inputs, so a new file that matches a glob makes the step stale before any content is compared.

inputs are:

- the files named on the command line
- the files a link reaches through `-L` with `-lname`, the pieces of `-Wl,a,b` and `@response` files, see [targets](targets.md)
- the headers from `clang -MMD`, so no header list is needed
- the globs of `inputs` in a step, test or check

a changed mtime with the same bytes stays fresh. an input younger than the job start is not recorded, so a file edited during a compile rebuilds on the next run.

## object cache

compile steps are cached in `$MEOW_CACHE/obj` (`~/.cache/meow/obj`), shared by every build tree. an entry is keyed by the compile command, the environment and the compiler binary, and lists every input with its hash, headers included, so it is used only when all of them have the same content now. a clean tree or a second checkout takes its objects from the cache and the summary says `cached N`.

- compiles with `-g` are not cached, their objects name the directory they were built in
- link steps, steps and packages are not cached
- the limit is `MEOW_CACHE_MAX` MiB (4096), trimmed least recently used first after a build that added entries
- `meow cache stats|gc|clear` shows, trims or empties it
- `meow build --no-cache` or `MEOW_NO_CACHE=1` skips it

## why and plan

```bash
meow plan app-fat
meow why app-fat
```

`plan` prints every command and whether it is fresh or stale, and runs nothing. `why` says why each stale step is stale: a changed command line, which input changed, which output is missing, which dependency is stale.

## compdb

```bash
meow compdb app
```

writes `compile_commands.json` for clangd from the compile steps of the names.

## clean

`meow clean NAME...` removes the files under `build/` and the records of the names. dependency caches and release archives stay.
