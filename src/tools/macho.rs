use std::process::Command;

// reads mach-o files: the code signature of each slice, the deployment target, the
// dylibs it links. signing is checked by reading the file back, ldid exits 0 on some
// failures, and the release checks (slices, minimum ios, allowed libraries) use the
// same reader

const LC_CODE_SIGNATURE: u32 = 0x1d;
const LC_VERSION_MIN_IPHONEOS: u32 = 0x25;
const LC_BUILD_VERSION: u32 = 0x32;
const DYLIB_COMMANDS: [u32; 5] = [0xc, 0x8000_0018, 0x8000_001f, 0x20, 0x8000_0023];
const CSMAGIC_EMBEDDED_SIGNATURE: u32 = 0xfade_0cc0;
const MAX_SLICES: usize = 16;
const MAX_LOAD_COMMANDS: u32 = 4096;

#[derive(Debug, PartialEq, Eq)]
pub struct SliceInfo {
    pub cpu: String,
    pub signed: bool,
    // deployment target as (major, minor)
    pub min_os: Option<(u32, u32)>,
    // install names of every linked dylib
    pub dylibs: Vec<String>,
}

fn be32(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(b.get(at..at.checked_add(4)?)?.try_into().ok()?))
}

fn le32(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(at..at.checked_add(4)?)?.try_into().ok()?))
}

fn be64(b: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_be_bytes(b.get(at..at.checked_add(8)?)?.try_into().ok()?))
}

fn cpu_name(cputype: u32, subtype: u32) -> String {
    let sub = subtype & 0x00ff_ffff;
    match (cputype, sub) {
        (12, 6) => "armv6".to_string(),
        (12, 9) => "armv7".to_string(),
        (12, 11) => "armv7s".to_string(),
        (0x0100_000c, 2) => "arm64e".to_string(),
        (0x0100_000c, _) => "arm64".to_string(),
        (7, _) => "i386".to_string(),
        (0x0100_0007, _) => "x86_64".to_string(),
        _ => format!("cpu {:#x}/{}", cputype, sub),
    }
}

fn thin_slice(file: &[u8], off: usize, size: usize) -> Result<SliceInfo, String> {
    let end = off.checked_add(size).filter(|&e| e <= file.len()).ok_or("slice runs past the end of the file")?;
    let s = &file[off..end];
    let magic = le32(s, 0).ok_or("truncated mach-o header")?;
    let header = match magic {
        0xfeed_face => 28,
        0xfeed_facf => 32,
        _ => return Err(format!("not a mach-o slice, magic {:#x}", magic)),
    };
    let cputype = le32(s, 4).ok_or("truncated mach-o header")?;
    let subtype = le32(s, 8).ok_or("truncated mach-o header")?;
    let ncmds = le32(s, 16).ok_or("truncated mach-o header")?;
    if ncmds > MAX_LOAD_COMMANDS {
        return Err(format!("{} load commands, refusing to walk them", ncmds));
    }
    let mut at = header;
    let mut signed = false;
    let mut min_os = None;
    let mut dylibs = Vec::new();
    for _ in 0..ncmds {
        let cmd = le32(s, at).ok_or("truncated load command")?;
        let cmdsize = le32(s, at + 4).ok_or("truncated load command")? as usize;
        if cmdsize < 8 {
            return Err("load command smaller than its header".to_string());
        }
        if cmd == LC_CODE_SIGNATURE {
            let dataoff = le32(s, at + 8).ok_or("truncated LC_CODE_SIGNATURE")? as usize;
            let datasize = le32(s, at + 12).ok_or("truncated LC_CODE_SIGNATURE")? as usize;
            let blob_end = dataoff.checked_add(datasize).ok_or("signature range overflows")?;
            if datasize == 0 || blob_end > s.len() {
                return Err("LC_CODE_SIGNATURE points outside the slice".to_string());
            }
            if be32(s, dataoff) != Some(CSMAGIC_EMBEDDED_SIGNATURE) {
                return Err("signature blob has a bad magic".to_string());
            }
            signed = true;
        }
        if cmd == LC_VERSION_MIN_IPHONEOS || cmd == LC_BUILD_VERSION {
            // version_min: version at +8, build_version: platform at +8, minos at +12
            let v = le32(s, at + if cmd == LC_BUILD_VERSION { 12 } else { 8 }).ok_or("truncated version command")?;
            min_os = Some((v >> 16, (v >> 8) & 0xff));
        }
        if DYLIB_COMMANDS.contains(&cmd) {
            let off = le32(s, at + 8).ok_or("truncated dylib command")? as usize;
            if off >= cmdsize {
                return Err("dylib name outside its load command".to_string());
            }
            let raw = s.get(at + off..at + cmdsize).ok_or("truncated dylib name")?;
            let end = raw.iter().position(|&b| b == 0).unwrap_or(raw.len());
            dylibs.push(String::from_utf8_lossy(&raw[..end]).into_owned());
        }
        at = at.checked_add(cmdsize).ok_or("load command size overflows")?;
    }
    Ok(SliceInfo { cpu: cpu_name(cputype, subtype), signed, min_os, dylibs })
}

pub fn inspect(file: &[u8]) -> Result<Vec<SliceInfo>, String> {
    let magic = be32(file, 0).ok_or("file is too short for a mach-o")?;
    match magic {
        0xcafe_babe | 0xcafe_babf => {
            let wide = magic == 0xcafe_babf;
            let n = be32(file, 4).ok_or("truncated fat header")? as usize;
            if n == 0 || n > MAX_SLICES {
                return Err(format!("fat header lists {} slices", n));
            }
            let stride = if wide { 32 } else { 20 };
            let mut out = Vec::new();
            for i in 0..n {
                let e = 8 + i * stride;
                let (off, size) = if wide {
                    (be64(file, e + 8).ok_or("truncated fat arch")?, be64(file, e + 16).ok_or("truncated fat arch")?)
                } else {
                    (be32(file, e + 8).ok_or("truncated fat arch")? as u64, be32(file, e + 12).ok_or("truncated fat arch")? as u64)
                };
                let (off, size) = (usize::try_from(off).map_err(|_| "fat offset too large")?, usize::try_from(size).map_err(|_| "fat size too large")?);
                out.push(thin_slice(file, off, size).map_err(|e| format!("slice {}: {}", i, e))?);
            }
            Ok(out)
        }
        _ => Ok(vec![thin_slice(file, 0, file.len())?]),
    }
}

// the <key> names of a plist, enough to compare entitlements without a plist parser
pub fn plist_keys(text: &str) -> Vec<String> {
    let mut keys = Vec::new();
    let mut rest = text;
    while let Some(i) = rest.find("<key>") {
        let tail = &rest[i + 5..];
        let Some(j) = tail.find("</key>") else { break };
        keys.push(tail[..j].trim().to_string());
        rest = &tail[j + 6..];
    }
    keys
}

// what a release binary has to look like, from the config of a [verify] table
#[derive(Debug, Default)]
pub struct Expect {
    pub slices: Vec<String>,
    // no slice may need a newer release than this, raised per cpu by the caller
    pub max_min_os: Vec<(String, (u32, u32))>,
    pub signed: bool,
    // when set, every linked dylib has to start with one of these
    pub allow_dylibs: Vec<String>,
    pub forbid_dylibs: Vec<String>,
}

pub fn check_release(path: &str, file: &[u8], e: &Expect) -> Result<String, String> {
    let slices = inspect(file).map_err(|x| format!("{}: {}", path, x))?;
    let have: Vec<&str> = slices.iter().map(|s| s.cpu.as_str()).collect();
    if !e.slices.is_empty() {
        let mut want: Vec<&str> = e.slices.iter().map(|s| s.as_str()).collect();
        let mut got = have.clone();
        want.sort_unstable();
        got.sort_unstable();
        if want != got {
            return Err(format!("{}: slices are {}, expected {}", path, have.join(" "), e.slices.join(" ")));
        }
    }
    for s in &slices {
        if e.signed && !s.signed {
            return Err(format!("{}: slice {} has no code signature", path, s.cpu));
        }
        if let (Some(have_min), Some((_, limit))) = (s.min_os, e.max_min_os.iter().find(|(c, _)| *c == s.cpu)) {
            if have_min > *limit {
                return Err(format!("{}: slice {} needs ios {}.{}, the limit is {}.{}", path, s.cpu, have_min.0, have_min.1, limit.0, limit.1));
            }
        }
        for d in &s.dylibs {
            if let Some(bad) = e.forbid_dylibs.iter().find(|f| d.starts_with(f.as_str())) {
                return Err(format!("{}: slice {} links {} which is forbidden ({})", path, s.cpu, d, bad));
            }
            if !e.allow_dylibs.is_empty() && !e.allow_dylibs.iter().any(|a| d.starts_with(a.as_str())) {
                return Err(format!("{}: slice {} links {} which is not under an allowed path", path, s.cpu, d));
            }
        }
    }
    Ok(format!("{}: {}", path, have.join(" ")))
}

// undefined symbols of every slice, read with nm, against a list that must not appear
pub fn check_symbols(path: &str, file: &[u8], nm: &str, forbidden: &[String]) -> Result<(), String> {
    let slices = inspect(file).map_err(|e| format!("{}: {}", path, e))?;
    let fat = slices.len() > 1;
    let mut hits: Vec<String> = Vec::new();
    for s in &slices {
        let mut cmd = Command::new(nm);
        cmd.arg("-u");
        if fat {
            cmd.arg("-arch").arg(&s.cpu);
        }
        let out = cmd.arg(path).output().map_err(|e| format!("cannot run {}: {}", nm, e))?;
        if !out.status.success() {
            return Err(format!("{} -u failed on {}: {}", nm, path, String::from_utf8_lossy(&out.stderr).trim()));
        }
        for line in String::from_utf8_lossy(&out.stdout).lines() {
            if line.ends_with(':') {
                continue;
            }
            let Some(name) = line.split_whitespace().last() else { continue };
            let bare = name.strip_prefix('_').unwrap_or(name);
            if forbidden.iter().any(|f| f == name || f == bare) {
                hits.push(format!("{} ({})", bare, s.cpu));
            }
        }
    }
    if hits.is_empty() {
        Ok(())
    } else {
        hits.sort();
        hits.dedup();
        Err(format!("{}: undefined symbols that are not allowed: {}", path, hits.join(", ")))
    }
}

// external symbols every slice has to define
pub fn check_defined(path: &str, file: &[u8], nm: &str, required: &[String]) -> Result<(), String> {
    let slices = inspect(file).map_err(|e| format!("{}: {}", path, e))?;
    let fat = slices.len() > 1;
    for s in &slices {
        let mut cmd = Command::new(nm);
        cmd.args(["-g", "-U"]);
        if fat {
            cmd.arg("-arch").arg(&s.cpu);
        }
        let out = cmd.arg(path).output().map_err(|e| format!("cannot run {}: {}", nm, e))?;
        if !out.status.success() {
            return Err(format!("{} -g -U failed on {}: {}", nm, path, String::from_utf8_lossy(&out.stderr).trim()));
        }
        let text = String::from_utf8_lossy(&out.stdout).into_owned();
        let have: Vec<&str> = text.lines().filter(|l| !l.ends_with(':')).filter_map(|l| l.split_whitespace().last()).collect();
        for r in required {
            if !have.iter().any(|h| *h == r || h.strip_prefix('_') == Some(r.as_str())) {
                return Err(format!("{}: slice {} does not define {}", path, s.cpu, r));
            }
        }
    }
    Ok(())
}

pub fn verify(path: &str, ldid: Option<&str>, entitlements: Option<&str>) -> Result<String, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {}", path, e))?;
    let slices = inspect(&bytes).map_err(|e| format!("{}: {}", path, e))?;
    let unsigned: Vec<&str> = slices.iter().filter(|s| !s.signed).map(|s| s.cpu.as_str()).collect();
    if !unsigned.is_empty() {
        return Err(format!("{}: no code signature in {}", path, unsigned.join(", ")));
    }
    if let Some(ents) = entitlements {
        let ldid = ldid.ok_or("entitlements check needs --ldid")?;
        let wanted = plist_keys(&std::fs::read_to_string(ents).map_err(|e| format!("{}: {}", ents, e))?);
        let out = Command::new(ldid)
            .arg("-e")
            .arg(path)
            .output()
            .map_err(|e| format!("cannot run {}: {}", ldid, e))?;
        if !out.status.success() {
            return Err(format!("{} -e {} failed: {}", ldid, path, String::from_utf8_lossy(&out.stderr).trim()));
        }
        let have = String::from_utf8_lossy(&out.stdout);
        let missing: Vec<&String> = wanted.iter().filter(|k| !have.contains(&format!("<key>{}</key>", k))).collect();
        if !missing.is_empty() {
            let list: Vec<&str> = missing.iter().map(|s| s.as_str()).collect();
            return Err(format!("{}: entitlements missing from the signature: {}", path, list.join(", ")));
        }
    }
    let cpus: Vec<&str> = slices.iter().map(|s| s.cpu.as_str()).collect();
    Ok(format!("{}: signed {}", path, cpus.join(" ")))
}

#[cfg(test)]
mod tests {
    use super::*;

    // a 64-bit arm mach-o header with one LC_CODE_SIGNATURE whose blob follows
    fn thin(cputype: u32, subtype: u32, signed: bool) -> Vec<u8> {
        let mut b = Vec::new();
        let ncmds: u32 = if signed { 1 } else { 0 };
        for v in [0xfeed_facfu32, cputype, subtype, 2, ncmds, if signed { 16 } else { 0 }, 0, 0] {
            b.extend_from_slice(&v.to_le_bytes());
        }
        if signed {
            let dataoff = 32 + 16;
            for v in [LC_CODE_SIGNATURE, 16, dataoff, 12] {
                b.extend_from_slice(&v.to_le_bytes());
            }
            b.extend_from_slice(&CSMAGIC_EMBEDDED_SIGNATURE.to_be_bytes());
            b.extend_from_slice(&[0, 0, 0, 12, 0, 0, 0, 0]);
        }
        b
    }

    fn fat(slices: &[Vec<u8>]) -> Vec<u8> {
        let mut b = Vec::new();
        b.extend_from_slice(&0xcafe_babeu32.to_be_bytes());
        b.extend_from_slice(&(slices.len() as u32).to_be_bytes());
        let mut off = 8 + 20 * slices.len();
        for s in slices {
            let cputype = u32::from_le_bytes(s[4..8].try_into().unwrap());
            let subtype = u32::from_le_bytes(s[8..12].try_into().unwrap());
            for v in [cputype, subtype, off as u32, s.len() as u32, 14] {
                b.extend_from_slice(&v.to_be_bytes());
            }
            off += s.len();
        }
        for s in slices {
            b.extend_from_slice(s);
        }
        b
    }

    #[test]
    fn signed_thin_and_fat_files_are_read() {
        let a64 = thin(0x0100_000c, 0, true);
        let info = inspect(&a64).unwrap();
        assert_eq!(info.len(), 1);
        assert_eq!((info[0].cpu.as_str(), info[0].signed), ("arm64", true));
        let f = fat(&[thin(0x0100_000c, 0, true), thin(0x0100_000c, 2, true)]);
        let info = inspect(&f).unwrap();
        assert_eq!(info.iter().map(|s| s.cpu.as_str()).collect::<Vec<_>>(), ["arm64", "arm64e"]);
        assert!(info.iter().all(|s| s.signed));
    }

    #[test]
    fn an_unsigned_slice_is_named() {
        let f = fat(&[thin(0x0100_000c, 0, true), thin(0x0100_000c, 2, false)]);
        let info = inspect(&f).unwrap();
        assert!(info[0].signed && !info[1].signed);
    }

    #[test]
    fn damaged_input_is_an_error_not_a_panic() {
        assert!(inspect(&[]).is_err());
        assert!(inspect(&[0xca, 0xfe, 0xba, 0xbe]).is_err());
        let mut bad = thin(0x0100_000c, 0, true);
        // point the signature blob past the end of the slice
        bad[32 + 8..32 + 12].copy_from_slice(&4000u32.to_le_bytes());
        assert!(inspect(&bad).is_err());
        let mut blob = thin(0x0100_000c, 0, true);
        let at = 48;
        blob[at] = 0;
        assert!(inspect(&blob).unwrap_err().contains("bad magic"));
        let mut huge = thin(0x0100_000c, 0, false);
        huge[16..20].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(inspect(&huge).is_err());
        // a fat header that claims many slices than the cap
        let mut many = fat(&[thin(0x0100_000c, 0, true)]);
        many[4..8].copy_from_slice(&1000u32.to_be_bytes());
        assert!(inspect(&many).is_err());
    }

    // a thin armv7 header with a version_min and one dylib command
    fn with_load_commands(cmds: &[Vec<u8>]) -> Vec<u8> {
        let size: usize = cmds.iter().map(|c| c.len()).sum();
        let mut b = Vec::new();
        for v in [0xfeed_faceu32, 12, 9, 2, cmds.len() as u32, size as u32, 0] {
            b.extend_from_slice(&v.to_le_bytes());
        }
        for c in cmds {
            b.extend_from_slice(c);
        }
        b
    }

    fn cmd(id: u32, body: &[u8]) -> Vec<u8> {
        let mut c = Vec::new();
        c.extend_from_slice(&id.to_le_bytes());
        c.extend_from_slice(&((8 + body.len()) as u32).to_le_bytes());
        c.extend_from_slice(body);
        c
    }

    #[test]
    fn deployment_target_and_dylibs_are_read() {
        // 5.1.0 is 0x00050100, the sdk field follows it
        let version = cmd(LC_VERSION_MIN_IPHONEOS, &[0x00, 0x01, 0x05, 0x00, 0x00, 0x00, 0x06, 0x00]);
        let mut dy = vec![24u8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
        dy.extend_from_slice(b"/usr/lib/libSystem.B.dylib      ");
        let weak = {
            let mut w = vec![24u8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
            w.extend_from_slice(b"/System/Library/Frameworks/Social.framework/Social      ");
            cmd(0x8000_0018, &w)
        };
        let f = with_load_commands(&[version, cmd(0xc, &dy), weak]);
        let info = inspect(&f).unwrap();
        assert_eq!(info[0].cpu, "armv7");
        assert_eq!(info[0].min_os, Some((5, 1)));
        assert_eq!(info[0].dylibs, ["/usr/lib/libSystem.B.dylib", "/System/Library/Frameworks/Social.framework/Social"]);
        // build_version keeps minos at +12
        let bv = cmd(LC_BUILD_VERSION, &[2, 0, 0, 0, 0x00, 0x00, 0x0c, 0x00, 0, 0, 0, 0, 0, 0, 0, 0]);
        assert_eq!(inspect(&with_load_commands(&[bv])).unwrap()[0].min_os, Some((12, 0)));
        // a dylib name offset beyond its command is damage
        let bad = cmd(0xc, &[200, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        assert!(inspect(&with_load_commands(&[bad])).is_err());
    }

    #[test]
    fn plist_keys_are_listed() {
        let t = "<dict>\n<key>platform-application</key><true/>\n<key> task_for_pid-allow </key>\n</dict>";
        assert_eq!(plist_keys(t), ["platform-application", "task_for_pid-allow"]);
        assert!(plist_keys("<dict/>").is_empty());
    }
}
