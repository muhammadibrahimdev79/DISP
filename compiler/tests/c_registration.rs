use disp::{
    backend::{self, BuildOptions},
    check_source, lower_source,
};
use std::{fs, process::Command};

const PROGRAM: &str = include_str!("../examples/c_registration.disp");

#[test]
fn registration_adoption_is_explicit_linear_and_thread_affine() {
    check_source(PROGRAM).unwrap();

    check_source(
        "extern C { fn quiesce(context: mut ptr<Unit>) fn release(context: mut ptr<Unit>) } fn test(context: mut ptr<Unit>) uses Foreign { unsafe uses Foreign { registration = CRegistration.adopt_async(context, quiesce, release) print(registration.is_active()) } } fn main() {}",
    )
    .unwrap();

    let missing_unsafe = check_source(
        "extern C { fn free(context: mut ptr<Unit>) } fn test(context: mut ptr<Unit>) uses Foreign { registration = CRegistration.adopt(context, free) } fn main() {}",
    )
    .unwrap_err();
    assert!(
        missing_unsafe.message.contains("unsafe"),
        "{missing_unsafe}"
    );

    let wrong_release = check_source(
        "extern C { fn bad(context: ptr<Unit>) } fn test(context: mut ptr<Unit>) uses Foreign { unsafe uses Foreign { registration = CRegistration.adopt(context, bad) } } fn main() {}",
    )
    .unwrap_err();
    assert!(
        wrong_release.message.contains("release callback"),
        "{wrong_release}"
    );

    let wrong_quiesce = check_source(
        "extern C { fn bad(context: ptr<Unit>) fn release(context: mut ptr<Unit>) } fn test(context: mut ptr<Unit>) uses Foreign { unsafe uses Foreign { registration = CRegistration.adopt_async(context, bad, release) } } fn main() {}",
    )
    .unwrap_err();
    assert!(
        wrong_quiesce.message.contains("quiesce callback"),
        "{wrong_quiesce}"
    );

    let borrowed_handler = check_source(
        "extern C { fn provider_register(callback: CFunction<fn(mut ptr<Unit>, CInt, mut ptr<CInt>) -> CInt>, context: mut ptr<Unit>) -> mut ptr<Unit> fn quiesce(context: mut ptr<Unit>) fn release(context: mut ptr<Unit>) } fn test(offset: CInt) uses Foreign { unsafe uses Foreign { registration = CRegistration.register_async(|value: CInt| value + offset, provider_register, quiesce, release) } } fn main() {}",
    )
    .unwrap_err();
    assert!(
        borrowed_handler.message.contains("`move` closure"),
        "{borrowed_handler}"
    );

    let wrong_register = check_source(
        "extern C { fn provider_register(callback: CFunction<fn(mut ptr<Unit>, CInt) -> CInt>, context: mut ptr<Unit>) -> mut ptr<Unit> fn quiesce(context: mut ptr<Unit>) fn release(context: mut ptr<Unit>) } fn test() uses Foreign { unsafe uses Foreign { registration = CRegistration.register_async(move |value: CInt| value, provider_register, quiesce, release) } } fn main() {}",
    )
    .unwrap_err();
    assert!(
        wrong_register.message.contains("provider register"),
        "{wrong_register}"
    );

    let indirect_handler = check_source(
        "extern C { fn provider_register(callback: CFunction<fn(mut ptr<Unit>, CInt, mut ptr<CInt>) -> CInt>, context: mut ptr<Unit>) -> mut ptr<Unit> fn quiesce(context: mut ptr<Unit>) fn release(context: mut ptr<Unit>) } fn test() uses Foreign { handler = move |value: CInt| value unsafe uses Foreign { registration = CRegistration.register_async(handler, provider_register, quiesce, release) } } fn main() {}",
    )
    .unwrap_err();
    assert!(
        indirect_handler
            .message
            .contains("direct named DISP function"),
        "{indirect_handler}"
    );

    let non_send_capture = check_source(
        "extern C { fn provider_register(callback: CFunction<fn(mut ptr<Unit>, CInt, mut ptr<CInt>) -> CInt>, context: mut ptr<Unit>) -> mut ptr<Unit> fn quiesce(context: mut ptr<Unit>) fn release(context: mut ptr<Unit>) } fn test(secret: SecretBytes) uses Foreign { unsafe uses Foreign { registration = CRegistration.register_async(move |value: CInt| -> CInt { print(secret) return value }, provider_register, quiesce, release) } } fn main() {}",
    )
    .unwrap_err();
    assert!(
        non_send_capture.message.contains("cannot be transferred"),
        "{non_send_capture}"
    );

    let use_after_close = check_source(
        "extern C { fn free(context: mut ptr<Unit>) } fn test(context: mut ptr<Unit>) uses Foreign { unsafe uses Foreign { registration = CRegistration.adopt(context, free) registration.close() print(registration.is_active()) } } fn main() {}",
    )
    .unwrap_err();
    assert!(
        use_after_close.message.contains("moved")
            || use_after_close.message.contains("initialized"),
        "{use_after_close}"
    );

    let copied = check_source(
        "extern C { fn free(context: mut ptr<Unit>) } fn test(context: mut ptr<Unit>) uses Foreign { unsafe uses Foreign { registration = CRegistration.adopt(context, free) other = registration print(registration.is_active()) other.close() } } fn main() {}",
    )
    .unwrap_err();
    assert!(
        copied.message.contains("moved") || copied.message.contains("borrow"),
        "{copied}"
    );

    let transferred = check_source(
        "fn take(registration: CRegistration) { registration.close() } fn test(registration: CRegistration) { task = spawn take(registration) task.join() } fn main() {}",
    )
    .unwrap_err();
    assert!(
        transferred.message.contains("cannot be transferred")
            || transferred.message.contains("borrowed views")
            || transferred.message.contains("thread"),
        "{transferred}"
    );
}

#[test]
fn native_registration_closes_explicitly_or_at_scope_exit_exactly_once() {
    let root = std::env::temp_dir().join(format!("disp-c-registration-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    let source = root.join("registration.disp");
    fs::write(&source, PROGRAM).unwrap();
    let (hir, mir) = lower_source(PROGRAM).unwrap();
    let artifacts = backend::build(
        &hir,
        &mir,
        &source,
        BuildOptions {
            emit_c: true,
            ..BuildOptions::default()
        },
    )
    .unwrap();
    let generated = fs::read_to_string(artifacts.backend_ir.unwrap()).unwrap();
    let deactivate = generated
        .find("registration->active=false")
        .expect("registration cleanup must deactivate before release");
    let release = generated[deactivate..]
        .find("disp_c_registration_release_parts(context")
        .expect("registration cleanup must invoke the provider release path");
    assert!(release > 0);
    assert!(generated.contains("if(!registration||!registration->active)return"));
    assert!(generated.contains("C registration release callback is null"));

    let execution = match Command::new(&artifacts.executable).output() {
        Ok(output) => output,
        Err(error) if error.raw_os_error() == Some(4551) => {
            fs::remove_dir_all(root).unwrap();
            return;
        }
        Err(error) => panic!("native registration fixture failed to launch: {error}"),
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
        "true\ntrue\n"
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn asynchronous_registration_quiesces_before_releasing_owned_context() {
    let source = r#"
extern C {
    fn malloc(size: CSize) -> mut ptr<Unit>
    fn free(context: mut ptr<Unit>)
}
fn main() uses Foreign {
    unsafe uses Foreign {
        registration = CRegistration.adopt_async(malloc(1), free, free)
        print(registration.is_active())
    }
}
"#;
    let root =
        std::env::temp_dir().join(format!("disp-c-async-registration-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    let source_path = root.join("registration.disp");
    fs::write(&source_path, source).unwrap();
    let (hir, mir) = lower_source(source).unwrap();
    let artifacts = backend::build(
        &hir,
        &mir,
        &source_path,
        BuildOptions {
            emit_c: true,
            ..BuildOptions::default()
        },
    )
    .unwrap();
    let generated = fs::read_to_string(artifacts.backend_ir.unwrap()).unwrap();
    let deactivate = generated.find("registration->active=false").unwrap();
    let release_path = generated[deactivate..]
        .find("disp_c_registration_release_parts(context")
        .unwrap();
    let release_parts = generated.find("if(quiesce)quiesce(context)").unwrap();
    let provider_release = generated[release_parts..].find("release(context)").unwrap();
    assert!(release_path > 0 && provider_release > 0);
    assert!(generated.contains("C registration quiesce callback is null"));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn threaded_provider_is_joined_before_its_context_is_released() {
    let source = r#"
extern C {
    fn provider_start() -> mut ptr<Unit>
    fn provider_quiesce(context: mut ptr<Unit>)
    fn provider_release(context: mut ptr<Unit>)
}

fn main() uses Foreign {
    unsafe uses Foreign {
        registration = CRegistration.adopt_async(
            provider_start(),
            provider_quiesce,
            provider_release
        )
        print(registration.is_active())
    }
}
"#;
    let provider = r#"
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#ifdef _WIN32
#include <windows.h>
typedef struct { HANDLE thread; volatile LONG stop; volatile LONG ticks; } provider_context;
static DWORD WINAPI provider_worker(LPVOID raw){
  provider_context *context=(provider_context*)raw;
  InterlockedIncrement(&context->ticks);
  while(!InterlockedCompareExchange(&context->stop,0,0)){InterlockedIncrement(&context->ticks);Sleep(0);}
  return 0;
}
void *provider_start(void){
  provider_context *context=(provider_context*)calloc(1,sizeof(provider_context));
  if(!context)abort();
  context->thread=CreateThread(NULL,0,provider_worker,context,0,NULL);
  if(!context->thread)abort();
  return context;
}
void provider_quiesce(void *raw){
  provider_context *context=(provider_context*)raw;
  InterlockedExchange(&context->stop,1);
  if(WaitForSingleObject(context->thread,INFINITE)!=WAIT_OBJECT_0)abort();
  CloseHandle(context->thread);context->thread=NULL;
  puts("quiesced");
}
void provider_release(void *raw){
  provider_context *context=(provider_context*)raw;
  if(context->thread||!context->stop||context->ticks<1)abort();
  puts("released");free(context);
}
#else
#include <pthread.h>
#include <sched.h>
#include <stdatomic.h>
typedef struct { pthread_t thread; atomic_int stop; atomic_uint_fast64_t ticks; int joined; } provider_context;
static void *provider_worker(void *raw){
  provider_context *context=(provider_context*)raw;
  atomic_fetch_add(&context->ticks,1);
  while(!atomic_load(&context->stop)){atomic_fetch_add(&context->ticks,1);sched_yield();}
  return NULL;
}
void *provider_start(void){
  provider_context *context=(provider_context*)calloc(1,sizeof(provider_context));
  if(!context||pthread_create(&context->thread,NULL,provider_worker,context))abort();
  return context;
}
void provider_quiesce(void *raw){
  provider_context *context=(provider_context*)raw;
  atomic_store(&context->stop,1);
  if(pthread_join(context->thread,NULL))abort();
  context->joined=1;puts("quiesced");
}
void provider_release(void *raw){
  provider_context *context=(provider_context*)raw;
  if(!context->joined||atomic_load(&context->ticks)<1)abort();
  puts("released");free(context);
}
#endif
"#;
    let root =
        std::env::temp_dir().join(format!("disp-c-threaded-provider-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    let source_path = root.join("async_provider.disp");
    let provider_path = root.join("provider.c");
    fs::write(&source_path, source).unwrap();
    fs::write(&provider_path, provider).unwrap();
    let (hir, mir) = lower_source(source).unwrap();
    let error = backend::build(
        &hir,
        &mir,
        &source_path,
        BuildOptions {
            emit_c: true,
            ..BuildOptions::default()
        },
    )
    .unwrap_err();
    assert!(error.message.contains("native linking"), "{error}");
    let generated = root
        .join("build")
        .join("async_provider")
        .join("async_provider.backend.c");
    assert!(generated.is_file());
    let executable = root.join(if cfg!(windows) {
        "async_provider.exe"
    } else {
        "async_provider"
    });
    let mut compile = Command::new("gcc");
    compile
        .args(["-std=c11"])
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
        Err(error) => panic!("threaded provider fixture failed to launch: {error}"),
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
        "true\nquiesced\nreleased\n"
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn captured_disp_handler_runs_on_provider_thread_and_drops_after_quiescence() {
    let source = r#"
extern C {
    fn provider_register(
        callback: CFunction<fn(mut ptr<Unit>, CInt, mut ptr<CInt>) -> CInt>,
        callback_context: mut ptr<Unit>
    ) -> mut ptr<Unit>
    fn provider_quiesce(context: mut ptr<Unit>)
    fn provider_release(context: mut ptr<Unit>)
}

fn main() uses Foreign {
    var label = String.new()
    label.push_str("Outernet")
    let offset: CInt = 40
    unsafe uses Foreign {
        registration = CRegistration.register_async(
            move |value: CInt| -> CInt {
                if label.is_empty() { return value }
                return value + offset
            },
            provider_register,
            provider_quiesce,
            provider_release
        )
        print(registration.is_active())
    }
}
"#;
    let provider = r#"
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#ifdef _WIN32
#include <windows.h>
#else
#include <pthread.h>
#endif
typedef int32_t (*captured_callback)(void*,int32_t,int32_t*);
extern int32_t disp_c_thread_attach(void);
extern int32_t disp_c_thread_detach(void);
typedef struct { captured_callback callback; void *callback_context; int32_t first; int32_t second; int32_t status; int joined;
#ifdef _WIN32
HANDLE thread;
#else
pthread_t thread;
#endif
} provider_context;
static void provider_run(provider_context *context){
  context->first=-1;context->second=-1;
  if(disp_c_thread_attach()!=0)return;
  context->status=context->callback(context->callback_context,2,&context->first);
  if(context->status==0)context->status=context->callback(context->callback_context,3,&context->second);
  if(disp_c_thread_detach()!=0)context->status=-1;
}
#ifdef _WIN32
static DWORD WINAPI provider_entry(LPVOID raw){provider_run((provider_context*)raw);return 0;}
#else
static void *provider_entry(void *raw){provider_run((provider_context*)raw);return NULL;}
#endif
void *provider_register(captured_callback callback,void *callback_context){
  provider_context *context=(provider_context*)calloc(1,sizeof(provider_context));
  if(!context)abort();
  context->callback=callback;context->callback_context=callback_context;
#ifdef _WIN32
  context->thread=CreateThread(NULL,0,provider_entry,context,0,NULL);
  if(!context->thread)abort();
#else
  if(pthread_create(&context->thread,NULL,provider_entry,context))abort();
#endif
  return context;
}
void provider_quiesce(void *raw){
  provider_context *context=(provider_context*)raw;
#ifdef _WIN32
  if(WaitForSingleObject(context->thread,INFINITE)!=WAIT_OBJECT_0)abort();
  CloseHandle(context->thread);
#else
  if(pthread_join(context->thread,NULL))abort();
#endif
  context->joined=1;
}
void provider_release(void *raw){
  provider_context *context=(provider_context*)raw;
  if(!context->joined||context->status!=0||context->first!=42||context->second!=43)abort();
  printf("%d\n%d\n",context->first,context->second);free(context);
}
"#;
    let root =
        std::env::temp_dir().join(format!("disp-c-captured-callback-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    let source_path = root.join("captured_callback.disp");
    let provider_path = root.join("provider.c");
    fs::write(&source_path, source).unwrap();
    fs::write(&provider_path, provider).unwrap();
    let (hir, mir) = lower_source(source).unwrap();
    let error = backend::build(
        &hir,
        &mir,
        &source_path,
        BuildOptions {
            emit_c: true,
            ..BuildOptions::default()
        },
    )
    .unwrap_err();
    assert!(error.message.contains("native linking"), "{error}");
    let generated_path = root
        .join("build")
        .join("captured_callback")
        .join("captured_callback.backend.c");
    let generated = fs::read_to_string(&generated_path).unwrap();
    let quiesce = generated.find("if(quiesce)quiesce(context)").unwrap();
    let drop_callback = generated.find("if(callback){if(callback->drop)").unwrap();
    let release = generated.find("release(context)").unwrap();
    assert!(quiesce < drop_callback && drop_callback < release);
    assert!(generated.contains("disp_string_drop(&(_captures->f0))"));
    assert!(generated.contains("disp_c_context_callback_"));
    let executable = root.join(if cfg!(windows) {
        "captured_callback.exe"
    } else {
        "captured_callback"
    });
    let mut compile = Command::new("gcc");
    compile
        .args(["-std=c11"])
        .arg(&generated_path)
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
        Err(error) => panic!("captured callback fixture failed to launch: {error}"),
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
        "true\n42\n43\n"
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn captured_callback_graph_rejects_cleanup_bearing_work() {
    let source = r#"
extern C {
    fn provider_register(
        callback: CFunction<fn(mut ptr<Unit>, CInt, mut ptr<CInt>) -> CInt>,
        callback_context: mut ptr<Unit>
    ) -> mut ptr<Unit>
    fn provider_quiesce(context: mut ptr<Unit>)
    fn provider_release(context: mut ptr<Unit>)
}
fn handler(value: CInt) -> CInt {
    text = "owned callback storage"
    return value
}
fn main() uses Foreign {
    unsafe uses Foreign {
        registration = CRegistration.register_async(
            handler,
            provider_register,
            provider_quiesce,
            provider_release
        )
        print(registration.is_active())
    }
}
"#;
    let (hir, mir) = lower_source(source).unwrap();
    let path = std::env::temp_dir().join("disp-rejected-captured-callback.disp");
    let error = backend::build(&hir, &mir, &path, BuildOptions::default()).unwrap_err();
    assert!(error.message.contains("cleanup-bearing storage"), "{error}");
}
