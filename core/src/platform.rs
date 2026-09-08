//! Small Windows API boundary: known folders, ACL checks, atomic replacement,
//! and read-only Service Control Manager access. No localized output parsing.
use crate::{Error, Result};
use std::path::{Path, PathBuf};

pub fn ensure_windows() -> Result<()> {
    if cfg!(all(windows, target_arch = "x86_64")) {
        Ok(())
    } else {
        Err(Error::UnsupportedPlatform)
    }
}

#[cfg(not(windows))]
pub fn root_dir() -> Result<PathBuf> {
    Err(Error::UnsupportedPlatform)
}
#[cfg(not(windows))]
pub fn system_dir() -> Result<PathBuf> {
    Err(Error::UnsupportedPlatform)
}
#[cfg(not(windows))]
pub fn hosts_path() -> Result<PathBuf> {
    Err(Error::UnsupportedPlatform)
}
#[cfg(not(windows))]
pub fn is_admin() -> bool {
    false
}
#[cfg(not(windows))]
pub fn prepare_root(_: &Path) -> Result<()> {
    Err(Error::UnsupportedPlatform)
}
#[cfg(not(windows))]
pub fn query_service(_: &str) -> Result<Option<crate::service::ServiceSnapshot>> {
    Err(Error::UnsupportedPlatform)
}

#[cfg(windows)]
pub use native::{
    hosts_path, is_admin, prepare_root, query_service, replace_file, root_dir, system_dir,
};

#[cfg(windows)]
mod native {
    use super::*;
    use crate::service::{ServiceSnapshot, ServiceState};
    use std::ffi::c_void;
    use std::fs;
    use std::os::windows::ffi::OsStrExt;
    use std::ptr::{null, null_mut};

    type Handle = *mut c_void;
    #[repr(C)]
    struct SecurityAttributes {
        length: u32,
        descriptor: *mut c_void,
        inherit: i32,
    }
    #[repr(C)]
    struct Acl {
        revision: u8,
        reserved: u8,
        size: u16,
        count: u16,
        reserved2: u16,
    }
    #[repr(C)]
    struct AceHeader {
        kind: u8,
        flags: u8,
        size: u16,
    }
    #[repr(C)]
    struct AllowedAce {
        header: AceHeader,
        mask: u32,
        sid_start: u32,
    }
    #[repr(C)]
    #[derive(Default)]
    struct StatusProcess {
        service_type: u32,
        state: u32,
        accepted: u32,
        win32_exit: u32,
        service_exit: u32,
        checkpoint: u32,
        wait_hint: u32,
        pid: u32,
        flags: u32,
    }
    #[repr(C)]
    struct QueryConfig {
        service_type: u32,
        start_type: u32,
        error_control: u32,
        binary: *mut u16,
        group: *mut u16,
        tag: u32,
        dependencies: *mut u16,
        account: *mut u16,
        display: *mut u16,
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn GetLastError() -> u32;
        fn GetSystemDirectoryW(buffer: *mut u16, size: u32) -> u32;
        fn LocalFree(memory: Handle) -> Handle;
        fn CreateDirectoryW(path: *const u16, security: *const SecurityAttributes) -> i32;
        fn ReplaceFileW(
            replaced: *const u16,
            replacement: *const u16,
            backup: *const u16,
            flags: u32,
            exclude: Handle,
            reserved: Handle,
        ) -> i32;
        fn MoveFileExW(from: *const u16, to: *const u16, flags: u32) -> i32;
    }
    #[link(name = "shell32")]
    extern "system" {
        fn SHGetFolderPathW(
            window: Handle,
            folder: i32,
            token: Handle,
            flags: u32,
            path: *mut u16,
        ) -> i32;
        fn IsUserAnAdmin() -> i32;
    }
    #[link(name = "advapi32")]
    extern "system" {
        fn ConvertStringSecurityDescriptorToSecurityDescriptorW(
            text: *const u16,
            revision: u32,
            descriptor: *mut Handle,
            size: *mut u32,
        ) -> i32;
        fn GetNamedSecurityInfoW(
            name: *const u16,
            object_type: i32,
            information: u32,
            owner: *mut Handle,
            group: *mut Handle,
            dacl: *mut *mut Acl,
            sacl: *mut *mut Acl,
            descriptor: *mut Handle,
        ) -> u32;
        fn ConvertSidToStringSidW(sid: Handle, text: *mut *mut u16) -> i32;
        fn GetAce(acl: *const Acl, index: u32, ace: *mut Handle) -> i32;
        fn OpenSCManagerW(machine: *const u16, database: *const u16, access: u32) -> Handle;
        fn OpenServiceW(manager: Handle, name: *const u16, access: u32) -> Handle;
        fn CloseServiceHandle(handle: Handle) -> i32;
        fn QueryServiceStatusEx(
            service: Handle,
            level: i32,
            buffer: *mut u8,
            size: u32,
            needed: *mut u32,
        ) -> i32;
        fn QueryServiceConfigW(
            service: Handle,
            buffer: *mut QueryConfig,
            size: u32,
            needed: *mut u32,
        ) -> i32;
    }

    fn wide(value: &std::ffi::OsStr) -> Result<Vec<u16>> {
        let mut out: Vec<_> = value.encode_wide().collect();
        if out.contains(&0) {
            return Err(Error::Invalid("NUL in Windows string".into()));
        }
        out.push(0);
        Ok(out)
    }
    fn wide_str(value: &str) -> Result<Vec<u16>> {
        wide(std::ffi::OsStr::new(value))
    }
    fn os_error(operation: &str) -> Error {
        // SAFETY: GetLastError has no preconditions and is read immediately.
        Error::Windows {
            operation: operation.into(),
            code: unsafe { GetLastError() },
        }
    }
    struct LocalMemory(Handle);
    impl Drop for LocalMemory {
        fn drop(&mut self) {
            if !self.0.is_null() {
                unsafe {
                    LocalFree(self.0);
                }
            }
        }
    }
    struct ServiceHandle(Handle);
    impl Drop for ServiceHandle {
        fn drop(&mut self) {
            if !self.0.is_null() {
                unsafe {
                    CloseServiceHandle(self.0);
                }
            }
        }
    }
    // SAFETY: callers use NUL-terminated strings owned by Windows API buffers.
    unsafe fn from_wide(pointer: *const u16) -> Result<String> {
        if pointer.is_null() {
            return Ok(String::new());
        }
        let mut len = 0;
        while len < 32768 && *pointer.add(len) != 0 {
            len += 1;
        }
        if len == 32768 {
            return Err(Error::Invalid("Overlong Windows string".into()));
        }
        Ok(String::from_utf16_lossy(std::slice::from_raw_parts(
            pointer, len,
        )))
    }
    fn sid_text(sid: Handle) -> Result<String> {
        let mut ptr = null_mut();
        // SAFETY: SID comes from GetNamedSecurityInfoW or a validated ACE.
        if unsafe { ConvertSidToStringSidW(sid, &mut ptr) } == 0 {
            return Err(os_error("ConvertSidToStringSidW"));
        }
        let _allocation = LocalMemory(ptr.cast());
        unsafe { from_wide(ptr) }
    }
    fn trusted_sid(sid: &str) -> bool {
        matches!(
            sid,
            "S-1-5-18"
                | "S-1-5-32-544"
                | "S-1-5-80-956008885-3418522649-1831038044-1853292631-2271478464"
        )
    }

    /// Fail closed on writable/owner-controlled non-administrator paths.
    fn verify_acl(path: &Path) -> Result<()> {
        let name = wide(path.as_os_str())?;
        let mut owner = null_mut();
        let mut dacl = null_mut();
        let mut descriptor = null_mut();
        let code = unsafe {
            GetNamedSecurityInfoW(
                name.as_ptr(),
                1,
                0x5,
                &mut owner,
                null_mut(),
                &mut dacl,
                null_mut(),
                &mut descriptor,
            )
        };
        if code != 0 {
            return Err(Error::Windows {
                operation: "Read file ACL".into(),
                code,
            });
        }
        let _allocation = LocalMemory(descriptor);
        if owner.is_null() || dacl.is_null() || !trusted_sid(&sid_text(owner)?) {
            return Err(Error::Security(format!(
                "Untrusted owner or unrestricted DACL: {}",
                path.display()
            )));
        }
        // Write data/append/EA/attributes, delete/delete-child, change DACL/owner,
        // generic write/all. Read/execute grants to ordinary users are allowed.
        const WRITE_MASK: u32 = 0x0000_0156 | 0x000D_0000 | 0x5000_0000;
        for index in 0..unsafe { (*dacl).count } {
            let mut pointer = null_mut();
            if unsafe { GetAce(dacl, u32::from(index), &mut pointer) } == 0 {
                return Err(os_error("GetAce"));
            }
            let header = unsafe { &*(pointer as *const AceHeader) };
            if header.kind == 1 {
                continue;
            } // ACCESS_DENIED_ACE cannot grant writes.
            if header.kind != 0 || usize::from(header.size) < std::mem::size_of::<AllowedAce>() {
                return Err(Error::Security(
                    "Unsupported ACL entry; ask an administrator to review permissions".into(),
                ));
            }
            let ace = unsafe { &*(pointer as *const AllowedAce) };
            // INHERIT_ONLY applies to children, not this object. Children are
            // independently checked before use, so it does not grant here.
            if ace.header.flags & 0x08 == 0 && ace.mask & WRITE_MASK != 0 {
                let sid = (&ace.sid_start as *const u32).cast_mut().cast();
                if !trusted_sid(&sid_text(sid)?) {
                    return Err(Error::Security(format!(
                        "Non-administrator write permission: {}",
                        path.display()
                    )));
                }
            }
        }
        Ok(())
    }

    pub fn system_dir() -> Result<PathBuf> {
        let mut buffer = vec![0u16; 32768];
        let length = unsafe { GetSystemDirectoryW(buffer.as_mut_ptr(), buffer.len() as u32) };
        if length == 0 || length as usize >= buffer.len() {
            return Err(os_error("GetSystemDirectoryW"));
        }
        Ok(PathBuf::from(String::from_utf16_lossy(
            &buffer[..length as usize],
        )))
    }
    pub fn root_dir() -> Result<PathBuf> {
        super::ensure_windows()?;
        let mut buffer = [0u16; 260];
        // CSIDL_PROGRAM_FILES: x64 process receives protected 64-bit Program Files.
        let code =
            unsafe { SHGetFolderPathW(null_mut(), 0x26, null_mut(), 0, buffer.as_mut_ptr()) };
        if code < 0 {
            return Err(Error::Windows {
                operation: "Program Files known folder".into(),
                code: code as u32,
            });
        }
        let length = buffer
            .iter()
            .position(|c| *c == 0)
            .ok_or_else(|| Error::Invalid("Invalid known folder".into()))?;
        Ok(PathBuf::from(String::from_utf16_lossy(&buffer[..length])).join("TandemWorkbench"))
    }
    pub fn hosts_path() -> Result<PathBuf> {
        Ok(system_dir()?.join("drivers").join("etc").join("hosts"))
    }
    pub fn is_admin() -> bool {
        unsafe { IsUserAnAdmin() != 0 }
    }

    pub fn prepare_root(root: &Path) -> Result<()> {
        super::ensure_windows()?;
        if !is_admin() {
            return Err(Error::AdministratorRequired);
        }
        if root != root_dir()? {
            return Err(Error::Security(
                "Privileged operations require the fixed protected root".into(),
            ));
        }
        crate::files::reject_links(root)?;
        verify_acl(
            root.parent()
                .ok_or_else(|| Error::Security("Missing Program Files parent".into()))?,
        )?;
        if !root.exists() {
            let sddl = wide_str("O:BAG:BAD:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)(A;OICI;FRFX;;;BU)")?;
            let mut descriptor = null_mut();
            if unsafe {
                ConvertStringSecurityDescriptorToSecurityDescriptorW(
                    sddl.as_ptr(),
                    1,
                    &mut descriptor,
                    null_mut(),
                )
            } == 0
            {
                return Err(os_error("Create secure directory ACL"));
            }
            let _allocation = LocalMemory(descriptor);
            let attributes = SecurityAttributes {
                length: std::mem::size_of::<SecurityAttributes>() as u32,
                descriptor,
                inherit: 0,
            };
            let path = wide(root.as_os_str())?;
            if unsafe { CreateDirectoryW(path.as_ptr(), &attributes) } == 0 {
                let e = unsafe { GetLastError() };
                if e != 183 {
                    return Err(Error::Windows {
                        operation: "Create protected root".into(),
                        code: e,
                    });
                }
            }
        }
        let mut pending = vec![root.to_path_buf()];
        let mut count = 0;
        while let Some(path) = pending.pop() {
            count += 1;
            if count > 8192 {
                return Err(Error::Security(
                    "Protected root exceeds file-count policy".into(),
                ));
            }
            crate::files::reject_links(&path)?;
            verify_acl(&path)?;
            if path.is_dir() {
                for entry in fs::read_dir(path)? {
                    pending.push(entry?.path());
                }
            }
        }
        Ok(())
    }

    pub fn replace_file(source: &Path, destination: &Path) -> Result<()> {
        let source = wide(source.as_os_str())?;
        let destination_w = wide(destination.as_os_str())?;
        let success = if destination.exists() {
            unsafe {
                ReplaceFileW(
                    destination_w.as_ptr(),
                    source.as_ptr(),
                    null(),
                    0,
                    null_mut(),
                    null_mut(),
                )
            }
        } else {
            // WRITE_THROUGH; do not replace a destination created by a racer.
            unsafe { MoveFileExW(source.as_ptr(), destination_w.as_ptr(), 0x8) }
        };
        if success == 0 {
            return Err(os_error("Atomic file replacement"));
        }
        Ok(())
    }

    pub fn query_service(name: &str) -> Result<Option<ServiceSnapshot>> {
        let name = wide_str(name)?;
        let manager = unsafe { OpenSCManagerW(null(), null(), 1) };
        if manager.is_null() {
            return Err(os_error("OpenSCManagerW"));
        }
        let manager = ServiceHandle(manager);
        let service = unsafe { OpenServiceW(manager.0, name.as_ptr(), 0x5) };
        if service.is_null() {
            let code = unsafe { GetLastError() };
            if code == 1060 {
                return Ok(None);
            }
            return Err(Error::Windows {
                operation: "OpenServiceW".into(),
                code,
            });
        }
        let service = ServiceHandle(service);
        let mut status = StatusProcess::default();
        let mut needed = 0;
        if unsafe {
            QueryServiceStatusEx(
                service.0,
                0,
                (&mut status as *mut StatusProcess).cast(),
                std::mem::size_of::<StatusProcess>() as u32,
                &mut needed,
            )
        } == 0
        {
            return Err(os_error("QueryServiceStatusEx"));
        }
        unsafe {
            QueryServiceConfigW(service.0, null_mut(), 0, &mut needed);
        }
        if needed == 0 || needed > 65536 {
            return Err(os_error("QueryServiceConfigW size"));
        }
        // usize storage provides pointer alignment for QUERY_SERVICE_CONFIGW.
        let mut buffer = vec![0usize; (needed as usize).div_ceil(std::mem::size_of::<usize>())];
        let config_pointer = buffer.as_mut_ptr().cast::<QueryConfig>();
        if unsafe { QueryServiceConfigW(service.0, config_pointer, needed, &mut needed) } == 0 {
            return Err(os_error("QueryServiceConfigW"));
        }
        let config = unsafe { &*config_pointer };
        Ok(Some(ServiceSnapshot {
            state: ServiceState::from_code(status.state),
            binary_path: unsafe { from_wide(config.binary) }?,
            start_type: config.start_type,
            account: unsafe { from_wide(config.account) }?,
            process_id: status.pid,
        }))
    }
}
