use disp::{
    component_host::{ComponentCommand, ComponentError},
    limits,
};
use std::{fs, path::PathBuf, process::Command, sync::OnceLock};

fn component_probe() -> &'static PathBuf {
    static PROBE: OnceLock<PathBuf> = OnceLock::new();
    PROBE.get_or_init(|| {
        let directory = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("component-tests")
            .join("native-probe");
        fs::create_dir_all(&directory).unwrap();
        let source = directory.join("component.c");
        #[cfg(windows)]
        let executable = directory.join("component.exe");
        #[cfg(not(windows))]
        let executable = directory.join("component");
        fs::write(
            &source,
            r#"#include <stdio.h>
#include <errno.h>
#include <stdlib.h>
#include <string.h>
#ifdef _WIN32
#include <fcntl.h>
#include <io.h>
#include <winsock2.h>
#include <ws2tcpip.h>
#include <windows.h>
#else
#include <sys/socket.h>
#include <unistd.h>
#endif
#ifndef DISP_PROBE_VARIANT
#define DISP_PROBE_VARIANT 0
#endif
static volatile int disp_probe_variant = DISP_PROBE_VARIANT;
int main(int argc, char **argv) {
    unsigned char buffer[8192];
    size_t count;
    const char *mode = argc > 1 ? argv[1] : "echo";
    if (disp_probe_variant < 0) return 15;
#ifdef _WIN32
    if (_setmode(_fileno(stdin), _O_BINARY) == -1 ||
        _setmode(_fileno(stdout), _O_BINARY) == -1) return 12;
#endif
    if (getenv("PATH") != NULL || getenv("DISP_COMPONENT_PROTOCOL") == NULL ||
        strcmp(getenv("DISP_COMPONENT_PROTOCOL"), "disp.component.v1") != 0) return 9;
    if (strcmp(mode, "echo") == 0) {
        while ((count = fread(buffer, 1, sizeof(buffer), stdin)) != 0) {
            if (fwrite(buffer, 1, count, stdout) != count) return 10;
        }
        return ferror(stdin) ? 11 : 0;
    }
    while (fread(buffer, 1, sizeof(buffer), stdin) != 0) {}
#ifdef _WIN32
    if (strcmp(mode, "token") == 0) {
        HANDLE token = NULL;
        DWORD integrity_bytes = 0, privilege_bytes = 0;
        unsigned char *integrity = NULL, *privileges = NULL;
        DWORD index, enabled = 0, rid;
        DWORD is_appcontainer = 0, appcontainer_bytes = sizeof(is_appcontainer);
        DWORD is_lpac = 0, lpac_bytes = sizeof(is_lpac);
        BYTE any_package_sid[SECURITY_MAX_SID_SIZE];
        DWORD any_package_bytes = sizeof(any_package_sid);
        BOOL any_package_member = FALSE;
        TOKEN_MANDATORY_LABEL *label;
        TOKEN_PRIVILEGES *list;
        if (!OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &token)) return 16;
        if (!GetTokenInformation(token, TokenIsAppContainer, &is_appcontainer,
            appcontainer_bytes, &appcontainer_bytes) || !is_appcontainer) return 25;
        if (!GetTokenInformation(token, TokenIsLessPrivilegedAppContainer, &is_lpac,
            lpac_bytes, &lpac_bytes)) {
            if (GetLastError() != ERROR_INVALID_PARAMETER) return 33;
        } else if (!is_lpac) return 34;
        if (!CreateWellKnownSid(WinBuiltinAnyPackageSid, NULL, any_package_sid,
            &any_package_bytes)) return 35;
        if (!CheckTokenMembership(NULL, any_package_sid, &any_package_member)) return 36;
        if (any_package_member) return 37;
        GetTokenInformation(token, TokenIntegrityLevel, NULL, 0, &integrity_bytes);
        if (!integrity_bytes || !(integrity = malloc(integrity_bytes))) return 17;
        if (!GetTokenInformation(token, TokenIntegrityLevel, integrity, integrity_bytes,
            &integrity_bytes)) return 18;
        label = (TOKEN_MANDATORY_LABEL *)integrity;
        index = (DWORD)(*GetSidSubAuthorityCount(label->Label.Sid) - 1);
        rid = *GetSidSubAuthority(label->Label.Sid, index);
        if (rid != SECURITY_MANDATORY_LOW_RID) return 19;
        GetTokenInformation(token, TokenPrivileges, NULL, 0, &privilege_bytes);
        if (!privilege_bytes || !(privileges = malloc(privilege_bytes))) return 20;
        if (!GetTokenInformation(token, TokenPrivileges, privileges, privilege_bytes,
            &privilege_bytes)) return 21;
        list = (TOKEN_PRIVILEGES *)privileges;
        for (index = 0; index < list->PrivilegeCount; ++index) {
            if ((list->Privileges[index].Attributes & SE_PRIVILEGE_ENABLED) != 0) ++enabled;
        }
        if (enabled > 1) return 22;
        if (argc > 2) {
            HANDLE file = CreateFileA(argv[2], GENERIC_READ,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE, NULL,
                OPEN_EXISTING, FILE_ATTRIBUTE_NORMAL, NULL);
            if (file != INVALID_HANDLE_VALUE) { CloseHandle(file); return 23; }
            if (GetLastError() != ERROR_ACCESS_DENIED) return 24;
            file = CreateFileA(argv[2], GENERIC_WRITE,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE, NULL,
                OPEN_EXISTING, FILE_ATTRIBUTE_NORMAL, NULL);
            if (file != INVALID_HANDLE_VALUE) { CloseHandle(file); return 26; }
            if (GetLastError() != ERROR_ACCESS_DENIED) return 27;
        }
        {
            WSADATA data;
            SOCKET socket_handle;
            WSAEVENT event_handle;
            WSANETWORKEVENTS events;
            struct sockaddr_in endpoint;
            int connected, network_error;
            DWORD waited;
            network_error = WSAStartup(MAKEWORD(2, 2), &data);
            if (network_error == WSASYSCALLFAILURE) {
                /* LPAC can deny the provider before socket initialization. */
            } else if (network_error != 0) {
                fprintf(stderr, "WSAStartup error %d", network_error);
                return 28;
            } else {
                socket_handle = socket(AF_INET, SOCK_STREAM, IPPROTO_TCP);
                if (socket_handle == INVALID_SOCKET) {
                    network_error = WSAGetLastError();
                } else {
                    event_handle = WSACreateEvent();
                    if (event_handle == WSA_INVALID_EVENT) return 30;
                    if (WSAEventSelect(socket_handle, event_handle, FD_CONNECT) != 0) return 31;
                    memset(&endpoint, 0, sizeof(endpoint));
                    endpoint.sin_family = AF_INET;
                    endpoint.sin_port = htons(9);
                    endpoint.sin_addr.s_addr = htonl(0xc0000201UL);
                    connected = connect(socket_handle, (struct sockaddr *)&endpoint,
                        sizeof(endpoint));
                    network_error = connected == SOCKET_ERROR ? WSAGetLastError() : 0;
                    if (network_error == WSAEWOULDBLOCK) {
                        waited = WSAWaitForMultipleEvents(1, &event_handle, FALSE, 3000, FALSE);
                        if (waited == WSA_WAIT_EVENT_0) {
                            memset(&events, 0, sizeof(events));
                            if (WSAEnumNetworkEvents(socket_handle, event_handle, &events) != 0)
                                return 32;
                            network_error = events.iErrorCode[FD_CONNECT_BIT];
                        } else {
                            network_error = WSAETIMEDOUT;
                        }
                    }
                    WSACloseEvent(event_handle);
                    closesocket(socket_handle);
                }
                WSACleanup();
                if (network_error != WSAEACCES) {
                    fprintf(stderr, "network error %d", network_error);
                    return 29;
                }
            }
        }
        {
            const unsigned char response[18] = {
                'D','I','S','P','C','M','P','1', 0,0,0,0,0,0,0,2, 'o','k'
            };
            fwrite(response, 1, sizeof(response), stdout);
        }
        free(privileges);
        free(integrity);
        CloseHandle(token);
        return 0;
    }
#endif
#ifndef _WIN32
    if (strcmp(mode, "network") == 0) {
        int fd;
        errno = 0;
        fd = socket(AF_INET, SOCK_STREAM, 0);
        if (fd >= 0) { close(fd); return 13; }
        if (errno != EPERM) return 14;
        {
            const unsigned char response[18] = {
                'D','I','S','P','C','M','P','1', 0,0,0,0,0,0,0,2, 'o','k'
            };
            fwrite(response, 1, sizeof(response), stdout);
        }
        return 0;
    }
#endif
    if (strcmp(mode, "oversized") == 0) {
        const unsigned char header[16] = {
            'D','I','S','P','C','M','P','1', 0,0,0,0,0,128,0,1
        };
        fwrite(header, 1, sizeof(header), stdout);
        return 0;
    }
    if (strcmp(mode, "spam") == 0) {
        size_t index;
        for (index = 0; index < 1024 * 1024; ++index) fputc('X', stdout);
        return 0;
    }
    if (strcmp(mode, "sleep") == 0) {
#ifdef _WIN32
        Sleep(5000);
#else
        sleep(5);
#endif
        return 0;
    }
    if (strcmp(mode, "malformed") == 0) {
        fputc('A', stdout);
        return 0;
    }
    fputs("component denied", stderr);
    return 7;
}
"#,
        )
        .unwrap();
        #[cfg(windows)]
        let compiler = "gcc";
        #[cfg(not(windows))]
        let compiler = "cc";
        let output = Command::new(compiler)
            .arg(&source)
            .arg("-O2")
            .arg("-o")
            .arg(&executable)
            .args(if cfg!(windows) {
                &["-ladvapi32", "-lws2_32"][..]
            } else {
                &[][..]
            })
            .output()
            .unwrap_or_else(|error| panic!("could not start component probe compiler: {error}"));
        assert!(
            output.status.success(),
            "component probe compilation failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        executable
    })
}

fn invoke_probe_with_arguments(
    mode: &str,
    arguments: &[&std::ffi::OsStr],
    request: &[u8],
) -> Result<Vec<u8>, ComponentError> {
    let original = component_probe();
    #[cfg(windows)]
    let launch_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join("component-tests");
    #[cfg(windows)]
    fs::create_dir_all(&launch_root).unwrap();
    let mut blocked = None;
    #[cfg(windows)]
    let attempts = 20;
    #[cfg(not(windows))]
    let attempts = 1;
    for attempt in 0..attempts {
        #[cfg(windows)]
        let executable = if attempt == 0 {
            original.clone()
        } else {
            let alternate = launch_root.join(format!("disp-component-probe-{mode}-{attempt}.exe"));
            let source = original.parent().unwrap().join("component.c");
            let output = Command::new("gcc")
                .arg(&source)
                .arg("-O2")
                .arg(format!("-DDISP_PROBE_VARIANT={attempt}"))
                .arg("-o")
                .arg(&alternate)
                .arg("-ladvapi32")
                .arg("-lws2_32")
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "component probe variant compilation failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            alternate
        };
        #[cfg(not(windows))]
        let executable = original.clone();
        let mut component = ComponentCommand::new(executable);
        component.arg(mode).args(arguments);
        match component.invoke(request) {
            Err(ComponentError::Launch(error)) if error.raw_os_error() == Some(4551) => {
                blocked = Some(error);
            }
            result => return result,
        }
    }
    panic!(
        "Windows application policy blocked every component probe artifact: {}",
        blocked.unwrap()
    )
}

fn invoke_probe(mode: &str, request: &[u8]) -> Result<Vec<u8>, ComponentError> {
    invoke_probe_with_arguments(mode, &[], request)
}

fn invoke_output_flood() -> Result<Vec<u8>, ComponentError> {
    #[cfg(windows)]
    {
        let mut command = ComponentCommand::new("C:/Windows/System32/cmd.exe");
        command.args([
            "/D",
            "/Q",
            "/C",
            "for /L %i in (1,1,100000) do @echo 0123456789",
        ]);
        command.invoke(b"request")
    }
    #[cfg(not(windows))]
    {
        invoke_probe("spam", b"request")
    }
}

fn invoke_wall_hang() -> Result<Vec<u8>, ComponentError> {
    #[cfg(windows)]
    {
        let mut command = ComponentCommand::new("C:/Windows/System32/cmd.exe");
        command.args(["/D", "/Q", "/C", "for /L %i in (1,1,2147483647) do @rem"]);
        command.invoke(b"request")
    }
    #[cfg(not(windows))]
    {
        invoke_probe("sleep", b"request")
    }
}

#[test]
fn component_round_trip_is_binary_exact_and_environment_minimal() {
    let request = b"DISP component\0binary\xff\n";
    let response = invoke_probe("echo", request).unwrap();
    assert_eq!(response, request);
}

#[cfg(windows)]
#[test]
fn windows_component_is_appcontainer_and_denies_network_and_host_files() {
    let directory = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join("component-tests");
    fs::create_dir_all(&directory).unwrap();
    let protected = directory.join(format!("medium-integrity-{}.txt", std::process::id()));
    fs::write(&protected, b"parent-owned").unwrap();
    let response = invoke_probe_with_arguments("token", &[protected.as_os_str()], b"request");
    assert_eq!(fs::read(&protected).unwrap(), b"parent-owned");
    fs::remove_file(&protected).unwrap();
    assert_eq!(response.unwrap(), b"ok");
}

#[test]
fn oversized_component_requests_fail_before_executable_resolution() {
    let request = vec![0; limits::COMPONENT_MESSAGE_BYTES + 1];
    let error = ComponentCommand::new("definitely-missing-disp-component")
        .invoke(&request)
        .unwrap_err();
    let message = error.to_string();
    assert!(message.contains("request"), "{message}");
    assert!(message.contains("limit"), "{message}");
    assert!(!message.contains("not found"), "{message}");
}

#[test]
fn linux_networkless_contract_is_shared_by_direct_and_hard_launchers() {
    let compiler = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let rust = fs::read_to_string(compiler.join("src/process_sandbox.rs")).unwrap();
    let helper = fs::read_to_string(
        compiler
            .parent()
            .unwrap()
            .join("support/linux/disp-cgroup-launch.c"),
    )
    .unwrap();
    for invariant in [
        "--component-networkless",
        "SYS_socket",
        "SYS_connect",
        "SYS_io_uring_setup",
    ] {
        assert!(rust.contains(invariant), "Rust launcher lost {invariant}");
    }
    for invariant in [
        "--component-networkless",
        "__NR_socket",
        "__NR_connect",
        "__NR_io_uring_setup",
        "install_network_deny_filter",
    ] {
        assert!(helper.contains(invariant), "hard helper lost {invariant}");
    }
}

#[test]
fn windows_component_contract_combines_appcontainer_job_and_handle_restrictions() {
    let source = fs::read_to_string(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/process_sandbox.rs"),
    )
    .unwrap();
    for invariant in [
        "CreateAppContainerProfile",
        "DeriveAppContainerSidFromAppContainerName",
        "SECURITY_CAPABILITIES",
        "PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES",
        "PROC_THREAD_ATTRIBUTE_ALL_APPLICATION_PACKAGES_POLICY",
        "PROCESS_CREATION_ALL_APPLICATION_PACKAGES_OPT_OUT",
        "PROC_THREAD_ATTRIBUTE_JOB_LIST",
        "PROC_THREAD_ATTRIBUTE_HANDLE_LIST",
        "JOB_OBJECT_UILIMIT_HANDLES",
        "JOB_OBJECT_UILIMIT_READCLIPBOARD",
        "JOB_OBJECT_UILIMIT_WRITECLIPBOARD",
        "JOB_OBJECT_UILIMIT_DESKTOP",
    ] {
        assert!(
            source.contains(invariant),
            "Windows launcher lost {invariant}"
        );
    }
}

#[test]
fn malformed_component_output_and_failed_exit_are_distinct() {
    let protocol = invoke_probe("malformed", b"request")
        .unwrap_err()
        .to_string();
    assert!(
        protocol.contains("response header is truncated"),
        "{protocol}"
    );

    let failure = invoke_probe("fail", b"request").unwrap_err().to_string();
    assert!(failure.contains("status"), "{failure}");
    assert!(failure.contains("component denied"), "{failure}");

    let oversized = invoke_probe("oversized", b"request")
        .unwrap_err()
        .to_string();
    assert!(oversized.contains("limit"), "{oversized}");
}

#[test]
fn component_profile_limits_fail_closed_in_isolated_children() {
    const CHILD_MODE: &str = "DISP_COMPONENT_TEST_CHILD";
    if let Ok(mode) = std::env::var(CHILD_MODE) {
        let error = match mode.as_str() {
            "invalid" => ComponentCommand::new("definitely-missing-disp-component")
                .invoke(b"request")
                .unwrap_err(),
            "output" => invoke_output_flood().unwrap_err(),
            "wall" => invoke_wall_hang().unwrap_err(),
            _ => panic!("unknown component child mode"),
        };
        let message = error.to_string();
        match mode.as_str() {
            "invalid" => assert!(message.contains("greater than zero"), "{message}"),
            "output" => assert!(message.contains("output exceeds"), "{message}"),
            "wall" => assert!(message.contains("wall deadline"), "{message}"),
            _ => unreachable!(),
        }
        return;
    }

    for (mode, variable, value) in [
        ("invalid", "DISP_COMPONENT_MAX_MEMORY_BYTES", "0"),
        ("output", "DISP_COMPONENT_MAX_OUTPUT_BYTES", "64"),
        ("wall", "DISP_COMPONENT_MAX_WALL_MILLIS", "50"),
    ] {
        let output = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "component_profile_limits_fail_closed_in_isolated_children",
                "--nocapture",
            ])
            .env(CHILD_MODE, mode)
            .env(variable, value)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "component {mode} limit child failed:\n{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[cfg(target_os = "linux")]
#[test]
fn linux_component_profile_denies_network_syscalls() {
    assert_eq!(invoke_probe("network", b"request").unwrap(), b"ok");
}
