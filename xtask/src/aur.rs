//! `cargo xtask aur-pin NAME [--commit C]`: reviews an AUR package and prints the Lua entry that pins it (it goes into `packages.aur` of the `as` config).
use crate::Result;

pub fn pin(args: &[String]) -> Result<()> {
    let mut names: Vec<String> = Vec::new();
    let mut commit: Option<String> = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--commit" => commit = Some(it.next().ok_or("--commit needs a commit id")?.clone()),
            n => names.push(n.to_string()),
        }
    }
    if names.len() != 1 {
        return Err("usage: cargo xtask aur-pin NAME [--commit COMMIT]".into());
    }
    let name = &names[0];
    let info = aurbuild::rpc::info(&[name.clone()])?;
    let info = info.first().ok_or_else(|| format!("{name} is not on the AUR"))?;
    eprintln!("{} {} by {} (votes {}, out of date: {})", info.name, info.version, info.maintainer.as_deref().unwrap_or("nobody (orphaned)"), info.votes, info.out_of_date.is_some());
    let review = aurbuild::rpc::fetch_review(name, &info.pkgbase, commit.as_deref())?;
    for w in &review.warnings {
        eprintln!("warning: {w}");
    }
    for (file, line, text) in aurbuild::plan::risky_lines(&review.files) {
        eprintln!("  {file}:{line}: {text}");
    }
    let dbs = hostcfg::resolve::local_dbs().map_err(|e| format!("this host has no pacman sync databases to resolve dependencies against ({e}); run it on an Arch host"))?;
    let official = aurbuild::plan::Official::new(&dbs);
    let missing = aurbuild::plan::unresolved(std::slice::from_ref(&review), &official);
    if !missing.is_empty() {
        eprintln!("not in the official repositories (pin these AUR packages too): {}", missing.iter().map(|m| m.1.clone()).collect::<Vec<_>>().join(", "));
    }
    let plan = aurbuild::plan::plan(std::slice::from_ref(&review), &[name.clone()], &official);
    let entry = match plan {
        Ok(mut p) => p.remove(0),
        Err(e) => return Err(e.into()),
    };
    print!("{}", hostcfg::writer::aur_block(&[entry]));
    Ok(())
}
