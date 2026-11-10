//! Running processes of a harness, from /proc (never `pgrep`).

pub struct Proc {
    pub pid: i32,
    pub argv: Vec<String>,
    pub cwd: Option<String>,
}

/// Live (not zombie) processes whose executable name is `comm`, other than this one.
pub fn named(comm: &str) -> Vec<Proc> {
    let me = std::process::id() as i32;
    let Ok(dir) = std::fs::read_dir("/proc") else { return vec![] };
    let mut out = vec![];
    for e in dir.flatten() {
        let Some(pid) = e.file_name().to_str().and_then(|s| s.parse::<i32>().ok()) else { continue };
        if pid == me {
            continue;
        }
        let p = e.path();
        if std::fs::read_to_string(p.join("comm")).ok().as_deref().map(str::trim) != Some(comm) {
            continue;
        }
        // State follows the parenthesised name, which may itself hold ") ".
        let zombie = std::fs::read_to_string(p.join("stat")).ok().and_then(|s| s.rfind(") ").map(|i| s[i + 2..].starts_with('Z'))).unwrap_or(true);
        if zombie {
            continue;
        }
        let argv = std::fs::read(p.join("cmdline"))
            .map(|b| b.split(|c| *c == 0).filter(|a| !a.is_empty()).map(|a| String::from_utf8_lossy(a).into_owned()).collect())
            .unwrap_or_default();
        let cwd = std::fs::read_link(p.join("cwd")).ok().map(|c| c.to_string_lossy().into_owned());
        out.push(Proc { pid, argv, cwd });
    }
    out
}
