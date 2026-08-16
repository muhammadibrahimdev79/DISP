#![cfg(windows)]

use std::{ffi::OsStr, io, os::windows::ffi::OsStrExt, ptr};
use windows_sys::Win32::{
    Foundation::{CloseHandle, GetLastError, WAIT_OBJECT_0},
    Security::{
        CheckTokenMembership, CreateWellKnownSid, FreeSid, GetTokenInformation,
        Isolation::{CreateAppContainerProfile, DeleteAppContainerProfile},
        PSID, SECURITY_CAPABILITIES, SECURITY_MAX_SID_SIZE, TOKEN_QUERY,
        TokenIsLessPrivilegedAppContainer, WinBuiltinAnyPackageSid,
    },
    System::{
        Threading::{
            CREATE_UNICODE_ENVIRONMENT, CreateProcessW, DeleteProcThreadAttributeList,
            EXTENDED_STARTUPINFO_PRESENT, GetCurrentProcess, GetExitCodeProcess,
            InitializeProcThreadAttributeList, OpenProcessToken,
            PROC_THREAD_ATTRIBUTE_ALL_APPLICATION_PACKAGES_POLICY,
            PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES, PROCESS_INFORMATION, STARTUPINFOEXW,
            UpdateProcThreadAttribute, WaitForSingleObject,
        },
        WindowsProgramming::PROCESS_CREATION_ALL_APPLICATION_PACKAGES_OPT_OUT,
    },
};

fn wide(value: impl AsRef<OsStr>) -> Vec<u16> {
    value
        .as_ref()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

struct Profile(Vec<u16>);

impl Drop for Profile {
    fn drop(&mut self) {
        // SAFETY: this is the same terminated name used to create the test profile.
        unsafe { DeleteAppContainerProfile(self.0.as_ptr()) };
    }
}

#[test]
fn windows_minimal_lpac_contract_matches_the_os_documentation() {
    const CHILD: &str = "DISP_LPAC_OS_PROBE_CHILD";
    if std::env::var_os(CHILD).is_some() {
        let mut token = ptr::null_mut();
        // SAFETY: the output pointer is writable and the process pseudo-handle is valid.
        assert_ne!(
            unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) },
            0,
            "could not open LPAC child token: {}",
            io::Error::last_os_error()
        );
        let mut is_lpac = 0u32;
        let mut returned = 0u32;
        // SAFETY: the token is live and the DWORD output buffer is correctly sized.
        let queried = unsafe {
            GetTokenInformation(
                token,
                TokenIsLessPrivilegedAppContainer,
                (&raw mut is_lpac).cast(),
                std::mem::size_of_val(&is_lpac) as u32,
                &mut returned,
            )
        };
        if queried != 0 {
            assert_ne!(is_lpac, 0, "child token does not report LPAC identity");
        } else {
            assert_eq!(
                io::Error::last_os_error().raw_os_error(),
                Some(87),
                "unexpected TokenIsLessPrivilegedAppContainer query failure"
            );
        }
        let mut any_package_sid = [0u8; SECURITY_MAX_SID_SIZE as usize];
        let mut sid_bytes = any_package_sid.len() as u32;
        // SAFETY: the fixed buffer is the documented maximum SID size.
        assert_ne!(
            unsafe {
                CreateWellKnownSid(
                    WinBuiltinAnyPackageSid,
                    ptr::null_mut(),
                    any_package_sid.as_mut_ptr().cast(),
                    &mut sid_bytes,
                )
            },
            0
        );
        let mut is_member = 0;
        // SAFETY: a null token asks Windows to use the effective process token.
        assert_ne!(
            unsafe {
                CheckTokenMembership(
                    ptr::null_mut(),
                    any_package_sid.as_mut_ptr().cast(),
                    &mut is_member,
                )
            },
            0,
            "could not query ALL_APPLICATION_PACKAGES membership: {}",
            io::Error::last_os_error()
        );
        assert_eq!(
            is_member, 0,
            "LPAC token unexpectedly enables ALL_APPLICATION_PACKAGES"
        );
        // SAFETY: OpenProcessToken returned ownership of this handle.
        unsafe { CloseHandle(token) };
        return;
    }

    let name = wide(format!("DISP.Lpac.Probe.{}", std::process::id()));
    let _profile = Profile(name.clone());
    let display = wide("DISP LPAC OS contract probe");
    let description = wide("Temporary DISP LPAC test profile");
    let mut sid: PSID = ptr::null_mut();
    // SAFETY: all strings are terminated and a zero capability count permits null.
    let created = unsafe {
        CreateAppContainerProfile(
            name.as_ptr(),
            display.as_ptr(),
            description.as_ptr(),
            ptr::null(),
            0,
            &mut sid,
        )
    };
    assert_eq!(created, 0, "profile creation HRESULT {created:#x}");
    assert!(!sid.is_null());

    let capabilities = SECURITY_CAPABILITIES {
        AppContainerSid: sid,
        Capabilities: ptr::null_mut(),
        CapabilityCount: 0,
        Reserved: 0,
    };
    let package_policy = PROCESS_CREATION_ALL_APPLICATION_PACKAGES_OPT_OUT;
    let mut bytes = 0usize;
    // SAFETY: documented sizing call.
    unsafe { InitializeProcThreadAttributeList(ptr::null_mut(), 2, 0, &mut bytes) };
    assert_ne!(bytes, 0);
    let mut storage = vec![0usize; bytes.div_ceil(std::mem::size_of::<usize>())];
    let attributes = storage.as_mut_ptr().cast();
    // SAFETY: storage is aligned and sized by the preceding call.
    assert_ne!(
        unsafe { InitializeProcThreadAttributeList(attributes, 2, 0, &mut bytes) },
        0
    );
    // SAFETY: values remain live through process creation.
    assert_ne!(
        unsafe {
            UpdateProcThreadAttribute(
                attributes,
                0,
                PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES as usize,
                (&raw const capabilities).cast(),
                std::mem::size_of_val(&capabilities),
                ptr::null_mut(),
                ptr::null(),
            )
        },
        0
    );
    // SAFETY: values remain live through process creation.
    assert_ne!(
        unsafe {
            UpdateProcThreadAttribute(
                attributes,
                0,
                PROC_THREAD_ATTRIBUTE_ALL_APPLICATION_PACKAGES_POLICY as usize,
                (&raw const package_policy).cast(),
                std::mem::size_of_val(&package_policy),
                ptr::null_mut(),
                ptr::null(),
            )
        },
        0
    );

    let executable = wide(std::env::current_exe().unwrap());
    let mut command = wide(format!(
        "\"{}\" --exact windows_minimal_lpac_contract_matches_the_os_documentation --nocapture",
        std::env::current_exe().unwrap().display()
    ));
    let mut startup = STARTUPINFOEXW::default();
    startup.StartupInfo.cb = std::mem::size_of::<STARTUPINFOEXW>() as u32;
    startup.lpAttributeList = attributes;
    let mut process = PROCESS_INFORMATION::default();
    // The inherited environment carries only the private child marker for dispatch.
    unsafe { std::env::set_var(CHILD, "1") };
    // SAFETY: all pointers are terminated/live; the startup attributes are initialized.
    let launched = unsafe {
        CreateProcessW(
            executable.as_ptr(),
            command.as_mut_ptr(),
            ptr::null(),
            ptr::null(),
            0,
            CREATE_UNICODE_ENVIRONMENT | EXTENDED_STARTUPINFO_PRESENT,
            ptr::null(),
            ptr::null(),
            &raw const startup.StartupInfo,
            &mut process,
        )
    };
    unsafe { std::env::remove_var(CHILD) };
    let launch_error = unsafe { GetLastError() };
    // SAFETY: the list was initialized exactly once.
    unsafe { DeleteProcThreadAttributeList(attributes) };
    // SAFETY: the profile API returned this SID allocation.
    unsafe { FreeSid(sid) };
    assert_ne!(
        launched,
        0,
        "minimal LPAC launch failed: {}",
        io::Error::from_raw_os_error(launch_error as i32)
    );
    assert_eq!(
        unsafe { WaitForSingleObject(process.hProcess, 30_000) },
        WAIT_OBJECT_0
    );
    let mut exit_code = u32::MAX;
    assert_ne!(
        unsafe { GetExitCodeProcess(process.hProcess, &mut exit_code) },
        0
    );
    unsafe {
        CloseHandle(process.hThread);
        CloseHandle(process.hProcess);
    }
    assert_eq!(exit_code, 0, "LPAC child failed with exit code {exit_code}");
}
