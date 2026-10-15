//! Whether an installation Zetta found only by guessing where it is may run.
//!
//! MSYS2 and Cygwin are looked for first where an installer or the user said
//! they are — a Start Menu shortcut, an uninstall or Cygwin registry entry,
//! `PATH` — and last at the conventional `C:\msys64`, `C:\cygwin64` and
//! `C:\cygwin`. Nothing vouches for those last three. Any signed-in account
//! may create a folder at the root of the system drive, so on a machine
//! without MSYS2 another account can put one there and wait for the profile it
//! produces to be opened. And a folder an ordinary account creates there
//! inherits `C:\`'s grant of Modify to Authenticated Users, so another account
//! can replace the executables of a real installation unpacked into it.
//!
//! So a guessed root is offered only when nobody but this account and the
//! system's own principals — SYSTEM, Administrators and TrustedInstaller — can
//! change what its profiles run:
//!
//! - The root, and every path checked below it, is owned by one of those.
//!   Owning an object grants rewriting its DACL, whatever the DACL says.
//! - No allow entry in their DACLs grants anyone else a right that writes,
//!   deletes, re-permissions or takes ownership. Inheritable entries count:
//!   they are what the files underneath carry. A null DACL grants everyone
//!   everything.
//! - Every ancestor is owned the same way and grants nobody else delete,
//!   delete-child, re-permission or ownership of itself, so the root cannot be
//!   renamed away and replaced. Adding entries is allowed there: `C:\` lets
//!   every account create a folder, which is how a planted root begins, and
//!   the root's own check is what refuses one.
//! - None of these paths is a reparse point. Whoever owns a junction chooses
//!   its target, and can change it after the check.
//!
//! An installation this account created with those permissions passes, as
//! does one installed elevated, which Administrators owns. Deny entries are
//! ignored: they only narrow access, so ignoring them errs toward refusing.
//!
//! The paths checked below the root are the ones a profile starts — the
//! launcher, the runtime DLL and the shells — not the whole tree. Everything
//! else inherits the root's entries unless its owner, who needed a right the
//! root grants only to the principals above in order to create it, said
//! otherwise.
//!
//! The roots an installer or the user recorded are not checked: those records
//! are the user's, or an administrator's, own statement of where the
//! installation is.

#[cfg(windows)]
pub(crate) use native::guessed_root_is_trusted;

/// The system's own principals, by SID: SYSTEM, Administrators and
/// TrustedInstaller. They can replace anything on the machine anyway.
const SYSTEM_PRINCIPALS: &[&str] = &[
    "S-1-5-18",
    "S-1-5-32-544",
    "S-1-5-80-956008885-3418522649-1831038044-1853292631-2271478464",
];

/// CREATOR OWNER and OWNER RIGHTS. Each stands for an object's owner, which is
/// checked in its own right, so an entry naming one grants nobody new.
const OWNER_PLACEHOLDERS: &[&str] = &["S-1-3-0", "S-1-3-4"];

// Access rights, as `winnt.h` spells them. A directory's `FILE_ADD_FILE` and
// `FILE_ADD_SUBDIRECTORY` are the same bits as a file's `FILE_WRITE_DATA` and
// `FILE_APPEND_DATA`.
const FILE_WRITE_DATA: u32 = 0x0002;
const FILE_APPEND_DATA: u32 = 0x0004;
const FILE_DELETE_CHILD: u32 = 0x0040;
const DELETE: u32 = 0x0001_0000;
const WRITE_DAC: u32 = 0x0004_0000;
const WRITE_OWNER: u32 = 0x0008_0000;
const GENERIC_ALL: u32 = 0x1000_0000;
const GENERIC_WRITE: u32 = 0x4000_0000;

/// Rights that let another account change what an installed path holds.
const CHANGE_RIGHTS: u32 = FILE_WRITE_DATA
    | FILE_APPEND_DATA
    | FILE_DELETE_CHILD
    | DELETE
    | WRITE_DAC
    | WRITE_OWNER
    | GENERIC_WRITE
    | GENERIC_ALL;

/// Rights over an ancestor that let another account move what is under it
/// out of the way, or give itself the rights to.
const REPLACE_RIGHTS: u32 = FILE_DELETE_CHILD | DELETE | WRITE_DAC | WRITE_OWNER | GENERIC_ALL;

/// What a checked path is to the installation, which decides the rights that
/// matter on it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Role {
    /// The root or something under it: nobody else may change it, or what it
    /// passes on to its children.
    Installed,
    /// A directory above the root: nobody else may remove or re-permission it.
    Ancestor,
}

/// A security descriptor, reduced to what the policy reads.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Descriptor {
    /// The owner's SID, or `None` when it has none.
    pub(crate) owner: Option<String>,
    /// The DACL's allow entries; `None` is a null DACL.
    pub(crate) grants: Option<Vec<Grant>>,
}

/// One allow entry of a DACL.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Grant {
    /// Who it grants to, or `None` when the entry's layout does not say where
    /// the SID is — an object or unknown allow entry, read as anybody.
    pub(crate) trustee: Option<String>,
    pub(crate) mask: u32,
    /// `INHERIT_ONLY_ACE`: the entry is passed on to children and does not
    /// apply to the object itself.
    pub(crate) inherit_only: bool,
}

impl Descriptor {
    /// Whether nobody but `user` and the system's own principals can do to
    /// this path what `role` forbids.
    pub(crate) fn admits_only(&self, user: &str, role: Role) -> bool {
        let principal = |sid: &str| sid == user || SYSTEM_PRINCIPALS.contains(&sid);
        let Some(grants) = &self.grants else {
            return false;
        };
        self.owner.as_deref().is_some_and(principal)
            && grants.iter().all(|grant| {
                let forbidden = match role {
                    Role::Installed => CHANGE_RIGHTS,
                    Role::Ancestor if grant.inherit_only => 0,
                    Role::Ancestor => REPLACE_RIGHTS,
                };
                grant.mask & forbidden == 0
                    || grant
                        .trustee
                        .as_deref()
                        .is_some_and(|sid| principal(sid) || OWNER_PLACEHOLDERS.contains(&sid))
            })
    }
}

#[cfg(windows)]
mod native {
    use std::{
        fs, io,
        os::windows::fs::MetadataExt as _,
        path::{Path, PathBuf},
    };

    use windows::{
        Win32::{
            Foundation::{CloseHandle, HANDLE, HLOCAL, LocalFree},
            Security::{
                ACCESS_ALLOWED_ACE, ACE_HEADER, ACL,
                Authorization::{ConvertSidToStringSidW, GetNamedSecurityInfoW, SE_FILE_OBJECT},
                DACL_SECURITY_INFORMATION, GetAce, GetTokenInformation, INHERIT_ONLY_ACE,
                OWNER_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID, TOKEN_QUERY, TOKEN_USER,
                TokenUser,
            },
            Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT,
            System::{
                SystemServices::{
                    ACCESS_ALLOWED_ACE_TYPE, ACCESS_ALLOWED_CALLBACK_ACE_TYPE,
                    ACCESS_DENIED_ACE_TYPE, ACCESS_DENIED_CALLBACK_ACE_TYPE,
                    ACCESS_DENIED_CALLBACK_OBJECT_ACE_TYPE, ACCESS_DENIED_OBJECT_ACE_TYPE,
                },
                Threading::{GetCurrentProcess, OpenProcessToken},
            },
        },
        core::{HSTRING, PWSTR},
    };

    use super::{Descriptor, Grant, Role};

    /// Whether the guessed installation at `root` may be offered: see the
    /// module documentation. `launched` names, relative to the root, what its
    /// profiles start; one that is missing is skipped, as nothing would run
    /// it and nobody else could create it once its parent has passed.
    pub(crate) fn guessed_root_is_trusted(root: &Path, launched: &[&str]) -> bool {
        let Ok(user) = current_user_sid() else {
            return false;
        };
        let check = |path: &Path, role| path_admits_only(path, &user, role).unwrap_or(false);
        root.is_absolute()
            && check(root, Role::Installed)
            && root
                .ancestors()
                .skip(1)
                .all(|ancestor| check(ancestor, Role::Ancestor))
            && launched.iter().all(|relative| {
                let mut path = PathBuf::from(root);
                for component in Path::new(relative).components() {
                    path.push(component);
                    match path_admits_only(&path, &user, Role::Installed) {
                        Ok(true) => {}
                        Err(error) if error.kind() == io::ErrorKind::NotFound => return true,
                        Ok(false) | Err(_) => return false,
                    }
                }
                true
            })
    }

    fn path_admits_only(path: &Path, user: &str, role: Role) -> io::Result<bool> {
        let metadata = fs::symlink_metadata(path)?;
        if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT.0 != 0 {
            return Ok(false);
        }
        Ok(read_descriptor(path)?.admits_only(user, role))
    }

    fn read_descriptor(path: &Path) -> io::Result<Descriptor> {
        let name = HSTRING::from(path.as_os_str());
        let mut owner = PSID::default();
        let mut dacl: *mut ACL = std::ptr::null_mut();
        let mut security = PSECURITY_DESCRIPTOR::default();
        // SAFETY: every output pointer is a live local; the descriptor that
        // `owner` and `dacl` point into is freed only after they are read.
        unsafe {
            let status = GetNamedSecurityInfoW(
                &name,
                SE_FILE_OBJECT,
                OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
                Some(&mut owner),
                None,
                Some(&mut dacl),
                None,
                &mut security,
            );
            if status.is_err() {
                return Err(io::Error::from_raw_os_error(status.0 as i32));
            }
            let descriptor = describe(owner, dacl);
            LocalFree(Some(HLOCAL(security.0)));
            descriptor
        }
    }

    /// # Safety
    ///
    /// `owner` and `dacl` must be null or point into a live security
    /// descriptor.
    unsafe fn describe(owner: PSID, dacl: *const ACL) -> io::Result<Descriptor> {
        unsafe {
            let owner = if owner.is_invalid() {
                None
            } else {
                Some(sid_string(owner)?)
            };
            if dacl.is_null() {
                return Ok(Descriptor {
                    owner,
                    grants: None,
                });
            }
            let mut grants = Vec::new();
            for index in 0..u32::from((*dacl).AceCount) {
                let mut ace = std::ptr::null_mut();
                GetAce(dacl, index, &mut ace).map_err(io::Error::other)?;
                let header = &*ace.cast::<ACE_HEADER>();
                let kind = u32::from(header.AceType);
                if [
                    ACCESS_DENIED_ACE_TYPE,
                    ACCESS_DENIED_OBJECT_ACE_TYPE,
                    ACCESS_DENIED_CALLBACK_ACE_TYPE,
                    ACCESS_DENIED_CALLBACK_OBJECT_ACE_TYPE,
                ]
                .contains(&kind)
                {
                    continue;
                }
                // Every DACL entry keeps its mask right after the header; only
                // these two keep the SID right after the mask.
                let allowed = &*ace.cast::<ACCESS_ALLOWED_ACE>();
                let trustee = if kind == ACCESS_ALLOWED_ACE_TYPE
                    || kind == ACCESS_ALLOWED_CALLBACK_ACE_TYPE
                {
                    Some(sid_string(PSID(
                        (&raw const allowed.SidStart).cast_mut().cast(),
                    ))?)
                } else {
                    None
                };
                grants.push(Grant {
                    trustee,
                    mask: allowed.Mask,
                    inherit_only: u32::from(header.AceFlags) & INHERIT_ONLY_ACE.0 != 0,
                });
            }
            Ok(Descriptor {
                owner,
                grants: Some(grants),
            })
        }
    }

    /// # Safety
    ///
    /// `sid` must point to a valid SID.
    unsafe fn sid_string(sid: PSID) -> io::Result<String> {
        unsafe {
            let mut text = PWSTR::null();
            ConvertSidToStringSidW(sid, &mut text).map_err(io::Error::other)?;
            let string = text.to_string().map_err(io::Error::other);
            LocalFree(Some(HLOCAL(text.0.cast())));
            string
        }
    }

    fn current_user_sid() -> io::Result<String> {
        // SAFETY: the token handle is closed before returning, and the buffer
        // is sized by the first query and aligned for `TOKEN_USER`.
        unsafe {
            let mut token = HANDLE::default();
            OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token)
                .map_err(io::Error::other)?;
            let mut length = 0_u32;
            let _ = GetTokenInformation(token, TokenUser, None, 0, &mut length);
            let mut buffer = vec![0_u64; (length as usize).div_ceil(8)];
            let queried = GetTokenInformation(
                token,
                TokenUser,
                Some(buffer.as_mut_ptr().cast()),
                length,
                &mut length,
            );
            let _ = CloseHandle(token);
            queried.map_err(io::Error::other)?;
            sid_string((*buffer.as_ptr().cast::<TOKEN_USER>()).User.Sid)
        }
    }
}

#[cfg(test)]
#[path = "tests/installation_trust.rs"]
mod tests;
