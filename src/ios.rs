// which cpu slices run on which ios release. the ranges are the last release the
// oldest device of that cpu received (armv6: iphone 3g and ipod touch 2g stop at
// 4.2.1) and the first release that dropped 32-bit apps (ios 11)

pub type Version = (u32, u32);

pub struct ArchInfo {
    pub name: &'static str,
    // first release with a device that runs the slice
    pub first: Version,
    // last release that runs it, none while still supported
    pub last: Option<Version>,
    pub devices: &'static str,
}

pub const ARCHS: [ArchInfo; 4] = [
    ArchInfo { name: "armv6", first: (2, 0), last: Some((4, 2)), devices: "iphone 2g/3g, ipod touch 1g/2g" },
    ArchInfo { name: "armv7", first: (3, 0), last: Some((10, 3)), devices: "iphone 3gs to 5c, ipod touch 3g to 5g, ipad 1 to 4, ipad mini 1" },
    ArchInfo { name: "arm64", first: (7, 0), last: None, devices: "iphone 5s and later, ipad air and later" },
    ArchInfo { name: "arm64e", first: (12, 0), last: None, devices: "a12 and later" },
];

pub fn arch(name: &str) -> Option<&'static ArchInfo> {
    ARCHS.iter().find(|a| a.name == name)
}

// "5", "5.1" and "4.2.1" all parse, the patch level never changes what runs
pub fn parse(s: &str) -> Option<Version> {
    let mut it = s.split('.');
    let major: u32 = it.next()?.parse().ok()?;
    let minor: u32 = match it.next() {
        Some(m) => m.parse().ok()?,
        None => 0,
    };
    if let Some(p) = it.next() {
        p.parse::<u32>().ok()?;
    }
    if it.next().is_some() || major == 0 || major > 99 {
        return None;
    }
    Some((major, minor))
}

pub fn text(v: Version) -> String {
    format!("{}.{}", v.0, v.1)
}

pub fn runs(arch_name: &str, v: Version) -> bool {
    arch(arch_name).is_some_and(|a| v >= a.first && a.last.map_or(true, |l| v <= l))
}

pub fn archs_running(v: Version) -> Vec<&'static str> {
    ARCHS.iter().filter(|a| runs(a.name, v)).map(|a| a.name).collect()
}

// the lowest deployment target a slice can honestly carry: below it no device
// exists, above its last release no device is left
pub fn check_min(arch_name: &str, min: Version) -> Result<(), String> {
    let a = arch(arch_name).ok_or_else(|| format!("unknown arch {}", arch_name))?;
    if let Some(last) = a.last {
        if min > last {
            return Err(format!(
                "{} devices stop at ios {}, min_ios {} leaves no device for the {} slice",
                a.name,
                if a.name == "armv6" { "4.2.1".to_string() } else { "10.3.3".to_string() },
                text(min),
                a.name
            ));
        }
    }
    Ok(())
}

// the deployment target of a slice: the project minimum, raised to the first
// release that has a device for the cpu
pub fn slice_min(arch_name: &str, min: Version) -> Version {
    arch(arch_name).map_or(min, |a| min.max(a.first))
}

// objective-c arc needs the ios 4 runtime
pub fn check_arc(min: Version) -> Result<(), String> {
    if min < (4, 0) {
        return Err(format!("arc needs min_ios 4.0 or later, min_ios is {}", text(min)));
    }
    Ok(())
}

const MILESTONES: [Version; 20] = [
    (2, 0), (3, 0), (3, 1), (4, 0), (4, 2), (4, 3), (5, 0), (5, 1), (6, 0), (7, 0),
    (8, 0), (9, 0), (10, 0), (10, 3), (11, 0), (12, 0), (13, 0), (14, 0), (15, 0), (16, 0),
];

pub fn table() -> Vec<(Version, Vec<&'static str>)> {
    MILESTONES.iter().map(|&v| (v, archs_running(v))).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_parse() {
        assert_eq!(parse("5"), Some((5, 0)));
        assert_eq!(parse("4.2.1"), Some((4, 2)));
        assert_eq!(parse("10.3"), Some((10, 3)));
        for bad in ["", "x", "5.", "5.x", "1.2.3.4", "0", "5.1.z", "-1"] {
            assert_eq!(parse(bad), None, "{:?}", bad);
        }
    }

    #[test]
    fn each_release_runs_the_right_slices() {
        assert_eq!(archs_running((3, 0)), ["armv6", "armv7"]);
        assert_eq!(archs_running((4, 2)), ["armv6", "armv7"]);
        assert_eq!(archs_running((5, 0)), ["armv7"]);
        assert_eq!(archs_running((7, 0)), ["armv7", "arm64"]);
        assert_eq!(archs_running((10, 3)), ["armv7", "arm64"]);
        // ios 11 dropped 32-bit apps
        assert_eq!(archs_running((11, 0)), ["arm64"]);
        assert_eq!(archs_running((12, 0)), ["arm64", "arm64e"]);
        assert!(archs_running((1, 0)).is_empty());
    }

    #[test]
    fn a_minimum_with_no_device_is_refused() {
        assert!(check_min("armv6", (4, 2)).is_ok());
        assert!(check_min("armv6", (5, 0)).unwrap_err().contains("4.2.1"));
        assert!(check_min("armv7", (11, 0)).is_err());
        assert!(check_min("arm64", (16, 0)).is_ok());
        assert!(check_min("sparc", (1, 0)).is_err());
    }

    #[test]
    fn slice_minimums_are_raised_to_the_first_device() {
        assert_eq!(slice_min("armv7", (2, 0)), (3, 0));
        assert_eq!(slice_min("arm64", (5, 0)), (7, 0));
        assert_eq!(slice_min("arm64e", (5, 0)), (12, 0));
        assert_eq!(slice_min("armv6", (3, 0)), (3, 0));
        assert!(check_arc((3, 0)).is_err());
        assert!(check_arc((4, 0)).is_ok());
    }
}
