# sdk

`meow sdk` lists every ios sdk meow can install, `meow sdk VERSION` downloads one. the list is `sdk/manifest.toml`, built into the binary.

```bash
meow sdk                  # all sdks, 2.0 to 16.5, and which are installed
meow sdk 9.3              # download one
meow sdk 6.1 16.5         # several
meow sdk all              # everything, about 3 GB
meow sdk 7.1 --to DIR     # somewhere else
```

an sdk goes to `$THEOS/sdks`, or `theos/sdks` in the tools directory (`$MEOW_TOOLS`, or the default one of [setup](setup.md)), the directories the toolchain lookup reads, as `iPhoneOS<version>.sdk`. `--to` picks another directory. downloads are cached in the meow cache (`$MEOW_CACHE`, `--cache`) by hash.

- an archive is used only when its sha256 matches the manifest, a git source only at the commit the manifest names
- the result must have `usr/include/stdio.h`, otherwise nothing is installed
- a directory that meow did not install is left alone, `--force` replaces it. a dangling symlink is replaced without it
- `meow sdk` shows `installed` for an sdk meow put there with the same pin, `present` for one it did not, `other pin` after the manifest changed

## sources

| versions | source | form |
|---|---|---|
| 2.0 to 3.2, 8.1, 9.0 to 9.2, 12.0, 12.2 | [qianjigui/iOS-sdks](https://github.com/qianjigui/iOS-sdks) | archive |
| 3.1, 3.1.2, 4.0 to 4.2, 5.0, 6.0, 7.0, 7.1 | qianjigui/iOS-sdks | archive cut into pieces, each piece is hashed |
| 4.3, 5.1, 6.1 | [EachAndOther/Legacy-iOS-SDKs](https://github.com/EachAndOther/Legacy-iOS-SDKs) | git, only that directory is fetched |
| 9.3, 10.3, 11.4, 12.4, 13.7, 14.5, 15.6, 16.5 | [theos/sdks](https://github.com/theos/sdks) | git, only that directory is fetched |

the theos sdks are patched: apple removed private symbols from the sdk with ios 9.3, theos puts them back. the rest are the sdks of the xcode installs as they are, meow adds no patches. there is no sdk for 8.0, 8.2 to 8.4, 10.0 to 10.2 and 11.0 to 11.3, no source has them.

curl, tar or unzip, and git are host tools. a git source needs a unix host, it is copied with `cp -a`.

old sdks have slices on 4 KiB boundaries, see [toolchain](toolchain.md) for armv6 on 16 KiB page hosts.
