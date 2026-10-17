//! `cargo xtask aur-pin <NAME...|--from-system> [--commit C]`: reviews AUR packages and prints the TOML entry that pins them (it goes into `packages.aur` of the config).
use crate::Result;

pub fn pin(args: &[String]) -> Result<()> {
    let mut names: Vec<String> = Vec::new();
    let mut commit: Option<String> = None;
    let mut from_system = false;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--commit" => commit = Some(it.next().ok_or("--commit needs a commit id")?.clone()),
            "--from-system" | "--system" => from_system = true,
            n => names.push(n.to_string()),
        }
    }

    if from_system {
        let sys_names = aurbuild::autoload::system_foreign_packages()?;
        if sys_names.is_empty() {
            return Err("no foreign packages found on this system (pacman -Qqem returned empty)".into());
        }
        eprintln!(
            "Found {} foreign packages on system: {}",
            sys_names.len(),
            sys_names.join(", ")
        );
        let dbs = hostcfg::resolve::local_dbs().map_err(|e| {
            format!("this host has no pacman sync databases to resolve dependencies against ({e}); run it on an Arch host")
        })?;
        let official = aurbuild::plan::Official::new(&dbs);
        let rep = aurbuild::autoload::autoload_packages(&sys_names, &[], &official)?;
        for (skip, reason) in &rep.skipped {
            eprintln!("note: skipped {skip} ({reason})");
        }
        eprintln!("{}", rep.summary);
        print!("{}", hostcfg::writer::aur_block(&rep.entries));
        return Ok(());
    }

    if names.is_empty() {
        return Err("usage: cargo xtask aur-pin <NAME...|--from-system> [--commit COMMIT]".into());
    }

    if names.len() > 1 && commit.is_some() {
        return Err("--commit can only be used with a single package name".into());
    }

    let dbs = hostcfg::resolve::local_dbs().map_err(|e| {
        format!("this host has no pacman sync databases to resolve dependencies against ({e}); run it on an Arch host")
    })?;
    let official = aurbuild::plan::Official::new(&dbs);

    if names.len() > 1 {
        let rep = aurbuild::autoload::autoload_packages(&names, &[], &official)?;
        for (skip, reason) in &rep.skipped {
            eprintln!("note: skipped {skip} ({reason})");
        }
        eprintln!("{}", rep.summary);
        print!("{}", hostcfg::writer::aur_block(&rep.entries));
        return Ok(());
    }

    let Some(name) = names.first() else {
        return Err("usage: cargo xtask aur-pin <NAME...|--from-system> [--commit COMMIT]".into());
    };
    let info = aurbuild::rpc::info(std::slice::from_ref(name))?;
    let info = info
        .first()
        .ok_or_else(|| format!("{name} is not on the AUR"))?;
    eprintln!(
        "{} {} by {} (votes {}, out of date: {})",
        info.name,
        info.version,
        info.maintainer.as_deref().unwrap_or("nobody (orphaned)"),
        info.votes,
        info.out_of_date.is_some()
    );
    let review = aurbuild::rpc::fetch_review(name, &info.pkgbase, commit.as_deref())?;
    for w in &review.warnings {
        eprintln!("warning: {w}");
    }
    for (file, line, text) in aurbuild::plan::risky_lines(&review.files) {
        eprintln!("  {file}:{line}: {text}");
    }
    let missing = aurbuild::plan::unresolved(std::slice::from_ref(&review), &official);
    if !missing.is_empty() {
        eprintln!(
            "not in the official repositories (pin these AUR packages too): {}",
            missing
                .iter()
                .map(|m| m.1.clone())
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    let mut plan = aurbuild::plan::plan(
        std::slice::from_ref(&review),
        std::slice::from_ref(name),
        &official,
    )?;
    let entry = plan.remove(0);
    print!("{}", hostcfg::writer::aur_block(&[entry]));
    Ok(())
}
