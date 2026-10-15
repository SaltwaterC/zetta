//! Mirrors `src/installation_trust.rs`.
//!
//! The policy is checked against descriptors spelled out as `icacls` reports
//! them on a stock Windows install, so it runs on every host. The Windows-only
//! tests at the end read real descriptors from disk.

use super::*;

const USER: &str = "S-1-5-21-1004336348-1177238915-682003330-1001";
const OTHER_USER: &str = "S-1-5-21-1004336348-1177238915-682003330-1002";
const SYSTEM: &str = "S-1-5-18";
const ADMINISTRATORS: &str = "S-1-5-32-544";
const TRUSTED_INSTALLER: &str = "S-1-5-80-956008885-3418522649-1831038044-1853292631-2271478464";
const USERS: &str = "S-1-5-32-545";
const AUTHENTICATED_USERS: &str = "S-1-5-11";
const CREATOR_OWNER: &str = "S-1-3-0";

/// `FILE_ALL_ACCESS`, icacls's `F`.
const FULL: u32 = 0x001f_01ff;
/// `FILE_GENERIC_READ | FILE_GENERIC_EXECUTE`, icacls's `RX`.
const READ_EXECUTE: u32 = 0x0012_00a9;
/// icacls's `M`: read, execute, write and delete.
const MODIFY: u32 = 0x0013_01bf;
/// `FILE_APPEND_DATA` on a directory, icacls's `AD`: create subfolders.
const ADD_SUBDIRECTORY: u32 = 0x0000_0004;

fn grant(trustee: &str, mask: u32) -> Grant {
    Grant {
        trustee: Some(trustee.to_owned()),
        mask,
        inherit_only: false,
    }
}

fn inherited_only(trustee: &str, mask: u32) -> Grant {
    Grant {
        inherit_only: true,
        ..grant(trustee, mask)
    }
}

fn descriptor(owner: &str, grants: Vec<Grant>) -> Descriptor {
    Descriptor {
        owner: Some(owner.to_owned()),
        grants: Some(grants),
    }
}

/// `C:\` as Windows ships it.
fn system_drive_root() -> Descriptor {
    descriptor(
        TRUSTED_INSTALLER,
        vec![
            grant(ADMINISTRATORS, FULL),
            grant(SYSTEM, FULL),
            grant(USERS, READ_EXECUTE),
            inherited_only(AUTHENTICATED_USERS, MODIFY),
            grant(AUTHENTICATED_USERS, ADD_SUBDIRECTORY),
        ],
    )
}

/// What a folder this account creates at `C:\` inherits: everything the root
/// passes on, Authenticated Users' Modify included.
fn folder_created_at_system_drive_root(owner: &str) -> Descriptor {
    descriptor(
        owner,
        vec![
            grant(ADMINISTRATORS, FULL),
            grant(SYSTEM, FULL),
            grant(USERS, READ_EXECUTE),
            grant(AUTHENTICATED_USERS, MODIFY),
        ],
    )
}

/// The same folder after `icacls /inheritance:d` and
/// `icacls /remove:g *S-1-5-11`.
fn private_installation(owner: &str) -> Descriptor {
    descriptor(
        owner,
        vec![
            grant(ADMINISTRATORS, FULL),
            grant(SYSTEM, FULL),
            grant(USERS, READ_EXECUTE),
            grant(owner, FULL),
            inherited_only(CREATOR_OWNER, FULL),
        ],
    )
}

#[test]
fn the_system_drive_root_is_a_trusted_ancestor_though_everyone_may_add_to_it() {
    assert!(system_drive_root().admits_only(USER, Role::Ancestor));
}

#[test]
fn an_installation_only_this_account_and_the_system_can_change_is_trusted() {
    assert!(private_installation(USER).admits_only(USER, Role::Installed));
}

#[test]
fn an_installation_made_elevated_is_trusted() {
    assert!(private_installation(ADMINISTRATORS).admits_only(USER, Role::Installed));
}

#[test]
fn a_folder_unpacked_at_the_drive_root_with_inherited_permissions_is_refused() {
    // Owned by this account, and still writable by every other one.
    assert!(!folder_created_at_system_drive_root(USER).admits_only(USER, Role::Installed));
}

#[test]
fn a_root_another_account_planted_is_refused_even_with_a_private_dacl() {
    let planted = descriptor(
        OTHER_USER,
        vec![grant(OTHER_USER, FULL), grant(USERS, READ_EXECUTE)],
    );
    assert!(!planted.admits_only(USER, Role::Installed));
    // The owner may rewrite its DACL whatever it says now, even one that
    // grants it nothing.
    let deferential = descriptor(
        OTHER_USER,
        vec![
            grant(USER, FULL),
            grant(SYSTEM, FULL),
            grant(USERS, READ_EXECUTE),
        ],
    );
    assert!(!deferential.admits_only(USER, Role::Installed));
}

#[test]
fn each_right_that_changes_an_installation_is_refused_to_other_accounts() {
    for right in [
        0x0002,      // FILE_WRITE_DATA / FILE_ADD_FILE
        0x0004,      // FILE_APPEND_DATA / FILE_ADD_SUBDIRECTORY
        0x0040,      // FILE_DELETE_CHILD
        0x0001_0000, // DELETE
        0x0004_0000, // WRITE_DAC
        0x0008_0000, // WRITE_OWNER
        0x1000_0000, // GENERIC_ALL
        0x4000_0000, // GENERIC_WRITE
    ] {
        let mut installation = private_installation(USER);
        installation
            .grants
            .as_mut()
            .unwrap()
            .push(grant(OTHER_USER, READ_EXECUTE | right));
        assert!(
            !installation.admits_only(USER, Role::Installed),
            "{right:#x} granted to another account"
        );
    }
}

#[test]
fn an_inheritable_grant_to_others_refuses_the_installation_it_would_reach() {
    let mut installation = private_installation(USER);
    installation
        .grants
        .as_mut()
        .unwrap()
        .push(inherited_only(AUTHENTICATED_USERS, MODIFY));
    assert!(!installation.admits_only(USER, Role::Installed));
}

#[test]
fn an_ancestor_others_can_remove_or_repermission_is_refused() {
    for right in [0x0040, 0x0001_0000, 0x0004_0000, 0x0008_0000, 0x1000_0000] {
        let mut ancestor = system_drive_root();
        ancestor
            .grants
            .as_mut()
            .unwrap()
            .push(grant(AUTHENTICATED_USERS, READ_EXECUTE | right));
        assert!(
            !ancestor.admits_only(USER, Role::Ancestor),
            "{right:#x} granted on an ancestor"
        );
    }
}

#[test]
fn an_ancestor_another_account_owns_is_refused() {
    let mut ancestor = system_drive_root();
    ancestor.owner = Some(OTHER_USER.to_owned());
    assert!(!ancestor.admits_only(USER, Role::Ancestor));
}

#[test]
fn a_null_dacl_or_missing_owner_is_refused() {
    let null_dacl = Descriptor {
        owner: Some(USER.to_owned()),
        grants: None,
    };
    assert!(!null_dacl.admits_only(USER, Role::Installed));
    assert!(!null_dacl.admits_only(USER, Role::Ancestor));

    let ownerless = Descriptor {
        owner: None,
        ..private_installation(USER)
    };
    assert!(!ownerless.admits_only(USER, Role::Installed));
}

#[test]
fn an_entry_whose_trustee_cannot_be_read_counts_as_anybody() {
    let unread = |mask| Grant {
        trustee: None,
        mask,
        inherit_only: false,
    };
    let mut installation = private_installation(USER);
    installation
        .grants
        .as_mut()
        .unwrap()
        .push(unread(READ_EXECUTE));
    assert!(installation.admits_only(USER, Role::Installed));
    installation.grants.as_mut().unwrap().push(unread(MODIFY));
    assert!(!installation.admits_only(USER, Role::Installed));
}

#[cfg(windows)]
mod native {
    //! Real descriptors, set with `icacls` on a directory in the user's
    //! profile, which Windows creates for only this account, SYSTEM and
    //! Administrators to change. Not under `%TEMP%`: a host can grant other
    //! accounts Modify there (a sandboxing tool did on one), and then nothing
    //! below it is trusted, so the refusals here would pass for that reason
    //! instead of their own.

    use super::*;

    use std::{fs, path::Path, process::Command};

    fn icacls(path: &Path, arguments: &[&str]) {
        let status = Command::new("icacls")
            .arg(path)
            .args(arguments)
            .arg("/Q")
            .status()
            .unwrap();
        assert!(status.success(), "icacls {arguments:?}");
    }

    fn private_directory() -> tempfile::TempDir {
        let profile = std::env::var_os("USERPROFILE").expect("USERPROFILE is set");
        tempfile::Builder::new()
            .prefix("zetta-installation-trust-")
            .tempdir_in(profile)
            .unwrap()
    }

    fn installation() -> tempfile::TempDir {
        let root = private_directory();
        fs::create_dir_all(root.path().join("bin")).unwrap();
        fs::write(root.path().join("bin/bash.exe"), "").unwrap();
        root
    }

    #[test]
    fn a_private_installation_under_the_profile_is_trusted() {
        let root = installation();
        assert!(guessed_root_is_trusted(
            root.path(),
            &["bin/bash.exe", "bin/zsh.exe"]
        ));
    }

    #[test]
    fn a_root_every_account_may_modify_is_refused() {
        let root = installation();
        assert!(guessed_root_is_trusted(root.path(), &["bin/bash.exe"]));
        icacls(root.path(), &["/grant", "*S-1-5-11:(OI)(CI)M"]);
        assert!(!guessed_root_is_trusted(root.path(), &["bin/bash.exe"]));
    }

    #[test]
    fn a_launched_file_every_account_may_modify_is_refused() {
        let root = installation();
        icacls(
            &root.path().join("bin/bash.exe"),
            &["/grant", "*S-1-5-11:M"],
        );
        assert!(!guessed_root_is_trusted(root.path(), &["bin/bash.exe"]));
        // Not one the profile starts: not checked.
        assert!(guessed_root_is_trusted(root.path(), &["bin/zsh.exe"]));
    }

    #[test]
    fn a_root_reached_through_a_junction_is_refused() {
        let root = installation();
        let parent = private_directory();
        let junction = parent.path().join("junction");
        let status = Command::new("cmd")
            .args(["/d", "/c", "mklink", "/J"])
            .arg(&junction)
            .arg(root.path())
            .status()
            .unwrap();
        assert!(status.success());
        assert!(guessed_root_is_trusted(root.path(), &["bin/bash.exe"]));
        assert!(!guessed_root_is_trusted(&junction, &["bin/bash.exe"]));
    }

    #[test]
    fn a_missing_root_is_refused() {
        let parent = private_directory();
        assert!(!guessed_root_is_trusted(
            &parent.path().join("msys64"),
            &["msys2_shell.cmd"]
        ));
    }
}
