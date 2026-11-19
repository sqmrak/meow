# config

`meow.toml` is the whole project: slices, targets, steps, packages. meow looks for it in the current directory and then in its parents. `-C dir` starts from another directory, `-f file` names another file. every path in the file is relative to the directory that holds it.

## tables

- `[vars]`, `[slice.*]`, `[target.*]`, `[fat.*]`: see [toolchain](toolchain.md) and [targets](targets.md)
- `[step.*]`, `[dep.*]`, `[write.*]`, `[test.*]`, `[check.*]`: see [steps](steps.md)
- `[deb.*]`, `[ipa.*]`: see [packages](packages.md)
- `[verify.*]`: see [verify](verify.md)
- `[icons.*]`, `[launch.*]`, `[glyphs.*]`, `[image.*]`: see [pictures](pictures.md)
- `[project]`, `[package]`, `[product.*]`: see [projects](projects.md)
- `default = [...]`: the names `build` and `plan` use when given none, keep it above all tables

an unknown key is an error, and the message says the file may be written for a newer meow.

## variables

```toml
[vars]
TC = "${TOOLCHAIN}"
JOBS = "${JOBS:-4}"
```

- `${NAME}` is a project variable, then the environment. an unset name is an error
- `${NAME:-default}` gives a default when the name is unset
- a slice has its own `[slice.armv7.vars]`, a target slice has `[target.app.per_slice.arm64]` with `vars`. the narrowest scope wins
- `${slice}`, `${target}`, `${fat}`, `${step}`, `${deb}`, `${ipa}`: the name of what is being expanded
- `${out}`: the output, in `post` commands
- `${root}`: the project directory
- `${field:FILE:KEY}`: the value of a `KEY: value` line, for the version in a debian control file
- `${dep.NAME}`: the directory of a `[dep.NAME]` source
- `${MEOW_*}`: what `meow doctor` found, see [toolchain](toolchain.md). `${MEOW_JOBS}` is the cpu count

## include

```toml
include = ["../shared/ios.toml"]
```

in the main file only, up to 4 deep. the tables vars, slice, target, fat, step, deb, ipa, test, check, dep, verify, product and write are merged, a name defined twice is an error naming both files. default, project and package stay in the main file. paths in every file are relative to the main file.
