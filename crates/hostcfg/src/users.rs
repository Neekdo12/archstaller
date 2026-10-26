//! Host user discovery for pre-filling or importing user accounts from the running system.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostUser {
    pub name: String,
    pub uid: u32,
    pub shell: String,
    pub groups: Vec<String>,
    pub password_hash: Option<String>,
}

/// Parses `/etc/passwd` entries into `(name, uid, gid, shell)`.
/// Filters for regular interactive users: UID in `[1000, 60000)`, shell does not end with `nologin` or `false`.
pub fn parse_passwd(content: &str) -> Vec<(String, u32, u32, String)> {
    let mut users = Vec::new();
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let parts: Vec<&str> = line.split(':').collect();
        if parts.len() < 7 {
            continue;
        }
        let Some(&name) = parts.first() else {
            continue;
        };
        let Some(&uid_str) = parts.get(2) else {
            continue;
        };
        let Ok(uid) = uid_str.parse::<u32>() else {
            continue;
        };
        let Some(&gid_str) = parts.get(3) else {
            continue;
        };
        let Ok(gid) = gid_str.parse::<u32>() else {
            continue;
        };
        let Some(&shell) = parts.get(6) else {
            continue;
        };

        if (1000..60000).contains(&uid)
            && !shell.ends_with("/nologin")
            && !shell.ends_with("/false")
            && !name.is_empty()
        {
            users.push((name.to_string(), uid, gid, shell.to_string()));
        }
    }
    users
}

/// Parses `/etc/group` entries for supplemental groups that include `username` or match `user_gid`.
/// Excludes any group whose name matches `username`.
pub fn parse_groups(content: &str, username: &str, user_gid: u32) -> Vec<String> {
    let mut groups = Vec::new();
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let parts: Vec<&str> = line.split(':').collect();
        if parts.len() < 4 {
            continue;
        }
        let Some(&grp_name) = parts.first() else {
            continue;
        };
        let Some(&gid_str) = parts.get(2) else {
            continue;
        };
        let Ok(gid) = gid_str.parse::<u32>() else {
            continue;
        };
        let Some(&members_str) = parts.get(3) else {
            continue;
        };
        let members: Vec<&str> = members_str
            .split(',')
            .map(str::trim)
            .filter(|m| !m.is_empty())
            .collect();

        if grp_name == username {
            continue;
        }

        if (gid == user_gid || members.contains(&username))
            && !groups.iter().any(|g| g == grp_name)
        {
            groups.push(grp_name.to_string());
        }
    }
    groups.sort();
    groups
}

/// Parses `/etc/shadow` for a user's password crypt hash.
pub fn parse_shadow_hash(shadow_content: &str, username: &str) -> Option<String> {
    for line in shadow_content.lines() {
        let parts: Vec<&str> = line.trim().split(':').collect();
        if parts.len() >= 2 && parts.first().copied() == Some(username) {
            if let Some(&raw_hash) = parts.get(1) {
                // Strip any locking prefix '!' (e.g. !$y$... or !!$6$...) to extract the underlying crypt hash
                let hash = raw_hash.trim_start_matches('!');
                if hash.starts_with('$') && !hash.starts_with('*') && hash.len() > 13 {
                    return Some(hash.to_string());
                }
            }
        }
    }
    None
}

/// Discovers regular user accounts on the host system.
pub fn discover_host_users() -> Vec<HostUser> {
    let passwd_content = std::fs::read_to_string("/etc/passwd").unwrap_or_default();
    let group_content = std::fs::read_to_string("/etc/group").unwrap_or_default();
    let shadow_content = std::fs::read_to_string("/etc/shadow").ok();

    let mut parsed = parse_passwd(&passwd_content);

    // If USER is set and not yet in parsed list, check if USER exists in passwd even if UID < 1000 or custom
    if let Ok(user_env) = std::env::var("USER") {
        if !parsed.iter().any(|(n, _, _, _)| n == &user_env) {
            for line in passwd_content.lines() {
                let parts: Vec<&str> = line.trim().split(':').collect();
                if parts.len() >= 7 && parts.first().copied() == Some(user_env.as_str()) {
                    let uid = parts.get(2).and_then(|u| u.parse::<u32>().ok());
                    let gid = parts.get(3).and_then(|g| g.parse::<u32>().ok());
                    let shell = parts.get(6).copied();
                    if let (Some(uid), Some(gid), Some(shell)) = (uid, gid, shell) {
                        if !shell.ends_with("/nologin") && !shell.ends_with("/false") {
                            parsed.push((user_env.clone(), uid, gid, shell.to_string()));
                            break;
                        }
                    }
                }
            }
        }
    }

    let mut result = Vec::new();
    for (name, uid, gid, shell) in parsed {
        let mut groups = parse_groups(&group_content, &name, gid);
        // If groups is empty, try `id -Gn <name>` as a fallback
        if groups.is_empty() {
            if let Ok(out) = std::process::Command::new("id").args(["-Gn", &name]).output() {
                if out.status.success() {
                    let text = String::from_utf8_lossy(&out.stdout);
                    for g in text.split_whitespace() {
                        if g != name && !groups.iter().any(|x| x == g) {
                            groups.push(g.to_string());
                        }
                    }
                    groups.sort();
                }
            }
        }

        let password_hash = shadow_content
            .as_deref()
            .and_then(|sc| parse_shadow_hash(sc, &name));

        result.push(HostUser {
            name,
            uid,
            shell,
            groups,
            password_hash,
        });
    }

    result.sort_by_key(|a| a.uid);
    result
}

/// Returns the primary or current interactive user running this process.
pub fn current_user() -> Option<HostUser> {
    let users = discover_host_users();
    if let Ok(curr) = std::env::var("USER") {
        if let Some(u) = users.iter().find(|u| u.name == curr) {
            return Some(u.clone());
        }
    }
    users.into_iter().next()
}

/// Updates user password hashes from a shadow file string.
/// Returns the number of users whose hashes were updated.
pub fn update_users_from_shadow(users: &mut [config::User], shadow_content: &str) -> usize {
    let mut updated = 0;
    for u in users.iter_mut() {
        if let Some(h) = parse_shadow_hash(shadow_content, &u.name) {
            if u.password_hash != h {
                u.password_hash = h;
                updated += 1;
            }
        }
    }
    updated
}

/// Retrieves the contents of `/etc/shadow` via direct file read, password (sudo/su), or Polkit (`pkexec`).
pub fn authenticate_and_read_shadow(
    password: Option<&str>,
    use_pkexec: bool,
) -> Result<String, String> {
    if let Ok(content) = std::fs::read_to_string("/etc/shadow") {
        if content.contains(':') {
            return Ok(content);
        }
    }
    let snapshot = crate::privilege::collect_privileged_system_files(password, use_pkexec)?;
    snapshot
        .shadow
        .ok_or_else(|| "shadow file was not found in privileged snapshot".to_string())
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_passwd_lines() {
        let sample = "\
root:x:0:0::/root:/bin/bash
bin:x:1:1::/:/usr/bin/nologin
daemon:x:2:2::/:/usr/bin/nologin
alice:x:1000:1000:Alice:/home/alice:/bin/bash
bob:x:1001:1001:Bob:/home/bob:/usr/bin/zsh
service_acct:x:1002:1002::/home/service:/bin/false
nobody:x:65534:65534:Nobody:/:/usr/bin/nologin
";
        let users = parse_passwd(sample);
        assert_eq!(users.len(), 2);
        assert_eq!(users.first(), Some(&("alice".into(), 1000, 1000, "/bin/bash".into())));
        assert_eq!(users.get(1), Some(&("bob".into(), 1001, 1001, "/usr/bin/zsh".into())));
    }

    #[test]
    fn parses_group_lines() {
        let sample = "\
root:x:0:root
wheel:x:998:alice,bob
audio:x:995:alice
docker:x:960:bob
alice:x:1000:
bob:x:1001:
";
        let alice_groups = parse_groups(sample, "alice", 1000);
        assert_eq!(alice_groups, vec!["audio".to_string(), "wheel".to_string()]);

        let bob_groups = parse_groups(sample, "bob", 1001);
        assert_eq!(bob_groups, vec!["docker".to_string(), "wheel".to_string()]);
    }

    #[test]
    fn parses_shadow_hash_entries() {
        let sample = "\
root:*:19700:0:99999:7:::
alice:$6$rounds=5000$saltsalt$xyz123...:19700:0:99999:7:::
bob:!:19700:0:99999:7:::
dave:$y$j9T$saltsalt$xyz123...:19700:0:99999:7:::
eve:!$y$j9T$saltsalt$xyz123...:19700:0:99999:7:::
";
        assert_eq!(
            parse_shadow_hash(sample, "alice"),
            Some("$6$rounds=5000$saltsalt$xyz123...".into())
        );
        assert_eq!(
            parse_shadow_hash(sample, "dave"),
            Some("$y$j9T$saltsalt$xyz123...".into())
        );
        assert_eq!(
            parse_shadow_hash(sample, "eve"),
            Some("$y$j9T$saltsalt$xyz123...".into())
        );
        assert_eq!(parse_shadow_hash(sample, "root"), None);
        assert_eq!(parse_shadow_hash(sample, "bob"), None);
        assert_eq!(parse_shadow_hash(sample, "charlie"), None);
    }

    #[test]
    fn updates_users_from_shadow_text() {
        let shadow = "\
root:$6$rootpasshash...:19700:0:99999:7:::
alice:$6$alicepasshash...:19700:0:99999:7:::
bob:$6$bobpasshash...:19700:0:99999:7:::
";
        let mut users = vec![
            config::User {
                name: "alice".into(),
                password_hash: "old".into(),
                groups: vec!["wheel".into()],
                shell: "/bin/bash".into(),
                dotfiles: None,
                ..Default::default()
            },
            config::User {
                name: "charlie".into(),
                password_hash: "old".into(),
                groups: vec![],
                shell: "/bin/bash".into(),
                dotfiles: None,
                ..Default::default()
            },
        ];

        let updated = update_users_from_shadow(&mut users, shadow);
        assert_eq!(updated, 1);
        assert_eq!(users.first().unwrap().password_hash, "$6$alicepasshash...");
        assert_eq!(users[1].password_hash, "old");
    }
}
