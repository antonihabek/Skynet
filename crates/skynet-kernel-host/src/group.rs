//! Who may take §25's rings, now that a package is not this process's user (§23.4).
//!
//! On a robot every package but a driver runs as a user systemd allocates for that one unit
//! (`DynamicUser`), and this process binds its socket as root. A socket nobody else may write is
//! one nobody else may connect to, which is right for every package on the machine but one: the
//! commanding package, which `skynetd` starts with a single supplementary group. The socket is
//! given to that group, mode `0660`. The file mode decides who the rings are handed to, and the page
//! tables decide who may write them once they are.
//!
//! Looked up in `/etc/group` rather than through `getgrnam`, because this crate has no `unsafe` and
//! no dependency, and the group is a local one that `packaging/install.sh` creates. A directory
//! service would not hold it.

use std::path::Path;

/// A group, resolved to its number.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Group {
    /// The name it was asked for by.
    pub name: String,
    /// The number the kernel compares.
    pub gid: u32,
}

/// Where the local groups are.
pub const ETC_GROUP: &str = "/etc/group";

/// Find `name` in the text of an `/etc/group` file.
///
/// Pure, so the lookup can be tested without a machine whose groups anybody chose. Each line is
/// `name:password:gid:members`, and a line that does not have a number in the third field is
/// skipped rather than read as zero: gid 0 is root's group, and a malformed line resolving to it
/// would hand the robot's command ring to every process running as root's group.
pub fn find(name: &str, etc_group: &str) -> Result<Group, String> {
    for line in etc_group.lines() {
        let mut fields = line.split(':');
        if fields.next() != Some(name) {
            continue;
        }
        let gid = fields.nth(1).and_then(|gid| gid.trim().parse::<u32>().ok());
        return match gid {
            Some(gid) => Ok(Group {
                name: name.to_string(),
                gid,
            }),
            None => Err(format!(
                "the group `{name}` is in {ETC_GROUP} with no number this can read: `{line}`"
            )),
        };
    }
    Err(format!(
        "there is no group `{name}` on this machine, so the policy socket cannot be shared with \
         the commanding package (§25, §23.4). `packaging/install.sh` creates it; or drop \
         --policy-socket-group on a machine whose packages run as this process's own user"
    ))
}

/// Read `/etc/group` and find `name` in it.
pub fn resolve(name: &str) -> Result<Group, String> {
    let path = Path::new(ETC_GROUP);
    let text = std::fs::read_to_string(path)
        .map_err(|error| format!("{} could not be read: {error}", path.display()))?;
    find(name, &text)
}

#[cfg(test)]
mod tests {
    use super::*;

    const GROUPS: &str = "root:x:0:\n\
                          dialout:x:20:\n\
                          skynet-ring:x:997:\n\
                          broken:x::\n";

    #[test]
    fn a_group_is_found_by_name_with_its_number() {
        assert_eq!(
            find("skynet-ring", GROUPS),
            Ok(Group {
                name: "skynet-ring".to_string(),
                gid: 997
            })
        );
    }

    /// A prefix is not a name: `skynet` must not match `skynet-ring`, or the reverse.
    #[test]
    fn only_the_whole_name_matches() {
        assert!(find("skynet", GROUPS).is_err());
        assert!(find("skynet-ring-2", GROUPS).is_err());
    }

    #[test]
    fn a_missing_group_names_what_creates_it() {
        let error = find("skynet-ring", "root:x:0:\n").unwrap_err();
        assert!(error.contains("install.sh"), "{error}");
    }

    /// Never zero by default. Root's group is the one a malformed line must not resolve to.
    #[test]
    fn a_line_with_no_number_is_refused_rather_than_read_as_root() {
        let error = find("broken", GROUPS).unwrap_err();
        assert!(error.contains("no number"), "{error}");
    }
}
