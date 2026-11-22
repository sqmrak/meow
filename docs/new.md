# new

`meow new` lists the project templates, `meow new TEMPLATE NAME` writes one.

```bash
meow new                       # the list
meow new tweak mytweak         # ./mytweak with a meow.toml and sources
meow new app Notes --id com.me.notes --maintainer "me" --dir ~/dev/notes
cd mytweak && meow build
```

| template | what it is |
|---|---|
| tweak | a MobileSubstrate tweak with a logos source, armv7 and arm64 |
| tool | a command line tool installed in `/usr/bin` |
| library | a dynamic library with a header |
| framework | a framework in `/Library/Frameworks` |
| app | a UIKit application with an app delegate, deb and ipa |
| prefs | a preference bundle for PreferenceLoader with its entry plist |
| appex | an app extension |
| swift | a tool written in swift, ios 12.2 and later, arm64 |
| rust | a c tool that links a rust library, arm64 |

- `NAME`: letters, digits, `. _ -`. it is the project name and the binary name. the Objective-C classes get a prefix made from it
- `--id`: the package id, `com.example.<name>` when left out
- `--maintainer`: the Maintainer field, the `user.name` of git when left out
- `--dir`: where to write, `./NAME` when left out. a directory that exists and is not empty is never written into

every template builds as it is. the tweak and prefs templates read the substrate and preferences headers through `${MEOW_HEADERS}`, `meow setup headers` installs them. the files of the templates are in `templates/`, `src/project/new.rs` names them.
