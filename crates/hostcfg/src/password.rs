//! Password hashes for the config: `$6$` SHA-512 crypt, the only format `config` accepts.

/// Hashes a password with a fresh random salt. The plaintext is not kept anywhere.
pub fn hash(password: &str) -> Result<String, String> {
    let params = sha_crypt::Sha512Params::new(5000).map_err(|e| format!("{e:?}"))?;
    sha_crypt::sha512_simple(password, &params).map_err(|e| format!("{e:?}"))
}

/// Whether `hash` is a SHA-512 crypt hash of `password`.
pub fn verify(password: &str, hash: &str) -> bool {
    sha_crypt::sha512_check(password, hash).is_ok()
}

#[cfg(test)]
mod tests {
    #[test]
    fn hashes_verify_and_pass_config_validation() {
        let h = super::hash("correct horse").unwrap();
        assert!(h.starts_with("$6$"));
        assert!(super::verify("correct horse", &h));
        assert!(!super::verify("wrong", &h));
        let mut c = crate::lua::tests_support::minimal_config();
        c.users = vec![config::User { name: "arch".into(), password_hash: h.clone(), groups: vec![], shell: "/bin/bash".into() }];
        assert!(c.validate().is_ok());
        assert_ne!(h, super::hash("correct horse").unwrap(), "salt must be random");
    }
}
