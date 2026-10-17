//! Port of libalpm's `alpm_pkg_vercmp` (rpmvercmp with epoch and release handling).
use core::cmp::Ordering;

fn is_alnum(c: u8) -> bool {
    c.is_ascii_alphanumeric()
}

fn rpmvercmp(a: &str, b: &str) -> Ordering {
    if a == b {
        return Ordering::Equal;
    }
    let (a, b) = (a.as_bytes(), b.as_bytes());
    // `ptr*` marks the start of the last consumed segment end; `one`/`two` the current position.
    let (mut ptr1, mut ptr2) = (0usize, 0usize);
    let (mut one, mut two) = (0usize, 0usize);
    let at = |s: &[u8], i: usize| -> u8 { s.get(i).copied().unwrap_or(0) };

    while at(a, one) != 0 && at(b, two) != 0 {
        while at(a, one) != 0 && !is_alnum(at(a, one)) {
            one += 1;
        }
        while at(b, two) != 0 && !is_alnum(at(b, two)) {
            two += 1;
        }
        if at(a, one) == 0 || at(b, two) == 0 {
            break;
        }
        if one - ptr1 != two - ptr2 {
            return if one - ptr1 < two - ptr2 {
                Ordering::Less
            } else {
                Ordering::Greater
            };
        }
        ptr1 = one;
        ptr2 = two;
        let isnum = at(a, ptr1).is_ascii_digit();
        if isnum {
            while at(a, ptr1).is_ascii_digit() {
                ptr1 += 1;
            }
            while at(b, ptr2).is_ascii_digit() {
                ptr2 += 1;
            }
        } else {
            while at(a, ptr1).is_ascii_alphabetic() {
                ptr1 += 1;
            }
            while at(b, ptr2).is_ascii_alphabetic() {
                ptr2 += 1;
            }
        }
        if one == ptr1 {
            return Ordering::Less;
        }
        if two == ptr2 {
            return if isnum {
                Ordering::Greater
            } else {
                Ordering::Less
            };
        }
        let mut s1 = a.get(one..ptr1).unwrap_or(&[]);
        let mut s2 = b.get(two..ptr2).unwrap_or(&[]);
        if isnum {
            while let Some((&b'0', rest)) = s1.split_first() {
                s1 = rest;
            }
            while let Some((&b'0', rest)) = s2.split_first() {
                s2 = rest;
            }
            if s1.len() != s2.len() {
                return s1.len().cmp(&s2.len());
            }
        }
        match s1.cmp(s2) {
            Ordering::Equal => {}
            o => return o,
        }
        one = ptr1;
        two = ptr2;
    }

    let (c1, c2) = (at(a, one), at(b, two));
    if c1 == 0 && c2 == 0 {
        return Ordering::Equal;
    }
    if (c1 == 0 && !c2.is_ascii_alphabetic()) || c1.is_ascii_alphabetic() {
        Ordering::Less
    } else {
        Ordering::Greater
    }
}

/// Splits `[epoch:]version[-release]`.
fn parse_evr(evr: &str) -> (&str, &str, Option<&str>) {
    let digits = evr.bytes().take_while(|b| b.is_ascii_digit()).count();
    let (epoch, rest) = if evr.as_bytes().get(digits) == Some(&b':') {
        let (ep, after) = evr.split_at_checked(digits).unwrap_or(("", evr));
        let epoch = if ep.is_empty() { "0" } else { ep };
        let rest = after.strip_prefix(':').unwrap_or(after);
        (epoch, rest)
    } else {
        ("0", evr)
    };
    let (version, release) = match rest.rsplit_once('-') {
        Some((v, r)) => (v, Some(r)),
        None => (rest, None),
    };
    (epoch, version, release)
}

/// Compares two package versions the way pacman does.
pub fn vercmp(a: &str, b: &str) -> Ordering {
    if a == b {
        return Ordering::Equal;
    }
    let (e1, v1, r1) = parse_evr(a);
    let (e2, v2, r2) = parse_evr(b);
    match rpmvercmp(e1, e2) {
        Ordering::Equal => {}
        o => return o,
    }
    match rpmvercmp(v1, v2) {
        Ordering::Equal => {}
        o => return o,
    }
    match (r1, r2) {
        (Some(r1), Some(r2)) => rpmvercmp(r1, r2),
        _ => Ordering::Equal,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use Ordering::*;

    #[test]
    fn libalpm_vectors() {
        let cases = [
            ("1.5.0", "1.5.0", Equal),
            ("1.5.1", "1.5.0", Greater),
            ("1.5.1", "1.5", Greater),
            ("1.5.0-1", "1.5.0-1", Equal),
            ("1.5.0-1", "1.5.0-2", Less),
            ("1.5.0-1", "1.5.1-1", Less),
            ("1.5.0-2", "1.5.1-1", Less),
            ("1.5-1", "1.5.1-1", Less),
            ("1.5b-1", "1.5-1", Less),
            ("1.5-1", "1.5b-1", Greater),
            ("1.5.b-1", "1.5-1", Greater),
            ("1.0-1", "1.0", Equal),
            ("1:1.0", "2.0", Greater),
            ("1:1.0-1", "1.0-1", Greater),
            ("0:1.0", "1.0", Equal),
            ("1.0a", "1.0alpha", Less),
            ("1.0alpha", "1.0b", Less),
            ("1.0b", "1.0beta", Less),
            ("1.0beta", "1.0rc", Less),
            ("1.0rc", "1.0", Less),
            ("1.0", "1.0.a", Less),
            ("1.0.a", "1.0.1", Less),
            ("1.0a", "1.0.a", Less),
            ("1.001", "1.1", Equal),
            ("1.0.0", "1.0", Greater),
            ("1..0", "1.0", Greater),
            ("2.0", "1.0", Greater),
            ("1_0", "1.0", Equal),
        ];
        for (a, b, want) in cases {
            assert_eq!(vercmp(a, b), want, "{a} vs {b}");
            assert_eq!(vercmp(b, a), want.reverse(), "{b} vs {a}");
        }
    }
}
