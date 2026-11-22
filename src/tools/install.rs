use std::path::Path;
use std::process::{Command, Stdio};

// copies a deb to a jailbroken device and installs it with dpkg. the password, when the
// device has no key set up, comes from an environment variable and goes to sshpass in
// its environment, never on a command line where ps would show it

#[derive(Debug, Default)]
pub struct Spec {
    pub host: String,
    pub user: String,
    pub port: u16,
    pub deb: String,
    pub password_env: Option<String>,
    // algorithms that old openssh servers on ios 5 to 10 still offer
    pub legacy: bool,
}

pub fn parse_args(args: &[String]) -> Result<Spec, String> {
    let mut s = Spec { user: "root".to_string(), port: 22, ..Default::default() };
    let mut i = 0;
    while i < args.len() {
        let val = |i: &mut usize| -> Result<String, String> {
            *i += 1;
            args.get(*i).cloned().ok_or_else(|| format!("install: {} needs a value", args[*i - 1]))
        };
        match args[i].as_str() {
            "--host" => s.host = val(&mut i)?,
            "--user" => s.user = val(&mut i)?,
            "--port" => s.port = val(&mut i)?.parse().map_err(|_| "install: --port needs a number".to_string())?,
            "--password-env" => s.password_env = Some(val(&mut i)?),
            "--legacy" => s.legacy = true,
            a if a.starts_with('-') => return Err(format!("install: unknown option {}", a)),
            a if s.deb.is_empty() => s.deb = a.to_string(),
            _ => return Err("install takes one deb".to_string()),
        }
        i += 1;
    }
    if s.host.is_empty() || s.deb.is_empty() {
        return Err("usage: meow install --host H [--user root] [--port 22] [--password-env VAR] [--legacy] FILE.deb".to_string());
    }
    // host and user go into a command line for ssh, so only plain names are accepted
    let plain = |v: &str| !v.is_empty() && v.chars().all(|c| c.is_ascii_alphanumeric() || ".-_".contains(c));
    if !plain(&s.host) || !plain(&s.user) {
        return Err("install: host and user may only hold letters, digits and . - _".to_string());
    }
    if !s.deb.ends_with(".deb") || !Path::new(&s.deb).is_file() {
        return Err(format!("install: {} is not a .deb file", s.deb));
    }
    Ok(s)
}

fn ssh_options(s: &Spec) -> Vec<String> {
    let mut o: Vec<String> = ["-o", "StrictHostKeyChecking=accept-new", "-o", "ConnectTimeout=10", "-o", "NumberOfPasswordPrompts=1"].iter().map(|x| x.to_string()).collect();
    if s.legacy {
        for x in ["HostKeyAlgorithms=+ssh-rsa", "PubkeyAcceptedAlgorithms=+ssh-rsa", "KexAlgorithms=+diffie-hellman-group14-sha1", "Ciphers=+aes128-cbc,aes256-cbc"] {
            o.extend(["-o".to_string(), x.to_string()]);
        }
    }
    o
}

fn command(s: &Spec, tool: &str, port_flag: &str, args: Vec<String>) -> Result<Command, String> {
    let mut c = match &s.password_env {
        Some(var) => {
            let pw = std::env::var(var).map_err(|_| format!("install: {} is not set", var))?;
            let mut c = Command::new("sshpass");
            c.arg("-e").arg(tool).env("SSHPASS", pw);
            c
        }
        None => Command::new(tool),
    };
    c.args(ssh_options(s)).arg(port_flag).arg(s.port.to_string()).args(args);
    Ok(c)
}

fn run(mut c: Command, what: &str) -> Result<(), String> {
    let st = c.stdin(Stdio::null()).status().map_err(|e| format!("cannot run {}: {}", what, e))?;
    if st.success() { Ok(()) } else { Err(format!("{} failed ({})", what, st)) }
}

pub fn run_install(s: &Spec) -> Result<(), String> {
    let name = Path::new(&s.deb).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    if !name.chars().all(|c| c.is_ascii_alphanumeric() || ".-_+~".contains(c)) {
        return Err(format!("install: the file name {:?} has characters the device command cannot take", name));
    }
    let remote = format!("/tmp/meow-{}", name);
    let target = format!("{}@{}", s.user, s.host);
    eprintln!("copying {} to {}", name, target);
    run(command(s, "scp", "-P", vec![s.deb.clone(), format!("{}:{}", target, remote)])?, "scp")?;
    eprintln!("installing");
    let installed = run(command(s, "ssh", "-p", vec![target.clone(), format!("dpkg -i {}", remote)])?, "dpkg -i");
    // the file is removed whether or not dpkg was happy
    let cleaned = run(command(s, "ssh", "-p", vec![target, format!("rm -f {}", remote)])?, "rm");
    installed?;
    cleaned
}

#[cfg(test)]
mod tests {
    use super::*;

    fn a(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn arguments_are_checked_before_anything_runs() {
        let d = std::env::temp_dir().join(format!("meow-install-{}.deb", std::process::id()));
        std::fs::write(&d, "x").unwrap();
        let p = d.to_string_lossy().into_owned();
        let s = parse_args(&a(&["--host", "10.0.0.2", "--password-env", "PW", &p])).unwrap();
        assert_eq!((s.user.as_str(), s.port, s.password_env.as_deref()), ("root", 22, Some("PW")));
        for bad in [a(&["--host", "h;rm -rf /", &p]), a(&["--host", "h", "--user", "a b", &p]), a(&["--host", "h", "x.txt"]), a(&["--host", "h"]), a(&["--host", "h", "--nope", &p])] {
            assert!(parse_args(&bad).is_err(), "{:?}", bad);
        }
        // the password goes through the environment of sshpass, not its arguments
        std::env::set_var("MEOW_TEST_PW", "secret-pw");
        let spec = Spec { host: "h".into(), user: "root".into(), port: 22, deb: p, password_env: Some("MEOW_TEST_PW".into()), legacy: true };
        let c = command(&spec, "ssh", "-p", vec!["root@h".into(), "true".into()]).unwrap();
        let args: Vec<String> = c.get_args().map(|x| x.to_string_lossy().into_owned()).collect();
        assert!(!args.iter().any(|x| x.contains("secret-pw")));
        assert!(c.get_envs().any(|(k, v)| k == "SSHPASS" && v.is_some_and(|v| v == "secret-pw")));
        assert!(args.iter().any(|x| x == "HostKeyAlgorithms=+ssh-rsa"));
        let _ = std::fs::remove_file(d);
    }
}
