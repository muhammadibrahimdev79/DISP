pub const C_RUNTIME: &str = r#"
#include <stdint.h>
#include <stddef.h>
#include <stdbool.h>
#include <stdatomic.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <math.h>
#include <errno.h>
#include <time.h>
#include <sys/stat.h>
#ifdef _WIN32
#include <windows.h>
#include <direct.h>
#else
#include <dirent.h>
#include <pthread.h>
#include <unistd.h>
#endif

static void dv_panic(const char *message,int line,int column);
struct disp_mutex_state {
    atomic_size_t refs;
    void *data;
#ifdef _WIN32
    CRITICAL_SECTION native;
#else
    pthread_mutex_t native;
#endif
};
static disp_mutex_state *disp_mutex_create(void *data){disp_mutex_state *state=(disp_mutex_state*)disp_alloc(sizeof(disp_mutex_state),_Alignof(disp_mutex_state));atomic_init(&state->refs,1);state->data=data;
#ifdef _WIN32
InitializeCriticalSection(&state->native);
#else
if(pthread_mutex_init(&state->native,NULL)!=0)disp_allocation_failure("could not initialize Mutex");
#endif
return state;}
static void disp_mutex_retain(disp_mutex_state *state){if(!state)dv_panic("invalid Mutex handle",0,0);size_t previous=atomic_fetch_add_explicit(&state->refs,1,memory_order_relaxed);if(previous==SIZE_MAX)dv_panic("Mutex reference count overflow",0,0);}
static disp_native_mutex_guard disp_mutex_lock(disp_mutex_state *state,int line,int column){disp_mutex_retain(state);
#ifdef _WIN32
EnterCriticalSection(&state->native);
#else
if(pthread_mutex_lock(&state->native)!=0)dv_panic("could not lock Mutex",line,column);
#endif
return (disp_native_mutex_guard){.state=state};}
static void disp_mutex_unlock(disp_mutex_state *state){
#ifdef _WIN32
LeaveCriticalSection(&state->native);
#else
if(pthread_mutex_unlock(&state->native)!=0)dv_panic("could not unlock Mutex",0,0);
#endif
}
static bool disp_mutex_release(disp_mutex_state *state){if(!state)return false;if(atomic_fetch_sub_explicit(&state->refs,1,memory_order_acq_rel)!=1)return false;atomic_thread_fence(memory_order_acquire);
#ifdef _WIN32
DeleteCriticalSection(&state->native);
#else
if(pthread_mutex_destroy(&state->native)!=0)dv_panic("could not destroy Mutex",0,0);
#endif
return true;}
struct disp_atomic_int_state { atomic_size_t refs; atomic_int_fast64_t value; };
static disp_atomic_int_state *disp_atomic_int_create(int64_t value){disp_atomic_int_state *state=(disp_atomic_int_state*)disp_alloc(sizeof(disp_atomic_int_state),_Alignof(disp_atomic_int_state));atomic_init(&state->refs,1);atomic_init(&state->value,value);return state;}
static void disp_atomic_int_retain(disp_atomic_int_state *state){if(!state)dv_panic("invalid AtomicInt handle",0,0);size_t previous=atomic_fetch_add_explicit(&state->refs,1,memory_order_relaxed);if(previous==SIZE_MAX)dv_panic("AtomicInt reference count overflow",0,0);}
static bool disp_atomic_int_release(disp_atomic_int_state *state){if(!state)return false;if(atomic_fetch_sub_explicit(&state->refs,1,memory_order_acq_rel)!=1)return false;atomic_thread_fence(memory_order_acquire);return true;}
static int64_t disp_atomic_int_load(disp_atomic_int_state *state){return (int64_t)atomic_load_explicit(&state->value,memory_order_seq_cst);}
static void disp_atomic_int_store(disp_atomic_int_state *state,int64_t value){atomic_store_explicit(&state->value,value,memory_order_seq_cst);}
static int64_t disp_atomic_int_fetch_add(disp_atomic_int_state *state,int64_t delta,int line,int column){int_fast64_t expected=atomic_load_explicit(&state->value,memory_order_seq_cst);for(;;){int64_t desired;if(__builtin_add_overflow((int64_t)expected,delta,&desired))dv_panic("AtomicInt overflow",line,column);if(atomic_compare_exchange_weak_explicit(&state->value,&expected,(int_fast64_t)desired,memory_order_seq_cst,memory_order_seq_cst))return (int64_t)expected;}}

typedef void (*disp_thread_entry)(void *);
typedef struct { disp_thread_entry entry; void *context; } disp_thread_boot;
#ifdef _WIN32
static DWORD WINAPI disp_thread_bootstrap(LPVOID raw){disp_thread_boot boot=*(disp_thread_boot*)raw;disp_dealloc(raw);boot.entry(boot.context);return 0;}
static uintptr_t disp_thread_start(disp_thread_entry entry,void *context,int line,int column){disp_thread_boot *boot=(disp_thread_boot*)disp_alloc(sizeof(disp_thread_boot),_Alignof(disp_thread_boot));boot->entry=entry;boot->context=context;HANDLE handle=CreateThread(NULL,0,disp_thread_bootstrap,boot,0,NULL);if(!handle){disp_dealloc(boot);dv_panic("could not create native thread",line,column);}return (uintptr_t)handle;}
static void disp_thread_wait(disp_native_thread *thread){if(!thread->handle)return;HANDLE handle=(HANDLE)thread->handle;DWORD status=WaitForSingleObject(handle,INFINITE);CloseHandle(handle);thread->handle=0;if(status!=WAIT_OBJECT_0)dv_panic("could not join native thread",0,0);}
#else
static void *disp_thread_bootstrap(void *raw){disp_thread_boot boot=*(disp_thread_boot*)raw;disp_dealloc(raw);boot.entry(boot.context);return NULL;}
static uintptr_t disp_thread_start(disp_thread_entry entry,void *context,int line,int column){_Static_assert(sizeof(pthread_t)<=sizeof(uintptr_t),"pthread_t cannot fit DISP thread handle");disp_thread_boot *boot=(disp_thread_boot*)disp_alloc(sizeof(disp_thread_boot),_Alignof(disp_thread_boot));boot->entry=entry;boot->context=context;pthread_t native;if(pthread_create(&native,NULL,disp_thread_bootstrap,boot)!=0){disp_dealloc(boot);dv_panic("could not create native thread",line,column);}uintptr_t handle=0;memcpy(&handle,&native,sizeof(native));return handle;}
static void disp_thread_wait(disp_native_thread *thread){if(!thread->handle)return;pthread_t native;memcpy(&native,&thread->handle,sizeof(native));thread->handle=0;if(pthread_join(native,NULL)!=0)dv_panic("could not join native thread",0,0);}
#endif

static void disp_string_drop(disp_native_string *value){if(value->cap)disp_dealloc(value->data);value->data=NULL;value->len=0;value->cap=0;}
static disp_native_string disp_string_with_capacity(size_t capacity){disp_native_string value={0};if(capacity){value.data=(char*)disp_alloc(capacity,1);value.cap=capacity;}return value;}
static void disp_string_reserve(disp_native_string *value,size_t additional){size_t needed;if(__builtin_add_overflow(value->len,additional,&needed))disp_allocation_failure("string capacity overflow");if(needed<=value->cap)return;size_t capacity=value->cap?value->cap:8;while(capacity<needed){size_t grown;if(__builtin_mul_overflow(capacity,(size_t)2,&grown)){capacity=needed;break;}capacity=grown;}if(value->cap)value->data=(char*)disp_realloc(value->data,capacity,1);else{char *data=(char*)disp_alloc(capacity,1);if(value->len)memcpy(data,value->data,value->len);value->data=data;}value->cap=capacity;}
static void disp_string_push_bytes(disp_native_string *value,const char *bytes,size_t length){disp_string_reserve(value,length);if(length)memcpy(value->data+value->len,bytes,length);value->len+=length;}
static void disp_string_push_char(disp_native_string *value,uint32_t c){char out[4];size_t n;if(c<=0x7F){out[0]=(char)c;n=1;}else if(c<=0x7FF){out[0]=(char)(0xC0|(c>>6));out[1]=(char)(0x80|(c&0x3F));n=2;}else if(c<=0xFFFF && !(c>=0xD800&&c<=0xDFFF)){out[0]=(char)(0xE0|(c>>12));out[1]=(char)(0x80|((c>>6)&0x3F));out[2]=(char)(0x80|(c&0x3F));n=3;}else if(c<=0x10FFFF){out[0]=(char)(0xF0|(c>>18));out[1]=(char)(0x80|((c>>12)&0x3F));out[2]=(char)(0x80|((c>>6)&0x3F));out[3]=(char)(0x80|(c&0x3F));n=4;}else{dv_panic("invalid Unicode scalar",0,0);return;}disp_string_push_bytes(value,out,n);}
static bool disp_utf8_boundary(const char *value,size_t length,size_t index){return index<=length&&(index==0||index==length||(((unsigned char)value[index]&0xC0)!=0x80));}

static bool disp_string_starts_with(const char *value,size_t value_len,const char *prefix,size_t prefix_len){return prefix_len<=value_len&&(prefix_len==0||memcmp(value,prefix,prefix_len)==0);}
static bool disp_string_ends_with(const char *value,size_t value_len,const char *suffix,size_t suffix_len){return suffix_len<=value_len&&(suffix_len==0||memcmp(value+value_len-suffix_len,suffix,suffix_len)==0);}
static bool disp_string_contains(const char *value,size_t value_len,const char *needle,size_t needle_len){if(!needle_len)return true;if(needle_len>value_len)return false;for(size_t i=0;i<=value_len-needle_len;i++)if(memcmp(value+i,needle,needle_len)==0)return true;return false;}

static disp_native_string disp_owned_bytes(const char *bytes,size_t len){disp_native_string value={0};if(len){value.data=(char*)disp_alloc(len,1);memcpy(value.data,bytes,len);value.len=len;value.cap=len;}return value;}
static disp_native_cstring disp_cstring_from_bytes(const char *bytes,size_t len){disp_native_cstring value={0};size_t capacity;if(__builtin_add_overflow(len,(size_t)1,&capacity))disp_allocation_failure("CString length overflow");value.data=(char*)disp_alloc(capacity,1);if(len)memcpy(value.data,bytes,len);value.data[len]=0;value.len=len;value.cap=capacity;return value;}
static void disp_cstring_drop(disp_native_cstring *value){if(value->cap)disp_dealloc(value->data);value->data=NULL;value->len=0;value->cap=0;}
static void disp_memory_drop(disp_native_memory *value){disp_dealloc(value->data);value->data=NULL;value->len=0;value->align=0;}
static disp_native_string disp_io_error(void){const char *message=strerror(errno);return disp_owned_bytes(message,strlen(message));}
static disp_native_path disp_path_from_bytes(const char *bytes,size_t len,int line,int column){if(len&&memchr(bytes,0,len))dv_panic("Path cannot contain a NUL byte",line,column);disp_native_path path={0};path.data=(char*)disp_alloc(len+1,1);if(len)memcpy(path.data,bytes,len);path.data[len]=0;path.len=len;path.cap=len+1;return path;}
static void disp_path_drop(disp_native_path *path){disp_dealloc(path->data);path->data=NULL;path->len=0;path->cap=0;}
static disp_native_path disp_path_join(const disp_native_path *base,const char *child,size_t child_len,int line,int column){if(child_len&&memchr(child,0,child_len))dv_panic("Path cannot contain a NUL byte",line,column);if(!base->len)return disp_path_from_bytes(child,child_len,line,column);bool separator=base->data[base->len-1]!='/'&&base->data[base->len-1]!='\\';size_t len;if(__builtin_add_overflow(base->len,child_len,&len)||__builtin_add_overflow(len,(size_t)separator,&len))disp_allocation_failure("Path length overflow");disp_native_path out={0};out.data=(char*)disp_alloc(len+1,1);memcpy(out.data,base->data,base->len);size_t at=base->len;if(separator)out.data[at++]=
#ifdef _WIN32
'\\';
#else
'/';
#endif
if(child_len)memcpy(out.data+at,child,child_len);out.data[len]=0;out.len=len;out.cap=len+1;return out;}
static bool disp_file_read_text(const disp_native_path *path,disp_native_string *out,disp_native_string *error){FILE *file=fopen(path->data,"rb");if(!file){*error=disp_io_error();return false;}if(fseek(file,0,SEEK_END)!=0){*error=disp_io_error();fclose(file);return false;}long end=ftell(file);if(end<0){*error=disp_io_error();fclose(file);return false;}rewind(file);size_t len=(size_t)end;char *data=len?(char*)disp_alloc(len,1):NULL;size_t read=len?fread(data,1,len,file):0;if(read!=len){*error=disp_io_error();disp_dealloc(data);fclose(file);return false;}if(fclose(file)!=0){*error=disp_io_error();disp_dealloc(data);return false;}out->data=data;out->len=len;out->cap=len;return true;}
static bool disp_file_write_text(const disp_native_path *path,const char *data,size_t len,bool append,disp_native_string *error){FILE *file=fopen(path->data,append?"ab":"wb");if(!file){*error=disp_io_error();return false;}bool ok=!len||fwrite(data,1,len,file)==len;if(!ok)*error=disp_io_error();if(fclose(file)!=0&&ok){*error=disp_io_error();ok=false;}return ok;}
static bool disp_file_exists(const disp_native_path *path){struct stat info;return stat(path->data,&info)==0&&(info.st_mode&S_IFREG)!=0;}
static bool disp_file_metadata(const disp_native_path *path,uint64_t *size,uint64_t *modified,disp_native_string *error){struct stat info;if(stat(path->data,&info)!=0){*error=disp_io_error();return false;}*size=(uint64_t)info.st_size;*modified=(uint64_t)info.st_mtime;return true;}
static bool disp_directory_exists(const disp_native_path *path){struct stat info;return stat(path->data,&info)==0&&(info.st_mode&S_IFDIR)!=0;}
static bool disp_file_remove(const disp_native_path *path,disp_native_string *error){if(remove(path->data)==0)return true;*error=disp_io_error();return false;}
static bool disp_file_copy(const disp_native_path *from,const disp_native_path *to,disp_native_string *error){disp_native_string data={0};if(!disp_file_read_text(from,&data,error))return false;bool ok=disp_file_write_text(to,data.data,data.len,false,error);disp_string_drop(&data);return ok;}
static bool disp_file_move(const disp_native_path *from,const disp_native_path *to,disp_native_string *error){if(rename(from->data,to->data)==0)return true;*error=disp_io_error();return false;}
static bool disp_directory_create(const disp_native_path *path,disp_native_string *error){
#ifdef _WIN32
int result=_mkdir(path->data);
#else
int result=mkdir(path->data,0777);
#endif
if(result==0)return true;*error=disp_io_error();return false;}
static bool disp_directory_create_all(const disp_native_path *path,disp_native_string *error){char *copy=(char*)disp_alloc(path->len+1,1);memcpy(copy,path->data,path->len+1);for(size_t i=1;i<path->len;i++)if(copy[i]=='/'||copy[i]=='\\'){char saved=copy[i];copy[i]=0;
#ifdef _WIN32
if(strlen(copy)>2&&_mkdir(copy)!=0&&errno!=EEXIST){*error=disp_io_error();disp_dealloc(copy);return false;}
#else
if(mkdir(copy,0777)!=0&&errno!=EEXIST){*error=disp_io_error();disp_dealloc(copy);return false;}
#endif
copy[i]=saved;}
#ifdef _WIN32
int result=_mkdir(copy);
#else
int result=mkdir(copy,0777);
#endif
disp_dealloc(copy);if(result==0||errno==EEXIST)return true;*error=disp_io_error();return false;}
static bool disp_directory_remove(const disp_native_path *path,disp_native_string *error){
#ifdef _WIN32
int result=_rmdir(path->data);
#else
int result=rmdir(path->data);
#endif
if(result==0)return true;*error=disp_io_error();return false;}
static bool disp_directory_push_entry(disp_native_path **items,size_t *len,size_t *cap,disp_native_path value,disp_native_string *error){if(*len==*cap){size_t next=*cap?*cap*2:8,bytes;if(next<*cap||__builtin_mul_overflow(next,sizeof(disp_native_path),&bytes)){errno=EOVERFLOW;*error=disp_io_error();return false;}*items=(disp_native_path*)disp_realloc(*items,bytes,_Alignof(disp_native_path));*cap=next;}(*items)[(*len)++]=value;return true;}
static void disp_directory_entries_drop(disp_native_path *items,size_t len){for(size_t i=0;i<len;i++)disp_path_drop(&items[i]);disp_dealloc(items);}
static bool disp_directory_read(const disp_native_path *path,disp_native_path **items,size_t *len,size_t *cap,disp_native_string *error){*items=NULL;*len=0;*cap=0;
#ifdef _WIN32
disp_native_path pattern=disp_path_join(path,"*",1,0,0);WIN32_FIND_DATAA data;HANDLE handle=FindFirstFileA(pattern.data,&data);disp_path_drop(&pattern);if(handle==INVALID_HANDLE_VALUE){errno=ENOENT;*error=disp_io_error();return false;}do{const char *name=data.cFileName;if(strcmp(name,".")&&strcmp(name,"..")){disp_native_path entry=disp_path_join(path,name,strlen(name),0,0);if(!disp_directory_push_entry(items,len,cap,entry,error)){disp_path_drop(&entry);FindClose(handle);disp_directory_entries_drop(*items,*len);*items=NULL;*len=*cap=0;return false;}}}while(FindNextFileA(handle,&data));DWORD status=GetLastError();FindClose(handle);if(status!=ERROR_NO_MORE_FILES){errno=EIO;*error=disp_io_error();disp_directory_entries_drop(*items,*len);*items=NULL;*len=*cap=0;return false;}return true;
#else
DIR *directory=opendir(path->data);if(!directory){*error=disp_io_error();return false;}struct dirent *entry;while((entry=readdir(directory))){const char *name=entry->d_name;if(strcmp(name,".")&&strcmp(name,"..")){disp_native_path value=disp_path_join(path,name,strlen(name),0,0);if(!disp_directory_push_entry(items,len,cap,value,error)){disp_path_drop(&value);closedir(directory);disp_directory_entries_drop(*items,*len);*items=NULL;*len=*cap=0;return false;}}}if(closedir(directory)!=0){*error=disp_io_error();disp_directory_entries_drop(*items,*len);*items=NULL;*len=*cap=0;return false;}return true;
#endif
}
static uint64_t disp_time_now_nanos(void){
#ifdef _WIN32
LARGE_INTEGER counter,frequency;QueryPerformanceCounter(&counter);QueryPerformanceFrequency(&frequency);uint64_t value=(uint64_t)counter.QuadPart,rate=(uint64_t)frequency.QuadPart;return (value/rate)*1000000000ULL+((value%rate)*1000000000ULL)/rate;
#else
struct timespec value;clock_gettime(CLOCK_MONOTONIC,&value);return (uint64_t)value.tv_sec*1000000000ULL+(uint64_t)value.tv_nsec;
#endif
}
static uint64_t disp_time_unix_seconds(void){return (uint64_t)time(NULL);}
static void disp_time_sleep(uint64_t nanos){
#ifdef _WIN32
while(nanos){uint64_t millis=nanos/1000000ULL+(nanos%1000000ULL!=0);DWORD chunk=millis>0xffffffffULL?0xffffffffUL:(DWORD)millis;Sleep(chunk);uint64_t slept=(uint64_t)chunk*1000000ULL;if(slept>=nanos)break;nanos-=slept;}
#else
struct timespec value={(time_t)(nanos/1000000000ULL),(long)(nanos%1000000000ULL)};while(nanosleep(&value,&value)!=0&&errno==EINTR){}
#endif
}

typedef enum { DV_UNIT, DV_SIGNED, DV_UNSIGNED, DV_FLOAT, DV_BOOL, DV_CHAR, DV_STRING, DV_AGG, DV_REF, DV_RAW } DVTag;
typedef struct DV DV;
typedef struct { size_t refs, count; uint64_t disc; const char *type_name, *variant_name; DV *fields; } DVAgg;
struct DV { DVTag tag; uint16_t width; union { __int128 si; unsigned __int128 ui; double fp; bool boolean; uint32_t ch; struct { const char *data; size_t len; } string; DVAgg *agg; DV *reference; } as; };

static DV dv_unit(void){ DV v={0}; v.tag=DV_UNIT; return v; }
static DV dv_bool(bool x){ DV v=dv_unit(); v.tag=DV_BOOL; v.as.boolean=x; return v; }
static DV dv_i(__int128 x,uint16_t w){ DV v=dv_unit(); v.tag=DV_SIGNED; v.width=w?w:64; v.as.si=x; return v; }
static DV dv_u(unsigned __int128 x,uint16_t w){ DV v=dv_unit(); v.tag=DV_UNSIGNED; v.width=w?w:64; v.as.ui=x; return v; }
static DV dv_u128(uint64_t hi,uint64_t lo,uint16_t w){ return dv_u(((unsigned __int128)hi<<64)|lo,w); }
static DV dv_f(double x,uint16_t w){ DV v=dv_unit(); v.tag=DV_FLOAT; v.width=w; v.as.fp=x; return v; }
static DV dv_char(uint32_t x){ DV v=dv_unit(); v.tag=DV_CHAR; v.as.ch=x; return v; }
static DV dv_string(const char *x,size_t n){ DV v=dv_unit(); v.tag=DV_STRING; v.as.string.data=x; v.as.string.len=n; return v; }
static DV dv_ref(DV *x,bool raw){ DV v=dv_unit(); v.tag=raw?DV_RAW:DV_REF; v.as.reference=x; return v; }
static DV dv_aggregate(const char *type_name,const char *variant_name,uint64_t disc,size_t count,DV *values){ DV v=dv_unit(); DVAgg *a=(DVAgg*)disp_alloc_zeroed(1,sizeof(DVAgg),_Alignof(DVAgg)); a->refs=1;a->count=count;a->disc=disc;a->type_name=type_name;a->variant_name=variant_name;a->fields=(DV*)disp_alloc_zeroed(count?count:1,sizeof(DV),_Alignof(DV)); for(size_t i=0;i<count;i++)a->fields[i]=values[i];v.tag=DV_AGG;v.as.agg=a;return v; }
static DV dv_copy(DV v){ if(v.tag==DV_AGG)v.as.agg->refs++; return v; }
static void dv_drop(DV *v){ if(v->tag==DV_AGG && v->as.agg && --v->as.agg->refs==0){ for(size_t i=v->as.agg->count;i>0;i--)dv_drop(&v->as.agg->fields[i-1]);disp_dealloc(v->as.agg->fields);disp_dealloc(v->as.agg); } *v=dv_unit(); }
static DV dv_move(DV *v){ DV out=*v;*v=dv_unit();return out; }
static void dv_store(DV *place,DV value){ dv_drop(place);*place=value; }
static DV *dv_field(DV *base,size_t index){ if(base->tag!=DV_AGG||index>=base->as.agg->count){fputs("DISP runtime error: invalid field projection\n",stderr);exit(101);}return &base->as.agg->fields[index]; }
static DV *dv_deref(DV *base){ if((base->tag!=DV_REF&&base->tag!=DV_RAW)||!base->as.reference){fputs("DISP runtime error: invalid pointer dereference\n",stderr);exit(101);}return base->as.reference; }
static uint64_t dv_disc(DV v){ if(v.tag!=DV_AGG){fputs("DISP runtime error: discriminant of non-enum\n",stderr);exit(101);}return v.as.agg->disc; }
static bool dv_truth(DV v){ if(v.tag==DV_BOOL)return v.as.boolean; if(v.tag==DV_SIGNED)return v.as.si!=0; if(v.tag==DV_UNSIGNED)return v.as.ui!=0; return false; }
static void dv_panic(const char *message,int line,int column){fprintf(stderr,"DISP runtime error at %d:%d: %s\n",line,column,message);exit(101);}
static unsigned __int128 umax(uint16_t w){return w>=128?~(unsigned __int128)0:(((unsigned __int128)1<<w)-1);}
static __int128 smax(uint16_t w){return w>=128?(__int128)(~(unsigned __int128)0>>1):(((__int128)1<<(w-1))-1);}
static __int128 smin(uint16_t w){return w>=128?-smax(128)-1:-((__int128)1<<(w-1));}
static DV dv_coerce(DV v,int kind,uint16_t width,int is_signed,int line,int col){ if(kind==0)return v; if(kind==1){if(is_signed){if(v.tag==DV_UNSIGNED&&v.as.ui>(unsigned __int128)smax(width))dv_panic("integer value outside destination range",line,col);__int128 x=v.tag==DV_SIGNED?v.as.si:(__int128)v.as.ui;if(x<smin(width)||x>smax(width))dv_panic("integer value outside destination range",line,col);return dv_i(x,width);}if(v.tag==DV_SIGNED&&v.as.si<0)dv_panic("integer value outside destination range",line,col);unsigned __int128 x=v.tag==DV_SIGNED?(unsigned __int128)v.as.si:v.as.ui;if(x>umax(width))dv_panic("integer value outside destination range",line,col);return dv_u(x,width);}if(kind==2){double x=v.tag==DV_FLOAT?v.as.fp:(v.tag==DV_SIGNED?(double)v.as.si:(double)v.as.ui);return dv_f(width==32?(double)(float)x:x,width);}if(kind==3)return dv_bool(dv_truth(v));return v; }
static DV dv_binary(int op,DV a,DV b,int line,int col){ if(op>=6){bool r=false;if(op==12)return dv_bool(dv_truth(a)&&dv_truth(b));if(op==13)return dv_bool(dv_truth(a)||dv_truth(b));if(a.tag==DV_STRING&&b.tag==DV_STRING){int c=a.as.string.len==b.as.string.len?memcmp(a.as.string.data,b.as.string.data,a.as.string.len):(a.as.string.len<b.as.string.len?-1:1);r=op==6?c==0:op==7?c!=0:op==8?c<0:op==9?c<=0:op==10?c>0:c>=0;}else if(a.tag==DV_FLOAT||b.tag==DV_FLOAT){double x=a.tag==DV_FLOAT?a.as.fp:(a.tag==DV_SIGNED?(double)a.as.si:(double)a.as.ui),y=b.tag==DV_FLOAT?b.as.fp:(b.tag==DV_SIGNED?(double)b.as.si:(double)b.as.ui);r=op==6?x==y:op==7?x!=y:op==8?x<y:op==9?x<=y:op==10?x>y:x>=y;}else{__int128 x=a.tag==DV_SIGNED?a.as.si:(__int128)a.as.ui,y=b.tag==DV_SIGNED?b.as.si:(__int128)b.as.ui;r=op==6?x==y:op==7?x!=y:op==8?x<y:op==9?x<=y:op==10?x>y:x>=y;}return dv_bool(r);}if(a.tag==DV_FLOAT||b.tag==DV_FLOAT){double x=a.tag==DV_FLOAT?a.as.fp:(a.tag==DV_SIGNED?(double)a.as.si:(double)a.as.ui),y=b.tag==DV_FLOAT?b.as.fp:(b.tag==DV_SIGNED?(double)b.as.si:(double)b.as.ui);if((op==3||op==4)&&y==0)dv_panic("division by zero",line,col);return dv_f(op==0?x+y:op==1?x-y:op==2?x*y:op==3?x/y:fmod(x,y),a.width?a.width:b.width);}uint16_t w=a.width?a.width:b.width;if(a.tag==DV_UNSIGNED&&b.tag==DV_UNSIGNED){unsigned __int128 x=a.as.ui,y=b.as.ui,z=0;bool overflow=false;if((op==3||op==4)&&y==0)dv_panic("division by zero",line,col);if(op==0)overflow=__builtin_add_overflow(x,y,&z);else if(op==1)overflow=__builtin_sub_overflow(x,y,&z);else if(op==2)overflow=__builtin_mul_overflow(x,y,&z);else if(op==3)z=x/y;else z=x%y;if(overflow||z>umax(w))dv_panic("integer overflow",line,col);return dv_u(z,w);}__int128 x=a.tag==DV_SIGNED?a.as.si:(__int128)a.as.ui,y=b.tag==DV_SIGNED?b.as.si:(__int128)b.as.ui,z=0;bool overflow=false;if((op==3||op==4)&&y==0)dv_panic("division by zero",line,col);if((op==3||op==4)&&x==smin(w)&&y==-1)dv_panic("integer overflow",line,col);if(op==0)overflow=__builtin_add_overflow(x,y,&z);else if(op==1)overflow=__builtin_sub_overflow(x,y,&z);else if(op==2)overflow=__builtin_mul_overflow(x,y,&z);else if(op==3)z=x/y;else z=x%y;if(overflow||z<smin(w)||z>smax(w))dv_panic("integer overflow",line,col);return dv_i(z,w); }
static DV dv_unary(int op,DV value,int line,int col){if(op==1)return dv_bool(!dv_truth(value));if(value.tag==DV_FLOAT)return dv_f(-value.as.fp,value.width);if(value.tag==DV_SIGNED){if(value.as.si==smin(value.width))dv_panic("integer overflow",line,col);return dv_i(-value.as.si,value.width);}if(value.tag==DV_UNSIGNED&&value.as.ui<=(unsigned __int128)smax(value.width)+1)return dv_i(value.width>=128&&value.as.ui==(unsigned __int128)smax(128)+1?smin(128):-(__int128)value.as.ui,value.width);dv_panic("cannot negate unsigned value",line,col);return dv_unit();}
static bool dv_equal(DV a,DV b){return dv_truth(dv_binary(6,a,b,0,0));}
static DV dv_intrinsic(const char *name,size_t count,DV *args,int line,int col){if(strcmp(name,"wrapping_add")==0||strcmp(name,"wrapping_sub")==0||strcmp(name,"wrapping_mul")==0){DV a=args[0],b=args[1];uint16_t w=a.width;unsigned __int128 x=a.tag==DV_SIGNED?(unsigned __int128)a.as.si:a.as.ui,y=b.tag==DV_SIGNED?(unsigned __int128)b.as.si:b.as.ui,z=strstr(name,"add")?x+y:strstr(name,"sub")?x-y:x*y;z&=umax(w);if(a.tag==DV_SIGNED&&w<128&&z>((unsigned __int128)1<<(w-1))-1) return dv_i((__int128)(z-((unsigned __int128)1<<w)),w);return a.tag==DV_SIGNED?dv_i((__int128)z,w):dv_u(z,w);}if(strncmp(name,"saturating_",11)==0){DV a=args[0],b=args[1];int op=strstr(name,"add")?0:strstr(name,"sub")?1:2;DV r; if(a.tag==DV_UNSIGNED){unsigned __int128 x=a.as.ui,y=b.as.ui,z;bool ov=op==0?__builtin_add_overflow(x,y,&z):op==1?__builtin_sub_overflow(x,y,&z):__builtin_mul_overflow(x,y,&z);if(ov||z>umax(a.width))z=op==1?0:umax(a.width);r=dv_u(z,a.width);}else{__int128 x=a.as.si,y=b.as.si,z;bool ov=op==0?__builtin_add_overflow(x,y,&z):op==1?__builtin_sub_overflow(x,y,&z):__builtin_mul_overflow(x,y,&z);if(ov||z<smin(a.width)||z>smax(a.width)){bool toward_min=op==0?x<0:op==1?x<y:((x<0)!=(y<0));z=toward_min?smin(a.width):smax(a.width);}r=dv_i(z,a.width);}return r;}if(strstr(name,"try_from")){uint16_t w=(uint16_t)atoi(name+1);bool sign=name[0]=='i';__int128 x=args[0].tag==DV_SIGNED?args[0].as.si:(__int128)args[0].as.ui;bool valid=sign?(x>=smin(w)&&x<=smax(w)):(x>=0&&(unsigned __int128)x<=umax(w));if(valid){DV v=sign?dv_i(x,w):dv_u((unsigned __int128)x,w);DV fields[1]={v};return dv_aggregate("Result","Ok",18893,1,fields);}DV message=dv_string("conversion out of range",23);DV fields[1]={message};return dv_aggregate("Result","Err",76404,1,fields);}if(count==1)return args[0];dv_panic("unknown native intrinsic",line,col);return dv_unit();}
static void print_u128(unsigned __int128 x){char b[64];int n=0;if(!x){putchar('0');return;}while(x){b[n++]=(char)('0'+x%10);x/=10;}while(n)putchar(b[--n]);}
static void print_i128(__int128 x){if(x<0){putchar('-');print_u128((unsigned __int128)(-(x+1))+1);}else print_u128((unsigned __int128)x);}
static void print_char(uint32_t c){unsigned char out[4];size_t n;if(c<=0x7F){out[0]=(unsigned char)c;n=1;}else if(c<=0x7FF){out[0]=(unsigned char)(0xC0|(c>>6));out[1]=(unsigned char)(0x80|(c&0x3F));n=2;}else if(c<=0xFFFF){out[0]=(unsigned char)(0xE0|(c>>12));out[1]=(unsigned char)(0x80|((c>>6)&0x3F));out[2]=(unsigned char)(0x80|(c&0x3F));n=3;}else{out[0]=(unsigned char)(0xF0|(c>>18));out[1]=(unsigned char)(0x80|((c>>12)&0x3F));out[2]=(unsigned char)(0x80|((c>>6)&0x3F));out[3]=(unsigned char)(0x80|(c&0x3F));n=4;}fwrite(out,1,n,stdout);}
static void dv_print_value(DV v){switch(v.tag){case DV_UNIT:fputs("()",stdout);break;case DV_SIGNED:print_i128(v.as.si);break;case DV_UNSIGNED:print_u128(v.as.ui);break;case DV_FLOAT:printf("%.15g",v.as.fp);break;case DV_BOOL:fputs(v.as.boolean?"true":"false",stdout);break;case DV_CHAR:{uint32_t c=v.as.ch;print_char(c);break;}case DV_STRING:fwrite(v.as.string.data,1,v.as.string.len,stdout);break;case DV_REF:case DV_RAW:dv_print_value(*v.as.reference);break;case DV_AGG:if(v.as.agg->variant_name){fputs(v.as.agg->type_name,stdout);putchar('.');fputs(v.as.agg->variant_name,stdout);if(v.as.agg->count){putchar('(');for(size_t i=0;i<v.as.agg->count;i++){if(i)fputs(", ",stdout);dv_print_value(v.as.agg->fields[i]);}putchar(')');}}else{putchar('<');fputs(v.as.agg->type_name,stdout);putchar('>');}break;}}
static DV dv_print(DV value){dv_print_value(value);putchar('\n');dv_drop(&value);return dv_unit();}
"#;
