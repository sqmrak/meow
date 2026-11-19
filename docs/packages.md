# packages

## deb

a deb from files, for projects that keep their own graph. `[project]` builds one for you, see [projects](projects.md).

```toml
[deb.pkg]
output = "build/${field:control:Package}.deb"
control = "packaging/DEBIAN/control"
scripts = { postinst = "packaging/DEBIAN/postinst" }
prefix = "var/jb"
needs = ["app-fat"]
packages = "build/Packages"

[[deb.pkg.file]]
from = "build/app"
to = "Applications/App.app/App"
mode = "755"

[[deb.pkg.tree]]
from = "resources"
to = "Applications/App.app"
exclude = ["*.psd", "sub/*.tmp"]

[[deb.pkg.data]]
to = "Library/MobileSubstrate/DynamicLibraries/x.plist"
content = "{ Filter = { Bundles = ( \"com.apple.springboard\" ); }; }"
```

- `control`: a ready control file, or `fields = { Package = "...", ... }`, not both
- `scripts`: maintainer scripts by name
- `prefix`: put in front of every destination, `var/jb` on a rootless system
- `needs`: a file whose path is the output of a fat, step or target needs no entry
- `file`: `from`, `to`, `mode` (644 when left out), `replace`
- `tree`: a directory copied whole. executables keep 755, everything else gets 644, `mode` sets one mode for all. `exclude` globs are relative to the tree, `replace` lets the tree overwrite what an earlier entry put at the same path (for icons that differ between ipa and deb)
- `data`: a small file written from text
- `packages`: writes the `Packages` stanza next to the deb: the control fields, Filename, Size and md5, sha1 and sha256 of the file

symlinks in a tree are kept as symlinks. anything that is not a regular file, a directory or a symlink is an error.

debs are packed the way the old package managers read them: gnu tar members, gzip data, ar members in dpkg order, root ownership, mtime 2020-01-01 (`SOURCE_DATE_EPOCH` overrides it), md5sums, so equal input gives equal bytes. only gzip is written, ios before 8 cannot read xz. meow writes the tar and the gzip itself, nothing is needed on the host, and it checks the result with `dpkg-deb` when that is installed.

### checks

each failure names the path. paths are relative to the package root, `DEBIAN/x` for control files.

- `require`: files that must exist and not be empty
- `require_exec`: files that must exist and carry an execute bit
- `forbid`: globs, nothing in the package may match
- `forbid_text`: no text file may contain one of these strings
- `forbid_setuid`: refuse a setuid or setgid file
- `sh_check`: files run through `sh -n`
- `contains`: `[{ file = "...", text = "..." }]`, a file that has to contain a string

## ipa

the same without control data: a zip with deflate written by meow, no zip64, modes in the entries.

```toml
[ipa.app]
output = "build/app.ipa"
prefix = "Payload/App.app"
```

`prefix` is required and must start with `Payload/`. `file`, `tree`, `data` and `needs` work as in a deb, and their paths are relative to the prefix.

meow checks the bundle before it zips: one `Payload/X.app`, an `Info.plist` with `CFBundleIdentifier`, an executable that is a mach-o file, and it reads every entry back.

the ipa carries the binary as meow signed it: a pseudo signature for sideloading tools that sign again, or the entitlements of the project for installers that keep them. it is not signed with a developer identity.

## version in one place

```toml
[package]
version = "@packaging/DEBIAN/control:Version"
version_checks = [{ file = "Info.plist", key = "GKReleaseVersion" }]
```

`@file:key` reads the version from a text plist key or a control field. a `version-check` step fails the deb and ipa when a listed file carries another version. `meow check-version --expect V (--plist F K | --field F K)...` does it by hand.

`packages_stanza = true` in `[package]` writes `build/<id>_<version>_<arch>.Packages` next to the deb.

## install

```bash
meow install --host iphone.local build/app.deb
meow install --host iphone.local --user root --port 22 --password-env DEVICE_PW --legacy build/app.deb
```

copies the deb to `/tmp/meow-<name>` with scp, runs `dpkg -i` over ssh, then removes the file whether dpkg succeeded or not. a key is used when the device has one. otherwise the password comes from the environment variable named by `--password-env` and goes to `sshpass` in its environment, never on a command line where ps would show it. `--legacy` adds the key exchange, host key and cipher algorithms that the ssh servers of ios 5 to 10 still offer. host and user may hold letters, digits and `. - _` only. it runs only when you call it, `build` never installs.
