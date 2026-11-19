// gnu tar writer for the deb members, so a file mode comes from the package model
// and not from a filesystem that may have none, and no host tar is needed
// headers match gnu tar --format=gnu byte for byte, old debs stay comparable

const BLOCK: usize = 512;
// gnu tar pads the archive to its default blocking factor of 20
const RECORD: usize = 10_240;
const NAME_FIELD: usize = 100;
const MAX_OCTAL_11: u64 = 0o77_777_777_777;

pub struct Tar {
    out: Vec<u8>,
    mtime: u64,
}

fn octal(field: &mut [u8], value: u64) {
    let digits = field.len() - 1;
    let text = format!("{:0width$o}", value, width = digits);
    field[..digits].copy_from_slice(text.as_bytes());
    field[digits] = 0;
}

fn header(name: &[u8], mode: u32, size: u64, mtime: u64, kind: u8, link: &[u8]) -> Result<[u8; BLOCK], String> {
    if size > MAX_OCTAL_11 {
        return Err("a tar member over 8 GiB needs a format meow does not write".to_string());
    }
    if mtime > MAX_OCTAL_11 {
        return Err(format!("mtime {} does not fit a tar header", mtime));
    }
    let mut h = [0u8; BLOCK];
    let n = name.len().min(NAME_FIELD);
    h[..n].copy_from_slice(&name[..n]);
    octal(&mut h[100..108], (mode & 0o7777) as u64);
    octal(&mut h[108..116], 0);
    octal(&mut h[116..124], 0);
    octal(&mut h[124..136], size);
    octal(&mut h[136..148], mtime);
    h[156] = kind;
    let l = link.len().min(NAME_FIELD);
    h[157..157 + l].copy_from_slice(&link[..l]);
    h[257..265].copy_from_slice(b"ustar  \0");
    // the checksum is taken with its own field read as spaces
    h[148..156].copy_from_slice(b"        ");
    let sum: u32 = h.iter().map(|&b| b as u32).sum();
    let text = format!("{:06o}", sum);
    h[148..154].copy_from_slice(text.as_bytes());
    h[154] = 0;
    h[155] = b' ';
    Ok(h)
}

impl Tar {
    pub fn new(mtime: u64) -> Tar {
        Tar { out: Vec::new(), mtime }
    }

    // names over 100 bytes go in a preceding member of kind L or K, as gnu tar does
    fn long(&mut self, kind: u8, text: &[u8]) -> Result<(), String> {
        let body = text.len() as u64 + 1;
        self.out.extend_from_slice(&header(b"././@LongLink", 0o644, body, 0, kind, b"")?);
        self.out.extend_from_slice(text);
        self.out.push(0);
        self.pad();
        Ok(())
    }

    fn pad(&mut self) {
        let rest = self.out.len() % BLOCK;
        if rest != 0 {
            self.out.resize(self.out.len() + BLOCK - rest, 0);
        }
    }

    fn member(&mut self, name: &str, mode: u32, kind: u8, link: &str, data: &[u8]) -> Result<(), String> {
        if link.len() > NAME_FIELD {
            self.long(b'K', link.as_bytes())?;
        }
        if name.len() > NAME_FIELD {
            self.long(b'L', name.as_bytes())?;
        }
        self.out.extend_from_slice(&header(name.as_bytes(), mode, data.len() as u64, self.mtime, kind, link.as_bytes())?);
        self.out.extend_from_slice(data);
        self.pad();
        Ok(())
    }

    pub fn dir(&mut self, name: &str, mode: u32) -> Result<(), String> {
        if name.ends_with('/') {
            self.member(name, mode, b'5', "", &[])
        } else {
            self.member(&format!("{}/", name), mode, b'5', "", &[])
        }
    }

    pub fn file(&mut self, name: &str, mode: u32, data: &[u8]) -> Result<(), String> {
        self.member(name, mode, b'0', "", data)
    }

    // a symlink carries mode 777 on the systems the packages come from
    pub fn symlink(&mut self, name: &str, target: &str) -> Result<(), String> {
        self.member(name, 0o777, b'2', target, &[])
    }

    pub fn finish(mut self) -> Vec<u8> {
        self.out.resize(self.out.len() + 2 * BLOCK, 0);
        let rest = self.out.len() % RECORD;
        if rest != 0 {
            self.out.resize(self.out.len() + RECORD - rest, 0);
        }
        self.out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn field(h: &[u8], from: usize, to: usize) -> String {
        String::from_utf8_lossy(&h[from..to]).trim_end_matches('\0').to_string()
    }

    #[test]
    fn a_file_header_has_the_gnu_layout() {
        let mut t = Tar::new(1_577_836_800);
        t.file("./f", 0o755, b"abc").unwrap();
        let tar = t.finish();
        let h = &tar[..BLOCK];
        assert_eq!(field(h, 0, 100), "./f");
        assert_eq!(&h[100..108], b"0000755\0");
        assert_eq!(&h[124..136], b"00000000003\0");
        assert_eq!(&h[136..148], b"13602760400\0");
        assert_eq!(h[156], b'0');
        assert_eq!(&h[257..265], b"ustar  \0");
        // the stored checksum is the byte sum with the field as spaces
        let mut copy = h.to_vec();
        copy[148..156].copy_from_slice(b"        ");
        let want: u32 = copy.iter().map(|&b| b as u32).sum();
        assert_eq!(u32::from_str_radix(field(h, 148, 154).trim(), 8).unwrap(), want);
        assert_eq!(&tar[BLOCK..BLOCK + 3], b"abc");
        assert_eq!(tar.len(), RECORD);
    }

    #[test]
    fn names_over_100_bytes_get_a_longlink_member() {
        let long = format!("./{}", "d".repeat(120));
        let mut t = Tar::new(0);
        t.file(&long, 0o644, b"z").unwrap();
        let tar = t.finish();
        assert_eq!(field(&tar, 0, 13), "././@LongLink");
        assert_eq!(tar[156], b'L');
        assert_eq!(&tar[124..136], format!("{:011o}\0", long.len() + 1).as_bytes());
        assert_eq!(&tar[BLOCK..BLOCK + long.len()], long.as_bytes());
        assert_eq!(tar[BLOCK + long.len()], 0);
        // the real header follows after the name block, with the name cut at 100 bytes
        let real = &tar[2 * BLOCK..3 * BLOCK];
        assert_eq!(field(real, 0, 100), long[..100]);
        assert_eq!(real[156], b'0');
    }

    #[test]
    fn exactly_100_bytes_need_no_extra_member() {
        let name = "n".repeat(100);
        let mut t = Tar::new(0);
        t.file(&name, 0o644, b"").unwrap();
        let tar = t.finish();
        assert_eq!(tar[156], b'0');
        let mut t = Tar::new(0);
        t.file(&"n".repeat(101), 0o644, b"").unwrap();
        assert_eq!(t.finish()[156], b'L');
    }

    #[test]
    fn a_symlink_with_a_long_target_puts_the_k_member_first() {
        let target = "l".repeat(130);
        let mut t = Tar::new(5);
        t.symlink("./lnk", &target).unwrap();
        let tar = t.finish();
        assert_eq!(tar[156], b'K');
        let main = &tar[2 * BLOCK..3 * BLOCK];
        assert_eq!(main[156], b'2');
        assert_eq!(&main[100..108], b"0000777\0");
        assert_eq!(field(main, 157, 257), target[..100]);
    }

    #[test]
    fn a_directory_name_gets_its_slash_and_size_zero() {
        let mut t = Tar::new(0);
        t.dir("./usr", 0o755).unwrap();
        t.dir("./bin/", 0o755).unwrap();
        let tar = t.finish();
        assert_eq!(field(&tar, 0, 100), "./usr/");
        assert_eq!(field(&tar, BLOCK, BLOCK + 100), "./bin/");
        assert_eq!(tar[156], b'5');
        assert_eq!(&tar[124..136], b"00000000000\0");
    }

    #[test]
    fn oversized_members_are_refused() {
        assert!(header(b"x", 0o644, MAX_OCTAL_11 + 1, 0, b'0', b"").is_err());
        assert!(header(b"x", 0o644, 0, MAX_OCTAL_11 + 1, b'0', b"").is_err());
    }

    #[test]
    fn archives_end_with_two_zero_blocks_padded_to_a_record() {
        let mut t = Tar::new(0);
        t.file("a", 0o644, &vec![1u8; 10_000]).unwrap();
        let tar = t.finish();
        assert_eq!(tar.len() % RECORD, 0);
        let used = BLOCK + 10_000usize.div_ceil(BLOCK) * BLOCK;
        assert!(tar[used..].iter().all(|&b| b == 0));
        assert_eq!(tar.len(), 2 * RECORD);
    }
}
