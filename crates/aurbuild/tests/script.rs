//! Runs `firstboot/archstaler-aur.sh` against a local directory of git repositories, with stand-ins for
//! the commands that need root or Arch (`runuser`, `useradd`, `pacman`, `makepkg`, `systemctl`, ...).
use aurbuild::digest::{tree_digest, Files};
use std::path::{Path, PathBuf};
use std::process::Command;

fn sh(dir: &Path, cmd: &str) -> String {
    let o = Command::new("bash").arg("-c").arg(cmd).current_dir(dir).output().unwrap();
    assert!(o.status.success(), "{cmd}: {}", String::from_utf8_lossy(&o.stderr));
    String::from_utf8_lossy(&o.stdout).trim().to_string()
}

struct Env {
    root: PathBuf,
}

impl Env {
    fn new(name: &str) -> Env {
        let root = std::env::temp_dir().join(format!("aur-script-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for d in ["state", "bin", "repos", "cache", "calls"] {
            std::fs::create_dir_all(root.join(d)).unwrap();
        }
        // Stand-ins: each records its call; makepkg "builds" a file named after the recipe.
        let stubs: &[(&str, &str)] = &[
            ("runuser", "shift 2; exec \"$@\""), // runuser -u USER -- cmd...
            ("id", "exit 1"),
            ("useradd", "echo useradd \"$@\" >> \"$CALLS/useradd\"; mkdir -p \"$ARCHSTALER_STATE/aur-build\""),
            ("userdel", "echo userdel >> \"$CALLS/userdel\""),
            ("install", "mkdir -p \"${@: -1}\""),
            ("systemctl", "echo \"$@\" >> \"$CALLS/systemctl\""),
            ("getent", "exit 0"),
            ("pacman", "echo \"$@\" >> \"$CALLS/pacman\""),
            (
                "makepkg",
                "if [ \"$1\" = --packagelist ]; then n=$(grep -m1 '^pkgname' .SRCINFO | cut -d= -f2 | tr -d ' '); echo \"$PWD/$n-1-1-x86_64.pkg.tar.zst\"; else n=$(grep -m1 '^pkgname' .SRCINFO | cut -d= -f2 | tr -d ' '); [ -f FAIL ] && exit 1; echo built > \"$n-1-1-x86_64.pkg.tar.zst\"; fi",
            ),
        ];
        for (n, body) in stubs {
            let p = root.join("bin").join(n);
            std::fs::write(&p, format!("#!/bin/bash\n{body}\n")).unwrap();
            sh(&root, &format!("chmod +x {}", p.display()));
        }
        Env { root }
    }

    /// A repository `pkgbase.git` with these files committed; returns (commit, tree digest).
    fn repo(&self, pkgbase: &str, files: &[(&str, &str)]) -> (String, String) {
        let d = self.root.join("repos").join(format!("{pkgbase}.git"));
        std::fs::create_dir_all(&d).unwrap();
        let mut f = Files::new();
        for (n, c) in files {
            std::fs::write(d.join(n), c).unwrap();
            f.insert(n.to_string(), c.as_bytes().to_vec());
        }
        sh(&d, "git init -q && git add -A && git -c user.name=t -c user.email=t@t commit -q -m x");
        (sh(&d, "git rev-parse HEAD"), tree_digest(&f))
    }

    fn run(&self, list: &str) -> (i32, String) {
        std::fs::write(self.root.join("state/aur.list"), list).unwrap();
        let script = concat!(env!("CARGO_MANIFEST_DIR"), "/../../firstboot/archstaler-aur.sh");
        let path = format!("{}:{}", self.root.join("bin").display(), std::env::var("PATH").unwrap());
        let o = Command::new("bash")
            .arg(script)
            .env("PATH", path)
            .env("ARCHSTALER_STATE", self.root.join("state"))
            .env("ARCHSTALER_LOG", self.root.join("log"))
            .env("ARCHSTALER_AUR_BASE", self.root.join("repos"))
            .env("ARCHSTALER_CACHE", self.root.join("cache"))
            .env("CALLS", self.root.join("calls"))
            .output()
            .unwrap();
        (o.status.code().unwrap_or(-1), String::from_utf8_lossy(&o.stdout).into_owned() + &String::from_utf8_lossy(&o.stderr))
    }

    fn calls(&self, name: &str) -> String {
        std::fs::read_to_string(self.root.join("calls").join(name)).unwrap_or_default()
    }
}

impl Drop for Env {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn have_git() -> bool {
    Command::new("git").arg("--version").output().is_ok_and(|o| o.status.success())
}

const RECIPE: &[(&str, &str)] = &[("PKGBUILD", "pkgname=foo\n"), (".SRCINFO", "pkgbase = foo\n\tpkgver = 1\npkgname = foo\n")];

#[test]
fn builds_installs_and_cleans_up() {
    if !have_git() {
        return;
    }
    let e = Env::new("ok");
    let (c, d) = e.repo("foo", RECIPE);
    let (code, out) = e.run(&format!("foo:foo:{c}:{d}:0:0:foo.service\n"));
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("package foo: installed"), "{out}");
    let p = e.calls("pacman");
    assert!(p.contains("-U --noconfirm --needed") && p.contains("foo-1-1-x86_64.pkg.tar.zst") && !p.contains("--asdeps"), "{p}");
    assert!(e.calls("systemctl").contains("enable foo.service"));
    assert!(e.calls("systemctl").contains("disable archstaler-aur.service"), "the service switches itself off when done");
    assert!(e.root.join("cache/foo-1-1-x86_64.pkg.tar.zst").exists());
    assert!(!e.root.join("state/aur.pending").exists() && !e.root.join("state/aur-build").exists());
}

#[test]
fn marks_dependencies_and_keeps_the_order() {
    if !have_git() {
        return;
    }
    let e = Env::new("deps");
    let (c1, d1) = e.repo("lib", &[("PKGBUILD", "x\n"), (".SRCINFO", "pkgbase = lib\npkgname = lib\n")]);
    let (c2, d2) = e.repo("app", &[("PKGBUILD", "y\n"), (".SRCINFO", "pkgbase = app\npkgname = app\n")]);
    let (code, out) = e.run(&format!("lib:lib:{c1}:{d1}:0:1:\napp:app:{c2}:{d2}:0:0:\n"));
    assert_eq!(code, 0, "{out}");
    let p: Vec<String> = e.calls("pacman").lines().map(String::from).collect();
    assert!(p[0].contains("--asdeps") && p[0].contains("lib-1-1"), "{p:?}");
    assert!(!p[1].contains("--asdeps") && p[1].contains("app-1-1"), "{p:?}");
}

#[test]
fn refuses_a_recipe_that_differs_from_the_reviewed_one() {
    if !have_git() {
        return;
    }
    let e = Env::new("digest");
    let (c, _) = e.repo("foo", RECIPE);
    let (code, out) = e.run(&format!("foo:foo:{c}:{}:0:0:\n", "0".repeat(64)));
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("REFUSED foo"), "{out}");
    assert!(e.calls("pacman").is_empty(), "nothing is installed from a refused recipe");
    assert!(!e.root.join("state/aur.pending").exists(), "a refusal is final, not retried");
}

#[test]
fn refuses_a_commit_that_is_not_in_the_repository() {
    if !have_git() {
        return;
    }
    let e = Env::new("commit");
    let (_, d) = e.repo("foo", RECIPE);
    let (_, out) = e.run(&format!("foo:foo:{}:{d}:0:0:\n", "1".repeat(40)));
    assert!(out.contains("REFUSED foo"), "{out}");
    assert!(e.calls("pacman").is_empty());
}

#[test]
fn an_unreachable_repository_stays_pending_and_gives_up_after_five_attempts() {
    if !have_git() {
        return;
    }
    let e = Env::new("retry");
    let line = format!("ghost:ghost:{}:{}:0:0:\n", "1".repeat(40), "2".repeat(64));
    for n in 1..=5 {
        let (code, out) = e.run(&line);
        assert_eq!(code, 1, "attempt {n}: {out}");
        assert!(out.contains("will try again on the next boot"), "{out}");
        assert!(std::fs::read_to_string(e.root.join("state/aur.pending")).unwrap().contains("ghost"));
    }
    let (code, out) = e.run(&line);
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("giving up"), "{out}");
    assert!(e.calls("systemctl").contains("disable archstaler-aur.service"));
    assert!(!e.root.join("state/aur.pending").exists());
}

#[test]
fn a_failed_build_warns_and_the_next_package_still_runs() {
    if !have_git() {
        return;
    }
    let e = Env::new("fail");
    let (c1, d1) = e.repo("bad", &[("PKGBUILD", "x\n"), (".SRCINFO", "pkgbase = bad\npkgname = bad\n"), ("FAIL", "")]);
    let (c2, d2) = e.repo("good", &[("PKGBUILD", "y\n"), (".SRCINFO", "pkgbase = good\npkgname = good\n")]);
    let (code, out) = e.run(&format!("bad:bad:{c1}:{d1}:0:0:\ngood:good:{c2}:{d2}:0:0:\n"));
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("building bad failed"), "{out}");
    assert!(out.contains("package good: installed"), "{out}");
    assert!(e.calls("pacman").contains("good-1-1") && !e.calls("pacman").contains("bad-1-1"));
}
