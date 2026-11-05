//! Windows's owner-only (KI-365): a protected DACL with one entry, full control for the user
//! this process runs as. Nothing inherited from the directory above survives, so no group,
//! no `Users`, no `Everyone` — what 0600 / 0700 mean on Unix. `file/spit-private` CREATES
//! its file with it ([`create_private_file`]): the file never exists with its directory's
//! entries, not even for the moment between a create and a restrict. `file/mkdir-private`
//! sets it on a directory ([`restrict_to_owner`]), inherited by what is made inside it.

use std::os::windows::ffi::OsStrExt;

use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_ALREADY_EXISTS, ERROR_SUCCESS, GENERIC_WRITE, GetLastError, HANDLE,
    INVALID_HANDLE_VALUE, LocalFree,
};
use windows_sys::Win32::Security::Authorization::{
    EXPLICIT_ACCESS_W, NO_MULTIPLE_TRUSTEE, SE_FILE_OBJECT, SET_ACCESS, SetEntriesInAclW,
    SetNamedSecurityInfoW, SetSecurityInfo, TRUSTEE_IS_SID, TRUSTEE_IS_USER, TRUSTEE_W,
};
use windows_sys::Win32::Security::{
    ACL, DACL_SECURITY_INFORMATION, GetTokenInformation, InitializeSecurityDescriptor,
    NO_INHERITANCE, PROTECTED_DACL_SECURITY_INFORMATION, SE_DACL_PROTECTED, SECURITY_ATTRIBUTES,
    SECURITY_DESCRIPTOR, SUB_CONTAINERS_AND_OBJECTS_INHERIT, SetSecurityDescriptorControl,
    SetSecurityDescriptorDacl, TOKEN_QUERY, TOKEN_USER, TokenUser,
};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_ATTRIBUTE_NORMAL, OPEN_ALWAYS, WRITE_DAC,
};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

/// Full control of a file or directory, in its SPECIFIC rights (winnt.h's FILE_ALL_ACCESS,
/// whose windows-sys home needs a feature for one constant). Not GENERIC_ALL: given with
/// inheritance flags, a generic right is split into two ACEs — the mapped one for the
/// directory and an inherit-only generic one — so the directory would carry two entries.
const FILE_ALL_ACCESS: u32 = 0x001F_01FF;

/// winnt.h's SECURITY_DESCRIPTOR_REVISION (its windows-sys home is another feature).
const SECURITY_DESCRIPTOR_REVISION: u32 = 1;

/// The owner-only ACL, freed when dropped.
struct OwnerAcl(*mut ACL);

impl Drop for OwnerAcl {
    fn drop(&mut self) {
        // SAFETY: allocated by SetEntriesInAclW with LocalAlloc, freed once.
        unsafe { LocalFree(self.0.cast()) };
    }
}

/// The current user's SID, read from the process token, in a buffer that holds it.
fn current_user() -> std::io::Result<Vec<u64>> {
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
    Ok(buffer)
}

/// The one-entry ACL: full control for the current user. `inherited` (a directory): files
/// and directories made inside take the same entry.
fn owner_acl(inherited: bool) -> std::io::Result<OwnerAcl> {
    let user = current_user()?;
    // SAFETY: GetTokenInformation(TokenUser) filled the buffer with a TOKEN_USER, whose SID
    // points into the same buffer, alive until the end of this function — SetEntriesInAclW
    // copies the SID into the ACL it allocates.
    let user_sid = unsafe { (*user.as_ptr().cast::<TOKEN_USER>()).User.Sid };
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
    // SAFETY: one entry, no old ACL; `acl` is allocated by the call, owned by OwnerAcl.
    let made = unsafe { SetEntriesInAclW(1, &entry, std::ptr::null(), &mut acl) };
    if made != ERROR_SUCCESS {
        return Err(std::io::Error::from_raw_os_error(made as i32));
    }
    Ok(OwnerAcl(acl))
}

fn wide(path: &std::path::Path) -> Vec<u16> {
    path.as_os_str().encode_wide().chain(Some(0)).collect()
}

/// The DACL and protection flags every owner-only set uses.
const OWNER_ONLY_INFORMATION: u32 = DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION;

/// Give the existing `path` the owner-only DACL. `inherited` (a directory): files and
/// directories made inside it take the same single entry.
pub(crate) fn restrict_to_owner(path: &std::path::Path, inherited: bool) -> std::io::Result<()> {
    let acl = owner_acl(inherited)?;
    let name = wide(path);
    // SAFETY: `name` is NUL-terminated; the ACL is live; owner and group are left as they
    // are (null).
    let set = unsafe {
        SetNamedSecurityInfoW(
            name.as_ptr(),
            SE_FILE_OBJECT,
            OWNER_ONLY_INFORMATION,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            acl.0,
            std::ptr::null(),
        )
    };
    if set != ERROR_SUCCESS {
        return Err(std::io::Error::from_raw_os_error(set as i32));
    }
    Ok(())
}

/// Open `path` for writing, empty and owner-only, as 0600 is on Unix. A new file is CREATED
/// with the owner-only DACL (a security descriptor handed to `CreateFileW`, protected so
/// the directory's inheritable entries are not merged in), so no one else could open it at
/// any moment. A file already there — whose DACL `CreateFileW` leaves alone — gets the
/// owner-only DACL through the open handle before it is emptied, so the secret is never
/// written under its old one. Shared with nobody while it is open.
pub(crate) fn create_private_file(path: &std::path::Path) -> std::io::Result<std::fs::File> {
    use std::os::windows::io::FromRawHandle;
    let acl = owner_acl(false)?;
    let mut descriptor = SECURITY_DESCRIPTOR::default();
    let descriptor_pointer: *mut core::ffi::c_void =
        (&mut descriptor as *mut SECURITY_DESCRIPTOR).cast();
    // SAFETY: a live, zeroed SECURITY_DESCRIPTOR, initialised in place (an absolute
    // descriptor, pointing at the ACL, which outlives every use of it below).
    unsafe {
        if InitializeSecurityDescriptor(descriptor_pointer, SECURITY_DESCRIPTOR_REVISION) == 0
            || SetSecurityDescriptorDacl(descriptor_pointer, 1, acl.0, 0) == 0
            || SetSecurityDescriptorControl(
                descriptor_pointer,
                SE_DACL_PROTECTED,
                SE_DACL_PROTECTED,
            ) == 0
        {
            return Err(std::io::Error::last_os_error());
        }
    }
    let attributes = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: descriptor_pointer,
        bInheritHandle: 0,
    };
    let name = wide(path);
    // SAFETY: a NUL-terminated name and live security attributes; share mode 0 lets no one
    // else open the file while this handle is open.
    let handle = unsafe {
        CreateFileW(
            name.as_ptr(),
            GENERIC_WRITE | WRITE_DAC,
            0,
            &attributes,
            OPEN_ALWAYS,
            FILE_ATTRIBUTE_NORMAL,
            std::ptr::null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: read straight after the call that set it; OPEN_ALWAYS sets it when the file
    // was already there.
    let existed = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;
    // SAFETY: a handle just opened, owned from here by the File (closed once, on drop).
    let file = unsafe { std::fs::File::from_raw_handle(handle.cast()) };
    if existed {
        // SAFETY: an open handle with WRITE_DAC; the ACL is live; owner and group untouched.
        let set = unsafe {
            SetSecurityInfo(
                handle,
                SE_FILE_OBJECT,
                OWNER_ONLY_INFORMATION,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                acl.0,
                std::ptr::null(),
            )
        };
        if set != ERROR_SUCCESS {
            return Err(std::io::Error::from_raw_os_error(set as i32));
        }
        file.set_len(0)?;
    }
    Ok(file)
}
