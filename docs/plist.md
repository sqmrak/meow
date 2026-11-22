# plist

`meow plist` converts property lists. it reads xml, binary (`bplist00`) and old style text plists, and writes xml, binary or json. no `plutil` is needed.

```bash
meow plist Info.plist                       # xml to standard output
meow plist --to binary -o out.plist in.plist
meow plist --to xml --in-place a.plist b.plist
meow plist --to json Info.plist
meow plist --get Filter.Bundles.0 Tweak.plist   # dict keys and array indexes joined with dots
```

- `--to xml|binary|json`, xml when left out. json fails on dates and data, which it cannot hold
- `-o OUT` writes the one input file to OUT, `--in-place` rewrites every input file, keeping its format unless `--to` is given. `-` is standard input or output
- `--get PATH` prints a string, number or boolean as text and a dict or array as a plist
- the old text format (`{ Filter = { Bundles = ( "a" ); }; }`) holds strings and data only. a strings file (`"a" = "b";`) reads as a dict
- integers are 64 bit, dates are kept to the second as xml writes them. a nesting deeper than 128 levels is an error, it protects against files that refer to themselves

## in a project

`plist = "binary"` (or `"xml"`) on a product that has `info_plist` writes the `Info.plist` of the package in that format. a step `plist-NAME` converts it into `build/plist/NAME/`, the plist in the source tree is not touched and `CFBundleExecutable` is checked on the original.

```toml
[product.prefs]
kind = "bundle"
info_plist = "Info.plist"
plist = "binary"
```
