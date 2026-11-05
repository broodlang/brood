//! Windows's owner-only (KI-365): a protected DACL with one entry, full control for the user
//! this process runs as. Nothing inherited from the directory above survives, so no group,
//! no `Users`, no `Everyone` — what 0600 / 0700 mean on Unix. `file/spit-private` sets it on
//! a file before writing a byte; `file/mkdir-private` on a directory, inherited by what is
//! made inside it.

use windows_sys::Win32::Foundation::{CloseHandle, ERROR_SUCCESS, HANDLE, LocalFree};
use windows_sys::Win32::Security::Authorization::{
    EXPLICIT_ACCESS_W, NO_MULTIPLE_TRUSTEE, SE_FILE_OBJECT, SET_ACCESS, SetEntriesInAclW,
    SetNamedSecurityInfoW, TRUSTEE_IS_SID, TRUSTEE_IS_USER, TRUSTEE_W,
};
use windows_sys::Win32::Security::{
    ACL, DACL_SECURITY_INFORMATION, GetTokenInformation, NO_INHERITANCE,
    PROTECTED_DACL_SECURITY_INFORMATION, SUB_CONTAINERS_AND_OBJECTS_INHERIT, TOKEN_QUERY,
    TOKEN_USER, TokenUser,
};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

/// Full control of a file or directory, in its SPECIFIC rights (winnt.h's FILE_ALL_ACCESS,
/// whose windows-sys home needs a feature for one constant). Not GENERIC_ALL: given with
/// inheritance flags, a generic right is split into two ACEs — the mapped one for the
/// directory and an inherit-only generic one — so the directory would carry two entries.
const FILE_ALL_ACCESS: u32 = 0x001F_01FF;

/// Give `path` the owner-only DACL. `inherited` (a directory): files and directories made
/// inside it take the same single entry.
pub(crate) fn restrict_to_owner(path: &std::path::Path, inherited: bool) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    let failed = |code: u32| std::io::Error::from_raw_os_error(code as i32);
    // The current user's SID, from the process token.
    let mut token: HANDLE = std::ptr::null_mut();
    // SAFETY: the pseudo-handle of this process; `token` is written on success.
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
        return Err(std::io::Error::last_os_error());
    }
    let mut needed: u32 = 0;
    // SAFETY: a size query (null buffer, zero length); it fails and sets `needed`.
    unsafe { GetTokenInformation(token, TokenUser, std::ptr::null_mut(), 0, &mut needed) };
    // u64s, so the buffer is aligned for TOKEN_USER
    let mut buffer = vec![0u64; (needed as usize).div_ceil(8).max(1)];
    // SAFETY: `buffer` holds at least `needed` bytes.
    let queried = unsafe {
        GetTokenInformation(
            token,
            TokenUser,
            buffer.as_mut_ptr().cast(),
            needed,
            &mut needed,
        )
    };
    let query_error = std::io::Error::last_os_error();
    // SAFETY: opened above, closed once.
    unsafe { CloseHandle(token) };
    if queried == 0 {
        return Err(query_error);
    }
    // SAFETY: GetTokenInformation(TokenUser) filled the buffer with a TOKEN_USER, whose SID
    // points into the same buffer, alive until the end of this function.
    let user_sid = unsafe { (*buffer.as_ptr().cast::<TOKEN_USER>()).User.Sid };
    let entry = EXPLICIT_ACCESS_W {
        grfAccessPermissions: FILE_ALL_ACCESS,
        grfAccessMode: SET_ACCESS,
        grfInheritance: if inherited {
            SUB_CONTAINERS_AND_OBJECTS_INHERIT
        } else {
            NO_INHERITANCE
        },
        Trustee: TRUSTEE_W {
            pMultipleTrustee: std::ptr::null_mut(),
            MultipleTrusteeOperation: NO_MULTIPLE_TRUSTEE,
            TrusteeForm: TRUSTEE_IS_SID,
            TrusteeType: TRUSTEE_IS_USER,
            ptstrName: user_sid.cast(),
        },
    };
    let mut acl: *mut ACL = std::ptr::null_mut();
    // SAFETY: one entry, no old ACL; `acl` is allocated by the call and freed below.
    let made = unsafe { SetEntriesInAclW(1, &entry, std::ptr::null(), &mut acl) };
    if made != ERROR_SUCCESS {
        return Err(failed(made));
    }
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    // SAFETY: `wide` is NUL-terminated; `acl` is the ACL just built; owner and group are
    // left as they are (null).
    let set = unsafe {
        SetNamedSecurityInfoW(
            wide.as_ptr(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            acl,
            std::ptr::null(),
        )
    };
    // SAFETY: allocated by SetEntriesInAclW with LocalAlloc.
    unsafe { LocalFree(acl.cast()) };
    if set != ERROR_SUCCESS {
        return Err(failed(set));
    }
    Ok(())
}
