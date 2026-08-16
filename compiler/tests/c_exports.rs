use disp::{
    backend::{self, BuildOptions, c_header},
    check_source, lower_source,
};
use std::{fs, process::Command};

const SOURCE: &str = r#"
export C struct RouteAddress {
    node: u64,
    port: u16,
}
impl Copy for RouteAddress {}
export C struct PacketHeader {
    destination: RouteAddress,
    flags: u8,
    payload_length: u32,
    sequence: u64,
}
impl Copy for PacketHeader {}
fn divide_impl(left: CInt, right: CInt) -> CInt { return left / right }
fn factorial_impl(value: CInt) -> CInt {
    if value <= 1 { return 1 }
    return value * factorial_impl(value - 1)
}
export C fn fixture_add(left: CInt, right: CInt) -> CInt uses Pure { return left + right }
export C fn fixture_divide(left: CInt, right: CInt) -> CInt uses Pure {
    return divide_impl(left, right)
}
export C fn fixture_factorial(value: CInt) -> CInt uses Pure { return factorial_impl(value) }
export C fn fixture_cleanup(value: CInt) -> CInt uses Pure {
    var text = String.new()
    text.push_str("owned export allocation")
    if value == 0 { return 1 / value }
    return value
}
export C fn fixture_header(value: PacketHeader) -> PacketHeader uses Pure {
    return PacketHeader {
        destination: value.destination,
        flags: value.flags,
        payload_length: value.payload_length + 4,
        sequence: value.sequence + 1,
    }
}
export C fn fixture_callback(callback: CFunction<fn(CInt) -> CInt>, value: CInt) -> CInt uses Foreign {
    unsafe uses Foreign { return callback(value) }
}
export C fn fixture_ping() uses Pure {}
fn main() {}
"#;

#[test]
fn export_syntax_requires_a_stable_c_authority_contract() {
    check_source(SOURCE).unwrap();
    check_source("export C fn add(left: CInt, right: CInt) -> CInt uses Pure { return left + right } fn main() {}")
        .unwrap();
    for (source, expected) in [
        (
            "export Rust fn value() uses Pure {} fn main() {}",
            "unsupported export ABI",
        ),
        ("export C fn value() {} fn main() {}", "uses Pure"),
        (
            "export C async fn value() uses Pure {} fn main() {}",
            "cannot be async",
        ),
        (
            "export C fn value<T>(item: T) uses Pure {} fn main() {}",
            "cannot be generic",
        ),
        (
            "export C fn value(item: String) uses Pure {} fn main() {}",
            "not safe to pass",
        ),
        ("export C fn main() uses Pure {}", "cannot be exported"),
        (
            "export C fn value() uses FileSystem {} fn main() {}",
            "uses Pure` or `uses Foreign",
        ),
    ] {
        let error = check_source(source).unwrap_err();
        assert!(
            error.message.contains(expected),
            "unexpected diagnostic: {error}"
        );
    }
}

#[test]
fn exported_header_uses_status_out_parameters_and_error_retrieval() {
    let (hir, _) = lower_source(SOURCE).unwrap();
    let header = c_header::generate(&hir).unwrap();
    for required in [
        "#define DISP_C_STATUS_OK 0",
        "#define DISP_C_STATUS_PANIC 1",
        "DISP_C_API const char *disp_c_last_error(void);",
        "DISP_C_API int32_t disp_c_thread_attach(void);",
        "typedef int32_t (*disp_c_callback_fixture_add)",
        "DISP_C_API int32_t fixture_cleanup(int32_t arg1, int32_t *out_result);",
        "typedef disp_t_S0 disp_c_RouteAddress;",
        "typedef disp_t_S1 disp_c_PacketHeader;",
        "uint64_t node;",
        "uint16_t port;",
        "DISP_C_STATIC_ASSERT(offsetof(disp_c_PacketHeader, payload_length) == 20",
        "DISP_C_STATIC_ASSERT(sizeof(disp_c_PacketHeader) == 32",
        "DISP_C_API int32_t fixture_header(disp_c_PacketHeader arg1, disp_c_PacketHeader *out_result);",
    ] {
        assert!(
            header.contains(required),
            "header lacks `{required}`\n{header}"
        );
    }
}

#[test]
fn c_consumer_calls_shared_disp_library_and_observes_contained_failure() {
    let root = std::env::temp_dir().join(format!("disp-c-export-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    let source_path = root.join("exports.disp");
    fs::write(&source_path, SOURCE).unwrap();
    let (hir, mir) = lower_source(SOURCE).unwrap();
    let artifacts = backend::build(
        &hir,
        &mir,
        &source_path,
        BuildOptions {
            library: true,
            emit_c: true,
            ..BuildOptions::default()
        },
    )
    .unwrap();
    let generated = fs::read_to_string(artifacts.backend_ir.as_ref().unwrap()).unwrap();
    for required in [
        "disp_ffi_allocation_boundary_begin()",
        "disp_ffi_allocation_boundary_abort()",
        "disp_ffi_allocation_boundary_finish()",
        "union { uint64_t f0; uint64_t node; };",
    ] {
        assert!(generated.contains(required));
    }
    c_header::write(&hir, &source_path).unwrap();
    let consumer = root.join("consumer.c");
    fs::write(
        &consumer,
        r#"#include "exports.h"
#include <stdint.h>
#include <stdio.h>
#include <string.h>
#ifdef _WIN32
#include <windows.h>
#define LOAD(path) ((void*)LoadLibraryA(path))
#define SYMBOL(module,name) ((void*)GetProcAddress((HMODULE)(module),name))
#else
#include <dlfcn.h>
#define LOAD(path) dlopen(path,RTLD_NOW|RTLD_LOCAL)
#define SYMBOL(module,name) dlsym(module,name)
#endif
typedef const char *(*error_fn)(void);
typedef int32_t (*control_fn)(void);
static int32_t increment(int32_t value){return value+1;}
int main(int argc,char **argv){
  if(argc!=2)return 10;
  void *module=LOAD(argv[1]);if(!module)return 11;
  disp_c_callback_fixture_add add=(disp_c_callback_fixture_add)SYMBOL(module,"fixture_add");
  disp_c_callback_fixture_divide divide=(disp_c_callback_fixture_divide)SYMBOL(module,"fixture_divide");
  disp_c_callback_fixture_factorial factorial=(disp_c_callback_fixture_factorial)SYMBOL(module,"fixture_factorial");
  disp_c_callback_fixture_cleanup cleanup=(disp_c_callback_fixture_cleanup)SYMBOL(module,"fixture_cleanup");
  disp_c_callback_fixture_header header=(disp_c_callback_fixture_header)SYMBOL(module,"fixture_header");
  disp_c_callback_fixture_callback callback=(disp_c_callback_fixture_callback)SYMBOL(module,"fixture_callback");
  error_fn last_error=(error_fn)SYMBOL(module,"disp_c_last_error");
  control_fn attach=(control_fn)SYMBOL(module,"disp_c_thread_attach");
  control_fn detach=(control_fn)SYMBOL(module,"disp_c_thread_detach");
  if(!add||!divide||!factorial||!cleanup||!header||!callback||!last_error||!attach||!detach)return 12;
  int32_t out=77;
  if(add(1,2,&out)!=DISP_C_STATUS_THREAD_NOT_ATTACHED||out!=77)return 13;
  if(attach()!=DISP_C_STATUS_OK)return 14;
  if(add(20,22,&out)!=DISP_C_STATUS_OK||out!=42)return 15;
  out=77;if(divide(1,0,&out)!=DISP_C_STATUS_PANIC||out!=77)return 16;
  if(!strstr(last_error(),"division by zero"))return 17;
  if(factorial(5,&out)!=DISP_C_STATUS_OK||out!=120)return 18;
  for(int i=0;i<1000;i++){out=77;if(cleanup(0,&out)!=DISP_C_STATUS_PANIC||out!=77)return 19;}
  if(cleanup(42,&out)!=DISP_C_STATUS_OK||out!=42)return 20;
  disp_c_PacketHeader packet={0},transformed={0};
  packet.destination.node=9;packet.destination.port=443;packet.flags=5;packet.payload_length=1200;packet.sequence=99;
  if(header(packet,&transformed)!=0||transformed.payload_length!=1204||transformed.sequence!=100)return 21;
  if(callback(increment,41,&out)!=0||out!=42)return 22;
  if(detach()!=0)return 23;
  puts("DISP C export ABI v1 active");
  return 0;
}
"#,
    )
    .unwrap();
    let executable = root.join(if cfg!(windows) {
        "consumer.exe"
    } else {
        "consumer"
    });
    let mut compile = Command::new("gcc");
    compile
        .current_dir(&root)
        .arg("-std=c11")
        .arg("consumer.c")
        .arg("-o")
        .arg(&executable);
    if cfg!(target_os = "linux") {
        compile.args(["-ldl", "-pthread"]);
    }
    let compile = compile.output().unwrap();
    assert!(
        compile.status.success(),
        "{}",
        String::from_utf8_lossy(&compile.stderr)
    );
    let execution = match Command::new(&executable)
        .arg(&artifacts.executable)
        .env("DISP_MAX_MEMORY_BYTES", "64")
        .output()
    {
        Ok(output) => output,
        Err(error) if error.raw_os_error() == Some(4551) => {
            fs::remove_dir_all(root).unwrap();
            return;
        }
        Err(error) => panic!("C consumer failed to launch: {error}"),
    };
    assert!(
        execution.status.success(),
        "{}",
        String::from_utf8_lossy(&execution.stderr)
    );
    assert_eq!(
        String::from_utf8(execution.stdout)
            .unwrap()
            .replace("\r\n", "\n"),
        "DISP C export ABI v1 active\n"
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn disp_passes_checked_export_callbacks_to_a_threaded_c_provider() {
    let source = r#"
extern C { fn provider_invoke(add: CFunction<fn(CInt, CInt, mut ptr<CInt>) -> CInt>) -> CInt }
export C fn exported_add(left: CInt, right: CInt) -> CInt uses Pure { return left + right }
fn main() uses Foreign {
    unsafe uses Foreign { print(provider_invoke(CExport.callback(exported_add))) }
}
"#;
    let provider = r#"
#include <stdint.h>
#include <stdlib.h>
#ifdef _WIN32
#include <windows.h>
#else
#include <pthread.h>
#endif
typedef int32_t (*export_callback)(int32_t,int32_t,int32_t*);
extern int32_t disp_c_thread_attach(void);
extern int32_t disp_c_thread_detach(void);
typedef struct {export_callback callback;int32_t result;} context_t;
static void run(context_t *context){int32_t out=0;if(disp_c_thread_attach()!=0)return;if(context->callback(20,22,&out)!=0)return;if(disp_c_thread_detach()!=0)return;context->result=out;}
#ifdef _WIN32
static DWORD WINAPI entry(LPVOID raw){run((context_t*)raw);return 0;}
#else
static void *entry(void *raw){run((context_t*)raw);return NULL;}
#endif
int32_t provider_invoke(export_callback callback){
  context_t context={callback,0};
#ifdef _WIN32
  HANDLE thread=CreateThread(NULL,0,entry,&context,0,NULL);if(!thread||WaitForSingleObject(thread,INFINITE)!=WAIT_OBJECT_0)abort();CloseHandle(thread);
#else
  pthread_t thread;if(pthread_create(&thread,NULL,entry,&context)||pthread_join(thread,NULL))abort();
#endif
  return context.result;
}
"#;
    let root = std::env::temp_dir().join(format!("disp-export-provider-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    let source_path = root.join("provider.disp");
    let provider_path = root.join("provider.c");
    fs::write(&source_path, source).unwrap();
    fs::write(&provider_path, provider).unwrap();
    let (hir, mir) = lower_source(source).unwrap();
    let _ = backend::build(
        &hir,
        &mir,
        &source_path,
        BuildOptions {
            emit_c: true,
            ..BuildOptions::default()
        },
    );
    let generated = root
        .join("build")
        .join("provider")
        .join("provider.backend.c");
    let executable = root.join(if cfg!(windows) {
        "provider.exe"
    } else {
        "provider"
    });
    let mut compile = Command::new("gcc");
    compile
        .arg("-std=c11")
        .arg(&generated)
        .arg(&provider_path)
        .arg("-o")
        .arg(&executable)
        .arg("-lm");
    if cfg!(windows) {
        compile.args(["-lshell32", "-lbcrypt"]);
    } else {
        compile.arg("-pthread");
    }
    let compile = compile.output().unwrap();
    assert!(
        compile.status.success(),
        "{}",
        String::from_utf8_lossy(&compile.stderr)
    );
    let execution = match Command::new(&executable).output() {
        Ok(output) => output,
        Err(error) if error.raw_os_error() == Some(4551) => {
            fs::remove_dir_all(root).unwrap();
            return;
        }
        Err(error) => panic!("provider fixture failed to launch: {error}"),
    };
    assert!(
        execution.status.success(),
        "{}",
        String::from_utf8_lossy(&execution.stderr)
    );
    assert_eq!(
        String::from_utf8(execution.stdout)
            .unwrap()
            .replace("\r\n", "\n"),
        "42\n"
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn contained_export_failure_rolls_back_handle_resources_in_reverse_order() {
    let source = r#"
extern C { fn provider_release(context: mut ptr<Unit>) }
export C fn exercise(first: mut ptr<Unit>, second: mut ptr<Unit>, zero: CInt, fail: bool) -> CInt uses Foreign {
    unsafe uses Foreign {
        first_registration = CRegistration.adopt(first, provider_release)
        second_registration = CRegistration.adopt(second, provider_release)
        if fail { return zero / zero }
        return zero + 42
    }
}
fn main() {}
"#;
    let provider = r#"
#include <stdbool.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>
typedef struct {int id;int releases;} provider_context;
extern int32_t exercise(void*,void*,int32_t,bool,int32_t*);
extern int32_t disp_c_thread_attach(void);
extern int32_t disp_c_thread_detach(void);
extern const char *disp_c_last_error(void);
static int log_values[2],log_count;
void provider_release(void *raw){provider_context *context=(provider_context*)raw;context->releases++;log_values[log_count++]=context->id;}
static int run(bool fail){
  provider_context first={1,0},second={2,0};int32_t output=77;log_count=0;
  int32_t status=exercise(&first,&second,0,fail,&output);
  if(status!=(fail?1:0)||output!=(fail?77:42))return 10;
  if(first.releases!=1||second.releases!=1||log_count!=2)return 11;
  if(log_values[0]!=2||log_values[1]!=1)return 12;
  if(fail&&!strstr(disp_c_last_error(),"division by zero"))return 13;
  return 0;
}
int main(void){if(disp_c_thread_attach()!=0)return 20;for(int i=0;i<1000;i++){int status=run(true);if(status)return status;}int status=run(false);if(status)return status;if(disp_c_thread_detach()!=0)return 21;puts("typed handle rollback active");return 0;}
"#;
    let root = std::env::temp_dir().join(format!("disp-handle-rollback-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    let source_path = root.join("rollback.disp");
    let provider_path = root.join("provider.c");
    fs::write(&source_path, source).unwrap();
    fs::write(&provider_path, provider).unwrap();
    let (hir, mir) = lower_source(source).unwrap();
    let build = backend::build(
        &hir,
        &mir,
        &source_path,
        BuildOptions {
            library: true,
            emit_c: true,
            ..BuildOptions::default()
        },
    );
    let generated = match build {
        Ok(artifacts) => artifacts.backend_ir.unwrap(),
        Err(_) => root
            .join("build")
            .join("rollback")
            .join("rollback.backend.c"),
    };
    let emitted = fs::read_to_string(&generated).unwrap();
    assert!(emitted.contains("disp_ffi_track_rollback"));
    assert!(emitted.contains("disp_c_registration_rollback_cleanup"));
    let executable = root.join(if cfg!(windows) {
        "rollback.exe"
    } else {
        "rollback"
    });
    let mut compile = Command::new("gcc");
    compile
        .arg("-std=c11")
        .arg(&generated)
        .arg(&provider_path)
        .arg("-o")
        .arg(&executable)
        .arg("-lm");
    if cfg!(windows) {
        compile.args(["-lshell32", "-lbcrypt"]);
    } else {
        compile.arg("-pthread");
    }
    let compile = compile.output().unwrap();
    assert!(
        compile.status.success(),
        "{}",
        String::from_utf8_lossy(&compile.stderr)
    );
    let execution = match Command::new(&executable).output() {
        Ok(output) => output,
        Err(error) if error.raw_os_error() == Some(4551) => {
            fs::remove_dir_all(root).unwrap();
            return;
        }
        Err(error) => panic!("rollback fixture failed to launch: {error}"),
    };
    assert!(
        execution.status.success(),
        "{}",
        String::from_utf8_lossy(&execution.stderr)
    );
    assert_eq!(
        String::from_utf8(execution.stdout)
            .unwrap()
            .replace("\r\n", "\n"),
        "typed handle rollback active\n"
    );
    fs::remove_dir_all(root).unwrap();
}
