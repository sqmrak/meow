use std::fs;
use std::path::Path;

// copies a sdk with every fat mach-o file cut down to one architecture.
// ld64 maps a slice at its offset in the fat file, and an old sdk lays slices out on
// 4 KiB boundaries, so on a host with 16 KiB pages the map fails with errno 22
// ("can't re-map file"). a thin copy has every slice at offset 0

const MAX_SLICES: usize = 16;
const MAX_FAT_FILE: u64 = 512 * 1024 * 1024;
pub const STAMP: &str = ".meow-sdk";

fn cpu(arch: &str) -> Option<(u32, u32)> {
    match arch {
        "armv6" => Some((12, 6)),
        "armv7" => Some((12, 9)),
        "arm64" => Some((0x0100_000c, 0)),
        _ => None,
    }
}

fn be32(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

// the byte range of the wanted slice when the file is a 32-bit fat mach-o that has it
fn find_slice(head: &[u8], want: (u32, u32)) -> Result<Option<(usize, usize)>, String> {
    if be32(head, 0) != Some(0xcafe_babe) {
        return Ok(None);
    }
    let n = be32(head, 4).ok_or("truncated fat header")? as usize;
    if n == 0 || n > MAX_SLICES {
        return Ok(None);
    }
    for i in 0..n {
        let e = 8 + i * 20;
        let (t, s) = (be32(head, e).ok_or("truncated fat arch")?, be32(head, e + 4).ok_or("truncated fat arch")?);
        if t == want.0 && (s & 0x00ff_ffff) == want.1 {
            let off = be32(head, e + 8).ok_or("truncated fat arch")? as usize;
            let size = be32(head, e + 12).ok_or("truncated fat arch")? as usize;
            return Ok(Some((off, size)));
        }
    }
    Ok(None)
}

#[derive(Debug, Default)]
pub struct Stats {
    pub thinned: usize,
    pub linked: usize,
}

fn walk(from: &Path, to: &Path, want: (u32, u32), stats: &mut Stats) -> Result<(), String> {
    let mut entries: Vec<_> = fs::read_dir(from).map_err(|e| format!("{}: {}", from.display(), e))?.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let src = e.path();
        let dst = to.join(e.file_name());
        #[cfg_attr(unix, allow(unused_mut))]
        let mut meta = fs::symlink_metadata(&src).map_err(|e2| format!("{}: {}", src.display(), e2))?;
        if meta.file_type().is_symlink() {
            #[cfg(unix)]
            {
                let link = fs::read_link(&src).map_err(|e2| format!("{}: {}", src.display(), e2))?;
                std::os::unix::fs::symlink(link, &dst).map_err(|e2| format!("{}: {}", dst.display(), e2))?;
                continue;
            }
            // windows makes a symlink only with a privilege, so a file link is copied as the file
            #[cfg(not(unix))]
            {
                meta = fs::metadata(&src).map_err(|e2| format!("{}: {}", src.display(), e2))?;
                if meta.is_dir() {
                    return Err(format!("{}: a link to a directory cannot be copied on this host", src.display()));
                }
            }
        }
        if meta.is_dir() {
            fs::create_dir(&dst).map_err(|e2| format!("{}: {}", dst.display(), e2))?;
            walk(&src, &dst, want, stats)?;
        } else if meta.is_file() {
            let mut head = vec![0u8; (8 + MAX_SLICES * 20).min(meta.len() as usize)];
            {
                use std::io::Read;
                let mut f = fs::File::open(&src).map_err(|e2| format!("{}: {}", src.display(), e2))?;
                f.read_exact(&mut head).map_err(|e2| format!("{}: {}", src.display(), e2))?;
            }
            match find_slice(&head, want).map_err(|e2| format!("{}: {}", src.display(), e2))? {
                Some((off, size)) => {
                    if meta.len() > MAX_FAT_FILE || off.checked_add(size).map_or(true, |end| end as u64 > meta.len()) {
                        return Err(format!("{}: slice runs past the end of the file", src.display()));
                    }
                    let bytes = fs::read(&src).map_err(|e2| format!("{}: {}", src.display(), e2))?;
                    fs::write(&dst, &bytes[off..off + size]).map_err(|e2| format!("{}: {}", dst.display(), e2))?;
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::PermissionsExt;
                        fs::set_permissions(&dst, fs::Permissions::from_mode(meta.permissions().mode() & 0o7777)).map_err(|e2| format!("{}: {}", dst.display(), e2))?;
                    }
                    stats.thinned += 1;
                }
                None => {
                    // a hard link costs no space, copy when the cache is on another filesystem
                    if fs::hard_link(&src, &dst).is_err() {
                        fs::copy(&src, &dst).map_err(|e2| format!("{}: {}", src.display(), e2))?;
                    }
                    stats.linked += 1;
                }
            }
        }
    }
    Ok(())
}

pub fn run(from: &Path, to: &Path, arch: &str) -> Result<Stats, String> {
    let want = cpu(arch).ok_or_else(|| format!("sdk-thin: unknown arch {}, use armv6, armv7 or arm64", arch))?;
    if !from.join("usr/include/stdio.h").is_file() {
        return Err(format!("{} is not a usable sdk, usr/include/stdio.h is missing", from.display()));
    }
    let tmp = to.with_extension("tmp");
    for d in [&tmp] {
        match fs::remove_dir_all(d) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(format!("{}: {}", d.display(), e)),
        }
    }
    fs::create_dir_all(&tmp).map_err(|e| format!("{}: {}", tmp.display(), e))?;
    let mut stats = Stats::default();
    walk(from, &tmp, want, &mut stats)?;
    fs::write(tmp.join(STAMP), format!("{}\n{}\n", from.display(), arch)).map_err(|e| format!("{}: {}", STAMP, e))?;
    match fs::remove_dir_all(to) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(format!("{}: {}", to.display(), e)),
    }
    fs::rename(&tmp, to).map_err(|e| format!("{}: {}", to.display(), e))?;
    Ok(stats)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fat(slices: &[((u32, u32), &[u8])]) -> Vec<u8> {
        let mut b = Vec::new();
        b.extend_from_slice(&0xcafe_babeu32.to_be_bytes());
        b.extend_from_slice(&(slices.len() as u32).to_be_bytes());
        let mut off = 4096usize;
        let mut body = Vec::new();
        for ((t, s), data) in slices {
            for v in [*t, *s, off as u32, data.len() as u32, 12] {
                b.extend_from_slice(&v.to_be_bytes());
            }
            body.push((off, *data));
            off += 4096;
        }
        b.resize(4096, 0);
        for (o, d) in body {
            b.resize(o, 0);
            b.extend_from_slice(d);
        }
        b
    }

    fn tmp(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("meow-thin-{}-{}", tag, std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[cfg(unix)]
    #[test]
    fn fat_files_become_the_wanted_slice_and_the_rest_is_linked() {
        use std::os::unix::fs::{symlink, PermissionsExt};
        let d = tmp("sdk");
        let sdk = d.join("sdk");
        fs::create_dir_all(sdk.join("usr/include")).unwrap();
        fs::create_dir_all(sdk.join("usr/lib")).unwrap();
        fs::write(sdk.join("usr/include/stdio.h"), "// h").unwrap();
        fs::write(sdk.join("usr/lib/libx.dylib"), fat(&[((12, 6), b"six-slice"), ((12, 9), b"seven-slice")])).unwrap();
        fs::set_permissions(sdk.join("usr/lib/libx.dylib"), fs::Permissions::from_mode(0o755)).unwrap();
        fs::write(sdk.join("usr/lib/only7.dylib"), fat(&[((12, 9), b"seven")])).unwrap();
        symlink("libx.dylib", sdk.join("usr/lib/libx.1.dylib")).unwrap();
        let out = d.join("thin");
        let st = run(&sdk, &out, "armv6").unwrap();
        assert_eq!(st.thinned, 1);
        assert_eq!(fs::read(out.join("usr/lib/libx.dylib")).unwrap(), b"six-slice");
        assert_eq!(fs::metadata(out.join("usr/lib/libx.dylib")).unwrap().permissions().mode() & 0o777, 0o755);
        // no armv6 slice there, the file is left as it was
        assert_eq!(fs::read(out.join("usr/lib/only7.dylib")).unwrap(), fat(&[((12, 9), b"seven")]));
        assert_eq!(fs::read_link(out.join("usr/lib/libx.1.dylib")).unwrap(), Path::new("libx.dylib"));
        assert!(out.join(STAMP).is_file());
        // a second run replaces the first copy
        run(&sdk, &out, "armv7").unwrap();
        assert_eq!(fs::read(out.join("usr/lib/libx.dylib")).unwrap(), b"seven-slice");
        fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn bad_input_is_refused() {
        let d = tmp("bad");
        assert!(run(&d, &d.join("o"), "armv6").unwrap_err().contains("not a usable sdk"));
        assert!(run(&d, &d.join("o"), "mips").unwrap_err().contains("unknown arch"));
        let sdk = d.join("sdk");
        fs::create_dir_all(sdk.join("usr/include")).unwrap();
        fs::write(sdk.join("usr/include/stdio.h"), "").unwrap();
        // a slice that claims bytes beyond the file
        let mut broken = fat(&[((12, 6), b"abc")]);
        broken[8 + 12..8 + 16].copy_from_slice(&999_999u32.to_be_bytes());
        fs::write(sdk.join("broken"), broken).unwrap();
        assert!(run(&sdk, &d.join("o"), "armv6").unwrap_err().contains("past the end"));
        fs::remove_dir_all(&d).unwrap();
    }
}
