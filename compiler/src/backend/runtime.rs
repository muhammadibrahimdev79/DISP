pub const C_RUNTIME: &str = r#"
#ifndef _WIN32
#ifndef _GNU_SOURCE
#define _GNU_SOURCE
#endif
#endif
#include <stdint.h>
#include <stddef.h>
#include <stdbool.h>
#include <stdatomic.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <math.h>
#include <errno.h>
#include <limits.h>
#include <ctype.h>
#include <time.h>
#include <setjmp.h>
#include <sys/stat.h>
#ifdef _WIN32
#ifdef DISP_NETWORKING
#include <winsock2.h>
#include <ws2tcpip.h>
#define SECURITY_WIN32
#include <security.h>
#define SCHANNEL_USE_BLACKLISTS
#include <schannel.h>
#endif
#include <windows.h>
#include <bcrypt.h>
#include <shellapi.h>
#include <io.h>
#ifdef DISP_HTTP
#include <winhttp.h>
#endif
#include <direct.h>
#else
#ifdef DISP_NETWORKING
#include <arpa/inet.h>
#include <fcntl.h>
#include <netdb.h>
#ifdef DISP_TLS
#include <openssl/err.h>
#include <openssl/ssl.h>
#include <openssl/x509v3.h>
#endif
#include <sys/socket.h>
#endif
#ifdef DISP_HTTP
#include <curl/curl.h>
#endif
#include <dirent.h>
#include <fcntl.h>
#include <pthread.h>
#include <sched.h>
#include <signal.h>
#include <sys/file.h>
#include <sys/resource.h>
#include <unistd.h>
#include <sys/wait.h>
#ifdef __linux__
#include <linux/audit.h>
#include <linux/filter.h>
#include <linux/if_alg.h>
#include <linux/seccomp.h>
#include <sys/prctl.h>
#include <sys/socket.h>
#include <sys/syscall.h>
#endif
#endif

static void dv_panic(const char *message,int line,int column);
static _Thread_local jmp_buf *disp_ffi_panic_target;
static _Thread_local char disp_ffi_last_error[512];
static _Thread_local bool disp_ffi_thread_attached;
typedef struct {
DispRollbackHook hook;
void *provider_context;
void (*quiesce)(void *);
void (*release)(void *);
disp_native_callable *callback;
} disp_c_registration_rollback;
static void disp_c_registration_release_parts(void *context,void (*quiesce)(void*),void (*release)(void*),disp_native_callable *callback){
if(quiesce)quiesce(context);
if(callback){if(callback->drop)callback->drop(callback->env);disp_dealloc(callback);}
release(context);
}
static void disp_c_registration_rollback_cleanup(void *raw){
disp_c_registration_rollback *rollback=(disp_c_registration_rollback*)raw;
disp_c_registration_release_parts(rollback->provider_context,rollback->quiesce,rollback->release,rollback->callback);
}
static disp_native_c_registration disp_c_registration_open(void *context,void (*quiesce)(void*),void (*release)(void*),disp_native_callable *callback,int line,int column){
if(!release){if(callback){if(callback->drop)callback->drop(callback->env);disp_dealloc(callback);}dv_panic("C registration release callback is null",line,column);}
disp_native_c_registration registration={.context=context,.quiesce=quiesce,.release=release,.callback=callback,.rollback=NULL,.active=true};
if(disp_ffi_allocation_boundary_active){
disp_c_registration_rollback *rollback=(disp_c_registration_rollback*)malloc(sizeof(disp_c_registration_rollback));
if(!rollback){disp_c_registration_release_parts(context,quiesce,release,callback);dv_panic("out of memory while recording C registration rollback",line,column);}
rollback->provider_context=context;rollback->quiesce=quiesce;rollback->release=release;rollback->callback=callback;
disp_ffi_track_rollback(&rollback->hook,disp_c_registration_rollback_cleanup,rollback);
registration.rollback=&rollback->hook;
}
return registration;
}
static void disp_c_registration_close(disp_native_c_registration *registration){
if(!registration||!registration->active)return;
void *context=registration->context;void (*quiesce)(void*)=registration->quiesce;void (*release)(void*)=registration->release;
disp_native_callable *callback=registration->callback;
DispRollbackHook *rollback=registration->rollback;
registration->active=false;registration->context=NULL;registration->quiesce=NULL;registration->release=NULL;registration->callback=NULL;registration->rollback=NULL;
if(!release)dv_panic("C registration release callback is null",0,0);
if(rollback){disp_ffi_untrack_rollback(rollback);free(rollback);}
disp_c_registration_release_parts(context,quiesce,release,callback);
}
static disp_native_string disp_owned_bytes(const char *bytes,size_t len);
static bool disp_utf8_valid(const char *value,size_t length);
static void disp_string_drop(disp_native_string *value);
static void disp_path_drop(disp_native_path *value);
static void disp_crypto_zero(void *value,size_t length){volatile unsigned char *bytes=(volatile unsigned char*)value;while(length--)*bytes++=0;}
static bool disp_crypto_random_bytes(size_t length,disp_native_string *out,disp_native_string *error){
if(!length||length>1048576){const char *message="secure-random byte length must be between 1 and 1048576";*error=disp_owned_bytes(message,strlen(message));return false;}
out->data=(char*)disp_alloc(length,1);out->len=out->cap=length;size_t offset=0;bool ok=true;
#ifdef _WIN32
NTSTATUS status=BCryptGenRandom(NULL,(PUCHAR)out->data,(ULONG)length,BCRYPT_USE_SYSTEM_PREFERRED_RNG);ok=status>=0;offset=ok?length:0;
#elif defined(__linux__)
while(offset<length){ssize_t count=(ssize_t)syscall(SYS_getrandom,out->data+offset,length-offset,0);if(count>0){offset+=(size_t)count;continue;}if(count<0&&errno==EINTR)continue;ok=false;break;}
#else
while(offset<length){size_t chunk=length-offset;if(chunk>256)chunk=256;if(getentropy(out->data+offset,chunk)!=0){ok=false;break;}offset+=chunk;}
#endif
if(ok&&offset==length)return true;disp_crypto_zero(out->data,length);disp_dealloc(out->data);*out=(disp_native_string){0};const char *message="secure operating-system entropy is unavailable";*error=disp_owned_bytes(message,strlen(message));return false;
}
static bool disp_crypto_random_secret(size_t length,disp_native_secret *out,disp_native_string *error){if(!length||length>1048576){const char *message="secure-random secret length must be between 1 and 1048576";*error=disp_owned_bytes(message,strlen(message));return false;}disp_native_string bytes={0};if(!disp_crypto_random_bytes(length,&bytes,error))return false;out->data=(uint8_t*)bytes.data;out->len=bytes.len;out->cap=bytes.cap;return true;}
static void disp_secret_drop(disp_native_secret *value){if(!value)return;if(value->data){disp_crypto_zero(value->data,value->cap);disp_dealloc(value->data);}*value=(disp_native_secret){0};}
static bool disp_crypto_import_secret(uint8_t *data,size_t len,size_t cap,disp_native_secret *out,disp_native_string *error){if(len>1048576){if(data){disp_crypto_zero(data,cap);disp_dealloc(data);}char message[128];int length=snprintf(message,sizeof(message),"SecretBytes requested %zu bytes but the maximum is 1048576",len);if(length<0)length=0;*error=disp_owned_bytes(message,(size_t)length);return false;}out->data=data;out->len=len;out->cap=cap;return true;}
static bool disp_secret_constant_time_equals(const disp_native_secret *left,const disp_native_secret *right){if(left->len!=right->len)return false;uint8_t difference=0;for(size_t index=0;index<left->len;index++)difference|=(uint8_t)(left->data[index]^right->data[index]);return difference==0;}
static disp_native_string disp_crypto_error(const char *operation){char message[128];int length=snprintf(message,sizeof(message),"%s provider operation failed",operation);if(length<0)length=0;return disp_owned_bytes(message,(size_t)length);}
static bool disp_crypto_sha256_provider(const uint8_t *message,size_t message_len,const uint8_t *key,size_t key_len,uint8_t digest[32]){
#ifdef _WIN32
BCRYPT_ALG_HANDLE algorithm=NULL;BCRYPT_HASH_HANDLE hash=NULL;PUCHAR object=NULL;DWORD object_len=0,written=0;ULONG flags=key?BCRYPT_ALG_HANDLE_HMAC_FLAG:0;NTSTATUS status=BCryptOpenAlgorithmProvider(&algorithm,BCRYPT_SHA256_ALGORITHM,NULL,flags);if(status<0)goto done;status=BCryptGetProperty(algorithm,BCRYPT_OBJECT_LENGTH,(PUCHAR)&object_len,sizeof(object_len),&written,0);if(status<0||written!=sizeof(object_len)||!object_len)goto done;object=(PUCHAR)disp_alloc(object_len,1);status=BCryptCreateHash(algorithm,&hash,object,object_len,(PUCHAR)key,(ULONG)key_len,0);if(status<0)goto done;status=BCryptHashData(hash,(PUCHAR)message,(ULONG)message_len,0);if(status<0)goto done;status=BCryptFinishHash(hash,digest,32,0);
done:if(hash)BCryptDestroyHash(hash);if(object){disp_crypto_zero(object,object_len);disp_dealloc(object);}if(algorithm)BCryptCloseAlgorithmProvider(algorithm,0);return status>=0;
#elif defined(__linux__)
int algorithm=-1,operation=-1;bool ok=false;struct sockaddr_alg address={0};address.salg_family=AF_ALG;memcpy(address.salg_type,"hash",5);const char *name=key?"hmac(sha256)":"sha256";memcpy(address.salg_name,name,strlen(name)+1);algorithm=socket(AF_ALG,SOCK_SEQPACKET,0);if(algorithm<0)goto done;if(bind(algorithm,(struct sockaddr*)&address,sizeof(address))!=0)goto done;if(key&&setsockopt(algorithm,SOL_ALG,ALG_SET_KEY,key,(socklen_t)key_len)!=0)goto done;operation=accept(algorithm,NULL,0);if(operation<0)goto done;ssize_t sent=send(operation,message,message_len,0);if(sent<0||(size_t)sent!=message_len)goto done;size_t offset=0;while(offset<32){ssize_t count=read(operation,digest+offset,32-offset);if(count>0){offset+=(size_t)count;continue;}if(count<0&&errno==EINTR)continue;goto done;}ok=true;
done:if(operation>=0)close(operation);if(algorithm>=0)close(algorithm);return ok;
#else
(void)message;(void)message_len;(void)key;(void)key_len;(void)digest;return false;
#endif
}
static bool disp_crypto_sha256(const uint8_t *message,size_t message_len,disp_native_string *out,disp_native_string *error){if(message_len>16777216){*error=disp_owned_bytes("SHA-256 message exceeds 16777216 bytes",strlen("SHA-256 message exceeds 16777216 bytes"));return false;}uint8_t digest[32];if(!disp_crypto_sha256_provider(message,message_len,NULL,0,digest)){*error=disp_crypto_error("SHA-256");return false;}out->data=(char*)disp_alloc(32,1);memcpy(out->data,digest,32);out->len=out->cap=32;return true;}
static bool disp_crypto_hmac_sha256(const disp_native_secret *key,const uint8_t *message,size_t message_len,disp_native_string *out,disp_native_string *error){if(!key->len||key->len>1048576){*error=disp_owned_bytes("HMAC-SHA-256 rejected the key",strlen("HMAC-SHA-256 rejected the key"));return false;}if(message_len>16777216){*error=disp_owned_bytes("HMAC-SHA-256 message exceeds 16777216 bytes",strlen("HMAC-SHA-256 message exceeds 16777216 bytes"));return false;}uint8_t digest[32];if(!disp_crypto_sha256_provider(message,message_len,key->data,key->len,digest)){*error=disp_crypto_error("HMAC-SHA-256");return false;}out->data=(char*)disp_alloc(32,1);memcpy(out->data,digest,32);out->len=out->cap=32;disp_crypto_zero(digest,sizeof(digest));return true;}
static bool disp_crypto_hmac_sha256_verify(const disp_native_secret *key,const uint8_t *message,size_t message_len,const uint8_t *expected,size_t expected_len,bool *valid,disp_native_string *error){disp_native_string actual={0};if(!disp_crypto_hmac_sha256(key,message,message_len,&actual,error))return false;uint8_t difference=(uint8_t)(expected_len!=32);if(expected_len==32)for(size_t index=0;index<32;index++)difference|=(uint8_t)(((uint8_t*)actual.data)[index]^expected[index]);*valid=difference==0;disp_crypto_zero(actual.data,actual.cap);disp_dealloc(actual.data);return true;}
static bool disp_crypto_hkdf_sha256(const uint8_t *salt,size_t salt_len,const disp_native_secret *input,const uint8_t *info,size_t info_len,size_t output_len,disp_native_secret *out,disp_native_string *error){if(salt_len>1048576||info_len>1048576){const char *message=salt_len>1048576?"HKDF-SHA-256 salt exceeds 1048576 bytes":"HKDF-SHA-256 info exceeds 1048576 bytes";*error=disp_owned_bytes(message,strlen(message));return false;}if(!output_len||output_len>8160){char message[128];int length=snprintf(message,sizeof(message),"HKDF-SHA-256 requested %zu bytes but the maximum is 8160",output_len);if(length<0)length=0;*error=disp_owned_bytes(message,(size_t)length);return false;}uint8_t zero_salt[32]={0},prk[32]={0},block[32]={0};const uint8_t *extract_key=salt_len?salt:zero_salt;size_t extract_key_len=salt_len?salt_len:sizeof(zero_salt);if(!disp_crypto_sha256_provider(input->data,input->len,extract_key,extract_key_len,prk)){*error=disp_crypto_error("HKDF-SHA-256");disp_crypto_zero(zero_salt,sizeof(zero_salt));disp_crypto_zero(prk,sizeof(prk));return false;}size_t message_cap=info_len+33;uint8_t *message=(uint8_t*)disp_alloc(message_cap,1);uint8_t *output=(uint8_t*)disp_alloc(output_len,1);size_t produced=0,previous_len=0;uint8_t counter=1;bool ok=true;while(produced<output_len){if(previous_len)memcpy(message,block,previous_len);if(info_len)memcpy(message+previous_len,info,info_len);message[previous_len+info_len]=counter;if(!disp_crypto_sha256_provider(message,previous_len+info_len+1,prk,sizeof(prk),block)){ok=false;break;}size_t take=output_len-produced;if(take>sizeof(block))take=sizeof(block);memcpy(output+produced,block,take);produced+=take;previous_len=sizeof(block);counter++;}disp_crypto_zero(zero_salt,sizeof(zero_salt));disp_crypto_zero(prk,sizeof(prk));disp_crypto_zero(block,sizeof(block));disp_crypto_zero(message,message_cap);disp_dealloc(message);if(!ok){disp_crypto_zero(output,output_len);disp_dealloc(output);*error=disp_crypto_error("HKDF-SHA-256");return false;}out->data=output;out->len=out->cap=output_len;return true;}
static disp_native_string disp_crypto_aead_encoding_error(void){return disp_owned_bytes("DISP AEAD envelope rejected malformed input",strlen("DISP AEAD envelope rejected malformed input"));}
static bool disp_crypto_aead_encode(const disp_native_string *envelope,disp_native_string *out,disp_native_string *error){if(envelope->len<28||envelope->len>1048604){*error=disp_crypto_aead_encoding_error();return false;}size_t ciphertext_len=envelope->len-12,total=envelope->len+16;uint8_t *encoded=(uint8_t*)disp_alloc(total,1);memcpy(encoded,"DISP",4);encoded[4]=1;encoded[5]=1;encoded[6]=12;encoded[7]=16;uint64_t length=(uint64_t)ciphertext_len;for(size_t index=0;index<8;index++)encoded[15-index]=(uint8_t)(length>>(index*8));memcpy(encoded+16,envelope->data,envelope->len);out->data=(char*)encoded;out->len=out->cap=total;return true;}
static bool disp_crypto_aead_decode(const uint8_t *encoded,size_t encoded_len,disp_native_string *out,disp_native_string *error){if(encoded_len<44||encoded_len>1048620||memcmp(encoded,"DISP",4)!=0||encoded[4]!=1||encoded[5]!=1||encoded[6]!=12||encoded[7]!=16){*error=disp_crypto_aead_encoding_error();return false;}uint64_t ciphertext_len=0;for(size_t index=8;index<16;index++)ciphertext_len=(ciphertext_len<<8)|encoded[index];if(ciphertext_len<16||ciphertext_len>1048592||ciphertext_len!=(uint64_t)(encoded_len-28)){*error=disp_crypto_aead_encoding_error();return false;}size_t envelope_len=encoded_len-16;char *envelope=(char*)disp_alloc(envelope_len,1);memcpy(envelope,encoded+16,envelope_len);out->data=envelope;out->len=out->cap=envelope_len;return true;}
static bool disp_crypto_ed25519_record(const uint8_t *input,size_t input_len,uint8_t kind,size_t payload_len,bool decode,disp_native_string *out,disp_native_string *error){bool valid=decode?(input_len==payload_len+8&&memcmp(input,"DISP",4)==0&&input[4]==1&&input[5]==kind&&input[6]==1&&input[7]==payload_len):(input_len==payload_len);if(!valid){const char *message=kind==2?"DISP Ed25519 public key rejected malformed input":"DISP Ed25519 signature rejected malformed input";*error=disp_owned_bytes(message,strlen(message));return false;}size_t output_len=decode?payload_len:payload_len+8;uint8_t *output=(uint8_t*)disp_alloc(output_len,1);if(decode){memcpy(output,input+8,payload_len);}else{memcpy(output,"DISP",4);output[4]=1;output[5]=kind;output[6]=1;output[7]=(uint8_t)payload_len;memcpy(output+8,input,payload_len);}out->data=(char*)output;out->len=out->cap=output_len;return true;}
#ifdef DISP_CRYPTO_NATIVE
extern uint32_t disp_crypto_native_abi_version(void);
extern int32_t disp_crypto_native_aes256_gcm_siv_seal(const uint8_t*,size_t,const uint8_t*,size_t,const uint8_t*,size_t,uint8_t*,uint8_t*,size_t,size_t*);
extern int32_t disp_crypto_native_aes256_gcm_siv_open(const uint8_t*,size_t,const uint8_t*,size_t,const uint8_t*,size_t,const uint8_t*,size_t,uint8_t*,size_t,size_t*);
extern int32_t disp_crypto_native_ed25519_generate(uint8_t*,size_t);
extern int32_t disp_crypto_native_ed25519_public_key(const uint8_t*,size_t,uint8_t*,size_t);
extern int32_t disp_crypto_native_ed25519_sign(const uint8_t*,size_t,const uint8_t*,size_t,uint8_t*,size_t);
extern int32_t disp_crypto_native_ed25519_verify(const uint8_t*,size_t,const uint8_t*,size_t,const uint8_t*,size_t,uint8_t*);
extern int32_t disp_crypto_native_ed25519_key_id(const uint8_t*,size_t,uint8_t*,size_t);
extern int32_t disp_crypto_native_argon2id_hash(const uint8_t*,size_t,uint8_t*,size_t,size_t*);
extern int32_t disp_crypto_native_argon2id_verify(const uint8_t*,size_t,const uint8_t*,size_t,uint8_t*);
static disp_native_string disp_crypto_native_status(int32_t status){const char *message=status==2?"AES-256-GCM-SIV rejected the key":status==3?"secure operating-system entropy is unavailable":status==4?"AES-256-GCM-SIV authentication failed":status==6?"AES-256-GCM-SIV native panic was contained":status==1?"AES-256-GCM-SIV rejected invalid input":"AES-256-GCM-SIV provider operation failed";return disp_owned_bytes(message,strlen(message));}
static bool disp_crypto_aead_seal(const disp_native_secret *key,const disp_native_secret *plaintext,const uint8_t *aad,size_t aad_len,disp_native_string *out,disp_native_string *error){if(disp_crypto_native_abi_version()!=1){*error=disp_owned_bytes("native cryptography ABI version mismatch",strlen("native cryptography ABI version mismatch"));return false;}if(plaintext->len>1048576||aad_len>1048576||key->len!=32){const char *message=key->len!=32?"AES-256-GCM-SIV rejected the key":"AES-256-GCM-SIV input exceeds 1048576 bytes";*error=disp_owned_bytes(message,strlen(message));return false;}size_t cap=plaintext->len+28;uint8_t *bytes=(uint8_t*)disp_alloc(cap,1);size_t ciphertext_len=0;int32_t status=disp_crypto_native_aes256_gcm_siv_seal(key->data,key->len,plaintext->data,plaintext->len,aad,aad_len,bytes,bytes+12,cap-12,&ciphertext_len);if(status||ciphertext_len!=plaintext->len+16){disp_crypto_zero(bytes,cap);disp_dealloc(bytes);*error=disp_crypto_native_status(status?status:5);return false;}out->data=(char*)bytes;out->len=out->cap=ciphertext_len+12;return true;}
static bool disp_crypto_aead_open(const disp_native_secret *key,const disp_native_string *envelope,const uint8_t *aad,size_t aad_len,disp_native_secret *out,disp_native_string *error){if(disp_crypto_native_abi_version()!=1){*error=disp_owned_bytes("native cryptography ABI version mismatch",strlen("native cryptography ABI version mismatch"));return false;}if(key->len!=32||envelope->len<28||envelope->len>1048604||aad_len>1048576){const char *message=key->len!=32?"AES-256-GCM-SIV rejected the key":"AES-256-GCM-SIV rejected malformed input";*error=disp_owned_bytes(message,strlen(message));return false;}size_t ciphertext_len=envelope->len-12,cap=ciphertext_len-16,plaintext_len=0;uint8_t *bytes=(uint8_t*)disp_alloc(cap?cap:1,1);int32_t status=disp_crypto_native_aes256_gcm_siv_open(key->data,key->len,(const uint8_t*)envelope->data,12,(const uint8_t*)envelope->data+12,ciphertext_len,aad,aad_len,bytes,cap,&plaintext_len);if(status||plaintext_len!=cap){disp_crypto_zero(bytes,cap);disp_dealloc(bytes);*error=disp_crypto_native_status(status?status:5);return false;}out->data=bytes;out->len=out->cap=plaintext_len;return true;}
static disp_native_string disp_crypto_ed25519_status(int32_t status){const char *message=status==3?"secure operating-system entropy is unavailable":status==6?"Ed25519 native panic was contained":status==1||status==2?"Ed25519 rejected invalid input":"Ed25519 provider operation failed";return disp_owned_bytes(message,strlen(message));}
static bool disp_crypto_ed25519_generate(disp_native_secret *out,disp_native_string *error){if(disp_crypto_native_abi_version()!=1){*error=disp_owned_bytes("native cryptography ABI version mismatch",strlen("native cryptography ABI version mismatch"));return false;}uint8_t *key=(uint8_t*)disp_alloc(32,1);int32_t status=disp_crypto_native_ed25519_generate(key,32);if(status){disp_crypto_zero(key,32);disp_dealloc(key);*error=disp_crypto_ed25519_status(status);return false;}out->data=key;out->len=out->cap=32;return true;}
static bool disp_crypto_ed25519_public_key(const disp_native_secret *key,disp_native_string *out,disp_native_string *error){if(disp_crypto_native_abi_version()!=1){*error=disp_owned_bytes("native cryptography ABI version mismatch",strlen("native cryptography ABI version mismatch"));return false;}if(key->len!=32){*error=disp_owned_bytes("Ed25519 rejected invalid signing key",strlen("Ed25519 rejected invalid signing key"));return false;}uint8_t *public_key=(uint8_t*)disp_alloc(32,1);int32_t status=disp_crypto_native_ed25519_public_key(key->data,key->len,public_key,32);if(status){disp_dealloc(public_key);*error=disp_crypto_ed25519_status(status);return false;}out->data=(char*)public_key;out->len=out->cap=32;return true;}
static bool disp_crypto_ed25519_sign(const disp_native_secret *key,const uint8_t *message,size_t message_len,disp_native_string *out,disp_native_string *error){if(disp_crypto_native_abi_version()!=1){*error=disp_owned_bytes("native cryptography ABI version mismatch",strlen("native cryptography ABI version mismatch"));return false;}if(message_len>16777216){*error=disp_owned_bytes("Ed25519 message exceeds 16777216 bytes",strlen("Ed25519 message exceeds 16777216 bytes"));return false;}if(key->len!=32){*error=disp_owned_bytes("Ed25519 rejected invalid signing key",strlen("Ed25519 rejected invalid signing key"));return false;}uint8_t *signature=(uint8_t*)disp_alloc(64,1);int32_t status=disp_crypto_native_ed25519_sign(key->data,key->len,message,message_len,signature,64);if(status){disp_dealloc(signature);*error=disp_crypto_ed25519_status(status);return false;}out->data=(char*)signature;out->len=out->cap=64;return true;}
static bool disp_crypto_ed25519_verify(const uint8_t *public_key,size_t public_key_len,const uint8_t *message,size_t message_len,const uint8_t *signature,size_t signature_len,bool *valid,disp_native_string *error){if(disp_crypto_native_abi_version()!=1){*error=disp_owned_bytes("native cryptography ABI version mismatch",strlen("native cryptography ABI version mismatch"));return false;}if(message_len>16777216){*error=disp_owned_bytes("Ed25519 message exceeds 16777216 bytes",strlen("Ed25519 message exceeds 16777216 bytes"));return false;}uint8_t result=0;int32_t status=disp_crypto_native_ed25519_verify(public_key,public_key_len,message,message_len,signature,signature_len,&result);if(status){*error=disp_crypto_ed25519_status(status);return false;}*valid=result!=0;return true;}
static bool disp_crypto_ed25519_key_id(const uint8_t *public_key,size_t public_key_len,disp_native_string *out,disp_native_string *error){if(disp_crypto_native_abi_version()!=1){*error=disp_owned_bytes("native cryptography ABI version mismatch",strlen("native cryptography ABI version mismatch"));return false;}if(public_key_len!=32){*error=disp_owned_bytes("Ed25519 public key rejected malformed input",strlen("Ed25519 public key rejected malformed input"));return false;}uint8_t *key_id=(uint8_t*)disp_alloc(32,1);int32_t status=disp_crypto_native_ed25519_key_id(public_key,public_key_len,key_id,32);if(status){disp_dealloc(key_id);*error=disp_crypto_ed25519_status(status);return false;}out->data=(char*)key_id;out->len=out->cap=32;return true;}
static bool disp_crypto_ed25519_verify_keyed(const uint8_t *expected,size_t expected_len,const uint8_t *public_key,size_t public_key_len,const uint8_t *message,size_t message_len,const uint8_t *signature,size_t signature_len,bool *valid,disp_native_string *error){if(expected_len!=32){*error=disp_owned_bytes("Ed25519 key identifier rejected malformed input",strlen("Ed25519 key identifier rejected malformed input"));return false;}disp_native_string actual={0};if(!disp_crypto_ed25519_key_id(public_key,public_key_len,&actual,error))return false;uint8_t difference=0;for(size_t index=0;index<32;index++)difference|=(uint8_t)(((uint8_t*)actual.data)[index]^expected[index]);disp_dealloc(actual.data);if(difference){*valid=false;return true;}return disp_crypto_ed25519_verify(public_key,public_key_len,message,message_len,signature,signature_len,valid,error);}
static bool disp_crypto_ed25519_verify_lifecycle(const uint8_t *expected,size_t expected_len,const uint8_t *public_key,size_t public_key_len,const uint8_t *message,size_t message_len,const uint8_t *signature,size_t signature_len,uint64_t valid_from,uint64_t valid_until,bool revoked,uint64_t evaluation_time,bool *valid,disp_native_string *error){if(valid_from>valid_until){*error=disp_owned_bytes("Ed25519 key lifecycle window rejected malformed input",strlen("Ed25519 key lifecycle window rejected malformed input"));return false;}if(expected_len!=32){*error=disp_owned_bytes("Ed25519 key identifier rejected malformed input",strlen("Ed25519 key identifier rejected malformed input"));return false;}disp_native_string actual={0};if(!disp_crypto_ed25519_key_id(public_key,public_key_len,&actual,error))return false;uint8_t difference=0;for(size_t index=0;index<32;index++)difference|=(uint8_t)(((uint8_t*)actual.data)[index]^expected[index]);disp_dealloc(actual.data);if(difference||revoked||evaluation_time<valid_from||evaluation_time>valid_until){*valid=false;return true;}return disp_crypto_ed25519_verify(public_key,public_key_len,message,message_len,signature,signature_len,valid,error);}
static disp_native_string disp_crypto_argon2id_status(int32_t status){const char *message=status==3?"secure operating-system entropy is unavailable":status==6?"Argon2id native panic was contained":status==1?"Argon2id rejected invalid input or hash policy":"Argon2id provider operation failed";return disp_owned_bytes(message,strlen(message));}
static bool disp_crypto_argon2id_hash(const disp_native_secret *password,disp_native_string *out,disp_native_string *error){if(disp_crypto_native_abi_version()!=1){*error=disp_owned_bytes("native cryptography ABI version mismatch",strlen("native cryptography ABI version mismatch"));return false;}if(!password->len||password->len>1024){*error=disp_owned_bytes("Argon2id password length must be between 1 and 1024 bytes",strlen("Argon2id password length must be between 1 and 1024 bytes"));return false;}uint8_t *encoded=(uint8_t*)disp_alloc(1024,1);size_t encoded_len=0;int32_t status=disp_crypto_native_argon2id_hash(password->data,password->len,encoded,1024,&encoded_len);if(status||!encoded_len||encoded_len>1024){disp_crypto_zero(encoded,1024);disp_dealloc(encoded);*error=disp_crypto_argon2id_status(status?status:5);return false;}out->data=(char*)encoded;out->len=encoded_len;out->cap=1024;return true;}
static bool disp_crypto_argon2id_verify(const disp_native_secret *password,const char *encoded,size_t encoded_len,bool *valid,disp_native_string *error){if(disp_crypto_native_abi_version()!=1){*error=disp_owned_bytes("native cryptography ABI version mismatch",strlen("native cryptography ABI version mismatch"));return false;}if(!password->len||password->len>1024||!encoded_len||encoded_len>1024){*error=disp_owned_bytes("Argon2id rejected invalid input or hash policy",strlen("Argon2id rejected invalid input or hash policy"));return false;}uint8_t result=0;int32_t status=disp_crypto_native_argon2id_verify(password->data,password->len,(const uint8_t*)encoded,encoded_len,&result);if(status){*error=disp_crypto_argon2id_status(status);return false;}*valid=result!=0;return true;}
#endif
static void disp_time_sleep(uint64_t nanos);
static uint64_t disp_time_now_nanos(void);
static void disp_async_file_drain(void);
static _Thread_local uint64_t disp_reactor_wait_nanos=UINT64_MAX;
static void disp_reactor_begin(void){disp_reactor_wait_nanos=UINT64_MAX;}
static void disp_reactor_offer(uint64_t nanos){if(nanos<disp_reactor_wait_nanos)disp_reactor_wait_nanos=nanos;}
static void disp_reactor_wait(void){if(disp_reactor_wait_nanos!=UINT64_MAX){disp_time_sleep(disp_reactor_wait_nanos);return;}
#ifdef _WIN32
SwitchToThread();
#else
sched_yield();
#endif
}
struct disp_task_state {
    struct disp_task_state *next;
    disp_native_future future;
    void *result;
    size_t result_size;
    size_t refs;
    bool complete;
    bool taken;
    bool cancelled;
    void (*result_drop)(void *);
};
static _Thread_local disp_task_state *disp_task_head;
static _Thread_local disp_task_state **disp_task_tail;
static void disp_task_release(disp_task_state *state){if(!state)return;if(--state->refs)return;if(state->future.context&&state->future.drop)state->future.drop(state->future.context);if(state->result){if(state->complete&&!state->taken&&state->result_drop)state->result_drop(state->result);disp_dealloc(state->result);}disp_runtime_release_task();disp_dealloc(state);}
static void disp_task_unlink(disp_task_state **link,disp_task_state *state){*link=state->next;if(!state->next)disp_task_tail=link;state->next=NULL;disp_task_release(state);}
static void disp_executor_tick(void){disp_task_state **link=&disp_task_head;while(*link){disp_task_state *state=*link;if(state->cancelled){if(state->future.context&&state->future.drop)state->future.drop(state->future.context);state->future=(disp_native_future){0};disp_task_unlink(link,state);disp_reactor_offer(0);continue;}if(!state->complete&&state->future.poll(state->future.context,state->result)){if(state->future.drop)state->future.drop(state->future.context);state->future=(disp_native_future){0};state->complete=true;disp_task_unlink(link,state);disp_reactor_offer(0);continue;}link=&state->next;}}
static disp_native_task disp_task_spawn(disp_native_future future,size_t result_size,size_t result_align,void (*result_drop)(void *)){if(!future.context||!future.poll)dv_panic("cannot spawn an empty Future",0,0);disp_runtime_acquire_task();disp_task_state *state=(disp_task_state*)disp_alloc_zeroed(1,sizeof(disp_task_state),_Alignof(disp_task_state));state->future=future;state->refs=2;state->result_size=result_size;state->result_drop=result_drop;state->result=disp_alloc_zeroed(1,result_size?result_size:1,result_align);if(!disp_task_tail)disp_task_tail=&disp_task_head;*disp_task_tail=state;disp_task_tail=&state->next;return (disp_native_task){.state=state};}
static bool disp_task_poll(disp_native_task *task,void *output,int line,int column){disp_task_state *state=task->state;if(!state||state->taken)dv_panic("task has already been awaited",line,column);if(state->cancelled)dv_panic("cancelled task cannot be awaited",line,column);if(!state->complete)return false;if(state->result_size)memcpy(output,state->result,state->result_size);state->taken=true;disp_dealloc(state->result);state->result=NULL;task->state=NULL;disp_task_release(state);return true;}
static void disp_task_drop(disp_native_task *task){disp_task_state *state=task->state;if(!state)return;if(!state->complete)state->cancelled=true;task->state=NULL;disp_task_release(state);}
static bool disp_task_is_finished(const disp_native_task *task,int line,int column){if(!task->state)dv_panic("invalid Task handle",line,column);return task->state->complete;}
static void disp_task_cancel(disp_native_task *task){disp_task_drop(task);disp_executor_tick();}
static void disp_task_wait(disp_native_task *task,void *output,int line,int column){for(;;){disp_reactor_begin();if(disp_task_poll(task,output,line,column))break;disp_executor_tick();disp_reactor_wait();}}
static void disp_future_wait(disp_native_future *future,void *output,int line,int column){if(!future->context||!future->poll)dv_panic("future has already been awaited",line,column);for(;;){disp_reactor_begin();if(future->poll(future->context,output))break;disp_executor_tick();disp_reactor_wait();}if(future->drop)future->drop(future->context);*future=(disp_native_future){0};disp_executor_tick();disp_async_file_drain();}
typedef struct { bool yielded; } disp_yield_future;
static bool disp_yield_poll(void *raw,void *output){disp_yield_future *state=(disp_yield_future*)raw;if(!state->yielded){state->yielded=true;return false;}*(disp_native_unit*)output=(disp_native_unit){0};return true;}
static void disp_yield_drop(void *raw){disp_dealloc(raw);}
static disp_native_future disp_future_yield(void){disp_yield_future *state=(disp_yield_future*)disp_alloc_zeroed(1,sizeof(disp_yield_future),_Alignof(disp_yield_future));return (disp_native_future){.context=state,.poll=disp_yield_poll,.drop=disp_yield_drop};}
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
pthread_mutexattr_t attributes;if(pthread_mutexattr_init(&attributes)!=0)disp_allocation_failure("could not initialize recursive Mutex attributes");if(pthread_mutexattr_settype(&attributes,PTHREAD_MUTEX_RECURSIVE)!=0){pthread_mutexattr_destroy(&attributes);disp_allocation_failure("could not select recursive Mutex behavior");}if(pthread_mutex_init(&state->native,&attributes)!=0){pthread_mutexattr_destroy(&attributes);disp_allocation_failure("could not initialize recursive Mutex");}pthread_mutexattr_destroy(&attributes);
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
static memory_order disp_atomic_failure_order(memory_order order){if(order==memory_order_release)return memory_order_relaxed;if(order==memory_order_acq_rel)return memory_order_acquire;return order;}
static int64_t disp_atomic_int_load(disp_atomic_int_state *state,memory_order order){return (int64_t)atomic_load_explicit(&state->value,order);}
static void disp_atomic_int_store(disp_atomic_int_state *state,int64_t value,memory_order order){atomic_store_explicit(&state->value,value,order);}
static int64_t disp_atomic_int_fetch_add(disp_atomic_int_state *state,int64_t delta,memory_order order,int line,int column){int_fast64_t expected=atomic_load_explicit(&state->value,memory_order_relaxed);for(;;){int64_t desired;if(__builtin_add_overflow((int64_t)expected,delta,&desired))dv_panic("AtomicInt overflow",line,column);if(atomic_compare_exchange_weak_explicit(&state->value,&expected,(int_fast64_t)desired,order,disp_atomic_failure_order(order)))return (int64_t)expected;}}

struct disp_channel_state {
    atomic_size_t refs;
    void *data;
    size_t element_size;
    size_t capacity;
    size_t len;
    size_t head;
    bool closed;
#ifdef _WIN32
    CRITICAL_SECTION native;
    CONDITION_VARIABLE not_empty;
    CONDITION_VARIABLE not_full;
#else
    pthread_mutex_t native;
    pthread_cond_t not_empty;
    pthread_cond_t not_full;
#endif
};
static void disp_channel_lock(disp_channel_state *state,int line,int column){
#ifdef _WIN32
EnterCriticalSection(&state->native);
#else
if(pthread_mutex_lock(&state->native)!=0)dv_panic("could not lock Channel",line,column);
#endif
}
static void disp_channel_unlock(disp_channel_state *state){
#ifdef _WIN32
LeaveCriticalSection(&state->native);
#else
if(pthread_mutex_unlock(&state->native)!=0)dv_panic("could not unlock Channel",0,0);
#endif
}
static void disp_channel_wait_not_empty(disp_channel_state *state,int line,int column){
#ifdef _WIN32
if(!SleepConditionVariableCS(&state->not_empty,&state->native,INFINITE))dv_panic("could not wait for Channel data",line,column);
#else
if(pthread_cond_wait(&state->not_empty,&state->native)!=0)dv_panic("could not wait for Channel data",line,column);
#endif
}
static void disp_channel_wait_not_full(disp_channel_state *state,int line,int column){
#ifdef _WIN32
if(!SleepConditionVariableCS(&state->not_full,&state->native,INFINITE))dv_panic("could not wait for Channel capacity",line,column);
#else
if(pthread_cond_wait(&state->not_full,&state->native)!=0)dv_panic("could not wait for Channel capacity",line,column);
#endif
}
static void disp_channel_signal_not_empty(disp_channel_state *state){
#ifdef _WIN32
WakeConditionVariable(&state->not_empty);
#else
if(pthread_cond_signal(&state->not_empty)!=0)dv_panic("could not signal Channel data",0,0);
#endif
}
static void disp_channel_signal_not_full(disp_channel_state *state){
#ifdef _WIN32
WakeConditionVariable(&state->not_full);
#else
if(pthread_cond_signal(&state->not_full)!=0)dv_panic("could not signal Channel capacity",0,0);
#endif
}
static void disp_channel_broadcast(disp_channel_state *state){
#ifdef _WIN32
WakeAllConditionVariable(&state->not_empty);WakeAllConditionVariable(&state->not_full);
#else
if(pthread_cond_broadcast(&state->not_empty)!=0||pthread_cond_broadcast(&state->not_full)!=0)dv_panic("could not close Channel waiters",0,0);
#endif
}
static disp_channel_state *disp_channel_create(size_t capacity,size_t element_size,size_t element_align){disp_channel_state *state=(disp_channel_state*)disp_alloc_zeroed(1,sizeof(disp_channel_state),_Alignof(disp_channel_state));atomic_init(&state->refs,1);state->element_size=element_size;state->capacity=capacity;state->data=disp_alloc_zeroed(capacity,element_size,element_align);disp_runtime_acquire_handle();
#ifdef _WIN32
InitializeCriticalSection(&state->native);InitializeConditionVariable(&state->not_empty);InitializeConditionVariable(&state->not_full);
#else
if(pthread_mutex_init(&state->native,NULL)!=0||pthread_cond_init(&state->not_empty,NULL)!=0||pthread_cond_init(&state->not_full,NULL)!=0)disp_allocation_failure("could not initialize Channel");
#endif
return state;}
static void disp_channel_retain(disp_channel_state *state){if(!state)dv_panic("invalid Channel handle",0,0);size_t previous=atomic_fetch_add_explicit(&state->refs,1,memory_order_relaxed);if(previous==SIZE_MAX)dv_panic("Channel reference count overflow",0,0);}
static bool disp_channel_send(disp_channel_state *state,const void *value,int line,int column){disp_channel_lock(state,line,column);while(state->len==state->capacity&&!state->closed)disp_channel_wait_not_full(state,line,column);if(state->closed){disp_channel_unlock(state);return false;}size_t tail=(state->head+state->len)%state->capacity;memcpy((unsigned char*)state->data+tail*state->element_size,value,state->element_size);state->len++;disp_channel_signal_not_empty(state);disp_channel_unlock(state);return true;}
static bool disp_channel_receive(disp_channel_state *state,void *value,int line,int column){disp_channel_lock(state,line,column);while(!state->len&&!state->closed)disp_channel_wait_not_empty(state,line,column);if(!state->len){disp_channel_unlock(state);return false;}memcpy(value,(unsigned char*)state->data+state->head*state->element_size,state->element_size);state->head=(state->head+1)%state->capacity;state->len--;disp_channel_signal_not_full(state);disp_channel_unlock(state);return true;}
static void disp_channel_close(disp_channel_state *state){disp_channel_lock(state,0,0);if(!state->closed){state->closed=true;disp_runtime_release_handle();}disp_channel_broadcast(state);disp_channel_unlock(state);}
static size_t disp_channel_len(disp_channel_state *state){disp_channel_lock(state,0,0);size_t value=state->len;disp_channel_unlock(state);return value;}
static size_t disp_channel_capacity(disp_channel_state *state){return state->capacity;}
static bool disp_channel_is_closed(disp_channel_state *state){disp_channel_lock(state,0,0);bool value=state->closed;disp_channel_unlock(state);return value;}
static bool disp_channel_release(disp_channel_state *state){if(!state)return false;if(atomic_fetch_sub_explicit(&state->refs,1,memory_order_acq_rel)!=1)return false;atomic_thread_fence(memory_order_acquire);
if(!state->closed)disp_runtime_release_handle();
#ifdef _WIN32
DeleteCriticalSection(&state->native);
#else
if(pthread_cond_destroy(&state->not_empty)!=0||pthread_cond_destroy(&state->not_full)!=0||pthread_mutex_destroy(&state->native)!=0)dv_panic("could not destroy Channel",0,0);
#endif
return true;}

typedef void (*disp_thread_entry)(void *);
typedef struct { disp_thread_entry entry; void *context; } disp_thread_boot;
#ifdef _WIN32
static DWORD WINAPI disp_thread_bootstrap(LPVOID raw){disp_thread_boot boot=*(disp_thread_boot*)raw;disp_dealloc(raw);boot.entry(boot.context);disp_runtime_release_thread();return 0;}
static uintptr_t disp_thread_start(disp_thread_entry entry,void *context,int line,int column){disp_runtime_acquire_thread();disp_thread_boot *boot=(disp_thread_boot*)disp_alloc(sizeof(disp_thread_boot),_Alignof(disp_thread_boot));boot->entry=entry;boot->context=context;HANDLE handle=CreateThread(NULL,0,disp_thread_bootstrap,boot,0,NULL);if(!handle){disp_dealloc(boot);disp_runtime_release_thread();dv_panic("could not create native thread",line,column);}return (uintptr_t)handle;}
static void disp_thread_wait(disp_native_thread *thread){if(!thread->handle)return;HANDLE handle=(HANDLE)thread->handle;DWORD status=WaitForSingleObject(handle,INFINITE);CloseHandle(handle);thread->handle=0;if(status!=WAIT_OBJECT_0)dv_panic("could not join native thread",0,0);}
static void disp_thread_detach(uintptr_t handle){if(handle)CloseHandle((HANDLE)handle);}
#else
static void *disp_thread_bootstrap(void *raw){disp_thread_boot boot=*(disp_thread_boot*)raw;disp_dealloc(raw);boot.entry(boot.context);disp_runtime_release_thread();return NULL;}
static uintptr_t disp_thread_start(disp_thread_entry entry,void *context,int line,int column){_Static_assert(sizeof(pthread_t)<=sizeof(uintptr_t),"pthread_t cannot fit DISP thread handle");disp_runtime_acquire_thread();disp_thread_boot *boot=(disp_thread_boot*)disp_alloc(sizeof(disp_thread_boot),_Alignof(disp_thread_boot));boot->entry=entry;boot->context=context;pthread_t native;if(pthread_create(&native,NULL,disp_thread_bootstrap,boot)!=0){disp_dealloc(boot);disp_runtime_release_thread();dv_panic("could not create native thread",line,column);}uintptr_t handle=0;memcpy(&handle,&native,sizeof(native));return handle;}
static void disp_thread_wait(disp_native_thread *thread){if(!thread->handle)return;pthread_t native;memcpy(&native,&thread->handle,sizeof(native));thread->handle=0;if(pthread_join(native,NULL)!=0)dv_panic("could not join native thread",0,0);}
static void disp_thread_detach(uintptr_t handle){pthread_t native;memcpy(&native,&handle,sizeof(native));if(pthread_detach(native)!=0)dv_panic("could not detach native thread",0,0);}
#endif

static int disp_program_argc;
static char **disp_program_argv;
typedef struct {
#ifdef _WIN32
    HANDLE source;
#else
    int source;
#endif
    uint8_t *data;
    size_t len;
    size_t cap;
    bool failed;
    bool overflow;
} disp_process_capture;
static void disp_process_capture_entry(void *raw){disp_process_capture *capture=(disp_process_capture*)raw;uint8_t chunk[8192];for(;;){size_t count=0;
#ifdef _WIN32
DWORD read=0;if(!ReadFile(capture->source,chunk,sizeof(chunk),&read,NULL)){if(GetLastError()!=ERROR_BROKEN_PIPE)capture->failed=true;break;}count=(size_t)read;
#else
ssize_t read_count=read(capture->source,chunk,sizeof(chunk));if(read_count<0){if(errno==EINTR)continue;capture->failed=true;break;}count=(size_t)read_count;
#endif
if(!count)break;if(capture->len>DISP_PROCESS_MAX_CAPTURE-count){capture->overflow=true;continue;}size_t needed=capture->len+count;if(needed>capture->cap){size_t cap=capture->cap?capture->cap:8192;while(cap<needed)cap*=2;if(cap>DISP_PROCESS_MAX_CAPTURE)cap=DISP_PROCESS_MAX_CAPTURE;capture->data=(uint8_t*)disp_realloc(capture->data,cap,1);capture->cap=cap;}memcpy(capture->data+capture->len,chunk,count);capture->len=needed;}}
static disp_native_string disp_process_error_text(const char *message){return disp_owned_bytes(message,strlen(message));}
#ifdef _WIN32
static bool disp_process_append(char **buffer,size_t *len,size_t *cap,const char *bytes,size_t count){if(*len>SIZE_MAX-count-1)return false;size_t needed=*len+count+1;if(needed>*cap){size_t next=*cap?*cap:64;while(next<needed){if(next>SIZE_MAX/2)return false;next*=2;}*buffer=(char*)disp_realloc(*buffer,next,1);*cap=next;}memcpy(*buffer+*len,bytes,count);*len+=count;(*buffer)[*len]=0;return true;}
static bool disp_process_quote(char **buffer,size_t *len,size_t *cap,const char *text){if(!disp_process_append(buffer,len,cap,"\"",1))return false;size_t slashes=0;for(const char *p=text;;p++){char ch=*p;if(ch=='\\'){slashes++;continue;}if(ch=='\"'||ch==0){for(size_t i=0;i<slashes*(ch?2:2);i++)if(!disp_process_append(buffer,len,cap,"\\",1))return false;if(ch&& !disp_process_append(buffer,len,cap,"\\\"",2))return false;slashes=0;if(!ch)break;}else{for(size_t i=0;i<slashes;i++)if(!disp_process_append(buffer,len,cap,"\\",1))return false;slashes=0;if(!disp_process_append(buffer,len,cap,&ch,1))return false;}}return disp_process_append(buffer,len,cap,"\"",1);}
static wchar_t *disp_process_wide(const char *text,size_t length){if(length>INT_MAX)return NULL;if(!length){wchar_t *wide=(wchar_t*)disp_alloc(sizeof(wchar_t),_Alignof(wchar_t));wide[0]=0;return wide;}int count=MultiByteToWideChar(CP_UTF8,MB_ERR_INVALID_CHARS,text,(int)length,NULL,0);if(count<=0)return NULL;wchar_t *wide=(wchar_t*)disp_alloc(((size_t)count+1)*sizeof(wchar_t),_Alignof(wchar_t));if(MultiByteToWideChar(CP_UTF8,MB_ERR_INVALID_CHARS,text,(int)length,wide,count)!=count){disp_dealloc(wide);return NULL;}wide[count]=0;return wide;}
static disp_native_string disp_process_windows_error(const char *operation,DWORD code){char message[160];snprintf(message,sizeof(message),"%s failed (Windows error %lu)",operation,(unsigned long)code);return disp_process_error_text(message);}
static HANDLE disp_process_sandbox_create(disp_native_string *error){size_t memory=disp_runtime_limit("DISP_CHILD_MAX_MEMORY_BYTES",(size_t)DISP_DEFAULT_CHILD_MEMORY_BYTES),cpu_millis=disp_runtime_limit("DISP_CHILD_MAX_CPU_MILLIS",(size_t)DISP_DEFAULT_CHILD_CPU_MILLIS),processes=disp_runtime_limit("DISP_CHILD_MAX_PROCESSES",(size_t)DISP_DEFAULT_CHILD_PROCESSES);if(processes>UINT32_MAX||cpu_millis>(size_t)(INT64_MAX/10000)){*error=disp_process_error_text("child sandbox configuration exceeds the Windows platform range");return NULL;}HANDLE job=CreateJobObjectW(NULL,NULL);if(!job){*error=disp_process_windows_error("CreateJobObject",GetLastError());return NULL;}JOBOBJECT_EXTENDED_LIMIT_INFORMATION limits={0};limits.BasicLimitInformation.LimitFlags=JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE|JOB_OBJECT_LIMIT_ACTIVE_PROCESS|JOB_OBJECT_LIMIT_JOB_TIME|JOB_OBJECT_LIMIT_JOB_MEMORY;limits.BasicLimitInformation.ActiveProcessLimit=(DWORD)processes;limits.BasicLimitInformation.PerJobUserTimeLimit.QuadPart=(LONGLONG)cpu_millis*10000LL;limits.JobMemoryLimit=(SIZE_T)memory;if(!SetInformationJobObject(job,JobObjectExtendedLimitInformation,&limits,sizeof(limits))){DWORD code=GetLastError();CloseHandle(job);*error=disp_process_windows_error("SetInformationJobObject",code);return NULL;}return job;}
static bool disp_process_sandbox_start(HANDLE job,PROCESS_INFORMATION *child,disp_native_string *error){if(!AssignProcessToJobObject(job,child->hProcess)){DWORD code=GetLastError();TerminateProcess(child->hProcess,125);WaitForSingleObject(child->hProcess,INFINITE);*error=disp_process_windows_error("AssignProcessToJobObject",code);return false;}if(ResumeThread(child->hThread)==(DWORD)-1){DWORD code=GetLastError();TerminateJobObject(job,125);WaitForSingleObject(child->hProcess,INFINITE);*error=disp_process_windows_error("ResumeThread",code);return false;}return true;}
typedef struct disp_process_job_entry {HANDLE process;HANDLE job;struct disp_process_job_entry *next;} disp_process_job_entry;
static SRWLOCK disp_process_job_lock=SRWLOCK_INIT;
static disp_process_job_entry *disp_process_jobs;
static BOOL disp_process_sandbox_create_process(LPCWSTR application,LPWSTR command,LPSECURITY_ATTRIBUTES process_attributes,LPSECURITY_ATTRIBUTES thread_attributes,BOOL inherit_handles,DWORD creation_flags,LPVOID environment,LPCWSTR directory,LPSTARTUPINFOW startup,LPPROCESS_INFORMATION child){disp_native_string error={0};HANDLE job=disp_process_sandbox_create(&error);if(!job){disp_string_drop(&error);SetLastError(ERROR_ACCESS_DENIED);return FALSE;}disp_process_job_entry *entry=(disp_process_job_entry*)HeapAlloc(GetProcessHeap(),HEAP_ZERO_MEMORY,sizeof(disp_process_job_entry));if(!entry){CloseHandle(job);SetLastError(ERROR_NOT_ENOUGH_MEMORY);return FALSE;}if(!CreateProcessW(application,command,process_attributes,thread_attributes,inherit_handles,creation_flags|CREATE_SUSPENDED,environment,directory,startup,child)){DWORD code=GetLastError();HeapFree(GetProcessHeap(),0,entry);CloseHandle(job);SetLastError(code);return FALSE;}if(!disp_process_sandbox_start(job,child,&error)){HeapFree(GetProcessHeap(),0,entry);CloseHandle(child->hThread);CloseHandle(child->hProcess);CloseHandle(job);disp_string_drop(&error);SetLastError(ERROR_ACCESS_DENIED);*child=(PROCESS_INFORMATION){0};return FALSE;}entry->process=child->hProcess;entry->job=job;AcquireSRWLockExclusive(&disp_process_job_lock);entry->next=disp_process_jobs;disp_process_jobs=entry;ReleaseSRWLockExclusive(&disp_process_job_lock);return TRUE;}
static BOOL disp_process_sandbox_terminate(HANDLE process,UINT exit_code){BOOL result=FALSE;AcquireSRWLockShared(&disp_process_job_lock);for(disp_process_job_entry *entry=disp_process_jobs;entry;entry=entry->next){if(entry->process==process){result=TerminateJobObject(entry->job,exit_code);break;}}ReleaseSRWLockShared(&disp_process_job_lock);return result?TRUE:TerminateProcess(process,exit_code);}
static BOOL disp_process_sandbox_close(HANDLE handle){disp_process_job_entry *removed=NULL;AcquireSRWLockExclusive(&disp_process_job_lock);disp_process_job_entry **link=&disp_process_jobs;while(*link){if((*link)->process==handle){removed=*link;*link=removed->next;break;}link=&(*link)->next;}ReleaseSRWLockExclusive(&disp_process_job_lock);if(removed){TerminateJobObject(removed->job,125);CloseHandle(removed->job);}BOOL result=CloseHandle(handle);if(removed)HeapFree(GetProcessHeap(),0,removed);return result;}
#define CreateProcessW(application,command,process_attributes,thread_attributes,inherit_handles,creation_flags,environment,directory,startup,child) disp_process_sandbox_create_process((application),(command),(process_attributes),(thread_attributes),(inherit_handles),(creation_flags),(environment),(directory),(startup),(child))
#define TerminateProcess(process,exit_code) disp_process_sandbox_terminate((process),(exit_code))
#define CloseHandle(handle) disp_process_sandbox_close((handle))
static disp_native_string disp_process_utf8(const wchar_t *text,size_t length){if(length>INT_MAX)return (disp_native_string){0};if(!length){disp_native_string empty={.data=(char*)disp_alloc(1,1),.cap=1};empty.data[0]=0;return empty;}int count=WideCharToMultiByte(CP_UTF8,WC_ERR_INVALID_CHARS,text,(int)length,NULL,0,NULL,NULL);if(count<=0)return (disp_native_string){0};disp_native_string result={0};result.data=(char*)disp_alloc((size_t)count+1,1);if(WideCharToMultiByte(CP_UTF8,WC_ERR_INVALID_CHARS,text,(int)length,result.data,count,NULL,NULL)!=count){disp_dealloc(result.data);return (disp_native_string){0};}result.data[count]=0;result.len=result.cap=(size_t)count;return result;}
static void disp_program_arguments_init(int argc,char **argv){(void)argc;(void)argv;int count=0;LPWSTR *wide=CommandLineToArgvW(GetCommandLineW(),&count);if(!wide){disp_program_argc=0;disp_program_argv=NULL;return;}disp_program_argc=count>0?count-1:0;disp_program_argv=(char**)disp_alloc_zeroed((size_t)disp_program_argc,sizeof(char*),_Alignof(char*));for(int i=0;i<disp_program_argc;i++){size_t length=wcslen(wide[i+1]);disp_native_string value=disp_process_utf8(wide[i+1],length);if(!value.data)dv_panic("program argument is not valid Unicode",0,0);disp_program_argv[i]=value.data;value.data[value.len]=0;}LocalFree(wide);}
static void disp_program_arguments_drop(void){for(int i=0;i<disp_program_argc;i++)disp_dealloc(disp_program_argv[i]);disp_dealloc(disp_program_argv);disp_program_argc=0;disp_program_argv=NULL;}
static bool disp_environment_get(const disp_native_string *name,disp_native_string *value,bool *found){wchar_t *wide=disp_process_wide(name->data,name->len);if(!wide)return false;SetLastError(ERROR_SUCCESS);DWORD needed=GetEnvironmentVariableW(wide,NULL,0);if(!needed){DWORD code=GetLastError();disp_dealloc(wide);*found=code!=ERROR_ENVVAR_NOT_FOUND;*value=(disp_native_string){0};return true;}wchar_t *buffer=(wchar_t*)disp_alloc((size_t)needed*sizeof(wchar_t),_Alignof(wchar_t));DWORD length=GetEnvironmentVariableW(wide,buffer,needed);disp_dealloc(wide);if(!length||length>=needed){disp_dealloc(buffer);return false;}*value=disp_process_utf8(buffer,length);disp_dealloc(buffer);*found=true;return value->data!=NULL||length==0;}
#endif
#ifndef _WIN32
static void disp_program_arguments_init(int argc,char **argv){disp_program_argc=argc>0?argc-1:0;disp_program_argv=argc>0?argv+1:argv;}
static void disp_program_arguments_drop(void){disp_program_argc=0;disp_program_argv=NULL;}
static bool disp_environment_get(const disp_native_string *name,disp_native_string *value,bool *found){char *key=(char*)disp_alloc(name->len+1,1);memcpy(key,name->data,name->len);key[name->len]=0;const char *raw=getenv(key);disp_dealloc(key);*found=raw!=NULL;if(raw&&!disp_utf8_valid(raw,strlen(raw)))return false;*value=raw?disp_owned_bytes(raw,strlen(raw)):(disp_native_string){0};return true;}
#ifdef __linux__
#ifndef SECCOMP_RET_KILL_PROCESS
#define SECCOMP_RET_KILL_PROCESS 0x80000000U
#endif
#ifndef CLONE_NEWTIME
#define CLONE_NEWTIME 0x00000080
#endif
#if defined(__x86_64__)
#define DISP_AUDIT_ARCH AUDIT_ARCH_X86_64
#elif defined(__i386__)
#define DISP_AUDIT_ARCH AUDIT_ARCH_I386
#elif defined(__aarch64__)
#define DISP_AUDIT_ARCH AUDIT_ARCH_AARCH64
#elif defined(__arm__)
#define DISP_AUDIT_ARCH AUDIT_ARCH_ARM
#elif defined(__riscv) && __riscv_xlen == 64
#define DISP_AUDIT_ARCH AUDIT_ARCH_RISCV64
#elif defined(__s390x__)
#define DISP_AUDIT_ARCH AUDIT_ARCH_S390X
#define DISP_CLONE_FLAGS_OFFSET offsetof(struct seccomp_data,args[1])
#elif defined(__powerpc64__) && __BYTE_ORDER__ == __ORDER_LITTLE_ENDIAN__
#define DISP_AUDIT_ARCH AUDIT_ARCH_PPC64LE
#elif defined(__powerpc64__)
#define DISP_AUDIT_ARCH AUDIT_ARCH_PPC64
#else
#error "DISP has no verified seccomp audit architecture for this Linux target"
#endif
#ifndef DISP_CLONE_FLAGS_OFFSET
#define DISP_CLONE_FLAGS_OFFSET offsetof(struct seccomp_data,args[0])
#endif
static int disp_process_linux_escape_filter(void){
const uint32_t denied=SECCOMP_RET_ERRNO|(uint32_t)EPERM,unsupported=SECCOMP_RET_ERRNO|(uint32_t)ENOSYS;
const uint32_t namespace_flags=(uint32_t)(CLONE_NEWCGROUP|CLONE_NEWIPC|CLONE_NEWNET|CLONE_NEWNS|CLONE_NEWPID|CLONE_NEWTIME|CLONE_NEWUSER|CLONE_NEWUTS);
struct sock_filter filter[]={
BPF_STMT(BPF_LD|BPF_W|BPF_ABS,(uint32_t)offsetof(struct seccomp_data,arch)),
BPF_JUMP(BPF_JMP|BPF_JEQ|BPF_K,DISP_AUDIT_ARCH,1,0),
BPF_STMT(BPF_RET|BPF_K,SECCOMP_RET_KILL_PROCESS),
BPF_STMT(BPF_LD|BPF_W|BPF_ABS,(uint32_t)offsetof(struct seccomp_data,nr)),
BPF_JUMP(BPF_JMP|BPF_JEQ|BPF_K,__NR_setpgid,0,1),BPF_STMT(BPF_RET|BPF_K,denied),
BPF_JUMP(BPF_JMP|BPF_JEQ|BPF_K,__NR_setsid,0,1),BPF_STMT(BPF_RET|BPF_K,denied),
BPF_JUMP(BPF_JMP|BPF_JEQ|BPF_K,__NR_unshare,0,1),BPF_STMT(BPF_RET|BPF_K,denied),
BPF_JUMP(BPF_JMP|BPF_JEQ|BPF_K,__NR_setns,0,1),BPF_STMT(BPF_RET|BPF_K,denied),
BPF_JUMP(BPF_JMP|BPF_JEQ|BPF_K,__NR_ptrace,0,1),BPF_STMT(BPF_RET|BPF_K,denied),
BPF_JUMP(BPF_JMP|BPF_JEQ|BPF_K,__NR_process_vm_readv,0,1),BPF_STMT(BPF_RET|BPF_K,denied),
BPF_JUMP(BPF_JMP|BPF_JEQ|BPF_K,__NR_process_vm_writev,0,1),BPF_STMT(BPF_RET|BPF_K,denied),
#ifdef __NR_pidfd_getfd
BPF_JUMP(BPF_JMP|BPF_JEQ|BPF_K,__NR_pidfd_getfd,0,1),BPF_STMT(BPF_RET|BPF_K,denied),
#endif
#ifdef __NR_clone3
BPF_JUMP(BPF_JMP|BPF_JEQ|BPF_K,__NR_clone3,0,1),BPF_STMT(BPF_RET|BPF_K,unsupported),
#endif
BPF_JUMP(BPF_JMP|BPF_JEQ|BPF_K,__NR_clone,0,4),
BPF_STMT(BPF_LD|BPF_W|BPF_ABS,(uint32_t)DISP_CLONE_FLAGS_OFFSET),
BPF_STMT(BPF_ALU|BPF_AND|BPF_K,namespace_flags),
BPF_JUMP(BPF_JMP|BPF_JEQ|BPF_K,0,1,0),
BPF_STMT(BPF_RET|BPF_K,denied),
BPF_STMT(BPF_RET|BPF_K,SECCOMP_RET_ALLOW)};
struct sock_fprog program={.len=(unsigned short)(sizeof(filter)/sizeof(filter[0])),.filter=filter};
if(prctl(PR_SET_NO_NEW_PRIVS,1,0,0,0)!=0)return -1;
return prctl(PR_SET_SECCOMP,SECCOMP_MODE_FILTER,&program);
}
#endif
static int disp_process_exec_error_fd=-1;
static int disp_process_close_on_exec(void){
#ifdef __linux__
#ifndef CLOSE_RANGE_CLOEXEC
#define CLOSE_RANGE_CLOEXEC (1U<<2)
#endif
#ifdef __NR_close_range
if(syscall(__NR_close_range,3U,~0U,CLOSE_RANGE_CLOEXEC)==0)return 0;
if(errno!=ENOSYS&&errno!=EINVAL)return -1;
#endif
#endif
long maximum=sysconf(_SC_OPEN_MAX);if(maximum<0)maximum=65536;
for(int fd=3;fd<maximum&&fd<INT_MAX;fd++){if(fcntl(fd,F_SETFD,FD_CLOEXEC)!=0&&errno!=EBADF)return -1;}
return 0;
}
static const char *disp_process_cgroup_helper="/usr/libexec/disp-cgroup-launch";
static int disp_process_hard_mode(void){const char *mode=getenv("DISP_LINUX_HARD_SANDBOX");if(!mode||!strcmp(mode,"auto"))return 1;if(!strcmp(mode,"required"))return 2;if(!strcmp(mode,"off"))return 0;errno=EINVAL;return -1;}
static bool disp_process_helper_trusted(void){struct stat info;if(stat(disp_process_cgroup_helper,&info)!=0)return false;return S_ISREG(info.st_mode)&&info.st_uid==0&&info.st_gid==0&&(info.st_mode&(S_ISUID|S_ISGID))==(S_ISUID|S_ISGID)&&(info.st_mode&(S_IWGRP|S_IWOTH))==0&&access(disp_process_cgroup_helper,X_OK)==0;}
static int disp_process_sandbox_child(void){size_t memory=disp_runtime_limit("DISP_CHILD_MAX_MEMORY_BYTES",(size_t)DISP_DEFAULT_CHILD_MEMORY_BYTES),cpu_millis=disp_runtime_limit("DISP_CHILD_MAX_CPU_MILLIS",(size_t)DISP_DEFAULT_CHILD_CPU_MILLIS),processes=disp_runtime_limit("DISP_CHILD_MAX_PROCESSES",(size_t)DISP_DEFAULT_CHILD_PROCESSES);struct rlimit memory_limit={.rlim_cur=(rlim_t)memory,.rlim_max=(rlim_t)memory};rlim_t cpu_seconds=(rlim_t)(cpu_millis/1000+(cpu_millis%1000!=0));struct rlimit cpu_limit={.rlim_cur=cpu_seconds,.rlim_max=cpu_seconds};struct rlimit process_limit={.rlim_cur=(rlim_t)processes,.rlim_max=(rlim_t)processes};if((size_t)memory_limit.rlim_cur!=memory||(size_t)cpu_limit.rlim_cur!=cpu_seconds||(size_t)process_limit.rlim_cur!=processes){errno=EOVERFLOW;return -1;}if(setpgid(0,0)!=0||setrlimit(RLIMIT_AS,&memory_limit)!=0||setrlimit(RLIMIT_CPU,&cpu_limit)!=0||setrlimit(RLIMIT_NPROC,&process_limit)!=0||disp_process_close_on_exec()!=0)return -1;
#ifdef __linux__
if(disp_process_linux_escape_filter()!=0)return -1;
#endif
return 0;}
static int disp_process_sandbox_exec(const char *path,char *const argv[],char *const environment[],bool custom_environment){
int mode=disp_process_hard_mode();if(mode<0)return -1;bool trusted=disp_process_helper_trusted();if(mode==2&&!trusted){errno=EPERM;return -1;}
if(mode>0&&trusted){
size_t memory=disp_runtime_limit("DISP_CHILD_MAX_MEMORY_BYTES",(size_t)DISP_DEFAULT_CHILD_MEMORY_BYTES),cpu=disp_runtime_limit("DISP_CHILD_MAX_CPU_MILLIS",(size_t)DISP_DEFAULT_CHILD_CPU_MILLIS),processes=disp_runtime_limit("DISP_CHILD_MAX_PROCESSES",(size_t)DISP_DEFAULT_CHILD_PROCESSES),wall=disp_runtime_limit("DISP_CHILD_MAX_WALL_MILLIS",(size_t)DISP_DEFAULT_CHILD_WALL_MILLIS);size_t count=0;
while(argv[count]){if(count>=DISP_PROCESS_MAX_ARGUMENTS+1){errno=E2BIG;return -1;}count++;}
char memory_text[32],cpu_text[32],process_text[32],wall_text[32],exec_error_text[32];
snprintf(memory_text,sizeof(memory_text),"%zu",memory);snprintf(cpu_text,sizeof(cpu_text),"%zu",cpu);snprintf(process_text,sizeof(process_text),"%zu",processes);snprintf(wall_text,sizeof(wall_text),"%zu",wall);
char *helper_argv[count+8];size_t at=0;helper_argv[at++]=(char*)disp_process_cgroup_helper;helper_argv[at++]=memory_text;helper_argv[at++]=cpu_text;helper_argv[at++]=process_text;helper_argv[at++]=wall_text;
if(disp_process_exec_error_fd>=3){snprintf(exec_error_text,sizeof(exec_error_text),"%d",disp_process_exec_error_fd);helper_argv[at++]=(char*)"--exec-error-fd";helper_argv[at++]=exec_error_text;}
helper_argv[at++]=(char*)path;for(size_t i=1;i<count;i++)helper_argv[at++]=argv[i];helper_argv[at]=NULL;
if(setpgid(0,0)!=0||disp_process_close_on_exec()!=0)return -1;
if(disp_process_exec_error_fd>=3){int flags=fcntl(disp_process_exec_error_fd,F_GETFD);if(flags<0||fcntl(disp_process_exec_error_fd,F_SETFD,flags&~FD_CLOEXEC)!=0)return -1;}
return custom_environment?execve(disp_process_cgroup_helper,helper_argv,environment):execv(disp_process_cgroup_helper,helper_argv);
}
if(disp_process_sandbox_child()<0)return -1;return custom_environment?execve(path,argv,environment):execv(path,argv);
}
static pid_t disp_process_sandbox_fork(void){pid_t pid=fork();if(pid>0&&setpgid(pid,pid)!=0&&errno!=EACCES&&errno!=ESRCH){int code=errno;kill(pid,SIGKILL);while(waitpid(pid,NULL,0)<0&&errno==EINTR){}errno=code;return -1;}return pid;}
static int disp_process_tree_kill(pid_t pid,int signal){if(pid<=0){errno=EINVAL;return -1;}return kill(-pid,signal);}
static pid_t disp_process_sandbox_waitpid(pid_t pid,int *status,int options){pid_t result=waitpid(pid,status,options);if(result>0){int saved=errno;kill(-result,SIGKILL);errno=saved;}return result;}
#define fork() disp_process_sandbox_fork()
#define execv(path,argv) disp_process_sandbox_exec((path),(argv),NULL,false)
#define execve(path,argv,environment) disp_process_sandbox_exec((path),(argv),(environment),true)
#define kill(pid,signal) disp_process_tree_kill((pid),(signal))
#define waitpid(pid,status,options) disp_process_sandbox_waitpid((pid),(status),(options))
#endif
static bool disp_process_run(const disp_native_path *program,const disp_native_string *args,size_t args_len,disp_native_process_output *output,disp_native_string *error){*output=(disp_native_process_output){0};*error=(disp_native_string){0};if(!program->data||!program->len){*error=disp_process_error_text("process program path cannot be empty");return false;}if(memchr(program->data,0,program->len)){*error=disp_process_error_text("process program path cannot contain NUL");return false;}if(args_len>DISP_PROCESS_MAX_ARGUMENTS){*error=disp_process_error_text("process argument count exceeds 4096");return false;}size_t argument_bytes=0;for(size_t i=0;i<args_len;i++){if(memchr(args[i].data,0,args[i].len)||argument_bytes>DISP_PROCESS_MAX_ARGUMENT_BYTES-args[i].len){*error=disp_process_error_text("process arguments exceed limits or contain NUL");return false;}argument_bytes+=args[i].len;}
#ifdef _WIN32
char *application=(char*)disp_alloc(program->len+1,1);memcpy(application,program->data,program->len);application[program->len]=0;char *command=NULL;size_t command_len=0,command_cap=0;if(!disp_process_quote(&command,&command_len,&command_cap,application))goto windows_fail;for(size_t i=0;i<args_len;i++){char *arg=(char*)disp_alloc(args[i].len+1,1);memcpy(arg,args[i].data,args[i].len);arg[args[i].len]=0;if(!disp_process_append(&command,&command_len,&command_cap," ",1)||!disp_process_quote(&command,&command_len,&command_cap,arg)){disp_dealloc(arg);goto windows_fail;}disp_dealloc(arg);}wchar_t *wide_application=disp_process_wide(application,program->len),*wide_command=disp_process_wide(command,command_len);if(!wide_application||!wide_command){disp_dealloc(wide_application);disp_dealloc(wide_command);goto windows_fail;}SECURITY_ATTRIBUTES security={sizeof(security),NULL,TRUE};HANDLE out_read=NULL,out_write=NULL,err_read=NULL,err_write=NULL,nul_input=NULL;if(!CreatePipe(&out_read,&out_write,&security,0)||!CreatePipe(&err_read,&err_write,&security,0))goto windows_handles_fail;SetHandleInformation(out_read,HANDLE_FLAG_INHERIT,0);SetHandleInformation(err_read,HANDLE_FLAG_INHERIT,0);nul_input=CreateFileW(L"NUL",GENERIC_READ,FILE_SHARE_READ|FILE_SHARE_WRITE,&security,OPEN_EXISTING,FILE_ATTRIBUTE_NORMAL,NULL);if(nul_input==INVALID_HANDLE_VALUE){nul_input=NULL;goto windows_handles_fail;}STARTUPINFOEXW startup={0};startup.StartupInfo.cb=sizeof(startup);startup.StartupInfo.dwFlags=STARTF_USESTDHANDLES;startup.StartupInfo.hStdInput=nul_input;startup.StartupInfo.hStdOutput=out_write;startup.StartupInfo.hStdError=err_write;SIZE_T attr_size=0;InitializeProcThreadAttributeList(NULL,1,0,&attr_size);startup.lpAttributeList=(LPPROC_THREAD_ATTRIBUTE_LIST)disp_alloc(attr_size,_Alignof(void*));if(!InitializeProcThreadAttributeList(startup.lpAttributeList,1,0,&attr_size))goto windows_attr_fail;HANDLE inherited[3]={nul_input,out_write,err_write};if(!UpdateProcThreadAttribute(startup.lpAttributeList,0,PROC_THREAD_ATTRIBUTE_HANDLE_LIST,inherited,sizeof(inherited),NULL,NULL))goto windows_attr_list_fail;PROCESS_INFORMATION child={0};if(!CreateProcessW(wide_application,wide_command,NULL,NULL,TRUE,EXTENDED_STARTUPINFO_PRESENT,NULL,NULL,&startup.StartupInfo,&child))goto windows_attr_list_fail;CloseHandle(out_write);out_write=NULL;CloseHandle(err_write);err_write=NULL;CloseHandle(nul_input);nul_input=NULL;DeleteProcThreadAttributeList(startup.lpAttributeList);disp_dealloc(startup.lpAttributeList);disp_dealloc(application);disp_dealloc(command);disp_dealloc(wide_application);disp_dealloc(wide_command);disp_process_capture out={.source=out_read},err={.source=err_read};disp_native_thread out_thread={.handle=disp_thread_start(disp_process_capture_entry,&out,0,0)},err_thread={.handle=disp_thread_start(disp_process_capture_entry,&err,0,0)};DWORD wait_status=WaitForSingleObject(child.hProcess,INFINITE),exit_code=0;if(wait_status==WAIT_OBJECT_0)GetExitCodeProcess(child.hProcess,&exit_code);else exit_code=(DWORD)-1;CloseHandle(child.hThread);CloseHandle(child.hProcess);disp_thread_wait(&out_thread);disp_thread_wait(&err_thread);CloseHandle(out_read);CloseHandle(err_read);if(out.failed||err.failed||out.overflow||err.overflow){disp_dealloc(out.data);disp_dealloc(err.data);*error=disp_process_error_text(out.overflow||err.overflow?"process output exceeds the 16 MiB capture limit":"could not capture process output");return false;}output->status=(int64_t)(int32_t)exit_code;output->stdout_data=out.data;output->stdout_len=out.len;output->stderr_data=err.data;output->stderr_len=err.len;return true;
windows_attr_list_fail:DeleteProcThreadAttributeList(startup.lpAttributeList);windows_attr_fail:disp_dealloc(startup.lpAttributeList);windows_handles_fail:if(out_read)CloseHandle(out_read);if(out_write)CloseHandle(out_write);if(err_read)CloseHandle(err_read);if(err_write)CloseHandle(err_write);if(nul_input)CloseHandle(nul_input);disp_dealloc(wide_application);disp_dealloc(wide_command);windows_fail:disp_dealloc(application);disp_dealloc(command);*error=disp_process_error_text("could not start child process");return false;
#else
int out_pipe[2],err_pipe[2];if(pipe(out_pipe)!=0){*error=disp_process_error_text(strerror(errno));return false;}if(pipe(err_pipe)!=0){close(out_pipe[0]);close(out_pipe[1]);*error=disp_process_error_text(strerror(errno));return false;}char *application=(char*)disp_alloc(program->len+1,1);memcpy(application,program->data,program->len);application[program->len]=0;char **argv=(char**)disp_alloc_zeroed(args_len+2,sizeof(char*),_Alignof(char*));argv[0]=application;for(size_t i=0;i<args_len;i++){argv[i+1]=(char*)disp_alloc(args[i].len+1,1);memcpy(argv[i+1],args[i].data,args[i].len);}pid_t pid=fork();if(pid==0){dup2(out_pipe[1],STDOUT_FILENO);dup2(err_pipe[1],STDERR_FILENO);close(out_pipe[0]);close(out_pipe[1]);close(err_pipe[0]);close(err_pipe[1]);execv(application,argv);_exit(127);}close(out_pipe[1]);close(err_pipe[1]);if(pid<0){close(out_pipe[0]);close(err_pipe[0]);for(size_t i=0;i<args_len;i++)disp_dealloc(argv[i+1]);disp_dealloc(argv);disp_dealloc(application);*error=disp_process_error_text(strerror(errno));return false;}disp_process_capture out={.source=out_pipe[0]},err={.source=err_pipe[0]};disp_native_thread out_thread={.handle=disp_thread_start(disp_process_capture_entry,&out,0,0)},err_thread={.handle=disp_thread_start(disp_process_capture_entry,&err,0,0)};int status=0;pid_t waited;do{waited=waitpid(pid,&status,0);}while(waited<0&&errno==EINTR);disp_thread_wait(&out_thread);disp_thread_wait(&err_thread);close(out_pipe[0]);close(err_pipe[0]);for(size_t i=0;i<args_len;i++)disp_dealloc(argv[i+1]);disp_dealloc(argv);disp_dealloc(application);if(waited<0||out.failed||err.failed||out.overflow||err.overflow){disp_dealloc(out.data);disp_dealloc(err.data);*error=disp_process_error_text(out.overflow||err.overflow?"process output exceeds the 16 MiB capture limit":(waited<0?strerror(errno):"could not capture process output"));return false;}output->status=WIFEXITED(status)?WEXITSTATUS(status):(WIFSIGNALED(status)?128+WTERMSIG(status):-1);output->stdout_data=out.data;output->stdout_len=out.len;output->stderr_data=err.data;output->stderr_len=err.len;return true;
#endif
}

static void disp_process_command_drop(disp_native_process_command *command){disp_path_drop(&command->program);for(size_t i=0;i<command->args_len;i++)disp_string_drop(&command->args[i]);disp_dealloc(command->args);if(command->has_directory)disp_path_drop(&command->directory);for(size_t i=0;i<command->environment_len;i++){disp_string_drop(&command->environment_keys[i]);disp_string_drop(&command->environment_values[i]);}disp_dealloc(command->environment_keys);disp_dealloc(command->environment_values);if(command->input_cap)disp_dealloc(command->input);*command=(disp_native_process_command){0};}
typedef struct {
#ifdef _WIN32
HANDLE target;
#else
int target;
#endif
const uint8_t *data;size_t len;bool failed;
} disp_process_input;
static void disp_process_input_entry(void *raw){disp_process_input *input=(disp_process_input*)raw;size_t written=0;while(written<input->len){
#ifdef _WIN32
DWORD count=0;if(!WriteFile(input->target,input->data+written,(DWORD)((input->len-written)>UINT32_MAX?UINT32_MAX:(input->len-written)),&count,NULL)){input->failed=true;break;}written+=(size_t)count;
#else
sigset_t blocked;sigemptyset(&blocked);sigaddset(&blocked,SIGPIPE);pthread_sigmask(SIG_BLOCK,&blocked,NULL);ssize_t count=write(input->target,input->data+written,input->len-written);if(count<0){if(errno==EINTR)continue;input->failed=true;break;}written+=(size_t)count;
#endif
}
#ifdef _WIN32
CloseHandle(input->target);
#else
close(input->target);
#endif
}
static bool disp_process_command_valid(const disp_native_process_command *command,disp_native_string *error){if(!command->program.data||!command->program.len||memchr(command->program.data,0,command->program.len)){*error=disp_process_error_text("process program path must be non-empty and contain no NUL");return false;}if(command->args_len>DISP_PROCESS_MAX_ARGUMENTS||command->environment_len>DISP_PROCESS_MAX_ARGUMENTS){*error=disp_process_error_text("process argument or environment count exceeds 4096");return false;}size_t bytes=0;for(size_t i=0;i<command->args_len;i++){if(memchr(command->args[i].data,0,command->args[i].len)||bytes>DISP_PROCESS_MAX_ARGUMENT_BYTES-command->args[i].len){*error=disp_process_error_text("process arguments exceed limits or contain NUL");return false;}bytes+=command->args[i].len;}if(command->input_len>DISP_PROCESS_MAX_CAPTURE){*error=disp_process_error_text("process input exceeds the 16 MiB limit");return false;}if(command->has_directory&&memchr(command->directory.data,0,command->directory.len)){*error=disp_process_error_text("process working directory cannot contain NUL");return false;}for(size_t i=0;i<command->environment_len;i++){disp_native_string key=command->environment_keys[i],value=command->environment_values[i];if(!key.len||memchr(key.data,0,key.len)||memchr(key.data,'=',key.len)||memchr(value.data,0,value.len)){*error=disp_process_error_text("process environment names must be non-empty, names cannot contain '=', and names or values cannot contain NUL");return false;}}return true;}
typedef struct {
#ifdef _WIN32
HANDLE source;CRITICAL_SECTION lock;
#else
int source;pthread_mutex_t lock;
#endif
uint8_t *data;size_t len;size_t cap;bool done;bool failed;bool overflow;
} disp_child_pipe;
struct disp_child_state {
#ifdef _WIN32
HANDLE process;HANDLE input;HANDLE job;
#else
pid_t process;int input;
#endif
disp_child_pipe out;disp_child_pipe err;disp_native_thread out_thread;disp_native_thread err_thread;uint64_t deadline;bool has_deadline;bool complete;bool joined;bool handle_charged;int64_t status;
};
static void disp_child_pipe_lock(disp_child_pipe *pipe){
#ifdef _WIN32
EnterCriticalSection(&pipe->lock);
#else
pthread_mutex_lock(&pipe->lock);
#endif
}
static void disp_child_pipe_unlock(disp_child_pipe *pipe){
#ifdef _WIN32
LeaveCriticalSection(&pipe->lock);
#else
pthread_mutex_unlock(&pipe->lock);
#endif
}
static void disp_child_pipe_init(disp_child_pipe *pipe){
#ifdef _WIN32
InitializeCriticalSection(&pipe->lock);
#else
if(pthread_mutex_init(&pipe->lock,NULL)!=0)dv_panic("could not initialize child-process pipe",0,0);
#endif
}
static void disp_child_pipe_destroy(disp_child_pipe *pipe){
#ifdef _WIN32
DeleteCriticalSection(&pipe->lock);
#else
pthread_mutex_destroy(&pipe->lock);
#endif
disp_dealloc(pipe->data);pipe->data=NULL;pipe->len=pipe->cap=0;
}
static void disp_child_pipe_entry(void *raw){disp_child_pipe *pipe=(disp_child_pipe*)raw;uint8_t chunk[8192];for(;;){size_t count=0;
#ifdef _WIN32
DWORD read_count=0;if(!ReadFile(pipe->source,chunk,sizeof(chunk),&read_count,NULL)){if(GetLastError()!=ERROR_BROKEN_PIPE){disp_child_pipe_lock(pipe);pipe->failed=true;disp_child_pipe_unlock(pipe);}break;}count=(size_t)read_count;
#else
ssize_t read_count=read(pipe->source,chunk,sizeof(chunk));if(read_count<0){if(errno==EINTR)continue;disp_child_pipe_lock(pipe);pipe->failed=true;disp_child_pipe_unlock(pipe);break;}count=(size_t)read_count;
#endif
if(!count)break;disp_child_pipe_lock(pipe);if(pipe->len>DISP_PROCESS_MAX_CAPTURE-count){pipe->overflow=true;disp_child_pipe_unlock(pipe);continue;}size_t needed=pipe->len+count;if(needed>pipe->cap){size_t cap=pipe->cap?pipe->cap:8192;while(cap<needed)cap*=2;if(cap>DISP_PROCESS_MAX_CAPTURE)cap=DISP_PROCESS_MAX_CAPTURE;pipe->data=(uint8_t*)disp_realloc(pipe->data,cap,1);pipe->cap=cap;}memcpy(pipe->data+pipe->len,chunk,count);pipe->len=needed;disp_child_pipe_unlock(pipe);}disp_child_pipe_lock(pipe);pipe->done=true;disp_child_pipe_unlock(pipe);
#ifdef _WIN32
CloseHandle(pipe->source);
#else
close(pipe->source);
#endif
}
static void disp_child_close_input(disp_child_state *state){
#ifdef _WIN32
if(state->input){CloseHandle(state->input);state->input=NULL;}
#else
if(state->input>=0){close(state->input);state->input=-1;}
#endif
}
static void disp_child_join_readers(disp_child_state *state){if(state->joined)return;disp_thread_wait(&state->out_thread);disp_thread_wait(&state->err_thread);state->joined=true;}
static bool disp_child_update(disp_child_state *state,bool block,disp_native_string *error);
static bool disp_child_write(disp_child_state *state,const uint8_t *data,size_t len,disp_native_string *error){if(len>DISP_PROCESS_MAX_CAPTURE){*error=disp_process_error_text("process write exceeds the 16 MiB limit");return false;}if(!disp_child_update(state,false,error))return false;
#ifdef _WIN32
if(!state->input){*error=disp_process_error_text("child-process input is closed");return false;}size_t written=0;while(written<len){DWORD count=0;if(!WriteFile(state->input,data+written,(DWORD)((len-written)>UINT32_MAX?UINT32_MAX:(len-written)),&count,NULL)){*error=disp_process_error_text("could not write child-process input");return false;}written+=(size_t)count;}
#else
if(state->input<0){*error=disp_process_error_text("child-process input is closed");return false;}sigset_t blocked;sigemptyset(&blocked);sigaddset(&blocked,SIGPIPE);pthread_sigmask(SIG_BLOCK,&blocked,NULL);size_t written=0;while(written<len){ssize_t count=write(state->input,data+written,len-written);if(count<0){if(errno==EINTR)continue;*error=disp_process_error_text(strerror(errno));return false;}written+=(size_t)count;}
#endif
return true;}
static bool disp_child_read(disp_child_state *state,bool stdout_pipe,size_t limit,uint8_t **data,size_t *len,disp_native_string *error){*data=NULL;*len=0;if(limit>DISP_PROCESS_MAX_CAPTURE){*error=disp_process_error_text("child-process read limit exceeds 16 MiB");return false;}if(!limit)return true;disp_child_pipe *pipe=stdout_pipe?&state->out:&state->err;for(;;){if(!disp_child_update(state,false,error))return false;disp_child_pipe_lock(pipe);if(pipe->failed||pipe->overflow){bool overflow=pipe->overflow;disp_child_pipe_unlock(pipe);*error=disp_process_error_text(overflow?"process output exceeds the 16 MiB capture limit":"could not read child-process output");return false;}if(pipe->len){size_t count=pipe->len<limit?pipe->len:limit;uint8_t *result=(uint8_t*)disp_alloc(count,1);memcpy(result,pipe->data,count);memmove(pipe->data,pipe->data+count,pipe->len-count);pipe->len-=count;disp_child_pipe_unlock(pipe);*data=result;*len=count;return true;}bool done=pipe->done;disp_child_pipe_unlock(pipe);if(done)return true;disp_time_sleep(1000000ULL);}}
static bool disp_child_wait_output(disp_child_state *state,disp_native_process_output *output,disp_native_string *error){*output=(disp_native_process_output){0};disp_child_close_input(state);if(!disp_child_update(state,true,error))return false;disp_child_join_readers(state);for(int i=0;i<2;i++){disp_child_pipe *pipe=i?&state->err:&state->out;disp_child_pipe_lock(pipe);if(pipe->failed||pipe->overflow){bool overflow=pipe->overflow;disp_child_pipe_unlock(pipe);*error=disp_process_error_text(overflow?"process output exceeds the 16 MiB capture limit":"could not read child-process output");return false;}if(i){output->stderr_data=pipe->data;output->stderr_len=pipe->len;}else{output->stdout_data=pipe->data;output->stdout_len=pipe->len;}pipe->data=NULL;pipe->len=pipe->cap=0;disp_child_pipe_unlock(pipe);}output->status=state->status;return true;}
static void disp_child_drop(disp_native_child_process *child){disp_child_state *state=child->state;if(!state)return;disp_native_string ignored={0};if(!state->complete){
#ifdef _WIN32
if(state->job)TerminateJobObject(state->job,124);else TerminateProcess(state->process,124);
#else
kill(state->process,SIGKILL);
#endif
}disp_child_close_input(state);disp_child_update(state,true,&ignored);disp_string_drop(&ignored);disp_child_join_readers(state);
#ifdef _WIN32
if(state->process)CloseHandle(state->process);
if(state->job)CloseHandle(state->job);
#endif
disp_child_pipe_destroy(&state->out);disp_child_pipe_destroy(&state->err);if(state->handle_charged)disp_runtime_release_handle();disp_dealloc(state);child->state=NULL;}
#ifdef _WIN32
static bool disp_process_env_match(const wchar_t *entry,const wchar_t *key){const wchar_t *equal=wcschr(entry,L'=');if(!equal||equal==entry)return false;size_t length=(size_t)(equal-entry);return wcslen(key)==length&&!_wcsnicmp(entry,key,length);}
static int disp_process_env_compare(const void *left,const void *right){return _wcsicmp(*(const wchar_t *const*)left,*(const wchar_t *const*)right);}
static wchar_t *disp_process_environment(const disp_native_process_command *command){wchar_t **keys=(wchar_t**)disp_alloc_zeroed(command->environment_len,sizeof(wchar_t*),_Alignof(wchar_t*));for(size_t i=0;i<command->environment_len;i++){keys[i]=disp_process_wide(command->environment_keys[i].data,command->environment_keys[i].len);if(!keys[i])goto fail;}LPWCH parent=command->clear_environment?NULL:GetEnvironmentStringsW();size_t parent_count=0;if(parent)for(const wchar_t *at=parent;*at;at+=wcslen(at)+1){bool replaced=false;for(size_t i=0;i<command->environment_len;i++)if(disp_process_env_match(at,keys[i])){replaced=true;break;}if(!replaced)parent_count++;}size_t total=parent_count+command->environment_len;wchar_t **items=(wchar_t**)disp_alloc_zeroed(total,sizeof(wchar_t*),_Alignof(wchar_t*));size_t index=0;if(parent)for(const wchar_t *at=parent;*at;at+=wcslen(at)+1){bool replaced=false;for(size_t i=0;i<command->environment_len;i++)if(disp_process_env_match(at,keys[i])){replaced=true;break;}if(!replaced){size_t n=wcslen(at);items[index]=(wchar_t*)disp_alloc((n+1)*sizeof(wchar_t),_Alignof(wchar_t));memcpy(items[index++],at,(n+1)*sizeof(wchar_t));}}if(parent){FreeEnvironmentStringsW(parent);parent=NULL;}for(size_t i=0;i<command->environment_len;i++){wchar_t *value=disp_process_wide(command->environment_values[i].data,command->environment_values[i].len);if(!value)goto items_fail;size_t key_len=wcslen(keys[i]),value_len=wcslen(value);items[index]=(wchar_t*)disp_alloc((key_len+value_len+2)*sizeof(wchar_t),_Alignof(wchar_t));memcpy(items[index],keys[i],key_len*sizeof(wchar_t));items[index][key_len]=L'=';memcpy(items[index]+key_len+1,value,(value_len+1)*sizeof(wchar_t));disp_dealloc(value);index++;}qsort(items,total,sizeof(wchar_t*),disp_process_env_compare);size_t block_chars=2;for(size_t i=0;i<total;i++)block_chars+=wcslen(items[i])+1;wchar_t *block=(wchar_t*)disp_alloc(block_chars*sizeof(wchar_t),_Alignof(wchar_t));size_t offset=0;for(size_t i=0;i<total;i++){size_t n=wcslen(items[i])+1;memcpy(block+offset,items[i],n*sizeof(wchar_t));offset+=n;disp_dealloc(items[i]);}block[offset++]=0;block[offset]=0;disp_dealloc(items);for(size_t i=0;i<command->environment_len;i++)disp_dealloc(keys[i]);disp_dealloc(keys);return block;items_fail:for(size_t i=0;i<index;i++)disp_dealloc(items[i]);disp_dealloc(items);if(parent)FreeEnvironmentStringsW(parent);fail:for(size_t i=0;i<command->environment_len;i++)disp_dealloc(keys[i]);disp_dealloc(keys);return NULL;}
static bool disp_child_update(disp_child_state *state,bool block,disp_native_string *error){if(state->complete)return true;DWORD wait_ms=block?INFINITE:0;if(state->has_deadline){uint64_t now=disp_time_now_nanos();if(now>=state->deadline){if(state->job)TerminateJobObject(state->job,124);else TerminateProcess(state->process,124);WaitForSingleObject(state->process,INFINITE);state->status=124;state->complete=true;disp_child_close_input(state);*error=disp_process_error_text("process exceeded its configured timeout");return false;}if(block){uint64_t remaining=state->deadline-now;uint64_t millis=remaining/1000000ULL+(remaining%1000000ULL!=0);wait_ms=millis>=INFINITE?INFINITE-1:(DWORD)millis;}}DWORD waited=WaitForSingleObject(state->process,wait_ms);if(waited==WAIT_TIMEOUT){if(block){if(state->job)TerminateJobObject(state->job,124);else TerminateProcess(state->process,124);WaitForSingleObject(state->process,INFINITE);state->status=124;state->complete=true;disp_child_close_input(state);*error=disp_process_error_text("process exceeded its configured timeout");return false;}return true;}if(waited!=WAIT_OBJECT_0){*error=disp_process_error_text("could not wait for child process");return false;}DWORD status=0;if(!GetExitCodeProcess(state->process,&status)){*error=disp_process_error_text("could not read child-process status");return false;}state->status=(int64_t)(int32_t)status;state->complete=true;disp_child_close_input(state);return true;}
static bool disp_child_kill(disp_child_state *state,disp_native_string *error){if(state->complete)return true;BOOL terminated=state->job?TerminateJobObject(state->job,124):TerminateProcess(state->process,124);if(!terminated||WaitForSingleObject(state->process,INFINITE)!=WAIT_OBJECT_0){*error=disp_process_error_text("could not terminate child-process tree");return false;}state->status=124;state->complete=true;disp_child_close_input(state);return true;}
static bool disp_process_start_command(const disp_native_process_command *command,disp_native_child_process *child_out,disp_native_string *error){*child_out=(disp_native_child_process){0};*error=(disp_native_string){0};if(!disp_process_command_valid(command,error))return false;wchar_t *application=disp_process_wide(command->program.data,command->program.len),*directory=command->has_directory?disp_process_wide(command->directory.data,command->directory.len):NULL,*environment=disp_process_environment(command);char *application_utf8=NULL,*line=NULL;wchar_t *wide_line=NULL;size_t line_len=0,line_cap=0;HANDLE out_read=NULL,out_write=NULL,err_read=NULL,err_write=NULL,in_read=NULL,in_write=NULL;STARTUPINFOEXW startup={0};PROCESS_INFORMATION child={0};bool attrs_initialized=false;if(!application||(command->has_directory&&!directory)||!environment)goto fail;application_utf8=(char*)disp_alloc(command->program.len+1,1);memcpy(application_utf8,command->program.data,command->program.len);application_utf8[command->program.len]=0;if(!disp_process_quote(&line,&line_len,&line_cap,application_utf8))goto fail;for(size_t i=0;i<command->args_len;i++){char *arg=(char*)disp_alloc(command->args[i].len+1,1);memcpy(arg,command->args[i].data,command->args[i].len);arg[command->args[i].len]=0;bool ok=disp_process_append(&line,&line_len,&line_cap," ",1)&&disp_process_quote(&line,&line_len,&line_cap,arg);disp_dealloc(arg);if(!ok)goto fail;}wide_line=disp_process_wide(line,line_len);SECURITY_ATTRIBUTES security={sizeof(security),NULL,TRUE};if(!wide_line||!CreatePipe(&out_read,&out_write,&security,0)||!CreatePipe(&err_read,&err_write,&security,0)||!CreatePipe(&in_read,&in_write,&security,0))goto fail;SetHandleInformation(out_read,HANDLE_FLAG_INHERIT,0);SetHandleInformation(err_read,HANDLE_FLAG_INHERIT,0);SetHandleInformation(in_write,HANDLE_FLAG_INHERIT,0);startup.StartupInfo.cb=sizeof(startup);startup.StartupInfo.dwFlags=STARTF_USESTDHANDLES;startup.StartupInfo.hStdInput=in_read;startup.StartupInfo.hStdOutput=out_write;startup.StartupInfo.hStdError=err_write;SIZE_T attr_size=0;InitializeProcThreadAttributeList(NULL,1,0,&attr_size);startup.lpAttributeList=(LPPROC_THREAD_ATTRIBUTE_LIST)disp_alloc(attr_size,_Alignof(void*));if(!InitializeProcThreadAttributeList(startup.lpAttributeList,1,0,&attr_size))goto fail;attrs_initialized=true;HANDLE inherited[3]={in_read,out_write,err_write};if(!UpdateProcThreadAttribute(startup.lpAttributeList,0,PROC_THREAD_ATTRIBUTE_HANDLE_LIST,inherited,sizeof(inherited),NULL,NULL)||!CreateProcessW(application,wide_line,NULL,NULL,TRUE,EXTENDED_STARTUPINFO_PRESENT|CREATE_UNICODE_ENVIRONMENT,environment,directory,&startup.StartupInfo,&child))goto fail;DeleteProcThreadAttributeList(startup.lpAttributeList);disp_dealloc(startup.lpAttributeList);startup.lpAttributeList=NULL;attrs_initialized=false;CloseHandle(child.hThread);CloseHandle(in_read);in_read=NULL;CloseHandle(out_write);out_write=NULL;CloseHandle(err_write);err_write=NULL;disp_child_state *state=(disp_child_state*)disp_alloc_zeroed(1,sizeof(disp_child_state),_Alignof(disp_child_state));state->process=child.hProcess;state->input=in_write;state->out.source=out_read;state->err.source=err_read;disp_child_pipe_init(&state->out);disp_child_pipe_init(&state->err);state->has_deadline=command->has_timeout;if(command->has_timeout){uint64_t now=disp_time_now_nanos();state->deadline=UINT64_MAX-now<command->timeout_nanos?UINT64_MAX:now+command->timeout_nanos;}state->out_thread.handle=disp_thread_start(disp_child_pipe_entry,&state->out,0,0);state->err_thread.handle=disp_thread_start(disp_child_pipe_entry,&state->err,0,0);child_out->state=state;if(command->input_len&&!disp_child_write(state,command->input,command->input_len,error)){disp_child_drop(child_out);goto cleanup;}disp_dealloc(application);disp_dealloc(directory);disp_dealloc(environment);disp_dealloc(application_utf8);disp_dealloc(line);disp_dealloc(wide_line);return true;fail:if(attrs_initialized)DeleteProcThreadAttributeList(startup.lpAttributeList);disp_dealloc(startup.lpAttributeList);if(child.hThread)CloseHandle(child.hThread);if(child.hProcess)CloseHandle(child.hProcess);if(out_read)CloseHandle(out_read);if(out_write)CloseHandle(out_write);if(err_read)CloseHandle(err_read);if(err_write)CloseHandle(err_write);if(in_read)CloseHandle(in_read);if(in_write)CloseHandle(in_write);if(!error->data)*error=disp_process_error_text("could not configure or start child process");cleanup:disp_dealloc(application);disp_dealloc(directory);disp_dealloc(environment);disp_dealloc(application_utf8);disp_dealloc(line);disp_dealloc(wide_line);return false;}
static bool disp_process_run_command(const disp_native_process_command *command,disp_native_process_output *output,disp_native_string *error){*output=(disp_native_process_output){0};*error=(disp_native_string){0};if(!disp_process_command_valid(command,error))return false;wchar_t *application=disp_process_wide(command->program.data,command->program.len),*directory=command->has_directory?disp_process_wide(command->directory.data,command->directory.len):NULL,*environment=disp_process_environment(command);if(!application||(command->has_directory&&!directory)||!environment)goto fail;char *application_utf8=(char*)disp_alloc(command->program.len+1,1);memcpy(application_utf8,command->program.data,command->program.len);application_utf8[command->program.len]=0;char *line=NULL;size_t line_len=0,line_cap=0;if(!disp_process_quote(&line,&line_len,&line_cap,application_utf8))goto fail_line;for(size_t i=0;i<command->args_len;i++){char *arg=(char*)disp_alloc(command->args[i].len+1,1);memcpy(arg,command->args[i].data,command->args[i].len);arg[command->args[i].len]=0;if(!disp_process_append(&line,&line_len,&line_cap," ",1)||!disp_process_quote(&line,&line_len,&line_cap,arg)){disp_dealloc(arg);goto fail_line;}disp_dealloc(arg);}wchar_t *wide_line=disp_process_wide(line,line_len);if(!wide_line)goto fail_line;SECURITY_ATTRIBUTES security={sizeof(security),NULL,TRUE};HANDLE out_read=NULL,out_write=NULL,err_read=NULL,err_write=NULL,in_read=NULL,in_write=NULL;if(!CreatePipe(&out_read,&out_write,&security,0)||!CreatePipe(&err_read,&err_write,&security,0)||!CreatePipe(&in_read,&in_write,&security,0))goto fail_handles;SetHandleInformation(out_read,HANDLE_FLAG_INHERIT,0);SetHandleInformation(err_read,HANDLE_FLAG_INHERIT,0);SetHandleInformation(in_write,HANDLE_FLAG_INHERIT,0);STARTUPINFOEXW startup={0};startup.StartupInfo.cb=sizeof(startup);startup.StartupInfo.dwFlags=STARTF_USESTDHANDLES;startup.StartupInfo.hStdInput=in_read;startup.StartupInfo.hStdOutput=out_write;startup.StartupInfo.hStdError=err_write;SIZE_T attr_size=0;InitializeProcThreadAttributeList(NULL,1,0,&attr_size);startup.lpAttributeList=(LPPROC_THREAD_ATTRIBUTE_LIST)disp_alloc(attr_size,_Alignof(void*));if(!InitializeProcThreadAttributeList(startup.lpAttributeList,1,0,&attr_size))goto fail_attr;HANDLE inherited[3]={in_read,out_write,err_write};if(!UpdateProcThreadAttribute(startup.lpAttributeList,0,PROC_THREAD_ATTRIBUTE_HANDLE_LIST,inherited,sizeof(inherited),NULL,NULL))goto fail_attr_list;PROCESS_INFORMATION child={0};if(!CreateProcessW(application,wide_line,NULL,NULL,TRUE,EXTENDED_STARTUPINFO_PRESENT|CREATE_UNICODE_ENVIRONMENT,environment,directory,&startup.StartupInfo,&child))goto fail_attr_list;DeleteProcThreadAttributeList(startup.lpAttributeList);disp_dealloc(startup.lpAttributeList);CloseHandle(in_read);CloseHandle(out_write);CloseHandle(err_write);disp_process_capture out={.source=out_read},err={.source=err_read};disp_process_input input={.target=in_write,.data=command->input,.len=command->input_len};disp_native_thread out_thread={.handle=disp_thread_start(disp_process_capture_entry,&out,0,0)},err_thread={.handle=disp_thread_start(disp_process_capture_entry,&err,0,0)},input_thread={.handle=disp_thread_start(disp_process_input_entry,&input,0,0)};DWORD wait_ms=INFINITE;if(command->has_timeout){uint64_t millis=command->timeout_nanos/1000000ULL+(command->timeout_nanos%1000000ULL!=0);wait_ms=millis>=INFINITE?INFINITE-1:(DWORD)millis;}DWORD waited=WaitForSingleObject(child.hProcess,wait_ms);bool timed_out=waited==WAIT_TIMEOUT;if(timed_out){TerminateProcess(child.hProcess,124);WaitForSingleObject(child.hProcess,INFINITE);}DWORD exit_code=0;GetExitCodeProcess(child.hProcess,&exit_code);CloseHandle(child.hThread);CloseHandle(child.hProcess);disp_thread_wait(&input_thread);disp_thread_wait(&out_thread);disp_thread_wait(&err_thread);CloseHandle(out_read);CloseHandle(err_read);disp_dealloc(application);disp_dealloc(directory);disp_dealloc(environment);disp_dealloc(application_utf8);disp_dealloc(line);disp_dealloc(wide_line);if(timed_out||input.failed||out.failed||err.failed||out.overflow||err.overflow){disp_dealloc(out.data);disp_dealloc(err.data);*error=disp_process_error_text(timed_out?"process exceeded its configured timeout":(out.overflow||err.overflow?"process output exceeds the 16 MiB capture limit":"could not transfer process data"));return false;}output->status=(int64_t)(int32_t)exit_code;output->stdout_data=out.data;output->stdout_len=out.len;output->stderr_data=err.data;output->stderr_len=err.len;return true;fail_attr_list:DeleteProcThreadAttributeList(startup.lpAttributeList);fail_attr:disp_dealloc(startup.lpAttributeList);fail_handles:if(out_read)CloseHandle(out_read);if(out_write)CloseHandle(out_write);if(err_read)CloseHandle(err_read);if(err_write)CloseHandle(err_write);if(in_read)CloseHandle(in_read);if(in_write)CloseHandle(in_write);disp_dealloc(wide_line);fail_line:disp_dealloc(application_utf8);disp_dealloc(line);fail:disp_dealloc(application);disp_dealloc(directory);disp_dealloc(environment);*error=disp_process_error_text("could not configure or start child process");return false;}
#else
extern char **environ;
static bool disp_process_env_match(const char *entry,const disp_native_string *key){const char *equal=strchr(entry,'=');return equal&&(size_t)(equal-entry)==key->len&&!memcmp(entry,key->data,key->len);}
static char **disp_process_environment(const disp_native_process_command *command,size_t *owned_start){size_t parent_count=0;if(!command->clear_environment)for(char **at=environ;at&&*at;at++){bool replaced=false;for(size_t i=0;i<command->environment_len;i++)if(disp_process_env_match(*at,&command->environment_keys[i])){replaced=true;break;}if(!replaced)parent_count++;}char **environment=(char**)disp_alloc_zeroed(parent_count+command->environment_len+1,sizeof(char*),_Alignof(char*));size_t index=0;if(!command->clear_environment)for(char **at=environ;at&&*at;at++){bool replaced=false;for(size_t i=0;i<command->environment_len;i++)if(disp_process_env_match(*at,&command->environment_keys[i])){replaced=true;break;}if(!replaced)environment[index++]=*at;}*owned_start=index;for(size_t i=0;i<command->environment_len;i++){size_t size=command->environment_keys[i].len+command->environment_values[i].len+2;environment[index]=(char*)disp_alloc(size,1);memcpy(environment[index],command->environment_keys[i].data,command->environment_keys[i].len);environment[index][command->environment_keys[i].len]='=';memcpy(environment[index]+command->environment_keys[i].len+1,command->environment_values[i].data,command->environment_values[i].len);environment[index][size-1]=0;index++;}return environment;}
static bool disp_process_open_pipes(int out_pipe[2],int err_pipe[2],int in_pipe[2],int exec_pipe[2],disp_native_string *error){out_pipe[0]=out_pipe[1]=err_pipe[0]=err_pipe[1]=in_pipe[0]=in_pipe[1]=exec_pipe[0]=exec_pipe[1]=-1;if(pipe(out_pipe)==0&&pipe(err_pipe)==0&&pipe(in_pipe)==0&&pipe(exec_pipe)==0)return true;int saved=errno;int *pipes[4]={out_pipe,err_pipe,in_pipe,exec_pipe};for(size_t i=0;i<4;i++)for(size_t j=0;j<2;j++)if(pipes[i][j]>=0)close(pipes[i][j]);*error=disp_process_error_text(strerror(saved));return false;}
static void disp_process_write_exec_error(int fd,int code){int32_t wire=(int32_t)(code>0?code:EIO);const unsigned char *bytes=(const unsigned char*)&wire;size_t written=0;while(written<sizeof(wire)){ssize_t count=write(fd,bytes+written,sizeof(wire)-written);if(count<0){if(errno==EINTR)continue;return;}if(!count)return;written+=(size_t)count;}}
static ssize_t disp_process_read_exec_error(int fd,int32_t *error){size_t received=0;while(received<sizeof(*error)){ssize_t count=read(fd,(unsigned char*)error+received,sizeof(*error)-received);if(count<0){if(errno==EINTR)continue;*error=(int32_t)(errno?errno:EIO);return -1;}if(!count){if(!received)return 0;*error=EPROTO;return -1;}received+=(size_t)count;}if(*error<=0)*error=EIO;return (ssize_t)received;}
static bool disp_child_update(disp_child_state *state,bool block,disp_native_string *error){if(state->complete)return true;for(;;){int status=0;pid_t waited=waitpid(state->process,&status,block&&!state->has_deadline?0:WNOHANG);if(waited==state->process){state->status=WIFEXITED(status)?WEXITSTATUS(status):(WIFSIGNALED(status)?128+WTERMSIG(status):-1);state->complete=true;disp_child_close_input(state);return true;}if(waited<0&&errno!=EINTR){*error=disp_process_error_text(strerror(errno));return false;}if(state->has_deadline&&disp_time_now_nanos()>=state->deadline){kill(state->process,SIGKILL);while(waitpid(state->process,&status,0)<0&&errno==EINTR){}state->status=124;state->complete=true;disp_child_close_input(state);*error=disp_process_error_text("process exceeded its configured timeout");return false;}if(!block)return true;disp_time_sleep(1000000ULL);}}
static bool disp_child_kill(disp_child_state *state,disp_native_string *error){if(state->complete)return true;if(kill(state->process,SIGKILL)!=0&&errno!=ESRCH){*error=disp_process_error_text(strerror(errno));return false;}int status=0;while(waitpid(state->process,&status,0)<0){if(errno==EINTR)continue;*error=disp_process_error_text(strerror(errno));return false;}state->status=WIFEXITED(status)?WEXITSTATUS(status):(WIFSIGNALED(status)?128+WTERMSIG(status):-1);state->complete=true;disp_child_close_input(state);return true;}
static bool disp_process_start_command(const disp_native_process_command *command,disp_native_child_process *child_out,disp_native_string *error){*child_out=(disp_native_child_process){0};*error=(disp_native_string){0};if(!disp_process_command_valid(command,error))return false;int out_pipe[2],err_pipe[2],in_pipe[2],exec_pipe[2];if(!disp_process_open_pipes(out_pipe,err_pipe,in_pipe,exec_pipe,error))return false;fcntl(exec_pipe[1],F_SETFD,FD_CLOEXEC);char *application=(char*)disp_alloc(command->program.len+1,1);memcpy(application,command->program.data,command->program.len);application[command->program.len]=0;char **argv=(char**)disp_alloc_zeroed(command->args_len+2,sizeof(char*),_Alignof(char*));argv[0]=application;for(size_t i=0;i<command->args_len;i++){argv[i+1]=(char*)disp_alloc(command->args[i].len+1,1);memcpy(argv[i+1],command->args[i].data,command->args[i].len);}char *directory=NULL;if(command->has_directory){directory=(char*)disp_alloc(command->directory.len+1,1);memcpy(directory,command->directory.data,command->directory.len);directory[command->directory.len]=0;}size_t owned_start=0;char **environment=disp_process_environment(command,&owned_start);pid_t pid=fork();int fork_error=pid<0?errno:0;if(pid==0){disp_process_exec_error_fd=exec_pipe[1];close(exec_pipe[0]);dup2(in_pipe[0],STDIN_FILENO);dup2(out_pipe[1],STDOUT_FILENO);dup2(err_pipe[1],STDERR_FILENO);close(in_pipe[0]);close(in_pipe[1]);close(out_pipe[0]);close(out_pipe[1]);close(err_pipe[0]);close(err_pipe[1]);if(directory&&chdir(directory)){disp_process_write_exec_error(exec_pipe[1],errno);_exit(126);}execve(application,argv,environment);disp_process_write_exec_error(exec_pipe[1],errno);_exit(127);}close(exec_pipe[1]);close(in_pipe[0]);close(out_pipe[1]);close(err_pipe[1]);int32_t start_error=0;ssize_t start_read=disp_process_read_exec_error(exec_pipe[0],&start_error);close(exec_pipe[0]);if(pid<0||start_read!=0){if(pid>=0){int ignored=0;waitpid(pid,&ignored,0);}close(in_pipe[1]);close(out_pipe[0]);close(err_pipe[0]);*error=disp_process_error_text(strerror(pid<0?fork_error:(int)start_error));goto cleanup;}disp_child_state *state=(disp_child_state*)disp_alloc_zeroed(1,sizeof(disp_child_state),_Alignof(disp_child_state));state->process=pid;state->input=in_pipe[1];state->out.source=out_pipe[0];state->err.source=err_pipe[0];disp_child_pipe_init(&state->out);disp_child_pipe_init(&state->err);state->has_deadline=command->has_timeout;if(command->has_timeout){uint64_t now=disp_time_now_nanos();state->deadline=UINT64_MAX-now<command->timeout_nanos?UINT64_MAX:now+command->timeout_nanos;}state->out_thread.handle=disp_thread_start(disp_child_pipe_entry,&state->out,0,0);state->err_thread.handle=disp_thread_start(disp_child_pipe_entry,&state->err,0,0);child_out->state=state;if(command->input_len&&!disp_child_write(state,command->input,command->input_len,error)){disp_child_drop(child_out);goto cleanup;}for(size_t i=0;i<command->args_len;i++)disp_dealloc(argv[i+1]);disp_dealloc(argv);disp_dealloc(application);disp_dealloc(directory);for(size_t i=owned_start;environment[i];i++)disp_dealloc(environment[i]);disp_dealloc(environment);return true;cleanup:for(size_t i=0;i<command->args_len;i++)disp_dealloc(argv[i+1]);disp_dealloc(argv);disp_dealloc(application);disp_dealloc(directory);for(size_t i=owned_start;environment[i];i++)disp_dealloc(environment[i]);disp_dealloc(environment);return false;}
static bool disp_process_run_command(const disp_native_process_command *command,disp_native_process_output *output,disp_native_string *error){
*output=(disp_native_process_output){0};*error=(disp_native_string){0};if(!disp_process_command_valid(command,error))return false;
int out_pipe[2],err_pipe[2],in_pipe[2],exec_pipe[2];if(!disp_process_open_pipes(out_pipe,err_pipe,in_pipe,exec_pipe,error))return false;
fcntl(exec_pipe[1],F_SETFD,FD_CLOEXEC);char *application=(char*)disp_alloc(command->program.len+1,1);memcpy(application,command->program.data,command->program.len);application[command->program.len]=0;char **argv=(char**)disp_alloc_zeroed(command->args_len+2,sizeof(char*),_Alignof(char*));argv[0]=application;for(size_t i=0;i<command->args_len;i++){argv[i+1]=(char*)disp_alloc(command->args[i].len+1,1);memcpy(argv[i+1],command->args[i].data,command->args[i].len);}char *directory=NULL;if(command->has_directory){directory=(char*)disp_alloc(command->directory.len+1,1);memcpy(directory,command->directory.data,command->directory.len);directory[command->directory.len]=0;}size_t owned_start=0;char **environment=disp_process_environment(command,&owned_start);pid_t pid=fork();if(pid==0){disp_process_exec_error_fd=exec_pipe[1];close(exec_pipe[0]);dup2(in_pipe[0],STDIN_FILENO);dup2(out_pipe[1],STDOUT_FILENO);dup2(err_pipe[1],STDERR_FILENO);close(in_pipe[0]);close(in_pipe[1]);close(out_pipe[0]);close(out_pipe[1]);close(err_pipe[0]);close(err_pipe[1]);if(directory&&chdir(directory)){disp_process_write_exec_error(exec_pipe[1],errno);_exit(126);}execve(application,argv,environment);disp_process_write_exec_error(exec_pipe[1],errno);_exit(127);}close(exec_pipe[1]);close(in_pipe[0]);close(out_pipe[1]);close(err_pipe[1]);if(pid<0){close(exec_pipe[0]);close(in_pipe[1]);close(out_pipe[0]);close(err_pipe[0]);*error=disp_process_error_text(strerror(errno));goto cleanup;}int32_t start_error=0;ssize_t start_read=disp_process_read_exec_error(exec_pipe[0],&start_error);close(exec_pipe[0]);disp_process_capture out={.source=out_pipe[0]},err={.source=err_pipe[0]};disp_process_input input={.target=in_pipe[1],.data=command->input,.len=command->input_len};disp_native_thread out_thread={.handle=disp_thread_start(disp_process_capture_entry,&out,0,0)},err_thread={.handle=disp_thread_start(disp_process_capture_entry,&err,0,0)},input_thread={.handle=disp_thread_start(disp_process_input_entry,&input,0,0)};int status=0;bool timed_out=false;uint64_t started=disp_time_now_nanos();for(;;){pid_t waited=waitpid(pid,&status,WNOHANG);if(waited==pid)break;if(waited<0&&errno!=EINTR){start_error=(int32_t)errno;break;}if(command->has_timeout&&disp_time_now_nanos()-started>=command->timeout_nanos){timed_out=true;kill(pid,SIGKILL);while(waitpid(pid,&status,0)<0&&errno==EINTR){}break;}disp_time_sleep(1000000ULL);}disp_thread_wait(&input_thread);disp_thread_wait(&out_thread);disp_thread_wait(&err_thread);close(out_pipe[0]);close(err_pipe[0]);if(start_read!=0||start_error||timed_out||input.failed||out.failed||err.failed||out.overflow||err.overflow){disp_dealloc(out.data);disp_dealloc(err.data);*error=disp_process_error_text(timed_out?"process exceeded its configured timeout":(start_error?strerror((int)start_error):(out.overflow||err.overflow?"process output exceeds the 16 MiB capture limit":"could not transfer process data")));goto cleanup;}output->status=WIFEXITED(status)?WEXITSTATUS(status):(WIFSIGNALED(status)?128+WTERMSIG(status):-1);output->stdout_data=out.data;output->stdout_len=out.len;output->stderr_data=err.data;output->stderr_len=err.len;for(size_t i=0;i<command->args_len;i++)disp_dealloc(argv[i+1]);disp_dealloc(argv);disp_dealloc(application);disp_dealloc(directory);for(size_t i=owned_start;environment[i];i++)disp_dealloc(environment[i]);disp_dealloc(environment);return true;cleanup:for(size_t i=0;i<command->args_len;i++)disp_dealloc(argv[i+1]);disp_dealloc(argv);disp_dealloc(application);disp_dealloc(directory);for(size_t i=owned_start;environment[i];i++)disp_dealloc(environment[i]);disp_dealloc(environment);return false;}
#endif

static void disp_string_drop(disp_native_string *value){if(value->cap)disp_dealloc(value->data);value->data=NULL;value->len=0;value->cap=0;}
static disp_native_string disp_string_with_capacity(size_t capacity){disp_native_string value={0};if(capacity){value.data=(char*)disp_alloc(capacity,1);value.cap=capacity;}return value;}
static void disp_string_reserve(disp_native_string *value,size_t additional){size_t needed;if(__builtin_add_overflow(value->len,additional,&needed))disp_allocation_failure("string capacity overflow");if(needed<=value->cap)return;size_t capacity=value->cap?value->cap:8;while(capacity<needed){size_t grown;if(__builtin_mul_overflow(capacity,(size_t)2,&grown)){capacity=needed;break;}capacity=grown;}if(value->cap)value->data=(char*)disp_realloc(value->data,capacity,1);else{char *data=(char*)disp_alloc(capacity,1);if(value->len)memcpy(data,value->data,value->len);value->data=data;}value->cap=capacity;}
static void disp_string_push_bytes(disp_native_string *value,const char *bytes,size_t length){disp_string_reserve(value,length);if(length)memcpy(value->data+value->len,bytes,length);value->len+=length;}
static void disp_string_push_char(disp_native_string *value,uint32_t c){char out[4];size_t n;if(c<=0x7F){out[0]=(char)c;n=1;}else if(c<=0x7FF){out[0]=(char)(0xC0|(c>>6));out[1]=(char)(0x80|(c&0x3F));n=2;}else if(c<=0xFFFF && !(c>=0xD800&&c<=0xDFFF)){out[0]=(char)(0xE0|(c>>12));out[1]=(char)(0x80|((c>>6)&0x3F));out[2]=(char)(0x80|(c&0x3F));n=3;}else if(c<=0x10FFFF){out[0]=(char)(0xF0|(c>>18));out[1]=(char)(0x80|((c>>12)&0x3F));out[2]=(char)(0x80|((c>>6)&0x3F));out[3]=(char)(0x80|(c&0x3F));n=4;}else{dv_panic("invalid Unicode scalar",0,0);return;}disp_string_push_bytes(value,out,n);}
static bool disp_utf8_boundary(const char *value,size_t length,size_t index){return index<=length&&(index==0||index==length||(((unsigned char)value[index]&0xC0)!=0x80));}
static bool disp_utf8_valid(const char *value,size_t length){size_t i=0;while(i<length){unsigned char a=(unsigned char)value[i++];if(a<=0x7f)continue;if(a>=0xc2&&a<=0xdf){if(i>=length||((unsigned char)value[i++]&0xc0)!=0x80)return false;continue;}if(a>=0xe0&&a<=0xef){if(i+1>=length)return false;unsigned char b=(unsigned char)value[i++],c=(unsigned char)value[i++];if((c&0xc0)!=0x80)return false;if(a==0xe0){if(b<0xa0||b>0xbf)return false;}else if(a==0xed){if(b<0x80||b>0x9f)return false;}else if((b&0xc0)!=0x80)return false;continue;}if(a>=0xf0&&a<=0xf4){if(i+2>=length)return false;unsigned char b=(unsigned char)value[i++],c=(unsigned char)value[i++],d=(unsigned char)value[i++];if((c&0xc0)!=0x80||(d&0xc0)!=0x80)return false;if(a==0xf0){if(b<0x90||b>0xbf)return false;}else if(a==0xf4){if(b<0x80||b>0x8f)return false;}else if((b&0xc0)!=0x80)return false;continue;}return false;}return true;}

static bool disp_string_starts_with(const char *value,size_t value_len,const char *prefix,size_t prefix_len){return prefix_len<=value_len&&(prefix_len==0||memcmp(value,prefix,prefix_len)==0);}
static bool disp_string_ends_with(const char *value,size_t value_len,const char *suffix,size_t suffix_len){return suffix_len<=value_len&&(suffix_len==0||memcmp(value+value_len-suffix_len,suffix,suffix_len)==0);}
static bool disp_string_contains(const char *value,size_t value_len,const char *needle,size_t needle_len){if(!needle_len)return true;if(needle_len>value_len)return false;for(size_t i=0;i<=value_len-needle_len;i++)if(memcmp(value+i,needle,needle_len)==0)return true;return false;}

typedef struct {const char *at,*end;size_t depth;const char *message;} disp_json_cursor;
static bool disp_json_from_string(const char *text,size_t length,disp_native_json *json,disp_native_string *error);
static void disp_json_space(disp_json_cursor *c){while(c->at<c->end&&(*c->at==' '||*c->at=='\t'||*c->at=='\r'||*c->at=='\n'))c->at++;}
static bool disp_json_value(disp_json_cursor *c);
static bool disp_json_unique(disp_json_cursor *c);
static bool disp_json_decode_string(const char *start,const char *end,disp_native_string *value);
static unsigned disp_json_hex(char value){if(value>='0'&&value<='9')return (unsigned)(value-'0');if(value>='a'&&value<='f')return (unsigned)(value-'a'+10);return (unsigned)(value-'A'+10);}
static bool disp_json_string(disp_json_cursor *c){if(c->at>=c->end||*c->at!='"')return false;c->at++;while(c->at<c->end){unsigned char ch=(unsigned char)*c->at++;if(ch=='"')return true;if(ch<0x20){c->message="JSON string contains a control character";return false;}if(ch=='\\'){if(c->at>=c->end){c->message="JSON escape is incomplete";return false;}char e=*c->at++;if(strchr("\"\\/bfnrt",e))continue;if(e!='u'){c->message="JSON escape is invalid";return false;}uint32_t code=0;for(int i=0;i<4;i++){if(c->at>=c->end||!isxdigit((unsigned char)*c->at)){c->message="JSON Unicode escape is invalid";return false;}code=(code<<4)|disp_json_hex(*c->at++);}if(code>=0xd800&&code<=0xdbff){if(c->end-c->at<6||c->at[0]!='\\'||c->at[1]!='u'){c->message="JSON Unicode surrogate pair is incomplete";return false;}c->at+=2;uint32_t low=0;for(int i=0;i<4;i++){if(c->at>=c->end||!isxdigit((unsigned char)*c->at)){c->message="JSON Unicode surrogate pair is invalid";return false;}low=(low<<4)|disp_json_hex(*c->at++);}if(low<0xdc00||low>0xdfff){c->message="JSON Unicode surrogate pair is invalid";return false;}}else if(code>=0xdc00&&code<=0xdfff){c->message="JSON Unicode surrogate pair is invalid";return false;}}}c->message="JSON string is unterminated";return false;}
static bool disp_json_number(disp_json_cursor *c){const char *start=c->at;if(c->at<c->end&&*c->at=='-')c->at++;if(c->at>=c->end)return false;if(*c->at=='0')c->at++;else{if(*c->at<'1'||*c->at>'9')return false;while(c->at<c->end&&*c->at>='0'&&*c->at<='9')c->at++;}if(c->at<c->end&&*c->at=='.'){c->at++;if(c->at>=c->end||*c->at<'0'||*c->at>'9')return false;while(c->at<c->end&&*c->at>='0'&&*c->at<='9')c->at++;}if(c->at<c->end&&(*c->at=='e'||*c->at=='E')){c->at++;if(c->at<c->end&&(*c->at=='+'||*c->at=='-'))c->at++;if(c->at>=c->end||*c->at<'0'||*c->at>'9')return false;while(c->at<c->end&&*c->at>='0'&&*c->at<='9')c->at++;}return c->at>start;}
static bool disp_json_value(disp_json_cursor *c){disp_json_space(c);if(c->at>=c->end){c->message="JSON value is missing";return false;}if(*c->at=='"')return disp_json_string(c);if(*c->at=='-'||(*c->at>='0'&&*c->at<='9')){if(disp_json_number(c))return true;c->message="JSON number is invalid";return false;}if((size_t)(c->end-c->at)>=4&&!memcmp(c->at,"null",4)){c->at+=4;return true;}if((size_t)(c->end-c->at)>=4&&!memcmp(c->at,"true",4)){c->at+=4;return true;}if((size_t)(c->end-c->at)>=5&&!memcmp(c->at,"false",5)){c->at+=5;return true;}char open=*c->at;if(open!='['&&open!='{'){c->message="JSON value is invalid";return false;}if(c->depth>=DISP_JSON_DEPTH_LIMIT){c->message="JSON nesting exceeds 128 levels";return false;}c->at++;c->depth++;disp_json_space(c);char close=open=='['?']':'}';if(c->at<c->end&&*c->at==close){c->at++;c->depth--;return true;}for(;;){if(open=='{'){if(!disp_json_string(c)){if(!c->message)c->message="JSON object key must be a string";c->depth--;return false;}disp_json_space(c);if(c->at>=c->end||*c->at++!=':'){c->message="JSON object key is missing ':'";c->depth--;return false;}}if(!disp_json_value(c)){c->depth--;return false;}disp_json_space(c);if(c->at<c->end&&*c->at==close){c->at++;c->depth--;return true;}if(c->at>=c->end||*c->at++!=','){c->message="JSON container is missing ',' or its closing delimiter";c->depth--;return false;}disp_json_space(c);}}
static bool disp_json_parse(const char *source,size_t length,disp_native_json *json,disp_native_string *error){if(length>DISP_JSON_LIMIT){*error=disp_owned_bytes("JSON document exceeds the 16 MiB limit",strlen("JSON document exceeds the 16 MiB limit"));return false;}if(!disp_utf8_valid(source,length)){*error=disp_owned_bytes("JSON document is not valid UTF-8",strlen("JSON document is not valid UTF-8"));return false;}disp_json_cursor c={.at=source,.end=source+length,.depth=0,.message=NULL};if(!disp_json_value(&c)){const char *message=c.message?c.message:"invalid JSON document";*error=disp_owned_bytes(message,strlen(message));return false;}disp_json_space(&c);if(c.at!=c.end){*error=disp_owned_bytes("JSON document has trailing data",strlen("JSON document has trailing data"));return false;}disp_json_cursor unique={.at=source,.end=source+length,.depth=0,.message=NULL};if(!disp_json_unique(&unique)){const char *message=unique.message?unique.message:"invalid JSON object";*error=disp_owned_bytes(message,strlen(message));return false;}json->data=(char*)disp_alloc(length?length:1,1);if(length)memcpy(json->data,source,length);json->len=json->cap=length;return true;}
static void disp_json_drop(disp_native_json *json){disp_dealloc(json->data);json->data=NULL;json->len=0;json->cap=0;}
static const char *disp_json_kind_name(const disp_native_json *json){const char *at=json->data,*end=json->data+json->len;while(at<end&&(*at==' '||*at=='\t'||*at=='\r'||*at=='\n'))at++;if(at==end)return "invalid";if(*at=='{')return "object";if(*at=='[')return "array";if(*at=='"')return "string";if(*at=='t'||*at=='f')return "bool";if(*at=='n')return "null";return "number";}
static disp_native_string disp_json_as_string(const disp_native_json *json){return disp_owned_bytes(json->data,json->len);}
static disp_native_string disp_json_kind(const disp_native_json *json){const char *kind=disp_json_kind_name(json);return disp_owned_bytes(kind,strlen(kind));}
static bool disp_json_is_kind(const disp_native_json *json,const char *kind){return !strcmp(disp_json_kind_name(json),kind);}
static disp_native_json disp_json_copy_range(const char *start,const char *end){while(start<end&&(*start==' '||*start=='\t'||*start=='\r'||*start=='\n'))start++;while(end>start&&(end[-1]==' '||end[-1]=='\t'||end[-1]=='\r'||end[-1]=='\n'))end--;disp_native_json value={0};size_t length=(size_t)(end-start);value.data=(char*)disp_alloc(length?length:1,1);if(length)memcpy(value.data,start,length);value.len=value.cap=length;return value;}
static bool disp_json_decode_string(const char *start,const char *end,disp_native_string *value){if(end-start<2||*start!='"'||end[-1]!='"')return false;start++;end--;while(start<end){unsigned char ch=(unsigned char)*start++;if(ch!='\\'){disp_string_push_bytes(value,(const char*)&ch,1);continue;}char escape=*start++;switch(escape){case '"':disp_string_push_bytes(value,"\"",1);break;case '\\':disp_string_push_bytes(value,"\\",1);break;case '/':disp_string_push_bytes(value,"/",1);break;case 'b':disp_string_push_bytes(value,"\b",1);break;case 'f':disp_string_push_bytes(value,"\f",1);break;case 'n':disp_string_push_bytes(value,"\n",1);break;case 'r':disp_string_push_bytes(value,"\r",1);break;case 't':disp_string_push_bytes(value,"\t",1);break;case 'u':{uint32_t first=0;for(int i=0;i<4;i++)first=(first<<4)|disp_json_hex(*start++);uint32_t scalar=first;if(first>=0xd800&&first<=0xdbff){if(end-start<6||start[0]!='\\'||start[1]!='u'){disp_string_drop(value);return false;}start+=2;uint32_t second=0;for(int i=0;i<4;i++)second=(second<<4)|disp_json_hex(*start++);if(second<0xdc00||second>0xdfff){disp_string_drop(value);return false;}scalar=0x10000+(((first-0xd800)<<10)|(second-0xdc00));}else if(first>=0xdc00&&first<=0xdfff){disp_string_drop(value);return false;}disp_string_push_char(value,scalar);break;}default:disp_string_drop(value);return false;}}return true;}
static void disp_json_keys_drop(disp_native_string *keys,size_t length){for(size_t i=0;i<length;i++)disp_string_drop(&keys[i]);disp_dealloc(keys);}
static bool disp_json_unique(disp_json_cursor *c){disp_json_space(c);if(c->at>=c->end)return false;if(*c->at=='['){c->at++;disp_json_space(c);if(c->at<c->end&&*c->at==']'){c->at++;return true;}for(;;){if(!disp_json_unique(c))return false;disp_json_space(c);if(c->at<c->end&&*c->at==']'){c->at++;return true;}if(c->at>=c->end||*c->at++!=',')return false;}}if(*c->at!='{')return disp_json_value(c);c->at++;disp_json_space(c);if(c->at<c->end&&*c->at=='}'){c->at++;return true;}disp_native_string *keys=NULL;size_t length=0,capacity=0;for(;;){disp_json_space(c);const char *start=c->at;if(!disp_json_string(c)){disp_json_keys_drop(keys,length);return false;}const char *end=c->at;disp_native_string key={0};if(!disp_json_decode_string(start,end,&key)){disp_json_keys_drop(keys,length);return false;}for(size_t i=0;i<length;i++)if(keys[i].len==key.len&&(!key.len||!memcmp(keys[i].data,key.data,key.len))){disp_string_drop(&key);disp_json_keys_drop(keys,length);c->message="JSON object contains a duplicate key";return false;}if(length>=DISP_JSON_KEY_LIMIT){disp_string_drop(&key);disp_json_keys_drop(keys,length);c->message="JSON object exceeds 4096 keys";return false;}if(length==capacity){size_t next=capacity?capacity*2:4;keys=(disp_native_string*)disp_realloc(keys,next*sizeof(disp_native_string),_Alignof(disp_native_string));capacity=next;}keys[length++]=key;disp_json_space(c);if(c->at>=c->end||*c->at++!=':'){disp_json_keys_drop(keys,length);return false;}if(!disp_json_unique(c)){disp_json_keys_drop(keys,length);return false;}disp_json_space(c);if(c->at<c->end&&*c->at=='}'){c->at++;disp_json_keys_drop(keys,length);return true;}if(c->at>=c->end||*c->at++!=','){disp_json_keys_drop(keys,length);return false;}}}
static bool disp_json_get(const disp_native_json *json,const char *key,size_t key_len,disp_native_json *value){if(strcmp(disp_json_kind_name(json),"object"))return false;disp_json_cursor c={.at=json->data,.end=json->data+json->len};disp_json_space(&c);c.at++;disp_json_space(&c);if(c.at<c.end&&*c.at=='}')return false;for(;;){disp_json_space(&c);const char *key_start=c.at;if(!disp_json_string(&c))return false;const char *key_end=c.at;disp_native_string decoded={0};if(!disp_json_decode_string(key_start,key_end,&decoded))return false;bool equal=decoded.len==key_len&&(!key_len||!memcmp(decoded.data,key,key_len));disp_string_drop(&decoded);disp_json_space(&c);if(c.at>=c.end||*c.at++!=':')return false;disp_json_space(&c);const char *start=c.at;if(!disp_json_value(&c))return false;const char *end=c.at;if(equal){*value=disp_json_copy_range(start,end);return true;}disp_json_space(&c);if(c.at<c.end&&*c.at=='}')return false;if(c.at>=c.end||*c.at++!=',')return false;}}
static bool disp_json_at(const disp_native_json *json,size_t wanted,disp_native_json *value){if(strcmp(disp_json_kind_name(json),"array"))return false;disp_json_cursor c={.at=json->data,.end=json->data+json->len};disp_json_space(&c);c.at++;disp_json_space(&c);if(c.at<c.end&&*c.at==']')return false;size_t index=0;for(;;){disp_json_space(&c);const char *start=c.at;if(!disp_json_value(&c))return false;const char *end=c.at;if(index++==wanted){*value=disp_json_copy_range(start,end);return true;}disp_json_space(&c);if(c.at<c.end&&*c.at==']')return false;if(c.at>=c.end||*c.at++!=',')return false;}}
static bool disp_json_as_bool(const disp_native_json *json,bool *value,disp_native_string *error){const char *at=json->data,*end=json->data+json->len;while(at<end&&isspace((unsigned char)*at))at++;while(end>at&&isspace((unsigned char)end[-1]))end--;if(end-at==4&&!memcmp(at,"true",4)){*value=true;return true;}if(end-at==5&&!memcmp(at,"false",5)){*value=false;return true;}const char *message="JSON value is not a bool";*error=disp_owned_bytes(message,strlen(message));return false;}
static bool disp_json_number_text(const disp_native_json *json,char **text,size_t *length,disp_native_string *error){if(strcmp(disp_json_kind_name(json),"number")){const char *message="JSON value is not a number";*error=disp_owned_bytes(message,strlen(message));return false;}const char *start=json->data,*end=json->data+json->len;while(start<end&&isspace((unsigned char)*start))start++;while(end>start&&isspace((unsigned char)end[-1]))end--;*length=(size_t)(end-start);*text=(char*)disp_alloc(*length+1,1);memcpy(*text,start,*length);(*text)[*length]=0;return true;}
static bool disp_json_as_int(const disp_native_json *json,int64_t *value,disp_native_string *error){char *text=NULL,*end=NULL;size_t length=0;if(!disp_json_number_text(json,&text,&length,error))return false;errno=0;long long parsed=strtoll(text,&end,10);bool ok=!errno&&end==text+length;if(ok)*value=(int64_t)parsed;else{const char *message="JSON value is not an integer representable as int";*error=disp_owned_bytes(message,strlen(message));}disp_dealloc(text);return ok;}
static bool disp_json_as_uint(const disp_native_json *json,uint64_t *value,disp_native_string *error){char *text=NULL,*end=NULL;size_t length=0;if(!disp_json_number_text(json,&text,&length,error))return false;if(length&&text[0]=='-'){disp_dealloc(text);const char *message="JSON value is not an integer representable as uint";*error=disp_owned_bytes(message,strlen(message));return false;}errno=0;unsigned long long parsed=strtoull(text,&end,10);bool ok=!errno&&end==text+length;if(ok)*value=(uint64_t)parsed;else{const char *message="JSON value is not an integer representable as uint";*error=disp_owned_bytes(message,strlen(message));}disp_dealloc(text);return ok;}
static bool disp_json_as_f64(const disp_native_json *json,double *value,disp_native_string *error){char *text=NULL,*end=NULL;size_t length=0;if(!disp_json_number_text(json,&text,&length,error))return false;errno=0;double parsed=strtod(text,&end);bool ok=!errno&&end==text+length&&isfinite(parsed);if(ok)*value=parsed;else{const char *message="JSON number is not representable as f64";*error=disp_owned_bytes(message,strlen(message));}disp_dealloc(text);return ok;}
static bool disp_json_as_text(const disp_native_json *json,disp_native_string *value,disp_native_string *error){const char *start=json->data,*end=json->data+json->len;while(start<end&&isspace((unsigned char)*start))start++;while(end>start&&isspace((unsigned char)end[-1]))end--;if(disp_json_decode_string(start,end,value))return true;const char *message="JSON value is not a string";*error=disp_owned_bytes(message,strlen(message));return false;}
static bool disp_json_collection_len(const disp_native_json *json,size_t *length){const char *kind=disp_json_kind_name(json);bool object=!strcmp(kind,"object");if(!object&&strcmp(kind,"array"))return false;disp_json_cursor c={.at=json->data,.end=json->data+json->len,.depth=0,.message=NULL};disp_json_space(&c);char close=object?'}':']';c.at++;disp_json_space(&c);size_t count=0;if(c.at<c.end&&*c.at==close){*length=0;return true;}for(;;){if(object){if(!disp_json_string(&c))return false;disp_json_space(&c);if(c.at>=c.end||*c.at++!=':')return false;disp_json_space(&c);}if(!disp_json_value(&c))return false;count++;disp_json_space(&c);if(c.at<c.end&&*c.at==close){*length=count;return true;}if(c.at>=c.end||*c.at++!=',')return false;disp_json_space(&c);}}
static bool disp_json_object_entry_at(const disp_native_json *json,size_t wanted,disp_native_string *key,disp_native_json *value){if(strcmp(disp_json_kind_name(json),"object"))return false;disp_json_cursor c={.at=json->data,.end=json->data+json->len,.depth=0,.message=NULL};disp_json_space(&c);c.at++;disp_json_space(&c);size_t index=0;if(c.at<c.end&&*c.at=='}')return false;for(;;){const char *key_start=c.at;if(!disp_json_string(&c))return false;const char *key_end=c.at;disp_json_space(&c);if(c.at>=c.end||*c.at++!=':')return false;disp_json_space(&c);const char *start=c.at;if(!disp_json_value(&c))return false;const char *end=c.at;if(index++==wanted){if(!disp_json_decode_string(key_start,key_end,key))return false;*value=disp_json_copy_range(start,end);return true;}disp_json_space(&c);if(c.at<c.end&&*c.at=='}')return false;if(c.at>=c.end||*c.at++!=',')return false;disp_json_space(&c);}}
static bool disp_json_as_i128(const disp_native_json *json,__int128 *value,disp_native_string *error){char *text=NULL;size_t length=0;if(!disp_json_number_text(json,&text,&length,error))return false;size_t i=0;bool negative=i<length&&text[i]=='-';if(negative)i++;unsigned __int128 magnitude=0,limit=negative?((unsigned __int128)1<<127):(((unsigned __int128)1<<127)-1);bool ok=i<length;for(;ok&&i<length;i++){unsigned char digit=(unsigned char)(text[i]-'0');if(digit>9||magnitude>(limit-digit)/10)ok=false;else magnitude=magnitude*10+digit;}if(ok)*value=negative?(magnitude==((unsigned __int128)1<<127)?-((__int128)(((unsigned __int128)1<<127)-1))-1:-(__int128)magnitude):(__int128)magnitude;else{const char *message="JSON value is not a representable signed integer";*error=disp_owned_bytes(message,strlen(message));}disp_dealloc(text);return ok;}
static bool disp_json_as_u128(const disp_native_json *json,unsigned __int128 *value,disp_native_string *error){char *text=NULL;size_t length=0;if(!disp_json_number_text(json,&text,&length,error))return false;unsigned __int128 magnitude=0,limit=~(unsigned __int128)0;bool ok=length>0;for(size_t i=0;ok&&i<length;i++){unsigned char digit=(unsigned char)(text[i]-'0');if(digit>9||magnitude>(limit-digit)/10)ok=false;else magnitude=magnitude*10+digit;}if(ok)*value=magnitude;else{const char *message="JSON value is not a representable unsigned integer";*error=disp_owned_bytes(message,strlen(message));}disp_dealloc(text);return ok;}
static bool disp_json_from_char(uint32_t scalar,disp_native_json *json,disp_native_string *error){char bytes[4];size_t length=0;if(scalar<=0x7f){bytes[length++]=(char)scalar;}else if(scalar<=0x7ff){bytes[length++]=(char)(0xc0|(scalar>>6));bytes[length++]=(char)(0x80|(scalar&63));}else if(scalar<=0xffff&&(scalar<0xd800||scalar>0xdfff)){bytes[length++]=(char)(0xe0|(scalar>>12));bytes[length++]=(char)(0x80|((scalar>>6)&63));bytes[length++]=(char)(0x80|(scalar&63));}else if(scalar<=0x10ffff){bytes[length++]=(char)(0xf0|(scalar>>18));bytes[length++]=(char)(0x80|((scalar>>12)&63));bytes[length++]=(char)(0x80|((scalar>>6)&63));bytes[length++]=(char)(0x80|(scalar&63));}else{const char *message="char is not a Unicode scalar";*error=disp_owned_bytes(message,strlen(message));return false;}return disp_json_from_string(bytes,length,json,error);}
static bool disp_json_as_char(const disp_native_json *json,uint32_t *scalar,disp_native_string *error){disp_native_string text={0};if(!disp_json_as_text(json,&text,error))return false;const unsigned char *p=(const unsigned char*)text.data;size_t n=text.len,used=0;uint32_t value=0;if(n&&p[0]<0x80){value=p[0];used=1;}else if(n>=2&&(p[0]&0xe0)==0xc0){value=((uint32_t)(p[0]&31)<<6)|(p[1]&63);used=2;}else if(n>=3&&(p[0]&0xf0)==0xe0){value=((uint32_t)(p[0]&15)<<12)|((uint32_t)(p[1]&63)<<6)|(p[2]&63);used=3;}else if(n>=4&&(p[0]&0xf8)==0xf0){value=((uint32_t)(p[0]&7)<<18)|((uint32_t)(p[1]&63)<<12)|((uint32_t)(p[2]&63)<<6)|(p[3]&63);used=4;}bool ok=used==n&&used&&value<=0x10ffff&&(value<0xd800||value>0xdfff);disp_string_drop(&text);if(ok){*scalar=value;return true;}const char *message="JSON char must contain one Unicode scalar";*error=disp_owned_bytes(message,strlen(message));return false;}
static void disp_json_escape_string(disp_native_string *out,const char *text,size_t length){disp_string_push_bytes(out,"\"",1);static const char hex[]="0123456789abcdef";for(size_t i=0;i<length;i++){unsigned char ch=(unsigned char)text[i];switch(ch){case '"':disp_string_push_bytes(out,"\\\"",2);break;case '\\':disp_string_push_bytes(out,"\\\\",2);break;case '\b':disp_string_push_bytes(out,"\\b",2);break;case '\f':disp_string_push_bytes(out,"\\f",2);break;case '\n':disp_string_push_bytes(out,"\\n",2);break;case '\r':disp_string_push_bytes(out,"\\r",2);break;case '\t':disp_string_push_bytes(out,"\\t",2);break;default:if(ch<0x20){char escaped[6]={'\\','u','0','0',hex[ch>>4],hex[ch&15]};disp_string_push_bytes(out,escaped,6);}else disp_string_push_bytes(out,(const char*)&ch,1);}}disp_string_push_bytes(out,"\"",1);}
static bool disp_json_escape_length(const char *text,size_t length,size_t *escaped){size_t result=2;for(size_t i=0;i<length;i++){unsigned char ch=(unsigned char)text[i];size_t add=(ch=='"'||ch=='\\'||ch=='\b'||ch=='\f'||ch=='\n'||ch=='\r'||ch=='\t')?2:(ch<0x20?6:1);if(__builtin_add_overflow(result,add,&result))return false;}*escaped=result;return true;}
static bool disp_json_finish_builder(disp_native_string *builder,disp_native_json *json,disp_native_string *error){if(builder->len>DISP_JSON_LIMIT){disp_string_drop(builder);const char *message="JSON document exceeds the 16 MiB limit";*error=disp_owned_bytes(message,strlen(message));return false;}bool ok=disp_json_parse(builder->data,builder->len,json,error);disp_string_drop(builder);return ok;}
static disp_native_json disp_json_literal(const char *text,size_t length){disp_native_json json={0};json.data=(char*)disp_alloc(length?length:1,1);if(length)memcpy(json.data,text,length);json.len=json.cap=length;return json;}
static disp_native_json disp_json_from_i128(__int128 value){char digits[41];char *at=digits+sizeof(digits);bool negative=value<0;unsigned __int128 magnitude=negative?(unsigned __int128)(-(value+1))+1:(unsigned __int128)value;do{*--at=(char)('0'+magnitude%10);magnitude/=10;}while(magnitude);if(negative)*--at='-';return disp_json_literal(at,(size_t)(digits+sizeof(digits)-at));}
static disp_native_json disp_json_from_u128(unsigned __int128 value){char digits[40];char *at=digits+sizeof(digits);do{*--at=(char)('0'+value%10);value/=10;}while(value);return disp_json_literal(at,(size_t)(digits+sizeof(digits)-at));}
static bool disp_json_from_f64(double value,disp_native_json *json,disp_native_string *error){if(!isfinite(value)){const char *message="JSON cannot represent NaN or infinity";*error=disp_owned_bytes(message,strlen(message));return false;}char text[32];int length=snprintf(text,sizeof(text),"%.17g",value);if(length<0||(size_t)length>=sizeof(text)){const char *message="could not format JSON number";*error=disp_owned_bytes(message,strlen(message));return false;}*json=disp_json_literal(text,(size_t)length);return true;}
static bool disp_json_from_string(const char *text,size_t length,disp_native_json *json,disp_native_string *error){size_t escaped=0;if(!disp_json_escape_length(text,length,&escaped)||escaped>DISP_JSON_LIMIT){const char *message="JSON document exceeds the 16 MiB limit";*error=disp_owned_bytes(message,strlen(message));return false;}disp_native_string builder=disp_string_with_capacity(escaped);disp_json_escape_string(&builder,text,length);return disp_json_finish_builder(&builder,json,error);}
static bool disp_json_from_array(const disp_native_json *values,size_t length,disp_native_json *json,disp_native_string *error){disp_native_string builder={0};disp_string_push_bytes(&builder,"[",1);for(size_t i=0;i<length;i++){size_t additional;if(__builtin_add_overflow(values[i].len,(size_t)(i?2:1),&additional)||additional>DISP_JSON_LIMIT-builder.len){disp_string_drop(&builder);const char *message="JSON document exceeds the 16 MiB limit";*error=disp_owned_bytes(message,strlen(message));return false;}if(i)disp_string_push_bytes(&builder,",",1);disp_string_push_bytes(&builder,values[i].data,values[i].len);}disp_string_push_bytes(&builder,"]",1);return disp_json_finish_builder(&builder,json,error);}
static bool disp_json_from_object(const disp_native_string *keys,const disp_native_json *values,size_t length,disp_native_json *json,disp_native_string *error){disp_native_string builder={0};disp_string_push_bytes(&builder,"{",1);for(size_t i=0;i<length;i++){size_t escaped=0,additional;if(!disp_json_escape_length(keys[i].data,keys[i].len,&escaped)||__builtin_add_overflow(escaped,values[i].len,&additional)||__builtin_add_overflow(additional,(size_t)(i?3:2),&additional)||additional>DISP_JSON_LIMIT-builder.len){disp_string_drop(&builder);const char *message="JSON document exceeds the 16 MiB limit";*error=disp_owned_bytes(message,strlen(message));return false;}if(i)disp_string_push_bytes(&builder,",",1);disp_json_escape_string(&builder,keys[i].data,keys[i].len);disp_string_push_bytes(&builder,":",1);disp_string_push_bytes(&builder,values[i].data,values[i].len);}disp_string_push_bytes(&builder,"}",1);return disp_json_finish_builder(&builder,json,error);}
#if defined(DISP_DATABASE) || defined(DISP_DATA)
typedef struct sqlite3 sqlite3;
typedef struct sqlite3_stmt sqlite3_stmt;
#ifdef DISP_DATABASE
int sqlite3_open_v2(const char*,sqlite3**,int,const char*);int sqlite3_close_v2(sqlite3*);const char *sqlite3_errmsg(sqlite3*);int sqlite3_busy_timeout(sqlite3*,int);int sqlite3_prepare_v2(sqlite3*,const char*,int,sqlite3_stmt**,const char**);int sqlite3_finalize(sqlite3_stmt*);int sqlite3_step(sqlite3_stmt*);int sqlite3_bind_parameter_count(sqlite3_stmt*);int sqlite3_bind_null(sqlite3_stmt*,int);int sqlite3_bind_int64(sqlite3_stmt*,int,int64_t);int sqlite3_bind_double(sqlite3_stmt*,int,double);int sqlite3_bind_text(sqlite3_stmt*,int,const char*,int,void(*)(void*));int sqlite3_column_count(sqlite3_stmt*);const char *sqlite3_column_name(sqlite3_stmt*,int);int sqlite3_column_type(sqlite3_stmt*,int);int64_t sqlite3_column_int64(sqlite3_stmt*,int);double sqlite3_column_double(sqlite3_stmt*,int);const unsigned char *sqlite3_column_text(sqlite3_stmt*,int);int sqlite3_column_bytes(sqlite3_stmt*,int);int sqlite3_changes(sqlite3*);int64_t sqlite3_last_insert_rowid(sqlite3*);int sqlite3_get_autocommit(sqlite3*);int sqlite3_exec(sqlite3*,const char*,int(*)(void*,int,char**,char**),void*,char**);
#endif
#define DISP_SQLITE_OK 0
#define DISP_SQLITE_ROW 100
#define DISP_SQLITE_DONE 101
#define DISP_SQLITE_INTEGER 1
#define DISP_SQLITE_FLOAT 2
#define DISP_SQLITE_TEXT 3
#define DISP_SQLITE_BLOB 4
#define DISP_SQLITE_NULL 5
typedef struct {char *name;size_t name_len;disp_native_string *field_names;disp_native_string *field_types;bool *required;bool *primary;bool *unique;size_t field_count;size_t primary_index;disp_native_json *rows;size_t rows_len;size_t rows_cap;} disp_data_table;
struct disp_database_state {
sqlite3 *handle;bool closed;bool native;bool handle_charged;char *path;size_t path_len;
uint64_t data_generation;bool data_lock_held;
#ifdef _WIN32
HANDLE data_lock;
#else
int data_lock_fd;
#endif
disp_data_table *tables;size_t tables_len;size_t tables_cap;
};
static bool disp_data_store_memory(disp_native_database *output,disp_native_string *error){*output=(disp_native_database){0};*error=(disp_native_string){0};disp_database_state *state=(disp_database_state*)disp_alloc_zeroed(1,sizeof(disp_database_state),_Alignof(disp_database_state));state->native=true;disp_runtime_acquire_handle();state->handle_charged=true;output->state=state;return true;}
static void disp_database_rows_drop(disp_native_json *rows,size_t length){for(size_t i=0;i<length;i++)disp_json_drop(&rows[i]);disp_dealloc(rows);}
#ifdef DISP_DATABASE
static disp_native_string disp_database_error(disp_database_state *state,const char *fallback){const char *message=state&&state->handle?sqlite3_errmsg(state->handle):fallback;return disp_owned_bytes(message?message:fallback,strlen(message?message:fallback));}
static bool disp_database_require(disp_database_state *state,disp_native_string *error){if(state&&!state->closed&&!state->native&&state->handle)return true;const char *message=state&&state->native?"raw SQL is unavailable on a DISP DataStore":"database is closed";*error=disp_owned_bytes(message,strlen(message));return false;}
static bool disp_database_open(const char *path,size_t length,disp_native_database *output,disp_native_string *error){*output=(disp_native_database){0};*error=(disp_native_string){0};if(!length||length>32768||memchr(path,0,length)){const char *message="database path must be non-empty UTF-8 without NUL";*error=disp_owned_bytes(message,strlen(message));return false;}char *terminated=(char*)disp_alloc(length+1,1);memcpy(terminated,path,length);terminated[length]=0;disp_database_state *state=(disp_database_state*)disp_alloc_zeroed(1,sizeof(disp_database_state),_Alignof(disp_database_state));int code=sqlite3_open_v2(terminated,&state->handle,0x2|0x4|0x10000,NULL);disp_dealloc(terminated);if(code!=DISP_SQLITE_OK){*error=disp_database_error(state,"could not open database");if(state->handle)sqlite3_close_v2(state->handle);disp_dealloc(state);return false;}if(sqlite3_busy_timeout(state->handle,5000)!=DISP_SQLITE_OK||sqlite3_exec(state->handle,"PRAGMA foreign_keys=ON",NULL,NULL,NULL)!=DISP_SQLITE_OK){*error=disp_database_error(state,"could not configure database safety defaults");sqlite3_close_v2(state->handle);disp_dealloc(state);return false;}disp_runtime_acquire_handle();state->handle_charged=true;output->state=state;return true;}
static bool disp_database_memory(disp_native_database *output,disp_native_string *error){return disp_database_open(":memory:",8,output,error);}
static bool disp_database_prepare(disp_database_state *state,const char *sql,size_t length,sqlite3_stmt **statement,disp_native_string *error){*statement=NULL;if(!disp_database_require(state,error))return false;if(!length||length>DISP_DATABASE_SQL_LIMIT||memchr(sql,0,length)){const char *message="SQL must be non-empty, at most 1 MiB, and contain no NUL";*error=disp_owned_bytes(message,strlen(message));return false;}if(length>INT_MAX){const char *message="SQL is too large";*error=disp_owned_bytes(message,strlen(message));return false;}const char *tail=NULL;int code=sqlite3_prepare_v2(state->handle,sql,(int)length,statement,&tail);if(code!=DISP_SQLITE_OK||!*statement){*error=disp_database_error(state,"could not prepare SQL");if(*statement)sqlite3_finalize(*statement);*statement=NULL;return false;}const char *end=sql+length;while(tail<end&&isspace((unsigned char)*tail))tail++;if(tail!=end){sqlite3_finalize(*statement);*statement=NULL;const char *message="exactly one SQL statement is allowed";*error=disp_owned_bytes(message,strlen(message));return false;}return true;}
static bool disp_database_bind(sqlite3_stmt *statement,const disp_native_json *parameters,size_t length,disp_native_string *error){int expected=sqlite3_bind_parameter_count(statement);if(expected<0||(size_t)expected!=length){*error=disp_owned_bytes("SQL parameter count does not match bound values",47);return false;}for(size_t i=0;i<length;i++){const disp_native_json *value=&parameters[i];const char *kind=disp_json_kind_name(value);int code=DISP_SQLITE_OK;if(!strcmp(kind,"null"))code=sqlite3_bind_null(statement,(int)i+1);else if(!strcmp(kind,"bool")){bool boolean=false;disp_native_string conversion={0};if(!disp_json_as_bool(value,&boolean,&conversion)){*error=conversion;return false;}code=sqlite3_bind_int64(statement,(int)i+1,boolean?1:0);}else if(!strcmp(kind,"number")){bool floating=memchr(value->data,'.',value->len)||memchr(value->data,'e',value->len)||memchr(value->data,'E',value->len);disp_native_string conversion={0};if(floating){double number=0;if(!disp_json_as_f64(value,&number,&conversion)){*error=conversion;return false;}code=sqlite3_bind_double(statement,(int)i+1,number);}else{int64_t number=0;if(!disp_json_as_int(value,&number,&conversion)){*error=conversion;return false;}code=sqlite3_bind_int64(statement,(int)i+1,number);}}else{disp_native_string text={0};if(!strcmp(kind,"string")){disp_native_string conversion={0};if(!disp_json_as_text(value,&text,&conversion)){*error=conversion;return false;}}else text=disp_owned_bytes(value->data,value->len);if(text.len>INT_MAX){disp_string_drop(&text);*error=disp_owned_bytes("database parameter is too large",31);return false;}code=sqlite3_bind_text(statement,(int)i+1,text.data,(int)text.len,(void(*)(void*))-1);disp_string_drop(&text);}if(code!=DISP_SQLITE_OK){*error=disp_owned_bytes("could not bind SQL parameter",28);return false;}}return true;}
static bool disp_database_execute(disp_database_state *state,const char *sql,size_t sql_len,const disp_native_json *parameters,size_t parameters_len,uint64_t *changes,disp_native_string *error){*changes=0;sqlite3_stmt *statement=NULL;if(!disp_database_prepare(state,sql,sql_len,&statement,error))return false;if(!disp_database_bind(statement,parameters,parameters_len,error)){sqlite3_finalize(statement);return false;}int code=sqlite3_step(statement);if(code==DISP_SQLITE_ROW){*error=disp_owned_bytes("execute cannot discard query rows; use query",44);sqlite3_finalize(statement);return false;}if(code!=DISP_SQLITE_DONE){*error=disp_database_error(state,"SQL execution failed");sqlite3_finalize(statement);return false;}*changes=(uint64_t)sqlite3_changes(state->handle);code=sqlite3_finalize(statement);if(code!=DISP_SQLITE_OK){*error=disp_database_error(state,"could not finalize SQL");return false;}return true;}
static bool disp_database_column(sqlite3_stmt *statement,int column,disp_native_json *value,disp_native_string *error){int kind=sqlite3_column_type(statement,column);if(kind==DISP_SQLITE_NULL){*value=disp_json_literal("null",4);return true;}if(kind==DISP_SQLITE_INTEGER){*value=disp_json_from_i128((__int128)sqlite3_column_int64(statement,column));return true;}if(kind==DISP_SQLITE_FLOAT)return disp_json_from_f64(sqlite3_column_double(statement,column),value,error);if(kind==DISP_SQLITE_BLOB){*error=disp_owned_bytes("SQLite BLOB columns require an explicit byte API",48);return false;}if(kind!=DISP_SQLITE_TEXT){*error=disp_owned_bytes("SQLite returned an unsupported column type",42);return false;}const unsigned char *text=sqlite3_column_text(statement,column);int length=sqlite3_column_bytes(statement,column);if(length<0||(!text&&length)){*error=disp_owned_bytes("could not read SQLite text column",33);return false;}if(!disp_utf8_valid((const char*)text,(size_t)length)){*error=disp_owned_bytes("SQLite text column is not valid UTF-8",37);return false;}return disp_json_from_string((const char*)text,(size_t)length,value,error);}
static bool disp_database_query(disp_database_state *state,const char *sql,size_t sql_len,const disp_native_json *parameters,size_t parameters_len,disp_native_json **rows,size_t *rows_len,size_t *rows_cap,disp_native_string *error){*rows=NULL;*rows_len=*rows_cap=0;sqlite3_stmt *statement=NULL;if(!disp_database_prepare(state,sql,sql_len,&statement,error))return false;if(!disp_database_bind(statement,parameters,parameters_len,error)){sqlite3_finalize(statement);return false;}int columns=sqlite3_column_count(statement);if(columns<0||columns>DISP_DATABASE_COLUMN_LIMIT){*error=disp_owned_bytes("query column count exceeds 4096",31);sqlite3_finalize(statement);return false;}size_t total=0;for(;;){int code=sqlite3_step(statement);if(code==DISP_SQLITE_DONE)break;if(code!=DISP_SQLITE_ROW){*error=disp_database_error(state,"SQL query failed");goto fail;}if(*rows_len>=DISP_DATABASE_ROW_LIMIT){*error=disp_owned_bytes("query exceeds the 100000-row limit",34);goto fail;}disp_native_string *keys=(disp_native_string*)disp_alloc_zeroed((size_t)columns,sizeof(disp_native_string),_Alignof(disp_native_string));disp_native_json *values=(disp_native_json*)disp_alloc_zeroed((size_t)columns,sizeof(disp_native_json),_Alignof(disp_native_json));bool valid=true;for(int column=0;column<columns;column++){const char *name=sqlite3_column_name(statement,column);if(!name||!disp_utf8_valid(name,strlen(name))){*error=disp_owned_bytes("SQLite column name is not valid UTF-8",37);valid=false;break;}keys[column]=disp_owned_bytes(name,strlen(name));for(int prior=0;prior<column;prior++)if(keys[prior].len==keys[column].len&&!memcmp(keys[prior].data,keys[column].data,keys[column].len)){*error=disp_owned_bytes("query contains duplicate column names",37);valid=false;break;}if(!valid||!disp_database_column(statement,column,&values[column],error)){valid=false;break;}}disp_native_json row={0};if(valid)valid=disp_json_from_object(keys,values,(size_t)columns,&row,error);for(int column=0;column<columns;column++){disp_string_drop(&keys[column]);disp_json_drop(&values[column]);}disp_dealloc(keys);disp_dealloc(values);if(!valid)goto fail;if(row.len>DISP_JSON_LIMIT-total){disp_json_drop(&row);*error=disp_owned_bytes("query JSON output exceeds the 16 MiB limit",42);goto fail;}total+=row.len;if(*rows_len==*rows_cap){size_t capacity=*rows_cap?*rows_cap*2:8;*rows=(disp_native_json*)disp_realloc(*rows,capacity*sizeof(disp_native_json),_Alignof(disp_native_json));*rows_cap=capacity;}(*rows)[(*rows_len)++]=row;}if(sqlite3_finalize(statement)!=DISP_SQLITE_OK){*error=disp_database_error(state,"could not finalize SQL");disp_database_rows_drop(*rows,*rows_len);*rows=NULL;*rows_len=*rows_cap=0;return false;}return true;fail:sqlite3_finalize(statement);disp_database_rows_drop(*rows,*rows_len);*rows=NULL;*rows_len=*rows_cap=0;return false;}
static bool disp_database_control(disp_database_state *state,const char *sql,bool expected_transaction,disp_native_string *error){if(!disp_database_require(state,error))return false;bool transaction=sqlite3_get_autocommit(state->handle)==0;if(transaction!=expected_transaction){*error=disp_owned_bytes(expected_transaction?"database has no active transaction":"database transaction is already active",expected_transaction?34:38);return false;}uint64_t ignored=0;return disp_database_execute(state,sql,strlen(sql),NULL,0,&ignored,error);}
#else
static bool disp_database_execute(disp_database_state *state,const char *sql,size_t sql_len,const disp_native_json *parameters,size_t parameters_len,uint64_t *changes,disp_native_string *error){(void)state;(void)sql;(void)sql_len;(void)parameters;(void)parameters_len;*changes=0;const char *message="raw SQL support is unavailable in this program";*error=disp_owned_bytes(message,strlen(message));return false;}
static bool disp_database_query(disp_database_state *state,const char *sql,size_t sql_len,const disp_native_json *parameters,size_t parameters_len,disp_native_json **rows,size_t *rows_len,size_t *rows_cap,disp_native_string *error){(void)state;(void)sql;(void)sql_len;(void)parameters;(void)parameters_len;*rows=NULL;*rows_len=*rows_cap=0;const char *message="raw SQL support is unavailable in this program";*error=disp_owned_bytes(message,strlen(message));return false;}
#endif
static void disp_data_table_drop(disp_data_table *table){disp_dealloc(table->name);for(size_t i=0;i<table->field_count;i++){disp_string_drop(&table->field_names[i]);disp_string_drop(&table->field_types[i]);}disp_dealloc(table->field_names);disp_dealloc(table->field_types);disp_dealloc(table->required);disp_dealloc(table->primary);disp_dealloc(table->unique);for(size_t i=0;i<table->rows_len;i++)disp_json_drop(&table->rows[i]);disp_dealloc(table->rows);*table=(disp_data_table){0};}
static void disp_data_native_drop(disp_database_state *state){for(size_t i=0;i<state->tables_len;i++)disp_data_table_drop(&state->tables[i]);disp_dealloc(state->tables);state->tables=NULL;state->tables_len=state->tables_cap=0;}
/* DISPDB is DISP's bounded, versioned native data format.  The Rust interpreter
   and this native runtime intentionally implement the same byte-level contract. */
#define DISP_DATA_FILE_LIMIT (64u*1024u*1024u)
#define DISP_DATA_NAME_LIMIT 1024u
#define DISP_DATA_TYPE_LIMIT 128u
#define DISP_DATA_HEADER_SIZE 32u
#define DISP_DATA_PAGE_SIZE 4096u
#define DISP_DATA_PAGE_HEADER 32u
#define DISP_DATA_PAGE_PAYLOAD (DISP_DATA_PAGE_SIZE-DISP_DATA_PAGE_HEADER)
#define DISP_DATA_MAX_PAGES (DISP_DATA_FILE_LIMIT/DISP_DATA_PAGE_SIZE)
#define DISP_DATA_WAL_HEADER 64u
#define DISP_DATA_WAL_RECORD (8u+DISP_DATA_PAGE_SIZE)
#define DISP_DATA_WAL_LIMIT (DISP_DATA_WAL_HEADER+DISP_DATA_MAX_PAGES*DISP_DATA_WAL_RECORD)
typedef struct {unsigned char *data;size_t len;size_t cap;} disp_data_buffer;
typedef struct {const unsigned char *at;const unsigned char *end;} disp_data_reader;
static uint64_t disp_data_temporary_counter;
static bool disp_data_fail(disp_native_string *error,const char *message){*error=disp_owned_bytes(message,strlen(message));return false;}
static bool disp_data_buffer_append(disp_data_buffer *buffer,const void *bytes,size_t count,disp_native_string *error){
if(count>DISP_DATA_FILE_LIMIT-buffer->len)return disp_data_fail(error,"DISP Data snapshot exceeds 64 MiB");
size_t needed=buffer->len+count;if(needed>buffer->cap){size_t capacity=buffer->cap?buffer->cap:256;while(capacity<needed){if(capacity>DISP_DATA_FILE_LIMIT/2){capacity=DISP_DATA_FILE_LIMIT;break;}capacity*=2;}buffer->data=(unsigned char*)disp_realloc(buffer->data,capacity,1);buffer->cap=capacity;}if(count)memcpy(buffer->data+buffer->len,bytes,count);buffer->len=needed;return true;}
static bool disp_data_buffer_u8(disp_data_buffer *buffer,uint8_t value,disp_native_string *error){return disp_data_buffer_append(buffer,&value,1,error);}
static bool disp_data_buffer_u32(disp_data_buffer *buffer,uint32_t value,disp_native_string *error){unsigned char bytes[4]={(unsigned char)value,(unsigned char)(value>>8),(unsigned char)(value>>16),(unsigned char)(value>>24)};return disp_data_buffer_append(buffer,bytes,4,error);}
static bool disp_data_buffer_u64(disp_data_buffer *buffer,uint64_t value,disp_native_string *error){unsigned char bytes[8];for(size_t i=0;i<8;i++)bytes[i]=(unsigned char)(value>>(i*8));return disp_data_buffer_append(buffer,bytes,8,error);}
static bool disp_data_buffer_bytes(disp_data_buffer *buffer,const char *bytes,size_t count,size_t limit,const char *context,disp_native_string *error){if(count>limit){char message[128];snprintf(message,sizeof(message),"%s exceeds its storage limit",context);return disp_data_fail(error,message);}return disp_data_buffer_u32(buffer,(uint32_t)count,error)&&disp_data_buffer_append(buffer,bytes,count,error);}
static uint64_t disp_data_checksum(const unsigned char *bytes,size_t count){uint64_t hash=UINT64_C(0xcbf29ce484222325);for(size_t i=0;i<count;i++){hash^=bytes[i];hash*=UINT64_C(0x100000001b3);}return hash;}
static uint32_t disp_data_at_u32(const unsigned char *bytes){return (uint32_t)bytes[0]|((uint32_t)bytes[1]<<8)|((uint32_t)bytes[2]<<16)|((uint32_t)bytes[3]<<24);}
static uint64_t disp_data_at_u64(const unsigned char *bytes){uint64_t value=0;for(size_t i=0;i<8;i++)value|=(uint64_t)bytes[i]<<(i*8);return value;}
static void disp_data_put_u32(unsigned char *bytes,uint32_t value){for(size_t i=0;i<4;i++)bytes[i]=(unsigned char)(value>>(i*8));}
static void disp_data_put_u64(unsigned char *bytes,uint64_t value){for(size_t i=0;i<8;i++)bytes[i]=(unsigned char)(value>>(i*8));}
static int disp_data_table_compare(const void *left,const void *right){const disp_data_table *a=*(disp_data_table*const*)left,*b=*(disp_data_table*const*)right;size_t common=a->name_len<b->name_len?a->name_len:b->name_len;int order=common?memcmp(a->name,b->name,common):0;if(order)return order;return a->name_len<b->name_len?-1:a->name_len>b->name_len?1:0;}
static bool disp_data_encode(disp_database_state *state,disp_data_buffer *payload,disp_native_string *error){
if(state->tables_len>4096)return disp_data_fail(error,"DISP Data snapshot exceeds 4096 tables");
disp_data_table **tables=state->tables_len?(disp_data_table**)disp_alloc(state->tables_len*sizeof(disp_data_table*),_Alignof(disp_data_table*)):NULL;for(size_t i=0;i<state->tables_len;i++)tables[i]=&state->tables[i];qsort(tables,state->tables_len,sizeof(disp_data_table*),disp_data_table_compare);
bool ok=disp_data_buffer_u32(payload,(uint32_t)state->tables_len,error);for(size_t t=0;ok&&t<state->tables_len;t++){disp_data_table *table=tables[t];if(t&&disp_data_table_compare(&tables[t-1],&tables[t])==0){ok=disp_data_fail(error,"DISP Data snapshot contains a duplicate table");break;}if(!table->field_count||table->field_count>4096||table->rows_len>DISP_DATABASE_ROW_LIMIT){ok=disp_data_fail(error,"DISP Data table exceeds storage safety limits");break;}ok=disp_data_buffer_bytes(payload,table->name,table->name_len,DISP_DATA_NAME_LIMIT,"table name",error)&&disp_data_buffer_u32(payload,(uint32_t)table->field_count,error);size_t primary_count=0;for(size_t f=0;ok&&f<table->field_count;f++){for(size_t prior=0;prior<f;prior++)if(table->field_names[prior].len==table->field_names[f].len&&!memcmp(table->field_names[prior].data,table->field_names[f].data,table->field_names[f].len)){ok=disp_data_fail(error,"DISP Data table contains a duplicate field");break;}if(!ok)break;ok=disp_data_buffer_bytes(payload,table->field_names[f].data,table->field_names[f].len,DISP_DATA_NAME_LIMIT,"field name",error)&&disp_data_buffer_bytes(payload,table->field_types[f].data,table->field_types[f].len,DISP_DATA_TYPE_LIMIT,"field storage type",error)&&disp_data_buffer_u8(payload,(uint8_t)((table->required[f]?0:1)|(table->primary[f]?2:0)|(table->unique[f]?4:0)),error);if(table->primary[f])primary_count++;}if(ok&&primary_count!=1)ok=disp_data_fail(error,"DISP Data table must contain exactly one primary field");if(ok)ok=disp_data_buffer_u64(payload,(uint64_t)table->rows_len,error);for(size_t r=0;ok&&r<table->rows_len;r++)ok=disp_data_buffer_bytes(payload,table->rows[r].data,table->rows[r].len,DISP_JSON_LIMIT,"stored row",error);}
disp_dealloc(tables);return ok;}
static bool disp_data_take(disp_data_reader *reader,size_t count,const unsigned char **bytes,disp_native_string *error){if(count>(size_t)(reader->end-reader->at))return disp_data_fail(error,"DISP Data snapshot is truncated");*bytes=reader->at;reader->at+=count;return true;}
static bool disp_data_read_u8(disp_data_reader *reader,uint8_t *value,disp_native_string *error){const unsigned char *bytes;if(!disp_data_take(reader,1,&bytes,error))return false;*value=bytes[0];return true;}
static bool disp_data_read_u32(disp_data_reader *reader,uint32_t *value,disp_native_string *error){const unsigned char *bytes;if(!disp_data_take(reader,4,&bytes,error))return false;*value=(uint32_t)bytes[0]|((uint32_t)bytes[1]<<8)|((uint32_t)bytes[2]<<16)|((uint32_t)bytes[3]<<24);return true;}
static bool disp_data_read_u64(disp_data_reader *reader,uint64_t *value,disp_native_string *error){const unsigned char *bytes;if(!disp_data_take(reader,8,&bytes,error))return false;uint64_t result=0;for(size_t i=0;i<8;i++)result|=(uint64_t)bytes[i]<<(i*8);*value=result;return true;}
static bool disp_data_read_bytes(disp_data_reader *reader,size_t limit,const char **bytes,size_t *count,const char *context,disp_native_string *error){uint32_t length;if(!disp_data_read_u32(reader,&length,error))return false;if(length>limit){char message[128];snprintf(message,sizeof(message),"%s exceeds its storage limit",context);return disp_data_fail(error,message);}const unsigned char *value;if(!disp_data_take(reader,length,&value,error))return false;*bytes=(const char*)value;*count=length;return true;}
static bool disp_data_decode(disp_database_state *state,const unsigned char *bytes,size_t length,bool supports_unique,disp_native_string *error){
static const unsigned char magic[8]={'D','I','S','P','D','B',0x1a,'\n'};if(length<DISP_DATA_HEADER_SIZE)return disp_data_fail(error,"DISP Data snapshot header is truncated");if(length>DISP_DATA_FILE_LIMIT)return disp_data_fail(error,"DISP Data snapshot exceeds 64 MiB");if(memcmp(bytes,magic,8))return disp_data_fail(error,"file is not a DISP Data snapshot");disp_data_reader header={bytes+8,bytes+DISP_DATA_HEADER_SIZE};uint32_t version,flags;uint64_t payload_length,checksum;if(!disp_data_read_u32(&header,&version,error)||!disp_data_read_u32(&header,&flags,error)||!disp_data_read_u64(&header,&payload_length,error)||!disp_data_read_u64(&header,&checksum,error))return false;if(version!=1)return disp_data_fail(error,"unsupported DISP Data snapshot version");if(flags)return disp_data_fail(error,"DISP Data snapshot uses unknown required flags");if(payload_length!=length-DISP_DATA_HEADER_SIZE)return disp_data_fail(error,"DISP Data snapshot length is inconsistent");if(disp_data_checksum(bytes+DISP_DATA_HEADER_SIZE,(size_t)payload_length)!=checksum)return disp_data_fail(error,"DISP Data snapshot integrity check failed");
disp_data_reader reader={bytes+DISP_DATA_HEADER_SIZE,bytes+length};uint32_t table_count;if(!disp_data_read_u32(&reader,&table_count,error)||table_count>4096)return error->len?false:disp_data_fail(error,"DISP Data snapshot exceeds 4096 tables");if(table_count){state->tables=(disp_data_table*)disp_alloc_zeroed(table_count,sizeof(disp_data_table),_Alignof(disp_data_table));state->tables_cap=table_count;}for(uint32_t t=0;t<table_count;t++){const char *name;size_t name_len;if(!disp_data_read_bytes(&reader,DISP_DATA_NAME_LIMIT,&name,&name_len,"table name",error)||!disp_utf8_valid(name,name_len))goto fail;for(size_t prior=0;prior<state->tables_len;prior++)if(state->tables[prior].name_len==name_len&&!memcmp(state->tables[prior].name,name,name_len)){disp_data_fail(error,"DISP Data snapshot contains a duplicate table");goto fail;}disp_data_table *table=&state->tables[state->tables_len++];table->name=(char*)disp_alloc(name_len?name_len:1,1);if(name_len)memcpy(table->name,name,name_len);table->name_len=name_len;uint32_t field_count;if(!disp_data_read_u32(&reader,&field_count,error)||!field_count||field_count>4096){if(!error->len)disp_data_fail(error,"DISP Data table must contain between 1 and 4096 fields");goto fail;}table->field_count=field_count;table->primary_index=SIZE_MAX;table->field_names=(disp_native_string*)disp_alloc_zeroed(field_count,sizeof(disp_native_string),_Alignof(disp_native_string));table->field_types=(disp_native_string*)disp_alloc_zeroed(field_count,sizeof(disp_native_string),_Alignof(disp_native_string));table->required=(bool*)disp_alloc(field_count,sizeof(bool));table->primary=(bool*)disp_alloc(field_count,sizeof(bool));table->unique=(bool*)disp_alloc(field_count,sizeof(bool));size_t primary_count=0;for(uint32_t f=0;f<field_count;f++){const char *field_name,*field_type;size_t field_name_len,field_type_len;uint8_t field_flags;if(!disp_data_read_bytes(&reader,DISP_DATA_NAME_LIMIT,&field_name,&field_name_len,"field name",error)||!disp_utf8_valid(field_name,field_name_len)||!disp_data_read_bytes(&reader,DISP_DATA_TYPE_LIMIT,&field_type,&field_type_len,"field storage type",error)||!disp_utf8_valid(field_type,field_type_len)||!disp_data_read_u8(&reader,&field_flags,error))goto fail;if(field_flags&~(supports_unique?7u:3u)){disp_data_fail(error,"DISP Data field uses unknown required flags");goto fail;}for(uint32_t prior=0;prior<f;prior++)if(table->field_names[prior].len==field_name_len&&!memcmp(table->field_names[prior].data,field_name,field_name_len)){disp_data_fail(error,"DISP Data table contains a duplicate field");goto fail;}table->field_names[f]=disp_owned_bytes(field_name,field_name_len);table->field_types[f]=disp_owned_bytes(field_type,field_type_len);table->required[f]=(field_flags&1u)==0;table->primary[f]=(field_flags&2u)!=0;table->unique[f]=(field_flags&4u)!=0;if(table->primary[f]){primary_count++;table->primary_index=f;}}if(primary_count!=1){disp_data_fail(error,"DISP Data table must contain exactly one primary field");goto fail;}uint64_t row_count;if(!disp_data_read_u64(&reader,&row_count,error)||row_count>DISP_DATABASE_ROW_LIMIT||row_count>SIZE_MAX/sizeof(disp_native_json)){if(!error->len)disp_data_fail(error,"DISP Data table exceeds the 100000-row limit");goto fail;}if(row_count){table->rows=(disp_native_json*)disp_alloc_zeroed((size_t)row_count,sizeof(disp_native_json),_Alignof(disp_native_json));table->rows_cap=(size_t)row_count;}for(uint64_t r=0;r<row_count;r++){const char *row;size_t row_len;if(!disp_data_read_bytes(&reader,DISP_JSON_LIMIT,&row,&row_len,"stored row",error)||!disp_json_parse(row,row_len,&table->rows[table->rows_len],error))goto fail;table->rows_len++;}}
if(reader.at!=reader.end){disp_data_fail(error,"DISP Data snapshot contains trailing payload bytes");goto fail;}return true;fail:disp_data_native_drop(state);return false;}
static bool disp_data_encode_pages(disp_database_state *state,uint64_t generation,disp_data_buffer *pages,disp_native_string *error){
disp_data_buffer payload={0};if(!disp_data_encode(state,&payload,error))return false;size_t data_pages=(payload.len+DISP_DATA_PAGE_PAYLOAD-1)/DISP_DATA_PAGE_PAYLOAD;if(!data_pages)data_pages=1;size_t page_count=data_pages+1;if(page_count>DISP_DATA_MAX_PAGES){disp_dealloc(payload.data);return disp_data_fail(error,"DISP Data page count exceeds its storage limit");}size_t length=page_count*DISP_DATA_PAGE_SIZE;pages->data=(unsigned char*)disp_alloc_zeroed(1,length,1);pages->len=pages->cap=length;static const unsigned char magic[8]={'D','I','S','P','D','B',0x1a,'\n'};memcpy(pages->data,magic,8);disp_data_put_u32(pages->data+8,3);disp_data_put_u32(pages->data+12,DISP_DATA_PAGE_SIZE);disp_data_put_u64(pages->data+16,generation);disp_data_put_u64(pages->data+24,payload.len);disp_data_put_u64(pages->data+32,page_count);disp_data_put_u64(pages->data+40,disp_data_checksum(payload.data,payload.len));disp_data_put_u64(pages->data+48,0);disp_data_put_u64(pages->data+56,disp_data_checksum(pages->data,56));for(size_t page=0;page<data_pages;page++){size_t logical=page*DISP_DATA_PAGE_PAYLOAD,used=payload.len-logical;if(used>DISP_DATA_PAGE_PAYLOAD)used=DISP_DATA_PAGE_PAYLOAD;unsigned char *target=pages->data+(page+1)*DISP_DATA_PAGE_SIZE;target[0]=1;disp_data_put_u32(target+4,(uint32_t)(page+1));disp_data_put_u32(target+8,(uint32_t)used);disp_data_put_u32(target+12,page+1==data_pages?0:(uint32_t)(page+2));disp_data_put_u64(target+16,disp_data_checksum(payload.data+logical,used));if(used)memcpy(target+DISP_DATA_PAGE_HEADER,payload.data+logical,used);}disp_dealloc(payload.data);return true;}
static bool disp_data_decode_any(disp_database_state *state,const unsigned char *bytes,size_t length,disp_native_string *error){
if(length<12)return disp_data_fail(error,"DISP Data snapshot header is truncated");uint32_t version=disp_data_at_u32(bytes+8);if(version==1){state->data_generation=0;return disp_data_decode(state,bytes,length,false,error);}if(version!=2&&version!=3)return disp_data_fail(error,"unsupported DISP Data snapshot version");if(length<DISP_DATA_PAGE_SIZE||length>DISP_DATA_FILE_LIMIT)return disp_data_fail(error,"DISP Data page file has an invalid size");static const unsigned char magic[8]={'D','I','S','P','D','B',0x1a,'\n'};if(memcmp(bytes,magic,8))return disp_data_fail(error,"file is not a DISP Data snapshot");if(disp_data_at_u32(bytes+12)!=DISP_DATA_PAGE_SIZE)return disp_data_fail(error,"DISP Data snapshot uses an unsupported page size");if(disp_data_at_u64(bytes+48)||disp_data_checksum(bytes,56)!=disp_data_at_u64(bytes+56))return disp_data_fail(error,"DISP Data page header integrity check failed");for(size_t i=64;i<DISP_DATA_PAGE_SIZE;i++)if(bytes[i])return disp_data_fail(error,"DISP Data page header contains unknown metadata");uint64_t payload_u64=disp_data_at_u64(bytes+24),pages_u64=disp_data_at_u64(bytes+32);if(payload_u64>SIZE_MAX||pages_u64>SIZE_MAX)return disp_data_fail(error,"DISP Data page metadata does not fit this target");size_t payload_len=(size_t)payload_u64,page_count=(size_t)pages_u64;if(page_count<2||page_count>DISP_DATA_MAX_PAGES||length!=page_count*DISP_DATA_PAGE_SIZE||payload_len>(page_count-1)*DISP_DATA_PAGE_PAYLOAD)return disp_data_fail(error,"DISP Data page count is inconsistent");unsigned char *payload=payload_len?(unsigned char*)disp_alloc(payload_len,1):NULL;size_t copied=0;for(size_t page=1;page<page_count;page++){const unsigned char *source=bytes+page*DISP_DATA_PAGE_SIZE;if(source[0]!=1||source[1]||source[2]||source[3]||disp_data_at_u32(source+4)!=page)return disp_dealloc(payload),disp_data_fail(error,"DISP Data page has an invalid type or identity");size_t used=disp_data_at_u32(source+8),remaining=payload_len-copied,expected=remaining<DISP_DATA_PAGE_PAYLOAD?remaining:DISP_DATA_PAGE_PAYLOAD;uint32_t next=page+1==page_count?0:(uint32_t)(page+1);if(used!=expected||disp_data_at_u32(source+12)!=next||disp_data_at_u64(source+24))return disp_dealloc(payload),disp_data_fail(error,"DISP Data page chain is inconsistent");const unsigned char *data=source+DISP_DATA_PAGE_HEADER;if(disp_data_checksum(data,used)!=disp_data_at_u64(source+16))return disp_dealloc(payload),disp_data_fail(error,"DISP Data page integrity check failed");for(size_t i=DISP_DATA_PAGE_HEADER+used;i<DISP_DATA_PAGE_SIZE;i++)if(source[i])return disp_dealloc(payload),disp_data_fail(error,"DISP Data page contains non-zero unused bytes");if(used)memcpy(payload+copied,data,used);copied+=used;}if(disp_data_checksum(payload,payload_len)!=disp_data_at_u64(bytes+40)){disp_dealloc(payload);return disp_data_fail(error,"DISP Data payload integrity check failed");}size_t legacy_len=DISP_DATA_HEADER_SIZE+payload_len;unsigned char *legacy=(unsigned char*)disp_alloc(legacy_len,1);memcpy(legacy,magic,8);disp_data_put_u32(legacy+8,1);disp_data_put_u32(legacy+12,0);disp_data_put_u64(legacy+16,payload_len);disp_data_put_u64(legacy+24,disp_data_checksum(payload,payload_len));if(payload_len)memcpy(legacy+DISP_DATA_HEADER_SIZE,payload,payload_len);disp_dealloc(payload);bool ok=disp_data_decode(state,legacy,legacy_len,version>=3,error);disp_dealloc(legacy);if(ok)state->data_generation=disp_data_at_u64(bytes+16);return ok;}
static char *disp_data_suffix(const char *path,size_t path_len,const char *suffix){size_t suffix_len=strlen(suffix);if(path_len>SIZE_MAX-suffix_len-1)return NULL;char *result=(char*)disp_alloc(path_len+suffix_len+1,1);memcpy(result,path,path_len);memcpy(result+path_len,suffix,suffix_len+1);return result;}
static bool disp_data_lock_acquire(disp_database_state *state,disp_native_string *error){char *lock_path=disp_data_suffix(state->path,state->path_len,".lock");if(!lock_path)return disp_data_fail(error,"DISP Data lock path is too long");
#ifdef _WIN32
if(strlen(lock_path)>INT_MAX){disp_dealloc(lock_path);return disp_data_fail(error,"DISP Data lock path is too long");}int wide_count=MultiByteToWideChar(CP_UTF8,MB_ERR_INVALID_CHARS,lock_path,-1,NULL,0);wchar_t *wide=wide_count>0?(wchar_t*)disp_alloc((size_t)wide_count*sizeof(wchar_t),_Alignof(wchar_t)):NULL;if(!wide||MultiByteToWideChar(CP_UTF8,MB_ERR_INVALID_CHARS,lock_path,-1,wide,wide_count)!=wide_count){disp_dealloc(wide);disp_dealloc(lock_path);return disp_data_fail(error,"DISP Data lock path is not valid UTF-8");}HANDLE handle=CreateFileW(wide,GENERIC_READ|GENERIC_WRITE,0,NULL,OPEN_ALWAYS,FILE_ATTRIBUTE_NORMAL,NULL);disp_dealloc(wide);if(handle==INVALID_HANDLE_VALUE){DWORD code=GetLastError();char message[128];snprintf(message,sizeof(message),"DISP Data store is already open or unavailable (Windows error %lu)",(unsigned long)code);disp_dealloc(lock_path);return disp_data_fail(error,message);}state->data_lock=handle;
#else
int descriptor=open(lock_path,O_RDWR|O_CREAT,0600);if(descriptor<0||flock(descriptor,LOCK_EX|LOCK_NB)!=0){int cause=errno;if(descriptor>=0)close(descriptor);errno=cause;*error=disp_owned_bytes(strerror(errno),strlen(strerror(errno)));disp_dealloc(lock_path);return false;}state->data_lock_fd=descriptor;
#endif
state->data_lock_held=true;disp_runtime_acquire_handle();state->handle_charged=true;disp_dealloc(lock_path);return true;}
static void disp_data_lock_release(disp_database_state *state){if(!state->data_lock_held){if(state->handle_charged){disp_runtime_release_handle();state->handle_charged=false;}return;}
#ifdef _WIN32
CloseHandle(state->data_lock);state->data_lock=NULL;
#else
close(state->data_lock_fd);state->data_lock_fd=-1;
#endif
state->data_lock_held=false;if(state->handle_charged){disp_runtime_release_handle();state->handle_charged=false;}}
static bool disp_data_read_file_limit(const char *path,size_t limit,unsigned char **bytes,size_t *length,disp_native_string *error){FILE *file=fopen(path,"rb");if(!file)return false;if(fseek(file,0,SEEK_END)||ftell(file)<0){*error=disp_owned_bytes(strerror(errno),strlen(strerror(errno)));fclose(file);return false;}long end=ftell(file);if((uint64_t)end>limit){fclose(file);return disp_data_fail(error,"DISP Data file exceeds its storage limit");}rewind(file);*length=(size_t)end;*bytes=*length?(unsigned char*)disp_alloc(*length,1):NULL;if(*length&&fread(*bytes,1,*length,file)!=*length){*error=disp_owned_bytes(strerror(errno),strlen(strerror(errno)));disp_dealloc(*bytes);*bytes=NULL;fclose(file);return false;}if(fclose(file)){*error=disp_owned_bytes(strerror(errno),strlen(strerror(errno)));disp_dealloc(*bytes);*bytes=NULL;return false;}return true;}
static bool disp_data_read_file(const char *path,unsigned char **bytes,size_t *length,disp_native_string *error){return disp_data_read_file_limit(path,DISP_DATA_FILE_LIMIT,bytes,length,error);}
static bool disp_data_sync(FILE *file){if(fflush(file))return false;
#ifdef _WIN32
return _commit(_fileno(file))==0;
#else
return fsync(fileno(file))==0;
#endif
}
static bool disp_data_wal_encode(const unsigned char *old,size_t old_len,const unsigned char *pages,size_t pages_len,uint64_t generation,disp_data_buffer *wal,disp_native_string *error){size_t page_count=pages_len/DISP_DATA_PAGE_SIZE,records=0;for(size_t page=0;page<page_count;page++){size_t at=page*DISP_DATA_PAGE_SIZE;if(at+DISP_DATA_PAGE_SIZE>old_len||memcmp(old+at,pages+at,DISP_DATA_PAGE_SIZE))records++;}size_t length=DISP_DATA_WAL_HEADER+records*DISP_DATA_WAL_RECORD;if(length>DISP_DATA_WAL_LIMIT)return disp_data_fail(error,"DISP Data write-ahead log exceeds its storage limit");wal->data=(unsigned char*)disp_alloc_zeroed(1,length,1);wal->len=wal->cap=length;memcpy(wal->data,"DISPWAL\n",8);disp_data_put_u32(wal->data+8,1);disp_data_put_u32(wal->data+12,DISP_DATA_PAGE_SIZE);disp_data_put_u64(wal->data+16,generation);disp_data_put_u64(wal->data+24,page_count);disp_data_put_u64(wal->data+32,records);size_t target=DISP_DATA_WAL_HEADER;for(size_t page=0;page<page_count;page++){size_t at=page*DISP_DATA_PAGE_SIZE;if(at+DISP_DATA_PAGE_SIZE<=old_len&&!memcmp(old+at,pages+at,DISP_DATA_PAGE_SIZE))continue;disp_data_put_u64(wal->data+target,page);memcpy(wal->data+target+8,pages+at,DISP_DATA_PAGE_SIZE);target+=DISP_DATA_WAL_RECORD;}disp_data_put_u64(wal->data+40,disp_data_checksum(wal->data+DISP_DATA_WAL_HEADER,wal->len-DISP_DATA_WAL_HEADER));disp_data_put_u64(wal->data+48,0);disp_data_put_u64(wal->data+56,disp_data_checksum(wal->data,56));return true;}
static bool disp_data_apply_wal(const char *path,const unsigned char *wal,size_t wal_len,disp_native_string *error){if(wal_len<DISP_DATA_WAL_HEADER||wal_len>DISP_DATA_WAL_LIMIT||memcmp(wal,"DISPWAL\n",8)||disp_data_at_u32(wal+8)!=1||disp_data_at_u32(wal+12)!=DISP_DATA_PAGE_SIZE||disp_data_at_u64(wal+48)||disp_data_checksum(wal,56)!=disp_data_at_u64(wal+56))return disp_data_fail(error,"DISP Data write-ahead log header is invalid");uint64_t pages_u64=disp_data_at_u64(wal+24),records_u64=disp_data_at_u64(wal+32);if(pages_u64>SIZE_MAX||records_u64>SIZE_MAX)return disp_data_fail(error,"DISP Data WAL metadata does not fit this target");size_t page_count=(size_t)pages_u64,records=(size_t)records_u64;if(page_count<2||page_count>DISP_DATA_MAX_PAGES||records>(SIZE_MAX-DISP_DATA_WAL_HEADER)/DISP_DATA_WAL_RECORD||DISP_DATA_WAL_HEADER+records*DISP_DATA_WAL_RECORD!=wal_len||disp_data_checksum(wal+DISP_DATA_WAL_HEADER,wal_len-DISP_DATA_WAL_HEADER)!=disp_data_at_u64(wal+40))return disp_data_fail(error,"DISP Data write-ahead log is inconsistent");bool *seen=(bool*)disp_alloc_zeroed(page_count,sizeof(bool),_Alignof(bool));for(size_t record=0;record<records;record++){size_t at=DISP_DATA_WAL_HEADER+record*DISP_DATA_WAL_RECORD;uint64_t page_u64=disp_data_at_u64(wal+at);if(page_u64>=page_count||seen[page_u64]){disp_dealloc(seen);return disp_data_fail(error,"DISP Data WAL contains an invalid page identity");}seen[page_u64]=true;}if(!seen[0]){disp_dealloc(seen);return disp_data_fail(error,"DISP Data WAL does not contain its commit page");}disp_dealloc(seen);size_t final_size=page_count*DISP_DATA_PAGE_SIZE;unsigned char *committed=(unsigned char*)disp_alloc_zeroed(1,final_size,1),*current=NULL;size_t current_len=0;errno=0;if(disp_data_read_file(path,&current,&current_len,error)){size_t copied=current_len<final_size?current_len:final_size;if(copied)memcpy(committed,current,copied);disp_dealloc(current);}else if(error->len){disp_dealloc(committed);return false;}else if(errno!=ENOENT){*error=disp_owned_bytes(strerror(errno),strlen(strerror(errno)));disp_dealloc(committed);return false;}for(size_t record=0;record<records;record++){size_t at=DISP_DATA_WAL_HEADER+record*DISP_DATA_WAL_RECORD,page=(size_t)disp_data_at_u64(wal+at);memcpy(committed+page*DISP_DATA_PAGE_SIZE,wal+at+8,DISP_DATA_PAGE_SIZE);}if(disp_data_at_u64(committed+16)!=disp_data_at_u64(wal+16)){disp_dealloc(committed);return disp_data_fail(error,"DISP Data WAL generation does not match its commit page");}disp_database_state validation={0};bool valid=disp_data_decode_any(&validation,committed,final_size,error);disp_data_native_drop(&validation);disp_dealloc(committed);if(!valid)return false;FILE *file=fopen(path,"r+b");if(!file&&errno==ENOENT)file=fopen(path,"w+b");if(!file){*error=disp_owned_bytes(strerror(errno),strlen(strerror(errno)));return false;}bool ok=true;for(size_t record=0;ok&&record<records;record++){size_t at=DISP_DATA_WAL_HEADER+record*DISP_DATA_WAL_RECORD,page=(size_t)disp_data_at_u64(wal+at);if(fseek(file,(long)(page*DISP_DATA_PAGE_SIZE),SEEK_SET)||fwrite(wal+at+8,1,DISP_DATA_PAGE_SIZE,file)!=DISP_DATA_PAGE_SIZE)ok=false;}uint64_t final_length=(uint64_t)final_size;
#ifdef _WIN32
if(ok&&_chsize_s(_fileno(file),final_length)!=0)ok=false;
#else
if(ok&&ftruncate(fileno(file),(off_t)final_length)!=0)ok=false;
#endif
if(ok)ok=disp_data_sync(file);if(fclose(file)&&ok)ok=false;if(!ok){*error=disp_owned_bytes(strerror(errno),strlen(strerror(errno)));return false;}return true;}
static bool disp_data_recover_wal(disp_database_state *state,disp_native_string *error){char *wal_path=disp_data_suffix(state->path,state->path_len,".wal");if(!wal_path)return disp_data_fail(error,"DISP Data WAL path is too long");unsigned char *wal=NULL;size_t wal_len=0;errno=0;if(!disp_data_read_file_limit(wal_path,DISP_DATA_WAL_LIMIT,&wal,&wal_len,error)){disp_dealloc(wal_path);if(error->len)return false;if(errno==ENOENT)return true;*error=disp_owned_bytes(strerror(errno),strlen(strerror(errno)));return false;}bool ok=disp_data_apply_wal(state->path,wal,wal_len,error);disp_dealloc(wal);if(ok&&remove(wal_path)){*error=disp_owned_bytes(strerror(errno),strlen(strerror(errno)));ok=false;}disp_dealloc(wal_path);return ok;}
static bool disp_data_commit(disp_database_state *state,disp_native_string *error){if(!state->path)return true;if(!disp_data_recover_wal(state,error))return false;if(state->data_generation==UINT64_MAX)return disp_data_fail(error,"DISP Data generation counter is exhausted");uint64_t generation=state->data_generation+1;disp_data_buffer pages={0};if(!disp_data_encode_pages(state,generation,&pages,error))return false;unsigned char *old=NULL;size_t old_len=0;errno=0;if(!disp_data_read_file(state->path,&old,&old_len,error)){if(error->len){disp_dealloc(pages.data);return false;}if(errno!=ENOENT){*error=disp_owned_bytes(strerror(errno),strlen(strerror(errno)));disp_dealloc(pages.data);return false;}}disp_data_buffer wal={0};bool encoded=disp_data_wal_encode(old,old_len,pages.data,pages.len,generation,&wal,error);disp_dealloc(old);disp_dealloc(pages.data);if(!encoded)return false;char suffix[64];
#ifdef _WIN32
snprintf(suffix,sizeof(suffix),".wal.tmp-%lu-%llu",(unsigned long)GetCurrentProcessId(),(unsigned long long)disp_data_temporary_counter++);
#else
snprintf(suffix,sizeof(suffix),".wal.tmp-%ld-%llu",(long)getpid(),(unsigned long long)disp_data_temporary_counter++);
#endif
char *temporary=disp_data_suffix(state->path,state->path_len,suffix),*wal_path=disp_data_suffix(state->path,state->path_len,".wal");if(!temporary||!wal_path){disp_dealloc(temporary);disp_dealloc(wal_path);disp_dealloc(wal.data);return disp_data_fail(error,"DISP Data WAL path is too long");}FILE *file=fopen(temporary,"wbx");if(!file){*error=disp_owned_bytes(strerror(errno),strlen(strerror(errno)));goto fail;}bool written=fwrite(wal.data,1,wal.len,file)==wal.len&&disp_data_sync(file);if(fclose(file)&&written)written=false;if(!written){*error=disp_owned_bytes(strerror(errno),strlen(strerror(errno)));goto fail;}if(rename(temporary,wal_path)){*error=disp_owned_bytes(strerror(errno),strlen(strerror(errno)));goto fail;}state->data_generation=generation;disp_native_string checkpoint_error={0};if(disp_data_apply_wal(state->path,wal.data,wal.len,&checkpoint_error)){remove(wal_path);char *backup=disp_data_suffix(state->path,state->path_len,".backup");if(backup){remove(backup);disp_dealloc(backup);}}disp_string_drop(&checkpoint_error);disp_dealloc(temporary);disp_dealloc(wal_path);disp_dealloc(wal.data);return true;fail:remove(temporary);disp_dealloc(temporary);disp_dealloc(wal_path);disp_dealloc(wal.data);return false;}
static bool disp_data_store_open(const char *path,size_t length,disp_native_database *output,disp_native_string *error){*output=(disp_native_database){0};*error=(disp_native_string){0};if(!length||length>32768||memchr(path,0,length)||!disp_utf8_valid(path,length))return disp_data_fail(error,"data path must be non-empty UTF-8 without NUL");disp_database_state *state=(disp_database_state*)disp_alloc_zeroed(1,sizeof(disp_database_state),_Alignof(disp_database_state));state->native=true;state->path=disp_data_suffix(path,length,"");state->path_len=length;
#ifndef _WIN32
state->data_lock_fd=-1;
#endif
unsigned char *bytes=NULL;size_t bytes_len=0;if(!disp_data_lock_acquire(state,error)||!disp_data_recover_wal(state,error))goto fail;errno=0;if(!disp_data_read_file(state->path,&bytes,&bytes_len,error)){if(error->len)goto fail;if(errno!=ENOENT){*error=disp_owned_bytes(strerror(errno),strlen(strerror(errno)));goto fail;}char *backup=disp_data_suffix(path,length,".backup");if(!backup){disp_data_fail(error,"DISP Data backup path is too long");goto fail;}errno=0;if(disp_data_read_file(backup,&bytes,&bytes_len,error)){if(!disp_data_decode_any(state,bytes,bytes_len,error)){disp_dealloc(backup);goto fail;}if(rename(backup,state->path)){*error=disp_owned_bytes(strerror(errno),strlen(strerror(errno)));disp_dealloc(backup);goto fail;}disp_dealloc(backup);disp_dealloc(bytes);output->state=state;return true;}disp_dealloc(backup);if(error->len)goto fail;if(errno!=ENOENT){*error=disp_owned_bytes(strerror(errno),strlen(strerror(errno)));goto fail;}output->state=state;return true;}if(!disp_data_decode_any(state,bytes,bytes_len,error))goto fail;disp_dealloc(bytes);output->state=state;return true;fail:disp_dealloc(bytes);disp_data_lock_release(state);disp_data_native_drop(state);disp_dealloc(state->path);disp_dealloc(state);return false;}
static bool disp_database_close(disp_native_database *database,disp_native_string *error){disp_database_state *state=database->state;if(!state)return true;if(state->native){disp_data_lock_release(state);disp_data_native_drop(state);disp_dealloc(state->path);state->path=NULL;state->closed=true;disp_dealloc(state);database->state=NULL;return true;}
#ifdef DISP_DATABASE
if(sqlite3_get_autocommit(state->handle)==0){uint64_t ignored=0;disp_native_string rollback_error={0};disp_database_execute(state,"ROLLBACK",8,NULL,0,&ignored,&rollback_error);disp_string_drop(&rollback_error);}int code=sqlite3_close_v2(state->handle);if(code!=DISP_SQLITE_OK){*error=disp_database_error(state,"could not close database");return false;}state->handle=NULL;state->closed=true;if(state->handle_charged){disp_runtime_release_handle();state->handle_charged=false;}disp_dealloc(state);database->state=NULL;return true;
#else
return disp_data_fail(error,"database support is unavailable in this program");
#endif
}
static void disp_database_drop(disp_native_database *database){if(!database||!database->state)return;disp_native_string ignored={0};disp_database_close(database,&ignored);disp_string_drop(&ignored);}
static disp_data_table *disp_data_native_table(disp_database_state *state,const char *name,size_t name_len){if(!state||!state->native||state->closed)return NULL;for(size_t i=0;i<state->tables_len;i++)if(state->tables[i].name_len==name_len&&!memcmp(state->tables[i].name,name,name_len))return &state->tables[i];return NULL;}
static bool disp_data_native_schema(disp_database_state *state,const char *schema,size_t schema_len,const char *const *names,const char *const *types,const bool *required,const bool *primary,const bool *unique,size_t count,disp_native_string *error){disp_data_table *table=disp_data_native_table(state,schema,schema_len);if(table){if(table->field_count!=count)goto mismatch;for(size_t i=0;i<count;i++)if(table->field_names[i].len!=strlen(names[i])||memcmp(table->field_names[i].data,names[i],table->field_names[i].len)||table->field_types[i].len!=strlen(types[i])||memcmp(table->field_types[i].data,types[i],table->field_types[i].len)||table->required[i]!=required[i]||table->primary[i]!=primary[i]||table->unique[i]!=unique[i])goto mismatch;return true;}if(!state||!state->native||state->closed){const char *message="data store is closed";*error=disp_owned_bytes(message,strlen(message));return false;}if(!count||count>4096||state->tables_len>=4096){const char *message="DISP Data schema exceeds storage safety limits";*error=disp_owned_bytes(message,strlen(message));return false;}if(state->tables_len==state->tables_cap){size_t capacity=state->tables_cap?state->tables_cap*2:4;state->tables=(disp_data_table*)disp_realloc(state->tables,capacity*sizeof(disp_data_table),_Alignof(disp_data_table));memset(state->tables+state->tables_cap,0,(capacity-state->tables_cap)*sizeof(disp_data_table));state->tables_cap=capacity;}table=&state->tables[state->tables_len];table->name=(char*)disp_alloc(schema_len?schema_len:1,1);if(schema_len)memcpy(table->name,schema,schema_len);table->name_len=schema_len;table->field_names=(disp_native_string*)disp_alloc_zeroed(count,sizeof(disp_native_string),_Alignof(disp_native_string));table->field_types=(disp_native_string*)disp_alloc_zeroed(count,sizeof(disp_native_string),_Alignof(disp_native_string));table->required=(bool*)disp_alloc(count,sizeof(bool));table->primary=(bool*)disp_alloc(count,sizeof(bool));table->unique=(bool*)disp_alloc(count,sizeof(bool));table->field_count=count;table->primary_index=SIZE_MAX;for(size_t i=0;i<count;i++){table->field_names[i]=disp_owned_bytes(names[i],strlen(names[i]));table->field_types[i]=disp_owned_bytes(types[i],strlen(types[i]));table->required[i]=required[i];table->primary[i]=primary[i];table->unique[i]=unique[i];if(primary[i]){if(table->primary_index!=SIZE_MAX)goto invalid;table->primary_index=i;}}if(table->primary_index==SIZE_MAX)goto invalid;state->tables_len++;if(disp_data_commit(state,error))return true;state->tables_len--;disp_data_table_drop(table);return false;invalid:disp_data_table_drop(table);{const char *message="DISP Data schema must have exactly one primary field";*error=disp_owned_bytes(message,strlen(message));return false;}mismatch:{const char *message="stored layout does not match its DISP Data schema";*error=disp_owned_bytes(message,strlen(message));return false;}}
static bool disp_data_native_unique(disp_data_table *table,const disp_native_json *row,size_t skip,disp_native_string *error){for(size_t field=0;field<table->field_count;field++){if(!table->unique[field])continue;disp_native_string *name=&table->field_names[field];disp_native_json candidate={0};if(!disp_json_get(row,name->data,name->len,&candidate)){*error=disp_owned_bytes("data value is missing a unique field",strlen("data value is missing a unique field"));return false;}for(size_t index=0;index<table->rows_len;index++){if(index==skip)continue;disp_native_json existing={0};bool found=disp_json_get(&table->rows[index],name->data,name->len,&existing);bool equal=found&&existing.len==candidate.len&&(!candidate.len||!memcmp(existing.data,candidate.data,candidate.len));disp_json_drop(&existing);if(equal){disp_json_drop(&candidate);*error=disp_owned_bytes("duplicate unique value in DISP Data table",strlen("duplicate unique value in DISP Data table"));return false;}}disp_json_drop(&candidate);}return true;}
static bool disp_data_native_write(disp_database_state *state,const char *schema,size_t schema_len,const disp_native_json *row,bool replace,uint64_t *changes,disp_native_string *error){*changes=0;disp_data_table *table=disp_data_native_table(state,schema,schema_len);if(!table){const char *message="DISP Data schema is not registered";*error=disp_owned_bytes(message,strlen(message));return false;}disp_native_json key={0};disp_native_string *primary=&table->field_names[table->primary_index];if(!disp_json_get(row,primary->data,primary->len,&key)){const char *message="data value is missing its primary field";*error=disp_owned_bytes(message,strlen(message));return false;}for(size_t i=0;i<table->rows_len;i++){disp_native_json existing={0};bool found=disp_json_get(&table->rows[i],primary->data,primary->len,&existing);bool equal=found&&existing.len==key.len&&(!key.len||!memcmp(existing.data,key.data,key.len));disp_json_drop(&existing);if(equal){disp_json_drop(&key);if(!replace){const char *message="duplicate primary value in DISP Data table";*error=disp_owned_bytes(message,strlen(message));return false;}if(!disp_data_native_unique(table,row,i,error))return false;disp_native_json previous=table->rows[i],replacement=disp_json_literal(row->data,row->len);table->rows[i]=replacement;if(!disp_data_commit(state,error)){table->rows[i]=previous;disp_json_drop(&replacement);return false;}disp_json_drop(&previous);*changes=1;return true;}}disp_json_drop(&key);if(!disp_data_native_unique(table,row,SIZE_MAX,error))return false;if(table->rows_len>=DISP_DATABASE_ROW_LIMIT){const char *message="data table exceeds the 100000-row limit";*error=disp_owned_bytes(message,strlen(message));return false;}if(table->rows_len==table->rows_cap){size_t capacity=table->rows_cap?table->rows_cap*2:8;if(capacity>DISP_DATABASE_ROW_LIMIT)capacity=DISP_DATABASE_ROW_LIMIT;table->rows=(disp_native_json*)disp_realloc(table->rows,capacity*sizeof(disp_native_json),_Alignof(disp_native_json));table->rows_cap=capacity;}table->rows[table->rows_len++]=disp_json_literal(row->data,row->len);if(!disp_data_commit(state,error)){disp_json_drop(&table->rows[--table->rows_len]);return false;}*changes=1;return true;}
static bool disp_data_native_snapshot(disp_database_state *state,const char *schema,size_t schema_len,disp_native_json **rows,size_t *rows_len,size_t *rows_cap,disp_native_string *error){*rows=NULL;*rows_len=*rows_cap=0;disp_data_table *table=disp_data_native_table(state,schema,schema_len);if(!table){const char *message="DISP Data schema is not registered";*error=disp_owned_bytes(message,strlen(message));return false;}if(table->rows_len){*rows=(disp_native_json*)disp_alloc_zeroed(table->rows_len,sizeof(disp_native_json),_Alignof(disp_native_json));for(size_t i=0;i<table->rows_len;i++)(*rows)[i]=disp_json_literal(table->rows[i].data,table->rows[i].len);}*rows_len=*rows_cap=table->rows_len;return true;}
static bool disp_data_native_delete(disp_database_state *state,const char *schema,size_t schema_len,const bool *remove,size_t count,uint64_t *changes,disp_native_string *error){*changes=0;disp_data_table *table=disp_data_native_table(state,schema,schema_len);if(!table){const char *message="DISP Data schema is not registered";*error=disp_owned_bytes(message,strlen(message));return false;}if(count!=table->rows_len){const char *message="DISP Data table changed during removal";*error=disp_owned_bytes(message,strlen(message));return false;}size_t kept=0;for(size_t i=0;i<count;i++){if(remove[i])(*changes)++;else kept++;}if(!*changes)return true;disp_native_json *replacement=kept?(disp_native_json*)disp_alloc_zeroed(kept,sizeof(disp_native_json),_Alignof(disp_native_json)):NULL;size_t at=0;for(size_t i=0;i<count;i++)if(!remove[i])replacement[at++]=disp_json_literal(table->rows[i].data,table->rows[i].len);disp_native_json *previous=table->rows;size_t previous_len=table->rows_len,previous_cap=table->rows_cap;table->rows=replacement;table->rows_len=table->rows_cap=kept;if(!disp_data_commit(state,error)){for(size_t i=0;i<kept;i++)disp_json_drop(&replacement[i]);disp_dealloc(replacement);table->rows=previous;table->rows_len=previous_len;table->rows_cap=previous_cap;*changes=0;return false;}for(size_t i=0;i<previous_len;i++)disp_json_drop(&previous[i]);disp_dealloc(previous);return true;}
static bool disp_data_ensure_schema(disp_database_state *state,const char *schema,size_t schema_len,const char *create_sql,size_t create_len,const char *inspect_sql,size_t inspect_len,const char *const *names,const char *const *types,const bool *required,const bool *primary,const bool *unique,size_t count,disp_native_string *error){
if(state&&state->native)return disp_data_native_schema(state,schema,schema_len,names,types,required,primary,unique,count,error);
#ifdef DISP_DATABASE
uint64_t ignored=0;
if(!disp_database_execute(state,create_sql,create_len,NULL,0,&ignored,error))return false;
sqlite3_stmt *statement=NULL;
if(!disp_database_prepare(state,inspect_sql,inspect_len,&statement,error))return false;
for(size_t i=0;i<count;i++){
int code=sqlite3_step(statement);
if(code!=DISP_SQLITE_ROW){const char *message="stored layout does not match its DISP Data schema";*error=disp_owned_bytes(message,strlen(message));sqlite3_finalize(statement);return false;}
const unsigned char *name=sqlite3_column_text(statement,1),*type=sqlite3_column_text(statement,2);
bool not_null=sqlite3_column_int64(statement,3)!=0,is_primary=sqlite3_column_int64(statement,5)!=0;
if(!name||!type||strcmp((const char*)name,names[i])||strcmp((const char*)type,types[i])||not_null!=required[i]||is_primary!=primary[i]){const char *message="stored field is incompatible with its DISP Data schema";*error=disp_owned_bytes(message,strlen(message));sqlite3_finalize(statement);return false;}
}
int code=sqlite3_step(statement);
if(code!=DISP_SQLITE_DONE){const char *message="stored layout has extra DISP Data fields";*error=disp_owned_bytes(message,strlen(message));sqlite3_finalize(statement);return false;}
if(sqlite3_finalize(statement)!=DISP_SQLITE_OK){*error=disp_database_error(state,"could not validate DISP Data schema");return false;}
return true;
#else
(void)create_sql;(void)create_len;(void)inspect_sql;(void)inspect_len;return disp_data_fail(error,"DISP Data requires a DataStore");
#endif
}
#endif

static disp_native_string disp_owned_bytes(const char *bytes,size_t len){disp_native_string value={0};if(len){value.data=(char*)disp_alloc(len,1);memcpy(value.data,bytes,len);value.len=len;value.cap=len;}return value;}
static disp_native_cstring disp_cstring_from_bytes(const char *bytes,size_t len){disp_native_cstring value={0};size_t capacity;if(__builtin_add_overflow(len,(size_t)1,&capacity))disp_allocation_failure("CString length overflow");value.data=(char*)disp_alloc(capacity,1);if(len)memcpy(value.data,bytes,len);value.data[len]=0;value.len=len;value.cap=capacity;return value;}
static void disp_cstring_drop(disp_native_cstring *value){if(value->cap)disp_dealloc(value->data);value->data=NULL;value->len=0;value->cap=0;}
static void disp_memory_drop(disp_native_memory *value){disp_dealloc(value->data);value->data=NULL;value->len=0;value->align=0;}
static disp_native_memory_pointer disp_memory_pointer_offset(disp_native_memory_pointer pointer,int64_t offset,int line,int column){
if(!pointer.element_size)dv_panic("Memory pointer has invalid element size",line,column);
uintptr_t base=(uintptr_t)pointer.base;uintptr_t address=(uintptr_t)pointer.address;
if(address<base||address-base>pointer.byte_len)dv_panic("Memory pointer provenance is invalid",line,column);
size_t current=(size_t)(address-base);
if(offset>=0){uint64_t amount=(uint64_t)offset;if(amount>(pointer.byte_len-current)/pointer.element_size)dv_panic("Memory pointer offset is out of bounds",line,column);if(amount)pointer.address+=amount*pointer.element_size;}
else{uint64_t amount=(uint64_t)(-(offset+1))+1;if(amount>current/pointer.element_size)dv_panic("Memory pointer offset is out of bounds",line,column);if(amount)pointer.address-=amount*pointer.element_size;}
return pointer;
}
static void *disp_memory_pointer_access(disp_native_memory_pointer pointer,size_t width,size_t alignment,int line,int column){
uintptr_t base=(uintptr_t)pointer.base;uintptr_t address=(uintptr_t)pointer.address;
if(!pointer.element_size||!pointer.element_align||width!=pointer.element_size||alignment!=pointer.element_align)dv_panic("Memory pointer element contract is invalid",line,column);
if(address<base||address-base>pointer.byte_len)dv_panic("Memory pointer provenance is invalid",line,column);
size_t displacement=(size_t)(address-base);
if(width>pointer.byte_len-displacement)dv_panic("Memory pointer access is out of bounds",line,column);
if(alignment>1&&address%alignment)dv_panic("Memory pointer access is misaligned",line,column);
return pointer.address;
}
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
static bool disp_file_read_bytes(const disp_native_path *path,disp_native_string *out,disp_native_string *error){FILE *file=fopen(path->data,"rb");if(!file){*error=disp_io_error();return false;}if(fseek(file,0,SEEK_END)!=0){*error=disp_io_error();fclose(file);return false;}long end=ftell(file);if(end<0){*error=disp_io_error();fclose(file);return false;}rewind(file);size_t len=(size_t)end;char *data=len?(char*)disp_alloc(len,1):NULL;size_t read=len?fread(data,1,len,file):0;if(read!=len){*error=disp_io_error();disp_dealloc(data);fclose(file);return false;}if(fclose(file)!=0){*error=disp_io_error();disp_dealloc(data);return false;}out->data=data;out->len=len;out->cap=len;return true;}
static bool disp_file_read_text(const disp_native_path *path,disp_native_string *out,disp_native_string *error){if(!disp_file_read_bytes(path,out,error))return false;if(disp_utf8_valid(out->data,out->len))return true;disp_string_drop(out);errno=EILSEQ;*error=disp_io_error();return false;}
static atomic_uint_fast64_t disp_file_temporary_counter=ATOMIC_VAR_INIT(0);
static bool disp_file_temporary_open(const disp_native_path *path,char **temporary,FILE **file,disp_native_string *error){for(size_t attempt=0;attempt<128;attempt++){char suffix[96];uint_fast64_t id=atomic_fetch_add_explicit(&disp_file_temporary_counter,1,memory_order_relaxed);
#ifdef _WIN32
snprintf(suffix,sizeof(suffix),".disp-tmp-%lu-%llu",(unsigned long)GetCurrentProcessId(),(unsigned long long)id);
#else
snprintf(suffix,sizeof(suffix),".disp-tmp-%ld-%llu",(long)getpid(),(unsigned long long)id);
#endif
size_t suffix_len=strlen(suffix);if(path->len>SIZE_MAX-suffix_len-1){*error=disp_owned_bytes("transactional file path is too long",strlen("transactional file path is too long"));return false;}char *candidate=(char*)disp_alloc(path->len+suffix_len+1,1);memcpy(candidate,path->data,path->len);memcpy(candidate+path->len,suffix,suffix_len+1);errno=0;FILE *created=fopen(candidate,"wbx");if(created){*temporary=candidate;*file=created;return true;}int cause=errno;disp_dealloc(candidate);if(cause!=EEXIST){errno=cause;*error=disp_io_error();return false;}}*error=disp_owned_bytes("could not create a unique transactional file",strlen("could not create a unique transactional file"));return false;}
static bool disp_file_replace(const char *temporary,const disp_native_path *path,disp_native_string *error){
#ifdef _WIN32
wchar_t *from=disp_process_wide(temporary,strlen(temporary)),*to=disp_process_wide(path->data,path->len);if(!from||!to){disp_dealloc(from);disp_dealloc(to);*error=disp_owned_bytes("file path is not valid UTF-8",strlen("file path is not valid UTF-8"));return false;}BOOL moved=MoveFileExW(from,to,MOVEFILE_REPLACE_EXISTING|MOVEFILE_WRITE_THROUGH);DWORD code=moved?ERROR_SUCCESS:GetLastError();disp_dealloc(from);disp_dealloc(to);if(moved)return true;char message[96];snprintf(message,sizeof(message),"transactional file replacement failed (Windows error %lu)",(unsigned long)code);*error=disp_owned_bytes(message,strlen(message));return false;
#else
if(rename(temporary,path->data)==0)return true;*error=disp_io_error();return false;
#endif
}
static bool disp_file_sync(FILE *file){if(fflush(file))return false;
#ifdef _WIN32
return _commit(_fileno(file))==0;
#else
return fsync(fileno(file))==0;
#endif
}
static bool disp_file_transaction(const disp_native_path *path,const char *permission_path,FILE *prefix,const char *data,size_t len,size_t limit,disp_native_string *error){struct stat permissions={0};bool preserve_permissions=false;errno=0;if(stat(permission_path,&permissions)==0)preserve_permissions=true;else if(errno!=ENOENT){*error=disp_io_error();if(prefix)fclose(prefix);return false;}char *temporary=NULL;FILE *target=NULL;if(!disp_file_temporary_open(path,&temporary,&target,error)){if(prefix)fclose(prefix);return false;}bool ok=true,exceeded=false;int cause=0;if(prefix){unsigned char chunk[8192];size_t copied=0;while(ok){size_t available=limit-len-copied,request=sizeof(chunk);if(available<request)request=available+1;size_t count=fread(chunk,1,request,prefix);if(count>available){exceeded=true;ok=false;break;}if(count&&fwrite(chunk,1,count,target)!=count){cause=errno?errno:EIO;ok=false;break;}copied+=count;if(count<request){if(ferror(prefix)){cause=errno?errno:EIO;ok=false;}break;}}}if(ok&&len&&fwrite(data,1,len,target)!=len){cause=errno?errno:EIO;ok=false;}if(prefix&&fclose(prefix)!=0&&ok){cause=errno?errno:EIO;ok=false;}if(ok&&preserve_permissions){
#ifdef _WIN32
if(_chmod(temporary,permissions.st_mode)!=0){cause=errno?errno:EIO;ok=false;}
#else
if(chmod(temporary,permissions.st_mode&07777)!=0){cause=errno?errno:EIO;ok=false;}
#endif
}if(ok&&!disp_file_sync(target)){cause=errno?errno:EIO;ok=false;}if(fclose(target)!=0&&ok){cause=errno?errno:EIO;ok=false;}if(!ok){remove(temporary);disp_dealloc(temporary);if(exceeded)*error=disp_owned_bytes("file write exceeds the configured byte limit",strlen("file write exceeds the configured byte limit"));else{errno=cause?cause:EIO;*error=disp_io_error();}return false;}if(!disp_file_replace(temporary,path,error)){remove(temporary);disp_dealloc(temporary);return false;}disp_dealloc(temporary);return true;}
static bool disp_file_write_text(const disp_native_path *path,const char *data,size_t len,bool append,disp_native_string *error){size_t limit=disp_runtime_limit("DISP_MAX_FILE_WRITE_BYTES",(size_t)DISP_DEFAULT_MAX_FILE_WRITE_BYTES);if(len>limit){*error=disp_owned_bytes("file write exceeds the configured byte limit",strlen("file write exceeds the configured byte limit"));return false;}FILE *prefix=NULL;if(append){errno=0;prefix=fopen(path->data,"rb");if(!prefix&&errno!=ENOENT){*error=disp_io_error();return false;}}return disp_file_transaction(path,path->data,prefix,data,len,limit,error);}
static bool disp_file_exists(const disp_native_path *path){struct stat info;return stat(path->data,&info)==0&&(info.st_mode&S_IFREG)!=0;}
static bool disp_file_metadata(const disp_native_path *path,uint64_t *size,uint64_t *modified,disp_native_string *error){struct stat info;if(stat(path->data,&info)!=0){*error=disp_io_error();return false;}*size=(uint64_t)info.st_size;*modified=(uint64_t)info.st_mtime;return true;}
static bool disp_directory_exists(const disp_native_path *path){struct stat info;return stat(path->data,&info)==0&&(info.st_mode&S_IFDIR)!=0;}
static bool disp_file_remove(const disp_native_path *path,disp_native_string *error){if(remove(path->data)==0)return true;*error=disp_io_error();return false;}
static bool disp_file_copy(const disp_native_path *from,const disp_native_path *to,disp_native_string *error){FILE *source=fopen(from->data,"rb");if(!source){*error=disp_io_error();return false;}return disp_file_transaction(to,from->data,source,NULL,0,disp_runtime_limit("DISP_MAX_FILE_WRITE_BYTES",(size_t)DISP_DEFAULT_MAX_FILE_WRITE_BYTES),error);}
static bool disp_file_move(const disp_native_path *from,const disp_native_path *to,disp_native_string *error){if(rename(from->data,to->data)==0)return true;*error=disp_io_error();return false;}
typedef enum { DISP_ASYNC_READ_TEXT, DISP_ASYNC_READ_BYTES, DISP_ASYNC_WRITE_TEXT, DISP_ASYNC_WRITE_BYTES } disp_async_file_operation;
typedef struct disp_async_file_state {
    atomic_size_t refs;
    atomic_bool done;
    atomic_bool cancelled;
    bool started;
    bool taken;
    bool ok;
    int line;
    int column;
    disp_async_file_operation operation;
    disp_native_path path;
    disp_native_string input;
    disp_native_string value;
    disp_native_string error;
} disp_async_file_state;
static atomic_size_t disp_async_jobs=ATOMIC_VAR_INIT(0);
static void disp_async_file_release(disp_async_file_state *state){if(atomic_fetch_sub_explicit(&state->refs,1,memory_order_acq_rel)!=1)return;atomic_thread_fence(memory_order_acquire);disp_path_drop(&state->path);disp_string_drop(&state->input);disp_string_drop(&state->value);disp_string_drop(&state->error);disp_dealloc(state);}
static void disp_async_file_worker(void *raw){disp_async_file_state *state=(disp_async_file_state*)raw;if(state->operation==DISP_ASYNC_READ_TEXT)state->ok=disp_file_read_text(&state->path,&state->value,&state->error);else if(state->operation==DISP_ASYNC_READ_BYTES)state->ok=disp_file_read_bytes(&state->path,&state->value,&state->error);else state->ok=disp_file_write_text(&state->path,state->input.data,state->input.len,false,&state->error);disp_path_drop(&state->path);disp_string_drop(&state->input);atomic_store_explicit(&state->done,true,memory_order_release);disp_async_file_release(state);atomic_fetch_sub_explicit(&disp_async_jobs,1,memory_order_acq_rel);}
static disp_async_file_state *disp_async_file_create(disp_async_file_operation operation,disp_native_path path,disp_native_string input,int line,int column){disp_async_file_state *state=(disp_async_file_state*)disp_alloc_zeroed(1,sizeof(disp_async_file_state),_Alignof(disp_async_file_state));atomic_init(&state->refs,1);atomic_init(&state->done,false);atomic_init(&state->cancelled,false);state->operation=operation;state->path=path;state->input=input;state->line=line;state->column=column;return state;}
static bool disp_async_file_poll(disp_async_file_state *state){if(!state||state->taken)dv_panic("async file operation has already completed",0,0);if(!state->started){state->started=true;atomic_fetch_add_explicit(&state->refs,1,memory_order_relaxed);atomic_fetch_add_explicit(&disp_async_jobs,1,memory_order_relaxed);uintptr_t handle=disp_thread_start(disp_async_file_worker,state,state->line,state->column);disp_thread_detach(handle);}if(!atomic_load_explicit(&state->done,memory_order_acquire)){disp_reactor_offer(1000000ULL);return false;}return true;}
static void disp_async_file_take(disp_async_file_state *state,bool *ok,disp_native_string *value,disp_native_string *error){if(!atomic_load_explicit(&state->done,memory_order_acquire)||state->taken)dv_panic("async file result is not ready",0,0);state->taken=true;*ok=state->ok;*value=state->value;*error=state->error;state->value=(disp_native_string){0};state->error=(disp_native_string){0};}
static void disp_async_file_drop(void *raw){disp_async_file_state *state=(disp_async_file_state*)raw;if(!state)return;atomic_store_explicit(&state->cancelled,true,memory_order_release);disp_async_file_release(state);}
static void disp_async_file_drain(void){while(atomic_load_explicit(&disp_async_jobs,memory_order_acquire))disp_time_sleep(1000000ULL);}
#ifdef DISP_NETWORKING
static disp_native_socket_address disp_socket_address_create(const char *host,size_t len,uint64_t port,int line,int column){if(!len)dv_panic("socket host cannot be empty",line,column);if(memchr(host,0,len))dv_panic("socket host cannot contain a NUL byte",line,column);if(port>65535)dv_panic("socket port is outside 0 through 65535",line,column);disp_native_socket_address address={0};address.host=(char*)disp_alloc(len+1,1);memcpy(address.host,host,len);address.host[len]=0;address.len=len;address.port=(uint16_t)port;return address;}
static bool disp_ip_address_parse(const char *text,size_t length,disp_native_ip_address *address,disp_native_string *error){if(!length||memchr(text,0,length)){*error=disp_owned_bytes("invalid IP address",strlen("invalid IP address"));return false;}char *copy=(char*)disp_alloc(length+1,1);memcpy(copy,text,length);copy[length]=0;disp_native_ip_address parsed={0};if(inet_pton(AF_INET,copy,parsed.bytes)==1)parsed.family=4;else if(inet_pton(AF_INET6,copy,parsed.bytes)==1)parsed.family=6;disp_dealloc(copy);if(!parsed.family){*error=disp_owned_bytes("invalid IP address",strlen("invalid IP address"));return false;}*address=parsed;return true;}
static disp_native_string disp_ip_address_string(const disp_native_ip_address *address){char text[INET6_ADDRSTRLEN];int family=address->family==4?AF_INET:address->family==6?AF_INET6:AF_UNSPEC;if(family==AF_UNSPEC||!inet_ntop(family,address->bytes,text,sizeof(text)))return disp_owned_bytes("<invalid IP address>",strlen("<invalid IP address>"));return disp_owned_bytes(text,strlen(text));}
static bool disp_ip_address_loopback(const disp_native_ip_address *address){if(address->family==4)return address->bytes[0]==127;if(address->family!=6)return false;for(size_t i=0;i<15;i++)if(address->bytes[i])return false;return address->bytes[15]==1;}
static bool disp_ip_address_unspecified(const disp_native_ip_address *address){size_t length=address->family==4?4:address->family==6?16:0;if(!length)return false;for(size_t i=0;i<length;i++)if(address->bytes[i])return false;return true;}
static disp_native_socket_address disp_socket_address_from_ip(const disp_native_ip_address *ip,uint64_t port,int line,int column){disp_native_string text=disp_ip_address_string(ip);disp_native_socket_address address=disp_socket_address_create(text.data,text.len,port,line,column);disp_string_drop(&text);return address;}
static disp_native_socket_address disp_socket_address_clone(const disp_native_socket_address *source){disp_native_socket_address address={0};if(!source||!source->host)return address;address.host=(char*)disp_alloc(source->len+1,1);memcpy(address.host,source->host,source->len+1);address.len=source->len;address.port=source->port;return address;}
static void disp_socket_address_drop(disp_native_socket_address *address){disp_dealloc(address->host);address->host=NULL;address->len=0;address->port=0;}
#ifdef _WIN32
typedef SOCKET disp_socket_handle;
#define DISP_INVALID_SOCKET INVALID_SOCKET
static INIT_ONCE disp_network_once=INIT_ONCE_STATIC_INIT;
static int disp_network_init_error=0;
static void disp_network_cleanup(void){WSACleanup();}
static BOOL CALLBACK disp_network_init_once(PINIT_ONCE once,PVOID parameter,PVOID *context){(void)once;(void)parameter;(void)context;WSADATA data;disp_network_init_error=WSAStartup(MAKEWORD(2,2),&data);if(!disp_network_init_error)atexit(disp_network_cleanup);return TRUE;}
static void disp_network_init(void){InitOnceExecuteOnce(&disp_network_once,disp_network_init_once,NULL,NULL);if(disp_network_init_error)dv_panic("could not initialize Windows networking",0,0);}
static void disp_socket_close(disp_socket_handle socket){if(socket!=DISP_INVALID_SOCKET)closesocket(socket);}
static int disp_socket_error_code(void){return WSAGetLastError();}
static bool disp_socket_would_block(int code){return code==WSAEWOULDBLOCK;}
static bool disp_socket_connect_pending(int code){return code==WSAEWOULDBLOCK||code==WSAEINPROGRESS||code==WSAEALREADY||code==WSAEINVAL;}
static bool disp_socket_not_connected(int code){return code==WSAENOTCONN;}
static bool disp_socket_set_blocking(disp_socket_handle socket,bool blocking){u_long mode=blocking?0:1;return ioctlsocket(socket,FIONBIO,&mode)==0;}
static int disp_socket_shutdown_handle(disp_socket_handle socket,int direction){return shutdown(socket,direction==0?SD_RECEIVE:direction==1?SD_SEND:SD_BOTH);}
#else
typedef int disp_socket_handle;
#define DISP_INVALID_SOCKET (-1)
static void disp_network_init(void){}
static void disp_socket_close(disp_socket_handle socket){if(socket!=DISP_INVALID_SOCKET)close(socket);}
static int disp_socket_error_code(void){return errno;}
static bool disp_socket_would_block(int code){return code==EAGAIN||code==EWOULDBLOCK;}
static bool disp_socket_connect_pending(int code){return code==EINPROGRESS||code==EALREADY||code==EAGAIN||code==EWOULDBLOCK;}
static bool disp_socket_not_connected(int code){return code==ENOTCONN;}
static bool disp_socket_set_blocking(disp_socket_handle socket,bool blocking){int flags=fcntl(socket,F_GETFL,0);return flags>=0&&fcntl(socket,F_SETFL,blocking?(flags&~O_NONBLOCK):(flags|O_NONBLOCK))==0;}
static int disp_socket_shutdown_handle(disp_socket_handle socket,int direction){return shutdown(socket,direction==0?SHUT_RD:direction==1?SHUT_WR:SHUT_RDWR);}
#endif
static int disp_socket_ready(disp_socket_handle socket,bool reading,int *error){fd_set set;FD_ZERO(&set);FD_SET(socket,&set);struct timeval timeout={0,0};int result=select((int)(socket+1),reading?&set:NULL,reading?NULL:&set,NULL,&timeout);if(result<0)*error=disp_socket_error_code();return result;}
static int disp_socket_connect_ready(disp_socket_handle socket,int *error){fd_set write_set,error_set;FD_ZERO(&write_set);FD_ZERO(&error_set);FD_SET(socket,&write_set);FD_SET(socket,&error_set);struct timeval timeout={0,0};int result=select((int)(socket+1),NULL,&write_set,&error_set,&timeout);if(result<0)*error=disp_socket_error_code();return result;}
static int disp_socket_connection_error(disp_socket_handle socket){int error=0;socklen_t length=(socklen_t)sizeof(error);if(getsockopt(socket,SOL_SOCKET,SO_ERROR,(char*)&error,&length)!=0)return disp_socket_error_code();return error;}
static int disp_ip_compare(const void *left,const void *right){const disp_native_ip_address *a=(const disp_native_ip_address*)left,*b=(const disp_native_ip_address*)right;if(a->family!=b->family)return a->family<b->family?-1:1;return memcmp(a->bytes,b->bytes,a->family==4?4:16);}
static bool disp_dns_resolve(const char *host,size_t length,disp_native_ip_list *result,disp_native_string *error){disp_network_init();if(!length||memchr(host,0,length)){*error=disp_owned_bytes("DNS host must not be empty or contain NUL",strlen("DNS host must not be empty or contain NUL"));return false;}char *copy=(char*)disp_alloc(length+1,1);memcpy(copy,host,length);copy[length]=0;struct addrinfo hints={0},*addresses=NULL;hints.ai_family=AF_UNSPEC;hints.ai_socktype=SOCK_STREAM;int status=getaddrinfo(copy,NULL,&hints,&addresses);disp_dealloc(copy);if(status!=0){
#ifdef _WIN32
const char *message=gai_strerrorA(status);
#else
const char *message=gai_strerror(status);
#endif
*error=disp_owned_bytes(message?message:"DNS resolution failed",message?strlen(message):strlen("DNS resolution failed"));return false;}size_t count=0;for(struct addrinfo *current=addresses;current;current=current->ai_next)if(current->ai_family==AF_INET||current->ai_family==AF_INET6)count++;if(!count){freeaddrinfo(addresses);*error=disp_owned_bytes("DNS resolution returned no addresses",strlen("DNS resolution returned no addresses"));return false;}disp_native_ip_address *values=(disp_native_ip_address*)disp_alloc(count*sizeof(disp_native_ip_address),_Alignof(disp_native_ip_address));size_t used=0;for(struct addrinfo *current=addresses;current;current=current->ai_next){disp_native_ip_address address={0};if(current->ai_family==AF_INET){address.family=4;memcpy(address.bytes,&((struct sockaddr_in*)current->ai_addr)->sin_addr,4);}else if(current->ai_family==AF_INET6){address.family=6;memcpy(address.bytes,&((struct sockaddr_in6*)current->ai_addr)->sin6_addr,16);}else continue;values[used++]=address;}freeaddrinfo(addresses);qsort(values,used,sizeof(disp_native_ip_address),disp_ip_compare);size_t unique=0;for(size_t i=0;i<used;i++)if(!unique||disp_ip_compare(&values[i],&values[unique-1])!=0)values[unique++]=values[i];result->data=values;result->len=unique;result->cap=count;return true;}
typedef struct { atomic_size_t refs;atomic_int owner;atomic_bool done;bool started;bool taken;bool ok;bool has_deadline;uint64_t timeout;uint64_t deadline;int line;int column;disp_native_string host;disp_native_ip_list addresses;disp_native_string error; } disp_dns_state;
static void disp_dns_release(disp_dns_state *state){if(atomic_fetch_sub_explicit(&state->refs,1,memory_order_acq_rel)!=1)return;atomic_thread_fence(memory_order_acquire);disp_string_drop(&state->host);disp_dealloc(state->addresses.data);disp_string_drop(&state->error);disp_dealloc(state);}
static void disp_dns_worker(void *raw){disp_dns_state *state=(disp_dns_state*)raw;disp_native_ip_list addresses={0};disp_native_string error={0};bool ok=disp_dns_resolve(state->host.data,state->host.len,&addresses,&error);int expected=0;if(atomic_compare_exchange_strong_explicit(&state->owner,&expected,1,memory_order_acq_rel,memory_order_acquire)){state->ok=ok;state->addresses=addresses;state->error=error;atomic_store_explicit(&state->done,true,memory_order_release);}else{disp_dealloc(addresses.data);disp_string_drop(&error);}disp_dns_release(state);atomic_fetch_sub_explicit(&disp_async_jobs,1,memory_order_acq_rel);}
static disp_dns_state *disp_dns_create(const char *host,size_t length,bool has_timeout,uint64_t timeout,int line,int column){disp_dns_state *state=(disp_dns_state*)disp_alloc_zeroed(1,sizeof(disp_dns_state),_Alignof(disp_dns_state));atomic_init(&state->refs,1);atomic_init(&state->owner,0);atomic_init(&state->done,false);state->host=disp_owned_bytes(host,length);state->has_deadline=has_timeout;state->timeout=timeout;state->line=line;state->column=column;return state;}
static bool disp_dns_poll(disp_dns_state *state){if(!state||state->taken)dv_panic("DNS future has already completed",0,0);if(atomic_load_explicit(&state->done,memory_order_acquire))return true;if(!state->started){state->started=true;uint64_t now=disp_time_now_nanos();state->deadline=UINT64_MAX-now<state->timeout?UINT64_MAX:now+state->timeout;if(state->has_deadline&&!state->timeout){atomic_store_explicit(&state->owner,2,memory_order_release);state->error=disp_owned_bytes("DNS resolution timed out",strlen("DNS resolution timed out"));state->ok=false;atomic_store_explicit(&state->done,true,memory_order_release);return true;}atomic_fetch_add_explicit(&state->refs,1,memory_order_relaxed);atomic_fetch_add_explicit(&disp_async_jobs,1,memory_order_relaxed);uintptr_t handle=disp_thread_start(disp_dns_worker,state,state->line,state->column);disp_thread_detach(handle);}uint64_t now=disp_time_now_nanos();if(state->has_deadline&&now>=state->deadline){int expected=0;if(atomic_compare_exchange_strong_explicit(&state->owner,&expected,2,memory_order_acq_rel,memory_order_acquire)){state->error=disp_owned_bytes("DNS resolution timed out",strlen("DNS resolution timed out"));state->ok=false;atomic_store_explicit(&state->done,true,memory_order_release);return true;}}disp_reactor_offer(1000000ULL);return false;}
static void disp_dns_take(disp_dns_state *state,bool *ok,disp_native_ip_list *addresses,disp_native_string *error){if(!atomic_load_explicit(&state->done,memory_order_acquire)||state->taken)dv_panic("DNS result is not ready",0,0);state->taken=true;*ok=state->ok;*addresses=state->addresses;state->addresses=(disp_native_ip_list){0};*error=state->error;state->error=(disp_native_string){0};}
static void disp_dns_drop(void *raw){disp_dns_state *state=(disp_dns_state*)raw;if(!state)return;int expected=0;atomic_compare_exchange_strong_explicit(&state->owner,&expected,2,memory_order_acq_rel,memory_order_acquire);disp_dns_release(state);}
struct disp_tcp_state { atomic_size_t refs;atomic_bool closed;atomic_bool read_shutdown;atomic_bool write_shutdown;atomic_bool read_busy;atomic_bool write_busy;disp_socket_handle socket; };
static disp_native_string disp_network_error_code(const char *operation,int code){char message[160];int length=snprintf(message,sizeof(message),"%s failed with network error %d",operation,code);if(length<0)length=0;return disp_owned_bytes(message,(size_t)length);}
static disp_tcp_state *disp_tcp_state_create(disp_socket_handle socket){disp_tcp_state *state=(disp_tcp_state*)disp_alloc(sizeof(disp_tcp_state),_Alignof(disp_tcp_state));atomic_init(&state->refs,1);atomic_init(&state->closed,false);atomic_init(&state->read_shutdown,false);atomic_init(&state->write_shutdown,false);atomic_init(&state->read_busy,false);atomic_init(&state->write_busy,false);state->socket=socket;disp_runtime_acquire_handle();return state;}
static void disp_tcp_state_retain(disp_tcp_state *state){atomic_fetch_add_explicit(&state->refs,1,memory_order_relaxed);}
static void disp_tcp_state_close(disp_tcp_state *state){if(!state)return;if(!atomic_exchange_explicit(&state->closed,true,memory_order_acq_rel)){disp_socket_shutdown_handle(state->socket,2);disp_socket_close(state->socket);disp_runtime_release_handle();}}
static void disp_tcp_state_release(disp_tcp_state *state){if(atomic_fetch_sub_explicit(&state->refs,1,memory_order_acq_rel)!=1)return;atomic_thread_fence(memory_order_acquire);disp_tcp_state_close(state);disp_dealloc(state);}
static void disp_tcp_stream_drop(disp_native_tcp_stream *stream){if(!stream->state)return;disp_tcp_state_close(stream->state);disp_tcp_state_release(stream->state);stream->state=NULL;}
static void disp_tcp_stream_close(disp_native_tcp_stream *stream){if(stream&&stream->state)disp_tcp_state_close(stream->state);}
static bool disp_tcp_stream_shutdown(disp_native_tcp_stream *stream,bool reading,disp_native_string *error){if(!stream->state||atomic_load_explicit(&stream->state->closed,memory_order_acquire)){*error=disp_owned_bytes("TCP stream is closed",strlen("TCP stream is closed"));return false;}atomic_bool *flag=reading?&stream->state->read_shutdown:&stream->state->write_shutdown;if(atomic_exchange_explicit(flag,true,memory_order_acq_rel))return true;if(disp_socket_shutdown_handle(stream->state->socket,reading?0:1)!=0){int code=disp_socket_error_code();if(!disp_socket_not_connected(code)){*error=disp_network_error_code(reading?"TCP shutdown read":"TCP shutdown write",code);return false;}}return true;}
static bool disp_tcp_claim(atomic_bool *busy){bool expected=false;return atomic_compare_exchange_strong_explicit(busy,&expected,true,memory_order_acq_rel,memory_order_acquire);}
static bool disp_tcp_stream_read(disp_native_tcp_stream *stream,size_t limit,disp_native_string *bytes,disp_native_string *error,int line,int column){if(!stream->state||atomic_load_explicit(&stream->state->closed,memory_order_acquire)){*error=disp_owned_bytes("TCP stream is closed",strlen("TCP stream is closed"));return false;}if(atomic_load_explicit(&stream->state->read_shutdown,memory_order_acquire)){*error=disp_owned_bytes("TCP read side is shut down",strlen("TCP read side is shut down"));return false;}if(!disp_tcp_claim(&stream->state->read_busy)){*error=disp_owned_bytes("TCP read is already in progress",strlen("TCP read is already in progress"));return false;}if(limit>DISP_TCP_READ_LIMIT)dv_panic("TCP read limit exceeds the 16 MiB safety limit",line,column);if(!limit){atomic_store_explicit(&stream->state->read_busy,false,memory_order_release);return true;}char *data=(char*)disp_alloc(limit,1);for(;;){
#ifdef _WIN32
int count=recv(stream->state->socket,data,(int)limit,0);
#else
ssize_t count=recv(stream->state->socket,data,limit,0);
#endif
if(count<0){int code=disp_socket_error_code();if(disp_socket_would_block(code)){disp_time_sleep(1000000ULL);continue;}disp_dealloc(data);atomic_store_explicit(&stream->state->read_busy,false,memory_order_release);*error=disp_network_error_code("TCP read",code);return false;}atomic_store_explicit(&stream->state->read_busy,false,memory_order_release);if(!count){disp_dealloc(data);return true;}bytes->data=data;bytes->len=(size_t)count;bytes->cap=limit;return true;}}
static bool disp_tcp_stream_write(disp_native_tcp_stream *stream,const char *bytes,size_t len,size_t *written,disp_native_string *error){if(!stream->state||atomic_load_explicit(&stream->state->closed,memory_order_acquire)){*error=disp_owned_bytes("TCP stream is closed",strlen("TCP stream is closed"));return false;}if(atomic_load_explicit(&stream->state->write_shutdown,memory_order_acquire)){*error=disp_owned_bytes("TCP write side is shut down",strlen("TCP write side is shut down"));return false;}if(!disp_tcp_claim(&stream->state->write_busy)){*error=disp_owned_bytes("TCP write is already in progress",strlen("TCP write is already in progress"));return false;}*written=0;while(*written<len){
#ifdef _WIN32
int chunk=(int)((len-*written)>INT_MAX?INT_MAX:(len-*written));int count=send(stream->state->socket,bytes+*written,chunk,0);
#else
ssize_t count=send(stream->state->socket,bytes+*written,len-*written,
#ifdef MSG_NOSIGNAL
MSG_NOSIGNAL
#else
0
#endif
);
#endif
if(count<0){int code=disp_socket_error_code();if(disp_socket_would_block(code)){disp_time_sleep(1000000ULL);continue;}atomic_store_explicit(&stream->state->write_busy,false,memory_order_release);*error=disp_network_error_code("TCP write",code);return false;}if(!count){atomic_store_explicit(&stream->state->write_busy,false,memory_order_release);*error=disp_owned_bytes("TCP write made no progress",strlen("TCP write made no progress"));return false;}*written+=(size_t)count;}atomic_store_explicit(&stream->state->write_busy,false,memory_order_release);return true;}
static bool disp_tcp_take_socket(disp_tcp_state *state,disp_socket_handle *socket){if(!state||atomic_exchange_explicit(&state->closed,true,memory_order_acq_rel))return false;*socket=state->socket;return true;}
#ifdef DISP_TLS
#ifdef _WIN32
struct disp_tls_state {
    atomic_size_t refs;
    atomic_bool closed;
    atomic_bool read_busy;
    atomic_bool write_busy;
    disp_socket_handle socket;
    CredHandle credential;
    CtxtHandle context;
    bool credential_valid;
    bool context_valid;
    SecPkgContext_StreamSizes sizes;
    unsigned long protocol;
    wchar_t *server_name;
    bool post_handshake;
    bool post_complete;
    bool post_need_input;
    unsigned char *post_output;
    size_t post_output_len;
    size_t post_output_pos;
    unsigned char *encrypted;
    size_t encrypted_len;
    size_t encrypted_cap;
    unsigned char *plain;
    size_t plain_len;
    size_t plain_pos;
};

typedef struct {
    disp_tcp_state *tcp;
    disp_socket_handle socket;
    bool socket_valid;
    CredHandle credential;
    CtxtHandle context;
    bool credential_valid;
    bool context_valid;
    bool started;
    bool first_call;
    bool need_input;
    bool handshake_complete;
    bool taken;
    bool done;
    bool ok;
    bool has_deadline;
    uint64_t timeout;
    uint64_t deadline;
    int line;
    int column;
    wchar_t *server_name;
    unsigned char *input;
    size_t input_len;
    size_t input_cap;
    unsigned char *output;
    size_t output_len;
    size_t output_pos;
    disp_native_tls_stream stream;
    disp_native_string error;
} disp_tls_handshake_state;

static disp_native_string disp_tls_status_error(const char *operation,SECURITY_STATUS status){char message[160];int length=snprintf(message,sizeof(message),"%s failed with TLS security status 0x%08lx",operation,(unsigned long)status);if(length<0)length=0;return disp_owned_bytes(message,(size_t)length);}
static bool disp_tls_reserve(unsigned char **data,size_t *capacity,size_t needed){if(needed>*capacity){size_t next=*capacity?*capacity:16384;while(next<needed){if(next>1048576ULL/2)return false;next*=2;}unsigned char *grown=(unsigned char*)disp_alloc(next,1);if(*data)memcpy(grown,*data,*capacity);disp_dealloc(*data);*data=grown;*capacity=next;}return true;}
static bool disp_tls_server_name(const char *text,size_t length,wchar_t **wide){if(!length||memchr(text,0,length)||length>INT_MAX)return false;int count=MultiByteToWideChar(CP_UTF8,MB_ERR_INVALID_CHARS,text,(int)length,NULL,0);if(count<=0)return false;wchar_t *value=(wchar_t*)disp_alloc(((size_t)count+1)*sizeof(wchar_t),_Alignof(wchar_t));if(MultiByteToWideChar(CP_UTF8,MB_ERR_INVALID_CHARS,text,(int)length,value,count)!=count){disp_dealloc(value);return false;}value[count]=0;*wide=value;return true;}
static bool disp_tls_flush_handshake(disp_tls_handshake_state *state){while(state->output_pos<state->output_len){size_t remaining=state->output_len-state->output_pos;int chunk=(int)(remaining>INT_MAX?INT_MAX:remaining);int count=send(state->socket,(const char*)state->output+state->output_pos,chunk,0);if(count<0){int code=disp_socket_error_code();if(disp_socket_would_block(code)){disp_reactor_offer(1000000ULL);return false;}state->error=disp_network_error_code("TLS handshake write",code);state->done=true;return true;}if(!count){state->error=disp_owned_bytes("TLS handshake write made no progress",strlen("TLS handshake write made no progress"));state->done=true;return true;}state->output_pos+=(size_t)count;}disp_dealloc(state->output);state->output=NULL;state->output_len=0;state->output_pos=0;return true;}
static bool disp_tls_receive_handshake(disp_tls_handshake_state *state){if(!disp_tls_reserve(&state->input,&state->input_cap,state->input_len+16384)){state->error=disp_owned_bytes("TLS handshake exceeded the 1 MiB safety limit",strlen("TLS handshake exceeded the 1 MiB safety limit"));state->done=true;return true;}int count=recv(state->socket,(char*)state->input+state->input_len,(int)(state->input_cap-state->input_len),0);if(count<0){int code=disp_socket_error_code();if(disp_socket_would_block(code)){disp_reactor_offer(1000000ULL);return false;}state->error=disp_network_error_code("TLS handshake read",code);state->done=true;return true;}if(!count){state->error=disp_owned_bytes("TLS peer closed during handshake",strlen("TLS peer closed during handshake"));state->done=true;return true;}state->input_len+=(size_t)count;state->need_input=false;return true;}
static bool disp_tls_acquire(disp_tls_handshake_state *state){TLS_PARAMETERS parameters={0};parameters.grbitDisabledProtocols=~(SP_PROT_TLS1_2_CLIENT|SP_PROT_TLS1_3_CLIENT);SCH_CREDENTIALS modern={0};modern.dwVersion=SCH_CREDENTIALS_VERSION;modern.dwFlags=SCH_USE_STRONG_CRYPTO|SCH_CRED_NO_DEFAULT_CREDS|SCH_CRED_AUTO_CRED_VALIDATION|SCH_CRED_REVOCATION_CHECK_CHAIN_EXCLUDE_ROOT;modern.cTlsParameters=1;modern.pTlsParameters=&parameters;TimeStamp expiry={0};SECURITY_STATUS status=AcquireCredentialsHandleW(NULL,UNISP_NAME_W,SECPKG_CRED_OUTBOUND,NULL,&modern,NULL,NULL,&state->credential,&expiry);if(status!=SEC_E_OK){SCHANNEL_CRED compatible={0};compatible.dwVersion=SCHANNEL_CRED_VERSION;compatible.grbitEnabledProtocols=SP_PROT_TLS1_2_CLIENT;compatible.dwFlags=SCH_USE_STRONG_CRYPTO|SCH_CRED_NO_DEFAULT_CREDS|SCH_CRED_AUTO_CRED_VALIDATION|SCH_CRED_REVOCATION_CHECK_CHAIN_EXCLUDE_ROOT;status=AcquireCredentialsHandleW(NULL,UNISP_NAME_W,SECPKG_CRED_OUTBOUND,NULL,&compatible,NULL,NULL,&state->credential,&expiry);}if(status!=SEC_E_OK){state->error=disp_tls_status_error("TLS credential acquisition",status);state->done=true;return false;}state->credential_valid=true;return true;}
static bool disp_tls_handshake_call(disp_tls_handshake_state *state){SecBuffer input_buffers[2]={{0}},output_buffer={0};SecBufferDesc input_desc={0},output_desc={0};SecBufferDesc *input_ptr=NULL;if(!state->first_call){input_buffers[0]=(SecBuffer){.cbBuffer=(unsigned long)state->input_len,.BufferType=SECBUFFER_TOKEN,.pvBuffer=state->input};input_buffers[1]=(SecBuffer){.BufferType=SECBUFFER_EMPTY};input_desc=(SecBufferDesc){.ulVersion=SECBUFFER_VERSION,.cBuffers=2,.pBuffers=input_buffers};input_ptr=&input_desc;}output_buffer.BufferType=SECBUFFER_TOKEN;output_desc=(SecBufferDesc){.ulVersion=SECBUFFER_VERSION,.cBuffers=1,.pBuffers=&output_buffer};unsigned long attributes=0;unsigned long requests=ISC_REQ_SEQUENCE_DETECT|ISC_REQ_REPLAY_DETECT|ISC_REQ_CONFIDENTIALITY|ISC_REQ_INTEGRITY|ISC_REQ_EXTENDED_ERROR|ISC_REQ_ALLOCATE_MEMORY|ISC_REQ_STREAM|ISC_REQ_USE_SUPPLIED_CREDS;TimeStamp expiry={0};SECURITY_STATUS status=InitializeSecurityContextW(&state->credential,state->first_call?NULL:&state->context,state->server_name,requests,0,SECURITY_NATIVE_DREP,input_ptr,0,&state->context,&output_desc,&attributes,&expiry);state->context_valid=true;state->first_call=false;if(status==SEC_I_COMPLETE_NEEDED||status==SEC_I_COMPLETE_AND_CONTINUE){SECURITY_STATUS completed=CompleteAuthToken(&state->context,&output_desc);if(completed!=SEC_E_OK){if(output_buffer.pvBuffer)FreeContextBuffer(output_buffer.pvBuffer);state->error=disp_tls_status_error("TLS authentication token completion",completed);state->done=true;return false;}}if(output_buffer.cbBuffer){state->output=(unsigned char*)disp_alloc(output_buffer.cbBuffer,1);memcpy(state->output,output_buffer.pvBuffer,output_buffer.cbBuffer);state->output_len=output_buffer.cbBuffer;}if(output_buffer.pvBuffer)FreeContextBuffer(output_buffer.pvBuffer);if(status==SEC_E_INCOMPLETE_MESSAGE){state->need_input=true;return true;}if(status!=SEC_E_OK&&status!=SEC_I_CONTINUE_NEEDED&&status!=SEC_I_COMPLETE_NEEDED&&status!=SEC_I_COMPLETE_AND_CONTINUE){state->error=disp_tls_status_error("TLS handshake",status);state->done=true;return false;}if(input_ptr){size_t extra=input_buffers[1].BufferType==SECBUFFER_EXTRA?input_buffers[1].cbBuffer:0;if(extra>state->input_len){state->error=disp_owned_bytes("TLS provider returned an invalid extra-data length",strlen("TLS provider returned an invalid extra-data length"));state->done=true;return false;}if(extra)memmove(state->input,state->input+state->input_len-extra,extra);state->input_len=extra;}if(status==SEC_E_OK||status==SEC_I_COMPLETE_NEEDED)state->handshake_complete=true;else state->need_input=state->input_len==0;return true;}
static disp_tls_state *disp_tls_state_finish(disp_tls_handshake_state *handshake){SecPkgContext_StreamSizes sizes={0};SECURITY_STATUS status=QueryContextAttributesW(&handshake->context,SECPKG_ATTR_STREAM_SIZES,&sizes);if(status!=SEC_E_OK){handshake->error=disp_tls_status_error("TLS stream-size query",status);handshake->done=true;return NULL;}SecPkgContext_ConnectionInfo connection={0};status=QueryContextAttributesW(&handshake->context,SECPKG_ATTR_CONNECTION_INFO,&connection);if(status!=SEC_E_OK){handshake->error=disp_tls_status_error("TLS connection query",status);handshake->done=true;return NULL;}disp_tls_state *state=(disp_tls_state*)disp_alloc_zeroed(1,sizeof(disp_tls_state),_Alignof(disp_tls_state));atomic_init(&state->refs,1);atomic_init(&state->closed,false);atomic_init(&state->read_busy,false);atomic_init(&state->write_busy,false);state->socket=handshake->socket;state->credential=handshake->credential;state->context=handshake->context;state->credential_valid=true;state->context_valid=true;state->sizes=sizes;state->protocol=connection.dwProtocol;state->server_name=handshake->server_name;state->encrypted=handshake->input;state->encrypted_len=handshake->input_len;state->encrypted_cap=handshake->input_cap;handshake->socket_valid=false;handshake->credential_valid=false;handshake->context_valid=false;handshake->server_name=NULL;handshake->input=NULL;handshake->input_len=handshake->input_cap=0;return state;}
static disp_tls_handshake_state *disp_tls_handshake_create(disp_tcp_state *tcp,const char *server_name,size_t server_name_len,bool has_timeout,uint64_t timeout,int line,int column){if(!tcp)dv_panic("TCP stream is unavailable",line,column);disp_tls_handshake_state *state=(disp_tls_handshake_state*)disp_alloc_zeroed(1,sizeof(disp_tls_handshake_state),_Alignof(disp_tls_handshake_state));state->tcp=tcp;state->has_deadline=has_timeout;state->timeout=timeout;state->line=line;state->column=column;state->first_call=true;if(!disp_tls_server_name(server_name,server_name_len,&state->server_name)){state->error=disp_owned_bytes("TLS server name must be non-empty valid UTF-8 without NUL",strlen("TLS server name must be non-empty valid UTF-8 without NUL"));state->done=true;}return state;}
static bool disp_tls_handshake_poll(disp_tls_handshake_state *state){if(!state||state->taken)dv_panic("TLS handshake future has already completed",0,0);if(state->done)return true;if(!state->started){state->started=true;uint64_t now=disp_time_now_nanos();state->deadline=UINT64_MAX-now<state->timeout?UINT64_MAX:now+state->timeout;if(state->has_deadline&&!state->timeout){state->error=disp_owned_bytes("TLS handshake timed out",strlen("TLS handshake timed out"));state->done=true;return true;}if(!disp_tcp_take_socket(state->tcp,&state->socket)){state->error=disp_owned_bytes("TCP stream is closed",strlen("TCP stream is closed"));state->done=true;return true;}state->socket_valid=true;if(!disp_tls_acquire(state))return true;}for(unsigned steps=0;steps<16&&!state->done;steps++){if(state->output_pos<state->output_len){if(!disp_tls_flush_handshake(state))break;if(state->done)break;}if(state->handshake_complete){disp_tls_state *stream=disp_tls_state_finish(state);if(stream){state->stream=(disp_native_tls_stream){.state=stream};state->ok=true;state->done=true;}break;}if(state->need_input){if(!disp_tls_receive_handshake(state))break;if(state->done)break;}if(!state->need_input&&!disp_tls_handshake_call(state))break;}if(!state->done&&state->has_deadline&&disp_time_now_nanos()>=state->deadline){state->error=disp_owned_bytes("TLS handshake timed out",strlen("TLS handshake timed out"));state->done=true;}if(!state->done)disp_reactor_offer(1000000ULL);return state->done;}
static void disp_tls_handshake_take(disp_tls_handshake_state *state,bool *ok,disp_native_tls_stream *stream,disp_native_string *error){if(!state->done||state->taken)dv_panic("TLS handshake result is not ready",0,0);state->taken=true;*ok=state->ok;*stream=state->stream;state->stream=(disp_native_tls_stream){0};*error=state->error;state->error=(disp_native_string){0};}
static void disp_tls_state_retain(disp_tls_state *state){if(!state)dv_panic("TLS stream is unavailable",0,0);size_t previous=atomic_fetch_add_explicit(&state->refs,1,memory_order_relaxed);if(previous==SIZE_MAX)dv_panic("TLS stream reference count overflow",0,0);}
static void disp_tls_state_close(disp_tls_state *state){if(!state||atomic_exchange_explicit(&state->closed,true,memory_order_acq_rel))return;if(state->context_valid){unsigned long shutdown=SCHANNEL_SHUTDOWN;SecBuffer control={.cbBuffer=sizeof(shutdown),.BufferType=SECBUFFER_TOKEN,.pvBuffer=&shutdown};SecBufferDesc control_desc={.ulVersion=SECBUFFER_VERSION,.cBuffers=1,.pBuffers=&control};if(ApplyControlToken(&state->context,&control_desc)==SEC_E_OK){SecBuffer output={.BufferType=SECBUFFER_TOKEN};SecBufferDesc output_desc={.ulVersion=SECBUFFER_VERSION,.cBuffers=1,.pBuffers=&output};unsigned long attributes=0;TimeStamp expiry={0};SECURITY_STATUS status=InitializeSecurityContextW(&state->credential,&state->context,NULL,ISC_REQ_SEQUENCE_DETECT|ISC_REQ_REPLAY_DETECT|ISC_REQ_CONFIDENTIALITY|ISC_REQ_INTEGRITY|ISC_REQ_ALLOCATE_MEMORY|ISC_REQ_STREAM,0,SECURITY_NATIVE_DREP,NULL,0,&state->context,&output_desc,&attributes,&expiry);if((status==SEC_E_OK||status==SEC_I_CONTEXT_EXPIRED)&&output.pvBuffer&&output.cbBuffer)send(state->socket,(const char*)output.pvBuffer,(int)output.cbBuffer,0);if(output.pvBuffer)FreeContextBuffer(output.pvBuffer);}}disp_socket_shutdown_handle(state->socket,2);disp_socket_close(state->socket);disp_runtime_release_handle();}
static void disp_tls_state_release(disp_tls_state *state){if(!state||atomic_fetch_sub_explicit(&state->refs,1,memory_order_acq_rel)!=1)return;atomic_thread_fence(memory_order_acquire);disp_tls_state_close(state);if(state->context_valid)DeleteSecurityContext(&state->context);if(state->credential_valid)FreeCredentialsHandle(&state->credential);disp_dealloc(state->server_name);disp_dealloc(state->encrypted);disp_dealloc(state->plain);disp_dealloc(state->post_output);disp_dealloc(state);}
static void disp_tls_stream_close(disp_native_tls_stream *stream){if(stream&&stream->state)disp_tls_state_close(stream->state);}
static void disp_tls_stream_drop(disp_native_tls_stream *stream){if(!stream||!stream->state)return;disp_tls_state_close(stream->state);disp_tls_state_release(stream->state);stream->state=NULL;}
static void disp_tls_handshake_drop(void *raw){disp_tls_handshake_state *state=(disp_tls_handshake_state*)raw;if(!state)return;if(state->stream.state)disp_tls_stream_drop(&state->stream);if(state->context_valid)DeleteSecurityContext(&state->context);if(state->credential_valid)FreeCredentialsHandle(&state->credential);if(state->socket_valid){disp_socket_shutdown_handle(state->socket,2);disp_socket_close(state->socket);disp_runtime_release_handle();}disp_tcp_state_release(state->tcp);disp_dealloc(state->server_name);disp_dealloc(state->input);disp_dealloc(state->output);disp_string_drop(&state->error);disp_dealloc(state);}
typedef enum { DISP_TLS_READ,DISP_TLS_WRITE } disp_tls_io_operation;
typedef struct {
    disp_tls_state *stream;
    disp_tls_io_operation operation;
    bool started;
    bool claimed;
    bool taken;
    bool done;
    bool ok;
    bool has_deadline;
    uint64_t timeout;
    uint64_t deadline;
    size_t limit;
    size_t offset;
    disp_native_string buffer;
    unsigned char *cipher;
    size_t cipher_len;
    size_t cipher_pos;
    disp_native_string error;
} disp_tls_io_state;
static void disp_tls_io_finish(disp_tls_io_state *state,bool ok){state->ok=ok;state->done=true;if(state->claimed){atomic_store_explicit(state->operation==DISP_TLS_READ?&state->stream->read_busy:&state->stream->write_busy,false,memory_order_release);state->claimed=false;}}
static void disp_tls_io_fail(disp_tls_io_state *state,disp_native_string error,bool corrupts_stream){state->error=error;if(corrupts_stream)disp_tls_state_close(state->stream);disp_tls_io_finish(state,false);}
static disp_tls_io_state *disp_tls_io_create(disp_tls_state *stream,disp_tls_io_operation operation,const char *bytes,size_t length,bool has_timeout,uint64_t timeout,int line,int column){if(!stream)dv_panic("TLS stream is unavailable",line,column);if(operation==DISP_TLS_READ&&length>DISP_TCP_READ_LIMIT)dv_panic("TLS read limit exceeds the 16 MiB safety limit",line,column);disp_tls_io_state *state=(disp_tls_io_state*)disp_alloc_zeroed(1,sizeof(disp_tls_io_state),_Alignof(disp_tls_io_state));state->stream=stream;disp_tls_state_retain(stream);state->operation=operation;state->has_deadline=has_timeout;state->timeout=timeout;state->limit=length;if(operation==DISP_TLS_WRITE&&length)state->buffer=disp_owned_bytes(bytes,length);return state;}
static bool disp_tls_read_plain(disp_tls_io_state *operation){disp_tls_state *stream=operation->stream;if(stream->plain_pos>=stream->plain_len)return false;size_t available=stream->plain_len-stream->plain_pos;size_t count=available<operation->limit?available:operation->limit;if(count){operation->buffer.data=(char*)disp_alloc(count,1);memcpy(operation->buffer.data,stream->plain+stream->plain_pos,count);operation->buffer.len=operation->buffer.cap=count;}stream->plain_pos+=count;if(stream->plain_pos==stream->plain_len){disp_dealloc(stream->plain);stream->plain=NULL;stream->plain_len=stream->plain_pos=0;}disp_tls_io_finish(operation,true);return true;}
static bool disp_tls_post_handshake_step(disp_tls_io_state *operation){disp_tls_state *stream=operation->stream;if(stream->post_output_pos<stream->post_output_len){size_t remaining=stream->post_output_len-stream->post_output_pos;int chunk=(int)(remaining>INT_MAX?INT_MAX:remaining);int count=send(stream->socket,(const char*)stream->post_output+stream->post_output_pos,chunk,0);if(count<0){int code=disp_socket_error_code();if(disp_socket_would_block(code))return false;disp_tls_io_fail(operation,disp_network_error_code("TLS post-handshake write",code),true);return true;}if(!count){disp_tls_io_fail(operation,disp_owned_bytes("TLS post-handshake write made no progress",strlen("TLS post-handshake write made no progress")),true);return true;}stream->post_output_pos+=(size_t)count;return true;}if(stream->post_output){disp_dealloc(stream->post_output);stream->post_output=NULL;stream->post_output_len=stream->post_output_pos=0;}if(stream->post_complete){stream->post_complete=false;stream->post_handshake=false;stream->post_need_input=false;return true;}if(stream->post_need_input&&stream->encrypted_len==0){if(!disp_tls_reserve(&stream->encrypted,&stream->encrypted_cap,16384)){disp_tls_io_fail(operation,disp_owned_bytes("TLS post-handshake message exceeded the safety limit",strlen("TLS post-handshake message exceeded the safety limit")),true);return true;}int count=recv(stream->socket,(char*)stream->encrypted,(int)stream->encrypted_cap,0);if(count<0){int code=disp_socket_error_code();if(disp_socket_would_block(code))return false;disp_tls_io_fail(operation,disp_network_error_code("TLS post-handshake read",code),true);return true;}if(!count){disp_tls_io_fail(operation,disp_owned_bytes("TLS peer closed during post-handshake authentication",strlen("TLS peer closed during post-handshake authentication")),true);return true;}stream->encrypted_len=(size_t)count;stream->post_need_input=false;}SecBuffer input_buffers[2]={{.cbBuffer=(unsigned long)stream->encrypted_len,.BufferType=SECBUFFER_TOKEN,.pvBuffer=stream->encrypted},{.BufferType=SECBUFFER_EMPTY}};SecBufferDesc input_desc={.ulVersion=SECBUFFER_VERSION,.cBuffers=2,.pBuffers=input_buffers};SecBuffer output={.BufferType=SECBUFFER_TOKEN};SecBufferDesc output_desc={.ulVersion=SECBUFFER_VERSION,.cBuffers=1,.pBuffers=&output};unsigned long attributes=0;unsigned long requests=ISC_REQ_SEQUENCE_DETECT|ISC_REQ_REPLAY_DETECT|ISC_REQ_CONFIDENTIALITY|ISC_REQ_INTEGRITY|ISC_REQ_EXTENDED_ERROR|ISC_REQ_ALLOCATE_MEMORY|ISC_REQ_STREAM|ISC_REQ_USE_SUPPLIED_CREDS;TimeStamp expiry={0};SECURITY_STATUS status=InitializeSecurityContextW(&stream->credential,&stream->context,stream->server_name,requests,0,SECURITY_NATIVE_DREP,&input_desc,0,NULL,&output_desc,&attributes,&expiry);if(status==SEC_I_COMPLETE_NEEDED||status==SEC_I_COMPLETE_AND_CONTINUE){SECURITY_STATUS completed=CompleteAuthToken(&stream->context,&output_desc);if(completed!=SEC_E_OK){if(output.pvBuffer)FreeContextBuffer(output.pvBuffer);disp_tls_io_fail(operation,disp_tls_status_error("TLS post-handshake token completion",completed),true);return true;}}if(output.cbBuffer){stream->post_output=(unsigned char*)disp_alloc(output.cbBuffer,1);memcpy(stream->post_output,output.pvBuffer,output.cbBuffer);stream->post_output_len=output.cbBuffer;}if(output.pvBuffer)FreeContextBuffer(output.pvBuffer);if(status==SEC_E_INCOMPLETE_MESSAGE){stream->post_need_input=true;return true;}if(status!=SEC_E_OK&&status!=SEC_I_CONTINUE_NEEDED&&status!=SEC_I_COMPLETE_NEEDED&&status!=SEC_I_COMPLETE_AND_CONTINUE){disp_tls_io_fail(operation,disp_tls_status_error("TLS post-handshake authentication",status),true);return true;}size_t extra=input_buffers[1].BufferType==SECBUFFER_EXTRA?input_buffers[1].cbBuffer:0;if(extra>stream->encrypted_len){disp_tls_io_fail(operation,disp_owned_bytes("TLS provider returned invalid post-handshake data",strlen("TLS provider returned invalid post-handshake data")),true);return true;}if(extra)memmove(stream->encrypted,stream->encrypted+stream->encrypted_len-extra,extra);stream->encrypted_len=extra;if(status==SEC_E_OK||status==SEC_I_COMPLETE_NEEDED){stream->post_complete=true;stream->post_need_input=false;}else stream->post_need_input=stream->encrypted_len==0;return true;}
static bool disp_tls_decrypt(disp_tls_io_state *operation){disp_tls_state *stream=operation->stream;if(!stream->encrypted_len)return false;SecBuffer buffers[4]={{.cbBuffer=(unsigned long)stream->encrypted_len,.BufferType=SECBUFFER_DATA,.pvBuffer=stream->encrypted},{.BufferType=SECBUFFER_EMPTY},{.BufferType=SECBUFFER_EMPTY},{.BufferType=SECBUFFER_EMPTY}};SecBufferDesc descriptor={.ulVersion=SECBUFFER_VERSION,.cBuffers=4,.pBuffers=buffers};SECURITY_STATUS status=DecryptMessage(&stream->context,&descriptor,0,NULL);if(status==SEC_E_INCOMPLETE_MESSAGE)return false;if(status==SEC_I_CONTEXT_EXPIRED){stream->encrypted_len=0;disp_tls_io_finish(operation,true);return true;}if(status!=SEC_E_OK&&status!=SEC_I_RENEGOTIATE){disp_tls_io_fail(operation,disp_tls_status_error("TLS decrypt",status),true);return true;}unsigned char *plain=NULL;size_t plain_len=0;size_t extra=0;for(size_t i=0;i<4;i++){if(buffers[i].BufferType==SECBUFFER_DATA){plain=(unsigned char*)buffers[i].pvBuffer;plain_len=buffers[i].cbBuffer;}else if(buffers[i].BufferType==SECBUFFER_EXTRA)extra=buffers[i].cbBuffer;}if(extra>stream->encrypted_len){disp_tls_io_fail(operation,disp_owned_bytes("TLS provider returned an invalid record length",strlen("TLS provider returned an invalid record length")),true);return true;}if(status==SEC_I_RENEGOTIATE){if(extra)memmove(stream->encrypted,stream->encrypted+stream->encrypted_len-extra,extra);stream->encrypted_len=extra;if(stream->protocol!=SP_PROT_TLS1_3_CLIENT){disp_tls_io_fail(operation,disp_owned_bytes("legacy TLS renegotiation is rejected by the safe client",strlen("legacy TLS renegotiation is rejected by the safe client")),true);return true;}stream->post_handshake=true;stream->post_need_input=stream->encrypted_len==0;return false;}unsigned char *plain_copy=NULL;if(plain_len){plain_copy=(unsigned char*)disp_alloc(plain_len,1);memcpy(plain_copy,plain,plain_len);}if(extra)memmove(stream->encrypted,stream->encrypted+stream->encrypted_len-extra,extra);stream->encrypted_len=extra;if(!plain_len)return false;size_t count=plain_len<operation->limit?plain_len:operation->limit;if(count){operation->buffer.data=(char*)disp_alloc(count,1);memcpy(operation->buffer.data,plain_copy,count);operation->buffer.len=operation->buffer.cap=count;}if(plain_len>count){size_t remaining=plain_len-count;stream->plain=(unsigned char*)disp_alloc(remaining,1);memcpy(stream->plain,plain_copy+count,remaining);stream->plain_len=remaining;stream->plain_pos=0;}disp_dealloc(plain_copy);disp_tls_io_finish(operation,true);return true;}
static bool disp_tls_receive_record(disp_tls_io_state *operation){disp_tls_state *stream=operation->stream;size_t maximum=(size_t)stream->sizes.cbHeader+(size_t)stream->sizes.cbMaximumMessage+(size_t)stream->sizes.cbTrailer;if(maximum>1048576ULL||!disp_tls_reserve(&stream->encrypted,&stream->encrypted_cap,stream->encrypted_len+(maximum?maximum:16384))){disp_tls_io_fail(operation,disp_owned_bytes("TLS record exceeds the 1 MiB safety limit",strlen("TLS record exceeds the 1 MiB safety limit")),true);return true;}size_t available=stream->encrypted_cap-stream->encrypted_len;int chunk=(int)(available>INT_MAX?INT_MAX:available);int count=recv(stream->socket,(char*)stream->encrypted+stream->encrypted_len,chunk,0);if(count<0){int code=disp_socket_error_code();if(disp_socket_would_block(code))return false;disp_tls_io_fail(operation,disp_network_error_code("TLS read",code),true);return true;}if(!count){disp_tls_io_fail(operation,disp_owned_bytes("TLS connection ended without a close notification",strlen("TLS connection ended without a close notification")),true);return true;}stream->encrypted_len+=(size_t)count;return false;}
static bool disp_tls_encrypt_next(disp_tls_io_state *operation){disp_tls_state *stream=operation->stream;if(operation->offset>=operation->limit)return false;size_t remaining=operation->limit-operation->offset;size_t count=remaining<stream->sizes.cbMaximumMessage?remaining:stream->sizes.cbMaximumMessage;if(!count){disp_tls_io_fail(operation,disp_owned_bytes("TLS provider reported a zero maximum message size",strlen("TLS provider reported a zero maximum message size")),true);return true;}size_t capacity=(size_t)stream->sizes.cbHeader+count+(size_t)stream->sizes.cbTrailer;operation->cipher=(unsigned char*)disp_alloc(capacity,1);memcpy(operation->cipher+stream->sizes.cbHeader,operation->buffer.data+operation->offset,count);SecBuffer buffers[4]={{.cbBuffer=stream->sizes.cbHeader,.BufferType=SECBUFFER_STREAM_HEADER,.pvBuffer=operation->cipher},{.cbBuffer=(unsigned long)count,.BufferType=SECBUFFER_DATA,.pvBuffer=operation->cipher+stream->sizes.cbHeader},{.cbBuffer=stream->sizes.cbTrailer,.BufferType=SECBUFFER_STREAM_TRAILER,.pvBuffer=operation->cipher+stream->sizes.cbHeader+count},{.BufferType=SECBUFFER_EMPTY}};SecBufferDesc descriptor={.ulVersion=SECBUFFER_VERSION,.cBuffers=4,.pBuffers=buffers};SECURITY_STATUS status=EncryptMessage(&stream->context,0,&descriptor,0);if(status!=SEC_E_OK){disp_dealloc(operation->cipher);operation->cipher=NULL;disp_tls_io_fail(operation,disp_tls_status_error("TLS encrypt",status),true);return true;}operation->cipher_len=(size_t)buffers[0].cbBuffer+(size_t)buffers[1].cbBuffer+(size_t)buffers[2].cbBuffer;operation->cipher_pos=0;operation->offset+=count;return false;}
static bool disp_tls_flush_cipher(disp_tls_io_state *operation){while(operation->cipher_pos<operation->cipher_len){size_t remaining=operation->cipher_len-operation->cipher_pos;int chunk=(int)(remaining>INT_MAX?INT_MAX:remaining);int count=send(operation->stream->socket,(const char*)operation->cipher+operation->cipher_pos,chunk,0);if(count<0){int code=disp_socket_error_code();if(disp_socket_would_block(code))return false;disp_tls_io_fail(operation,disp_network_error_code("TLS write",code),true);return true;}if(!count){disp_tls_io_fail(operation,disp_owned_bytes("TLS write made no progress",strlen("TLS write made no progress")),true);return true;}operation->cipher_pos+=(size_t)count;}disp_dealloc(operation->cipher);operation->cipher=NULL;operation->cipher_len=operation->cipher_pos=0;return true;}
static bool disp_tls_io_poll(disp_tls_io_state *state){if(!state||state->taken)dv_panic("TLS I/O future has already completed",0,0);if(state->done)return true;if(!state->started){state->started=true;uint64_t now=disp_time_now_nanos();state->deadline=UINT64_MAX-now<state->timeout?UINT64_MAX:now+state->timeout;if(state->has_deadline&&!state->timeout){disp_tls_io_fail(state,disp_owned_bytes(state->operation==DISP_TLS_READ?"TLS read timed out":"TLS write timed out",state->operation==DISP_TLS_READ?strlen("TLS read timed out"):strlen("TLS write timed out")),false);return true;}}if(atomic_load_explicit(&state->stream->closed,memory_order_acquire)){disp_tls_io_fail(state,disp_owned_bytes("TLS stream is closed",strlen("TLS stream is closed")),false);return true;}atomic_bool *busy=state->operation==DISP_TLS_READ?&state->stream->read_busy:&state->stream->write_busy;if(!state->claimed){if(!disp_tcp_claim(busy)){if(state->has_deadline&&disp_time_now_nanos()>=state->deadline){disp_tls_io_fail(state,disp_owned_bytes("TLS operation timed out",strlen("TLS operation timed out")),false);return true;}disp_reactor_offer(1000000ULL);return false;}state->claimed=true;}if(state->operation==DISP_TLS_WRITE&&state->stream->post_handshake){if(state->has_deadline&&disp_time_now_nanos()>=state->deadline){disp_tls_io_fail(state,disp_owned_bytes("TLS write timed out",strlen("TLS write timed out")),false);return true;}disp_reactor_offer(1000000ULL);return false;}if(!state->limit){disp_tls_io_finish(state,true);return true;}for(unsigned steps=0;steps<16&&!state->done;steps++){if(state->operation==DISP_TLS_READ){if(state->stream->post_handshake){if(!disp_tls_post_handshake_step(state))break;if(state->done)break;continue;}if(disp_tls_read_plain(state))break;if(disp_tls_decrypt(state))break;if(state->stream->post_handshake){if(atomic_load_explicit(&state->stream->write_busy,memory_order_acquire)){disp_tls_io_fail(state,disp_owned_bytes("TLS post-handshake authentication conflicted with an active write",strlen("TLS post-handshake authentication conflicted with an active write")),true);break;}continue;}if(!disp_tls_receive_record(state))break;}else{if(state->cipher){if(!disp_tls_flush_cipher(state))break;if(state->done)break;}if(state->offset==state->limit){disp_tls_io_finish(state,true);break;}if(disp_tls_encrypt_next(state))break;}}if(!state->done&&state->has_deadline&&disp_time_now_nanos()>=state->deadline){bool corrupts=state->operation==DISP_TLS_WRITE&&(state->offset||state->cipher);disp_tls_io_fail(state,disp_owned_bytes(state->operation==DISP_TLS_READ?"TLS read timed out":"TLS write timed out",state->operation==DISP_TLS_READ?strlen("TLS read timed out"):strlen("TLS write timed out")),corrupts);}if(!state->done)disp_reactor_offer(1000000ULL);return state->done;}
static void disp_tls_io_take(disp_tls_io_state *state,bool *ok,disp_native_string *bytes,size_t *written,disp_native_string *error){if(!state->done||state->taken)dv_panic("TLS I/O result is not ready",0,0);state->taken=true;*ok=state->ok;if(state->operation==DISP_TLS_READ){*bytes=state->buffer;state->buffer=(disp_native_string){0};}else *written=state->offset;*error=state->error;state->error=(disp_native_string){0};}
static void disp_tls_io_drop(void *raw){disp_tls_io_state *state=(disp_tls_io_state*)raw;if(!state)return;if(state->operation==DISP_TLS_WRITE&&!state->done&&(state->offset||state->cipher))disp_tls_state_close(state->stream);if(state->claimed)atomic_store_explicit(state->operation==DISP_TLS_READ?&state->stream->read_busy:&state->stream->write_busy,false,memory_order_release);disp_string_drop(&state->buffer);disp_dealloc(state->cipher);disp_string_drop(&state->error);disp_tls_state_release(state->stream);disp_dealloc(state);}
static bool disp_tls_stream_read(disp_native_tls_stream *stream,size_t limit,disp_native_string *bytes,disp_native_string *error,int line,int column){disp_tls_io_state *state=disp_tls_io_create(stream->state,DISP_TLS_READ,NULL,limit,false,0,line,column);while(!disp_tls_io_poll(state))disp_time_sleep(1000000ULL);bool ok=false;size_t ignored=0;disp_tls_io_take(state,&ok,bytes,&ignored,error);disp_tls_io_drop(state);return ok;}
static bool disp_tls_stream_write(disp_native_tls_stream *stream,const char *bytes,size_t length,size_t *written,disp_native_string *error,int line,int column){disp_tls_io_state *state=disp_tls_io_create(stream->state,DISP_TLS_WRITE,bytes,length,false,0,line,column);while(!disp_tls_io_poll(state))disp_time_sleep(1000000ULL);bool ok=false;disp_native_string ignored={0};disp_tls_io_take(state,&ok,&ignored,written,error);disp_tls_io_drop(state);return ok;}
#else
struct disp_tls_state {
    atomic_size_t refs;
    atomic_bool closed;
    atomic_bool read_busy;
    atomic_bool write_busy;
    disp_socket_handle socket;
    SSL_CTX *context;
    SSL *session;
};
typedef struct {
    disp_tcp_state *tcp;
    disp_socket_handle socket;
    bool socket_valid;
    bool started;
    bool done;
    bool taken;
    bool ok;
    bool has_deadline;
    uint64_t timeout;
    uint64_t deadline;
    int line;
    int column;
    char *server_name;
    size_t server_name_len;
    SSL_CTX *context;
    SSL *session;
    disp_native_tls_stream stream;
    disp_native_string error;
} disp_tls_handshake_state;
typedef enum { DISP_TLS_READ,DISP_TLS_WRITE } disp_tls_io_operation;
typedef struct {
    disp_tls_state *stream;
    disp_tls_io_operation operation;
    bool started;
    bool done;
    bool taken;
    bool ok;
    bool claimed;
    bool write_attempted;
    bool has_deadline;
    uint64_t timeout;
    uint64_t deadline;
    size_t limit;
    size_t offset;
    disp_native_string buffer;
    disp_native_string error;
} disp_tls_io_state;
static int disp_tls_bio_create(BIO *bio){BIO_set_init(bio,1);BIO_set_data(bio,NULL);BIO_set_shutdown(bio,BIO_NOCLOSE);return 1;}
static int disp_tls_bio_destroy(BIO *bio){if(!bio)return 0;BIO_set_init(bio,0);BIO_set_data(bio,NULL);return 1;}
static int disp_tls_bio_read(BIO *bio,char *bytes,int length){
    BIO_clear_retry_flags(bio);
    if(!bytes||length<=0)return 0;
    disp_socket_handle socket=(disp_socket_handle)(intptr_t)BIO_get_data(bio);
    ssize_t count=recv(socket,bytes,(size_t)length,0);
    if(count>=0)return (int)count;
    int code=disp_socket_error_code();
    if(code==EINTR||disp_socket_would_block(code))BIO_set_retry_read(bio);
    return -1;
}
static int disp_tls_bio_write(BIO *bio,const char *bytes,int length){
    BIO_clear_retry_flags(bio);
    if(!bytes||length<=0)return 0;
    disp_socket_handle socket=(disp_socket_handle)(intptr_t)BIO_get_data(bio);
    ssize_t count=send(socket,bytes,(size_t)length,
#ifdef MSG_NOSIGNAL
        MSG_NOSIGNAL
#else
        0
#endif
    );
    if(count>=0)return (int)count;
    int code=disp_socket_error_code();
    if(code==EINTR||disp_socket_would_block(code))BIO_set_retry_write(bio);
    return -1;
}
static long disp_tls_bio_control(BIO *bio,int command,long argument,void *pointer){
    (void)pointer;
    switch(command){
        case BIO_CTRL_FLUSH:return 1;
        case BIO_CTRL_DUP:return 1;
        case BIO_CTRL_GET_CLOSE:return BIO_get_shutdown(bio);
        case BIO_CTRL_SET_CLOSE:BIO_set_shutdown(bio,(int)argument);return 1;
        case BIO_CTRL_PENDING:
        case BIO_CTRL_WPENDING:return 0;
        default:return 0;
    }
}
static int disp_tls_bio_puts(BIO *bio,const char *text){return disp_tls_bio_write(bio,text,(int)strlen(text));}
static pthread_once_t disp_tls_bio_once=PTHREAD_ONCE_INIT;
static BIO_METHOD *disp_tls_bio_shared_method=NULL;
static void disp_tls_bio_initialize(void){
    BIO_METHOD *method=BIO_meth_new(BIO_get_new_index()|BIO_TYPE_SOURCE_SINK,"DISP nonblocking socket");
    if(!method)return;
    if(BIO_meth_set_create(method,disp_tls_bio_create)!=1||
       BIO_meth_set_destroy(method,disp_tls_bio_destroy)!=1||
       BIO_meth_set_read(method,disp_tls_bio_read)!=1||
       BIO_meth_set_write(method,disp_tls_bio_write)!=1||
       BIO_meth_set_ctrl(method,disp_tls_bio_control)!=1||
       BIO_meth_set_puts(method,disp_tls_bio_puts)!=1){
        BIO_meth_free(method);
        return;
    }
    disp_tls_bio_shared_method=method;
}
static BIO_METHOD *disp_tls_bio_method(void){pthread_once(&disp_tls_bio_once,disp_tls_bio_initialize);return disp_tls_bio_shared_method;}
static disp_native_string disp_tls_openssl_error(const char *operation,int ssl_error,int os_error){
    char detail[256]={0};
    unsigned long code=ERR_get_error();
    if(code)ERR_error_string_n(code,detail,sizeof(detail));
    else if(ssl_error==SSL_ERROR_SYSCALL&&os_error)snprintf(detail,sizeof(detail),"%s",strerror(os_error));
    else snprintf(detail,sizeof(detail),"TLS provider error %d",ssl_error);
    char message[448];
    int length=snprintf(message,sizeof(message),"%s failed: %s",operation,detail);
    if(length<0)length=0;
    if((size_t)length>=sizeof(message))length=(int)sizeof(message)-1;
    return disp_owned_bytes(message,(size_t)length);
}
static bool disp_tls_server_name(const char *text,size_t length,char **name){
    if(!length||length>INT_MAX||memchr(text,0,length)||!disp_utf8_valid(text,length))return false;
    char *copy=(char*)disp_alloc(length+1,1);
    memcpy(copy,text,length);
    copy[length]=0;
    *name=copy;
    return true;
}
static bool disp_tls_configure_name(SSL *session,const char *server_name,size_t server_name_len){
    unsigned char address[sizeof(struct in6_addr)];
    X509_VERIFY_PARAM *parameters=SSL_get0_param(session);
    if(!parameters)return false;
    X509_VERIFY_PARAM_set_hostflags(parameters,X509_CHECK_FLAG_NO_PARTIAL_WILDCARDS);
    if(inet_pton(AF_INET,server_name,address)==1||inet_pton(AF_INET6,server_name,address)==1)
        return X509_VERIFY_PARAM_set1_ip_asc(parameters,server_name)==1;
    return SSL_set_tlsext_host_name(session,server_name)==1&&
           X509_VERIFY_PARAM_set1_host(parameters,server_name,server_name_len)==1;
}
static bool disp_tls_handshake_initialize(disp_tls_handshake_state *state){
    BIO_METHOD *method=disp_tls_bio_method();
    if(!method){
        state->error=disp_owned_bytes("TLS socket provider initialization failed",strlen("TLS socket provider initialization failed"));
        state->done=true;
        return false;
    }
    ERR_clear_error();
    state->context=SSL_CTX_new(TLS_client_method());
    if(!state->context){
        state->error=disp_tls_openssl_error("TLS context creation",SSL_ERROR_SSL,0);
        state->done=true;
        return false;
    }
    SSL_CTX_set_verify(state->context,SSL_VERIFY_PEER,NULL);
    SSL_CTX_set_options(state->context,SSL_OP_NO_COMPRESSION);
#ifdef SSL_OP_NO_RENEGOTIATION
    SSL_CTX_set_options(state->context,SSL_OP_NO_RENEGOTIATION);
#endif
    if(SSL_CTX_set_min_proto_version(state->context,TLS1_2_VERSION)!=1||
       SSL_CTX_set_default_verify_paths(state->context)!=1){
        state->error=disp_tls_openssl_error("TLS trust configuration",SSL_ERROR_SSL,0);
        state->done=true;
        return false;
    }
    state->session=SSL_new(state->context);
    if(!state->session){
        state->error=disp_tls_openssl_error("TLS session creation",SSL_ERROR_SSL,0);
        state->done=true;
        return false;
    }
    SSL_set_mode(state->session,SSL_MODE_ENABLE_PARTIAL_WRITE|SSL_MODE_ACCEPT_MOVING_WRITE_BUFFER);
    if(!disp_tls_configure_name(state->session,state->server_name,state->server_name_len)){
        state->error=disp_tls_openssl_error("TLS server-name verification setup",SSL_ERROR_SSL,0);
        state->done=true;
        return false;
    }
    BIO *bio=BIO_new(method);
    if(!bio){
        state->error=disp_tls_openssl_error("TLS socket allocation",SSL_ERROR_SSL,0);
        state->done=true;
        return false;
    }
    BIO_set_data(bio,(void*)(intptr_t)state->socket);
    BIO_set_init(bio,1);
    BIO_set_shutdown(bio,BIO_NOCLOSE);
    SSL_set_bio(state->session,bio,bio);
    SSL_set_connect_state(state->session);
    return true;
}
static disp_tls_state *disp_tls_state_finish(disp_tls_handshake_state *handshake){
    disp_tls_state *state=(disp_tls_state*)disp_alloc_zeroed(1,sizeof(disp_tls_state),_Alignof(disp_tls_state));
    atomic_init(&state->refs,1);
    atomic_init(&state->closed,false);
    atomic_init(&state->read_busy,false);
    atomic_init(&state->write_busy,false);
    state->socket=handshake->socket;
    state->context=handshake->context;
    state->session=handshake->session;
    handshake->socket_valid=false;
    handshake->context=NULL;
    handshake->session=NULL;
    return state;
}
static disp_tls_handshake_state *disp_tls_handshake_create(disp_tcp_state *tcp,const char *server_name,size_t server_name_len,bool has_timeout,uint64_t timeout,int line,int column){
    if(!tcp)dv_panic("TCP stream is unavailable",line,column);
    disp_tls_handshake_state *state=(disp_tls_handshake_state*)disp_alloc_zeroed(1,sizeof(disp_tls_handshake_state),_Alignof(disp_tls_handshake_state));
    state->tcp=tcp;
    state->socket=DISP_INVALID_SOCKET;
    state->has_deadline=has_timeout;
    state->timeout=timeout;
    state->line=line;
    state->column=column;
    state->server_name_len=server_name_len;
    if(!disp_tls_server_name(server_name,server_name_len,&state->server_name)){
        state->error=disp_owned_bytes("TLS server name must be non-empty valid UTF-8 without NUL",strlen("TLS server name must be non-empty valid UTF-8 without NUL"));
        state->done=true;
    }
    return state;
}
static bool disp_tls_handshake_poll(disp_tls_handshake_state *state){
    if(!state||state->taken)dv_panic("TLS handshake future has already completed",0,0);
    if(state->done)return true;
    if(!state->started){
        state->started=true;
        uint64_t now=disp_time_now_nanos();
        state->deadline=UINT64_MAX-now<state->timeout?UINT64_MAX:now+state->timeout;
        if(state->has_deadline&&!state->timeout){
            state->error=disp_owned_bytes("TLS handshake timed out",strlen("TLS handshake timed out"));
            state->done=true;
            return true;
        }
        if(!disp_tcp_take_socket(state->tcp,&state->socket)){
            state->error=disp_owned_bytes("TCP stream is closed",strlen("TCP stream is closed"));
            state->done=true;
            return true;
        }
        state->socket_valid=true;
        if(!disp_tls_handshake_initialize(state))return true;
    }
    if(state->has_deadline&&disp_time_now_nanos()>=state->deadline){
        state->error=disp_owned_bytes("TLS handshake timed out",strlen("TLS handshake timed out"));
        state->done=true;
        return true;
    }
    ERR_clear_error();
    int result=SSL_connect(state->session);
    int os_error=errno;
    if(result==1){
        if(SSL_get_verify_result(state->session)!=X509_V_OK||!SSL_get0_peer_certificate(state->session)){
            state->error=disp_owned_bytes("TLS certificate verification failed",strlen("TLS certificate verification failed"));
            state->done=true;
            return true;
        }
        disp_tls_state *stream=disp_tls_state_finish(state);
        state->stream=(disp_native_tls_stream){.state=stream};
        state->ok=true;
        state->done=true;
        return true;
    }
    int ssl_error=SSL_get_error(state->session,result);
    if(ssl_error!=SSL_ERROR_WANT_READ&&ssl_error!=SSL_ERROR_WANT_WRITE){
        state->error=disp_tls_openssl_error("TLS handshake",ssl_error,os_error);
        state->done=true;
        return true;
    }
    if(state->has_deadline&&disp_time_now_nanos()>=state->deadline){
        state->error=disp_owned_bytes("TLS handshake timed out",strlen("TLS handshake timed out"));
        state->done=true;
        return true;
    }
    disp_reactor_offer(1000000ULL);
    return false;
}
static void disp_tls_handshake_take(disp_tls_handshake_state *state,bool *ok,disp_native_tls_stream *stream,disp_native_string *error){
    if(!state->done||state->taken)dv_panic("TLS handshake result is not ready",0,0);
    state->taken=true;
    *ok=state->ok;
    *stream=state->stream;
    state->stream=(disp_native_tls_stream){0};
    *error=state->error;
    state->error=(disp_native_string){0};
}
static void disp_tls_state_retain(disp_tls_state *state){
    if(!state)dv_panic("TLS stream is unavailable",0,0);
    size_t previous=atomic_fetch_add_explicit(&state->refs,1,memory_order_relaxed);
    if(previous==SIZE_MAX)dv_panic("TLS stream reference count overflow",0,0);
}
static void disp_tls_state_close(disp_tls_state *state){
    if(!state||atomic_exchange_explicit(&state->closed,true,memory_order_acq_rel))return;
    if(state->session){ERR_clear_error();SSL_shutdown(state->session);}
    disp_socket_shutdown_handle(state->socket,2);
    disp_socket_close(state->socket);
    disp_runtime_release_handle();
}
static void disp_tls_state_release(disp_tls_state *state){
    if(!state||atomic_fetch_sub_explicit(&state->refs,1,memory_order_acq_rel)!=1)return;
    atomic_thread_fence(memory_order_acquire);
    disp_tls_state_close(state);
    SSL_free(state->session);
    SSL_CTX_free(state->context);
    disp_dealloc(state);
}
static void disp_tls_stream_close(disp_native_tls_stream *stream){if(stream&&stream->state)disp_tls_state_close(stream->state);}
static void disp_tls_stream_drop(disp_native_tls_stream *stream){if(!stream||!stream->state)return;disp_tls_state_close(stream->state);disp_tls_state_release(stream->state);stream->state=NULL;}
static void disp_tls_handshake_drop(void *raw){
    disp_tls_handshake_state *state=(disp_tls_handshake_state*)raw;
    if(!state)return;
    if(state->stream.state)disp_tls_stream_drop(&state->stream);
    SSL_free(state->session);
    SSL_CTX_free(state->context);
    if(state->socket_valid){
        disp_socket_shutdown_handle(state->socket,2);
        disp_socket_close(state->socket);
        disp_runtime_release_handle();
    }
    disp_tcp_state_release(state->tcp);
    disp_dealloc(state->server_name);
    disp_string_drop(&state->error);
    disp_dealloc(state);
}
static void disp_tls_io_finish(disp_tls_io_state *state,bool ok){
    state->ok=ok;
    state->done=true;
    if(state->claimed){
        atomic_store_explicit(state->operation==DISP_TLS_READ?&state->stream->read_busy:&state->stream->write_busy,false,memory_order_release);
        state->claimed=false;
    }
}
static void disp_tls_io_fail(disp_tls_io_state *state,disp_native_string error,bool corrupts_stream){
    if(state->operation==DISP_TLS_READ)disp_string_drop(&state->buffer);
    state->error=error;
    if(corrupts_stream)disp_tls_state_close(state->stream);
    disp_tls_io_finish(state,false);
}
static disp_tls_io_state *disp_tls_io_create(disp_tls_state *stream,disp_tls_io_operation operation,const char *bytes,size_t length,bool has_timeout,uint64_t timeout,int line,int column){
    if(!stream)dv_panic("TLS stream is unavailable",line,column);
    if(operation==DISP_TLS_READ&&length>DISP_TCP_READ_LIMIT)dv_panic("TLS read limit exceeds the 16 MiB safety limit",line,column);
    disp_tls_io_state *state=(disp_tls_io_state*)disp_alloc_zeroed(1,sizeof(disp_tls_io_state),_Alignof(disp_tls_io_state));
    state->stream=stream;
    disp_tls_state_retain(stream);
    state->operation=operation;
    state->has_deadline=has_timeout;
    state->timeout=timeout;
    state->limit=length;
    if(operation==DISP_TLS_WRITE&&length)state->buffer=disp_owned_bytes(bytes,length);
    return state;
}
static bool disp_tls_io_poll(disp_tls_io_state *state){
    if(!state||state->taken)dv_panic("TLS I/O future has already completed",0,0);
    if(state->done)return true;
    if(!state->started){
        state->started=true;
        uint64_t now=disp_time_now_nanos();
        state->deadline=UINT64_MAX-now<state->timeout?UINT64_MAX:now+state->timeout;
        if(state->has_deadline&&!state->timeout){
            disp_tls_io_fail(state,disp_owned_bytes(state->operation==DISP_TLS_READ?"TLS read timed out":"TLS write timed out",state->operation==DISP_TLS_READ?strlen("TLS read timed out"):strlen("TLS write timed out")),false);
            return true;
        }
    }
    if(atomic_load_explicit(&state->stream->closed,memory_order_acquire)){
        disp_tls_io_fail(state,disp_owned_bytes("TLS stream is closed",strlen("TLS stream is closed")),false);
        return true;
    }
    atomic_bool *busy=state->operation==DISP_TLS_READ?&state->stream->read_busy:&state->stream->write_busy;
    if(!state->claimed){
        if(!disp_tcp_claim(busy)){
            if(state->has_deadline&&disp_time_now_nanos()>=state->deadline){
                disp_tls_io_fail(state,disp_owned_bytes("TLS operation timed out",strlen("TLS operation timed out")),false);
                return true;
            }
            disp_reactor_offer(1000000ULL);
            return false;
        }
        state->claimed=true;
    }
    if(!state->limit){disp_tls_io_finish(state,true);return true;}
    for(unsigned steps=0;steps<16&&!state->done;steps++){
        ERR_clear_error();
        int result=0;
        if(state->operation==DISP_TLS_READ){
            if(!state->buffer.data){
                state->buffer.data=(char*)disp_alloc(state->limit,1);
                state->buffer.cap=state->limit;
            }
            result=SSL_read(state->stream->session,state->buffer.data,(int)state->limit);
        }else{
            size_t remaining=state->limit-state->offset;
            int chunk=(int)(remaining>INT_MAX?INT_MAX:remaining);
            state->write_attempted=true;
            result=SSL_write(state->stream->session,state->buffer.data+state->offset,chunk);
        }
        int os_error=errno;
        if(result>0){
            if(state->operation==DISP_TLS_READ){
                state->buffer.len=(size_t)result;
                disp_tls_io_finish(state,true);
            }else{
                state->offset+=(size_t)result;
                if(state->offset==state->limit)disp_tls_io_finish(state,true);
            }
            continue;
        }
        int ssl_error=SSL_get_error(state->stream->session,result);
        if(state->operation==DISP_TLS_READ&&ssl_error==SSL_ERROR_ZERO_RETURN){
            disp_string_drop(&state->buffer);
            disp_tls_io_finish(state,true);
            break;
        }
        if(ssl_error==SSL_ERROR_WANT_READ||ssl_error==SSL_ERROR_WANT_WRITE)break;
        disp_tls_io_fail(state,disp_tls_openssl_error(state->operation==DISP_TLS_READ?"TLS read":"TLS write",ssl_error,os_error),true);
    }
    if(!state->done&&state->has_deadline&&disp_time_now_nanos()>=state->deadline){
        bool corrupts=state->operation==DISP_TLS_WRITE&&state->write_attempted;
        disp_tls_io_fail(state,disp_owned_bytes(state->operation==DISP_TLS_READ?"TLS read timed out":"TLS write timed out",state->operation==DISP_TLS_READ?strlen("TLS read timed out"):strlen("TLS write timed out")),corrupts);
    }
    if(!state->done)disp_reactor_offer(1000000ULL);
    return state->done;
}
static void disp_tls_io_take(disp_tls_io_state *state,bool *ok,disp_native_string *bytes,size_t *written,disp_native_string *error){
    if(!state->done||state->taken)dv_panic("TLS I/O result is not ready",0,0);
    state->taken=true;
    *ok=state->ok;
    if(state->operation==DISP_TLS_READ){
        *bytes=state->buffer;
        state->buffer=(disp_native_string){0};
    }else *written=state->offset;
    *error=state->error;
    state->error=(disp_native_string){0};
}
static void disp_tls_io_drop(void *raw){
    disp_tls_io_state *state=(disp_tls_io_state*)raw;
    if(!state)return;
    if(state->operation==DISP_TLS_WRITE&&!state->done&&state->write_attempted)disp_tls_state_close(state->stream);
    if(state->claimed)atomic_store_explicit(state->operation==DISP_TLS_READ?&state->stream->read_busy:&state->stream->write_busy,false,memory_order_release);
    disp_string_drop(&state->buffer);
    disp_string_drop(&state->error);
    disp_tls_state_release(state->stream);
    disp_dealloc(state);
}
static bool disp_tls_stream_read(disp_native_tls_stream *stream,size_t limit,disp_native_string *bytes,disp_native_string *error,int line,int column){disp_tls_io_state *state=disp_tls_io_create(stream->state,DISP_TLS_READ,NULL,limit,false,0,line,column);while(!disp_tls_io_poll(state))disp_time_sleep(1000000ULL);bool ok=false;size_t ignored=0;disp_tls_io_take(state,&ok,bytes,&ignored,error);disp_tls_io_drop(state);return ok;}
static bool disp_tls_stream_write(disp_native_tls_stream *stream,const char *bytes,size_t length,size_t *written,disp_native_string *error,int line,int column){disp_tls_io_state *state=disp_tls_io_create(stream->state,DISP_TLS_WRITE,bytes,length,false,0,line,column);while(!disp_tls_io_poll(state))disp_time_sleep(1000000ULL);bool ok=false;disp_native_string ignored={0};disp_tls_io_take(state,&ok,&ignored,written,error);disp_tls_io_drop(state);return ok;}
#endif
#endif
#ifdef DISP_HTTP
struct disp_http_response_state {
    uint16_t status;
    disp_native_string url;
    disp_native_string headers;
    disp_native_string body;
};
struct disp_http_builder_state {
    disp_native_string method;
    disp_native_string url;
    disp_native_string headers;
    disp_native_string body;
    size_t header_count;
    bool has_content_type;
};
typedef struct {
    atomic_size_t refs;
    atomic_int owner;
    atomic_bool done;
    atomic_bool cancelled;
    bool started;
    bool taken;
    bool ok;
    uint64_t timeout;
    uint64_t deadline;
    int line;
    int column;
    disp_native_string method;
    disp_native_string url;
    disp_native_string headers;
    disp_native_string body;
    disp_native_http_response response;
    disp_native_string error;
} disp_http_request_state;
#ifdef _WIN32
static disp_native_string disp_http_win_error(const char *operation){DWORD code=GetLastError();char system[256]={0};DWORD count=FormatMessageA(FORMAT_MESSAGE_FROM_SYSTEM|FORMAT_MESSAGE_IGNORE_INSERTS,NULL,code,0,system,(DWORD)sizeof(system),NULL);while(count&&(system[count-1]=='\r'||system[count-1]=='\n'||system[count-1]==' '))system[--count]=0;char message[448];int length=snprintf(message,sizeof(message),"%s failed with HTTP error %lu%s%s",operation,(unsigned long)code,count?": ":"",count?system:"");if(length<0)length=0;if((size_t)length>=sizeof(message))length=(int)sizeof(message)-1;return disp_owned_bytes(message,(size_t)length);}
#endif
static void disp_http_response_state_free(disp_http_response_state *state){if(!state)return;disp_string_drop(&state->url);disp_string_drop(&state->headers);disp_string_drop(&state->body);disp_dealloc(state);}
static void disp_http_response_drop(disp_native_http_response *response){if(!response||!response->state)return;disp_http_response_state_free(response->state);response->state=NULL;}
static disp_http_response_state *disp_http_response_require(const disp_native_http_response *response){if(!response||!response->state)dv_panic("HTTP response is unavailable",0,0);return response->state;}
static uint64_t disp_http_response_status(const disp_native_http_response *response){return disp_http_response_require(response)->status;}
static bool disp_http_response_is_success(const disp_native_http_response *response){uint16_t status=disp_http_response_require(response)->status;return status>=200&&status<300;}
static size_t disp_http_response_len(const disp_native_http_response *response){return disp_http_response_require(response)->body.len;}
static disp_native_string disp_http_response_body(const disp_native_http_response *response){disp_http_response_state *state=disp_http_response_require(response);return disp_owned_bytes(state->body.data,state->body.len);}
static disp_native_string disp_http_response_url(const disp_native_http_response *response){disp_http_response_state *state=disp_http_response_require(response);return disp_owned_bytes(state->url.data,state->url.len);}
static bool disp_http_response_text(const disp_native_http_response *response,disp_native_string *text,disp_native_string *error){disp_http_response_state *state=disp_http_response_require(response);if(!disp_utf8_valid(state->body.data,state->body.len)){*error=disp_owned_bytes("HTTP response body is not valid UTF-8",strlen("HTTP response body is not valid UTF-8"));return false;}*text=disp_owned_bytes(state->body.data,state->body.len);return true;}
static bool disp_http_response_json(const disp_native_http_response *response,disp_native_json *json,disp_native_string *error){disp_http_response_state *state=disp_http_response_require(response);return disp_json_parse(state->body.data,state->body.len,json,error);}
static bool disp_http_token(const char *name,size_t length){if(!length)return false;for(size_t i=0;i<length;i++){unsigned char c=(unsigned char)name[i];if((c>='a'&&c<='z')||(c>='A'&&c<='Z')||(c>='0'&&c<='9'))continue;if(strchr("!#$%&'*+-.^_`|~",c))continue;return false;}return true;}
static bool disp_http_name_equal(const char *left,size_t left_len,const char *right,size_t right_len){if(left_len!=right_len)return false;for(size_t i=0;i<left_len;i++){unsigned char a=(unsigned char)left[i],b=(unsigned char)right[i];if(a>='A'&&a<='Z')a=(unsigned char)(a+('a'-'A'));if(b>='A'&&b<='Z')b=(unsigned char)(b+('a'-'A'));if(a!=b)return false;}return true;}
static bool disp_http_response_header(const disp_native_http_response *response,const char *name,size_t name_len,disp_native_string *value,int line,int column){if(!disp_http_token(name,name_len))dv_panic("HTTP header name contains invalid characters",line,column);disp_http_response_state *state=disp_http_response_require(response);size_t position=0;bool first=true;while(position<state->headers.len){size_t end=position;while(end+1<state->headers.len&&(state->headers.data[end]!='\r'||state->headers.data[end+1]!='\n'))end++;if(end+1>=state->headers.len)break;if(!first&&end>position){size_t colon=position;while(colon<end&&state->headers.data[colon]!=':')colon++;if(colon<end&&disp_http_name_equal(state->headers.data+position,colon-position,name,name_len)){size_t start=colon+1;while(start<end&&(state->headers.data[start]==' '||state->headers.data[start]=='\t'))start++;size_t finish=end;while(finish>start&&(state->headers.data[finish-1]==' '||state->headers.data[finish-1]=='\t'))finish--;size_t item=finish-start,new_len=value->len+(value->len?2:0)+item;if(new_len<value->len)dv_panic("HTTP header value size overflow",line,column);value->data=(char*)disp_realloc(value->data,new_len?new_len:1,1);if(value->len){value->data[value->len++]=',';value->data[value->len++]=' ';}if(item)memcpy(value->data+value->len,state->headers.data+start,item);value->len+=item;value->cap=new_len;}}first=false;position=end+2;}return value->data!=NULL;}
#ifdef _WIN32
static bool disp_http_utf8_to_wide(const char *text,size_t length,wchar_t **wide,disp_native_string *error){if(!length||length>8192||memchr(text,0,length)){*error=disp_owned_bytes("HTTP URL must be non-empty, at most 8192 bytes, and contain no NUL",strlen("HTTP URL must be non-empty, at most 8192 bytes, and contain no NUL"));return false;}for(size_t i=0;i<length;i++){unsigned char c=(unsigned char)text[i];if(c<=0x20||c==0x7f){*error=disp_owned_bytes("HTTP URL contains a control character or space",strlen("HTTP URL contains a control character or space"));return false;}if(c=='#'){*error=disp_owned_bytes("HTTP URL must not contain a fragment",strlen("HTTP URL must not contain a fragment"));return false;}}int count=MultiByteToWideChar(CP_UTF8,MB_ERR_INVALID_CHARS,text,(int)length,NULL,0);if(count<=0){*error=disp_owned_bytes("HTTP URL is not valid UTF-8",strlen("HTTP URL is not valid UTF-8"));return false;}*wide=(wchar_t*)disp_alloc(((size_t)count+1)*sizeof(wchar_t),_Alignof(wchar_t));if(MultiByteToWideChar(CP_UTF8,MB_ERR_INVALID_CHARS,text,(int)length,*wide,count)!=count){disp_dealloc(*wide);*wide=NULL;*error=disp_http_win_error("HTTP URL conversion");return false;}(*wide)[count]=0;return true;}
static bool disp_http_wide_to_utf8(const wchar_t *wide,size_t length,disp_native_string *text,disp_native_string *error){if(length>INT_MAX){*error=disp_owned_bytes("HTTP text is too large",strlen("HTTP text is too large"));return false;}int count=WideCharToMultiByte(CP_UTF8,WC_ERR_INVALID_CHARS,wide,(int)length,NULL,0,NULL,NULL);if(count<0||(!count&&length)){*error=disp_http_win_error("HTTP text conversion");return false;}if(count){text->data=(char*)disp_alloc((size_t)count,1);if(WideCharToMultiByte(CP_UTF8,WC_ERR_INVALID_CHARS,wide,(int)length,text->data,count,NULL,NULL)!=count){disp_string_drop(text);*error=disp_http_win_error("HTTP text conversion");return false;}text->len=text->cap=(size_t)count;}return true;}
#endif
static bool disp_http_cancelled(disp_http_request_state *state,disp_native_string *error){if(!atomic_load_explicit(&state->cancelled,memory_order_acquire))return false;*error=disp_owned_bytes("HTTP request was cancelled",strlen("HTTP request was cancelled"));return true;}
static bool disp_http_timeout_ms(disp_http_request_state *state,int *milliseconds,disp_native_string *error){if(disp_http_cancelled(state,error))return false;uint64_t now=disp_time_now_nanos();if(now>=state->deadline){*error=disp_owned_bytes("HTTP request timed out",strlen("HTTP request timed out"));return false;}uint64_t remaining=state->deadline-now;uint64_t millis=remaining/1000000ULL+(remaining%1000000ULL!=0);*milliseconds=(int)(millis>(uint64_t)INT_MAX?INT_MAX:millis);return true;}
#ifdef _WIN32
static bool disp_http_query_wide_option(HINTERNET request,DWORD option,disp_native_string *value,disp_native_string *error){DWORD bytes=0;if(WinHttpQueryOption(request,option,NULL,&bytes)||GetLastError()!=ERROR_INSUFFICIENT_BUFFER){*error=disp_http_win_error("HTTP response URL query");return false;}if(bytes>131072){*error=disp_owned_bytes("HTTP response URL exceeds its safety limit",strlen("HTTP response URL exceeds its safety limit"));return false;}wchar_t *wide=(wchar_t*)disp_alloc(bytes?bytes:sizeof(wchar_t),_Alignof(wchar_t));if(!WinHttpQueryOption(request,option,wide,&bytes)){disp_dealloc(wide);*error=disp_http_win_error("HTTP response URL query");return false;}size_t length=bytes/sizeof(wchar_t);while(length&&wide[length-1]==0)length--;bool ok=disp_http_wide_to_utf8(wide,length,value,error);disp_dealloc(wide);return ok;}
static bool disp_http_query_headers(HINTERNET request,disp_native_string *headers,disp_native_string *error){DWORD bytes=0;if(WinHttpQueryHeaders(request,WINHTTP_QUERY_RAW_HEADERS_CRLF,WINHTTP_HEADER_NAME_BY_INDEX,NULL,&bytes,WINHTTP_NO_HEADER_INDEX)||GetLastError()!=ERROR_INSUFFICIENT_BUFFER){*error=disp_http_win_error("HTTP response header query");return false;}if(bytes>DISP_HTTP_HEADER_LIMIT*2+2){*error=disp_owned_bytes("HTTP response headers exceed the 64 KiB limit",strlen("HTTP response headers exceed the 64 KiB limit"));return false;}wchar_t *wide=(wchar_t*)disp_alloc(bytes?bytes:sizeof(wchar_t),_Alignof(wchar_t));if(!WinHttpQueryHeaders(request,WINHTTP_QUERY_RAW_HEADERS_CRLF,WINHTTP_HEADER_NAME_BY_INDEX,wide,&bytes,WINHTTP_NO_HEADER_INDEX)){disp_dealloc(wide);*error=disp_http_win_error("HTTP response header query");return false;}size_t length=bytes/sizeof(wchar_t);while(length&&wide[length-1]==0)length--;bool ok=disp_http_wide_to_utf8(wide,length,headers,error);disp_dealloc(wide);if(ok&&headers->len>DISP_HTTP_HEADER_LIMIT){disp_string_drop(headers);*error=disp_owned_bytes("HTTP response headers exceed the 64 KiB limit",strlen("HTTP response headers exceed the 64 KiB limit"));return false;}return ok;}
#endif
static void disp_http_builder_state_free(disp_http_builder_state *state){if(!state)return;disp_string_drop(&state->method);disp_string_drop(&state->url);disp_string_drop(&state->headers);disp_string_drop(&state->body);disp_dealloc(state);}
static void disp_http_builder_drop(disp_native_http_request *request){if(!request||!request->state)return;disp_http_builder_state_free(request->state);request->state=NULL;}
static bool disp_http_method_valid(const char *method,size_t length){if(!length||length>32||!disp_http_token(method,length))return false;if(length==7&&disp_http_name_equal(method,length,"CONNECT",7))return false;if(length==5&&disp_http_name_equal(method,length,"TRACE",5))return false;return true;}
static bool disp_http_header_forbidden(const char *name,size_t length){static const char *blocked[]={"host","content-length","transfer-encoding","connection","proxy-connection","proxy-authorization","trailer","te","upgrade"};for(size_t i=0;i<sizeof(blocked)/sizeof(blocked[0]);i++)if(disp_http_name_equal(name,length,blocked[i],strlen(blocked[i])))return true;return false;}
static bool disp_http_header_value_valid(const char *value,size_t length){for(size_t i=0;i<length;i++){unsigned char c=(unsigned char)value[i];if(c>0x7e||c==0x7f||(c<0x20&&c!='\t'))return false;}return true;}
#ifdef _WIN32
static bool disp_http_url_valid(const char *url,size_t length,disp_native_string *error){wchar_t *wide=NULL;if(!disp_http_utf8_to_wide(url,length,&wide,error))return false;URL_COMPONENTS parts={0};parts.dwStructSize=sizeof(parts);parts.dwHostNameLength=(DWORD)-1;parts.dwUserNameLength=(DWORD)-1;parts.dwPasswordLength=(DWORD)-1;bool ok=WinHttpCrackUrl(wide,0,0,&parts)&&(parts.nScheme==INTERNET_SCHEME_HTTP||parts.nScheme==INTERNET_SCHEME_HTTPS)&&parts.dwHostNameLength&&!parts.dwUserNameLength&&!parts.dwPasswordLength;disp_dealloc(wide);if(!ok)*error=disp_owned_bytes("HTTP URL must use http or https, contain a host, and contain no credentials",strlen("HTTP URL must use http or https, contain a host, and contain no credentials"));return ok;}
#else
static bool disp_http_url_valid(const char *url,size_t length,disp_native_string *error){
    const char *invalid="HTTP URL must use http or https, contain a host, and contain no credentials";
    if(!length||length>8192||memchr(url,0,length)||!disp_utf8_valid(url,length)){*error=disp_owned_bytes("HTTP URL must be non-empty, valid UTF-8, at most 8192 bytes, and contain no NUL",strlen("HTTP URL must be non-empty, valid UTF-8, at most 8192 bytes, and contain no NUL"));return false;}
    for(size_t i=0;i<length;i++){unsigned char c=(unsigned char)url[i];if(c<=0x20||c==0x7f||c=='#'){*error=disp_owned_bytes("HTTP URL contains a control character, space, or fragment",strlen("HTTP URL contains a control character, space, or fragment"));return false;}}
    size_t scheme=0;while(scheme<length&&url[scheme]!=':')scheme++;
    bool http=scheme==4&&disp_http_name_equal(url,4,"http",4),https=scheme==5&&disp_http_name_equal(url,5,"https",5);
    if((!http&&!https)||scheme+3>length||url[scheme+1]!='/'||url[scheme+2]!='/'){*error=disp_owned_bytes(invalid,strlen(invalid));return false;}
    size_t start=scheme+3,end=start;while(end<length&&url[end]!='/'&&url[end]!='?')end++;
    if(start==end||memchr(url+start,'@',end-start)){*error=disp_owned_bytes(invalid,strlen(invalid));return false;}
    size_t host_end=end,port=end;bool has_port=false;
    if(url[start]=='['){size_t close=start+1;while(close<end&&url[close]!=']')close++;if(close==end||close==start+1||(close+1<end&&url[close+1]!=':')){*error=disp_owned_bytes(invalid,strlen(invalid));return false;}host_end=close+1;if(close+1<end){port=close+2;has_port=true;}}
    else{size_t colons=0;for(size_t i=start;i<end;i++)if(url[i]==':'){port=i+1;host_end=i;has_port=true;colons++;}if(colons>1){*error=disp_owned_bytes(invalid,strlen(invalid));return false;}}
    if(host_end==start){*error=disp_owned_bytes(invalid,strlen(invalid));return false;}
    if(has_port){unsigned value=0;if(port==end){*error=disp_owned_bytes(invalid,strlen(invalid));return false;}for(size_t i=port;i<end;i++){unsigned char c=(unsigned char)url[i];if(c<'0'||c>'9'||value>6553){*error=disp_owned_bytes(invalid,strlen(invalid));return false;}value=value*10+(unsigned)(c-'0');}if(!value||value>65535){*error=disp_owned_bytes(invalid,strlen(invalid));return false;}}
    return true;
}
#endif
static void disp_url_drop(disp_native_url *url){disp_dealloc(url->data);url->data=NULL;url->len=0;url->cap=0;}
static bool disp_url_parse(const char *source,size_t length,disp_native_url *url,disp_native_string *error){if(!disp_http_url_valid(source,length,error))return false;url->data=(char*)disp_alloc(length+1,1);memcpy(url->data,source,length);url->data[length]=0;url->len=length;url->cap=length+1;return true;}
static size_t disp_url_scheme_end(const disp_native_url *url){for(size_t i=0;i<url->len;i++)if(url->data[i]==':')return i;return 0;}
static size_t disp_url_authority_end(const disp_native_url *url){size_t start=disp_url_scheme_end(url)+3;size_t end=start;while(end<url->len&&url->data[end]!='/'&&url->data[end]!='?')end++;return end;}
static disp_native_string disp_url_as_string(const disp_native_url *url){return disp_owned_bytes(url->data,url->len);}
static disp_native_string disp_url_scheme(const disp_native_url *url){size_t end=disp_url_scheme_end(url);return disp_owned_bytes(url->data,end);}
static bool disp_url_host(const disp_native_url *url,disp_native_string *host){size_t start=disp_url_scheme_end(url)+3,end=disp_url_authority_end(url);if(start>=end)return false;if(url->data[start]=='['){size_t close=start+1;while(close<end&&url->data[close]!=']')close++;if(close>=end)return false;start++;end=close;}else{for(size_t i=end;i>start;i--)if(url->data[i-1]==':'){end=i-1;break;}}*host=disp_owned_bytes(url->data+start,end-start);return true;}
static bool disp_url_port(const disp_native_url *url,uint64_t *port){size_t start=disp_url_scheme_end(url)+3,end=disp_url_authority_end(url),colon=end;if(start<end&&url->data[start]=='['){size_t close=start+1;while(close<end&&url->data[close]!=']')close++;if(close+1<end&&url->data[close+1]==':')colon=close+1;}else{for(size_t i=end;i>start;i--)if(url->data[i-1]==':'){colon=i-1;break;}}if(colon==end)return false;uint64_t value=0;for(size_t i=colon+1;i<end;i++){unsigned char c=(unsigned char)url->data[i];if(c<'0'||c>'9')return false;value=value*10+(uint64_t)(c-'0');if(value>65535)return false;}*port=value;return true;}
static disp_native_string disp_url_path(const disp_native_url *url){size_t start=disp_url_authority_end(url);if(start>=url->len||url->data[start]=='?')return disp_owned_bytes("/",1);size_t end=start;while(end<url->len&&url->data[end]!='?')end++;return disp_owned_bytes(url->data+start,end-start);}
static bool disp_url_query(const disp_native_url *url,disp_native_string *query){size_t start=disp_url_authority_end(url);while(start<url->len&&url->data[start]!='?')start++;if(start>=url->len)return false;start++;*query=disp_owned_bytes(url->data+start,url->len-start);return true;}
static bool disp_url_is_secure(const disp_native_url *url){return disp_url_scheme_end(url)==5&&disp_http_name_equal(url->data,5,"https",5);}
static bool disp_url_encoded_length(const char *value,size_t length,size_t *encoded){size_t result=0;for(size_t i=0;i<length;i++){unsigned char ch=(unsigned char)value[i];size_t add=((ch>='a'&&ch<='z')||(ch>='A'&&ch<='Z')||(ch>='0'&&ch<='9')||ch=='-'||ch=='.'||ch=='_'||ch=='~')?1:3;if(__builtin_add_overflow(result,add,&result))return false;}*encoded=result;return true;}
static void disp_url_append_encoded(disp_native_string *builder,const char *value,size_t length){static const char hex[]="0123456789ABCDEF";for(size_t i=0;i<length;i++){unsigned char ch=(unsigned char)value[i];if((ch>='a'&&ch<='z')||(ch>='A'&&ch<='Z')||(ch>='0'&&ch<='9')||ch=='-'||ch=='.'||ch=='_'||ch=='~')disp_string_push_bytes(builder,(const char*)&ch,1);else{char encoded[3]={'%',hex[ch>>4],hex[ch&15]};disp_string_push_bytes(builder,encoded,3);}}}
static bool disp_url_join_path(const disp_native_url *base,const char *segment,size_t segment_len,disp_native_url *url,disp_native_string *error){if(!segment_len||(segment_len==1&&segment[0]=='.')||(segment_len==2&&segment[0]=='.'&&segment[1]=='.')){const char *message="URL path segment must be non-empty and cannot be '.' or '..'";*error=disp_owned_bytes(message,strlen(message));return false;}size_t encoded=0,query=base->len;while(query&&base->data[query-1]!='?')query--;if(query)query--;size_t path_end=query?query:base->len;bool slash=path_end&&base->data[path_end-1]=='/';size_t needed;if(!disp_url_encoded_length(segment,segment_len,&encoded)||__builtin_add_overflow(base->len,encoded,&needed)||(!slash&&__builtin_add_overflow(needed,(size_t)1,&needed))||needed>8192){const char *message="URL exceeds the 8192-byte safety limit";*error=disp_owned_bytes(message,strlen(message));return false;}disp_native_string builder=disp_string_with_capacity(needed);disp_string_push_bytes(&builder,base->data,path_end);if(!slash)disp_string_push_bytes(&builder,"/",1);disp_url_append_encoded(&builder,segment,segment_len);if(query)disp_string_push_bytes(&builder,base->data+query,base->len-query);bool ok=disp_url_parse(builder.data,builder.len,url,error);disp_string_drop(&builder);return ok;}
static bool disp_url_query_param(const disp_native_url *base,const char *name,size_t name_len,const char *value,size_t value_len,disp_native_url *url,disp_native_string *error){if(!name_len){const char *message="URL query parameter name must not be empty";*error=disp_owned_bytes(message,strlen(message));return false;}bool has_query=memchr(base->data,'?',base->len)!=NULL,needs_separator=!has_query||(base->len&&base->data[base->len-1]!='?'&&base->data[base->len-1]!='&');size_t name_encoded=0,value_encoded=0,needed=base->len;if(!disp_url_encoded_length(name,name_len,&name_encoded)||!disp_url_encoded_length(value,value_len,&value_encoded)||__builtin_add_overflow(needed,name_encoded,&needed)||__builtin_add_overflow(needed,value_encoded,&needed)||__builtin_add_overflow(needed,(size_t)1+(size_t)needs_separator,&needed)||needed>8192){const char *message="URL exceeds the 8192-byte safety limit";*error=disp_owned_bytes(message,strlen(message));return false;}disp_native_string builder=disp_string_with_capacity(needed);disp_string_push_bytes(&builder,base->data,base->len);if(!has_query)disp_string_push_bytes(&builder,"?",1);else if(needs_separator)disp_string_push_bytes(&builder,"&",1);disp_url_append_encoded(&builder,name,name_len);disp_string_push_bytes(&builder,"=",1);disp_url_append_encoded(&builder,value,value_len);bool ok=disp_url_parse(builder.data,builder.len,url,error);disp_string_drop(&builder);return ok;}
static bool disp_http_builder_append_header(disp_http_builder_state *state,const char *name,size_t name_len,const char *value,size_t value_len,disp_native_string *error){if(!disp_http_token(name,name_len)||disp_http_header_forbidden(name,name_len)){*error=disp_owned_bytes("HTTP header name is invalid or controlled by the safe client",strlen("HTTP header name is invalid or controlled by the safe client"));return false;}if(!disp_http_header_value_valid(value,value_len)){*error=disp_owned_bytes("HTTP header value must contain only safe ASCII text",strlen("HTTP header value must contain only safe ASCII text"));return false;}if(state->header_count>=DISP_HTTP_HEADER_COUNT_LIMIT){*error=disp_owned_bytes("HTTP request contains more than 100 headers",strlen("HTTP request contains more than 100 headers"));return false;}size_t addition=name_len+value_len+4;if(addition>DISP_HTTP_HEADER_LIMIT-state->headers.len){*error=disp_owned_bytes("HTTP request headers exceed the 64 KiB limit",strlen("HTTP request headers exceed the 64 KiB limit"));return false;}size_t next=state->headers.len+addition;state->headers.data=(char*)disp_realloc(state->headers.data,next,1);memcpy(state->headers.data+state->headers.len,name,name_len);state->headers.len+=name_len;memcpy(state->headers.data+state->headers.len,": ",2);state->headers.len+=2;if(value_len)memcpy(state->headers.data+state->headers.len,value,value_len);state->headers.len+=value_len;memcpy(state->headers.data+state->headers.len,"\r\n",2);state->headers.len+=2;state->headers.cap=next;state->header_count++;if(disp_http_name_equal(name,name_len,"content-type",12))state->has_content_type=true;return true;}
static bool disp_http_builder_create(const char *method,size_t method_len,const char *url,size_t url_len,disp_native_http_request *output,disp_native_string *error){if(!disp_http_method_valid(method,method_len)){*error=disp_owned_bytes("HTTP method is invalid or forbidden by the safe client",strlen("HTTP method is invalid or forbidden by the safe client"));return false;}if(!disp_http_url_valid(url,url_len,error))return false;disp_http_builder_state *state=(disp_http_builder_state*)disp_alloc_zeroed(1,sizeof(disp_http_builder_state),_Alignof(disp_http_builder_state));state->method=disp_owned_bytes(method,method_len);for(size_t i=0;i<method_len;i++)if(state->method.data[i]>='a'&&state->method.data[i]<='z')state->method.data[i]=(char)(state->method.data[i]-('a'-'A'));state->url=disp_owned_bytes(url,url_len);output->state=state;return true;}
static disp_http_builder_state *disp_http_builder_take(disp_native_http_request *request){if(!request||!request->state)return NULL;disp_http_builder_state *state=request->state;request->state=NULL;return state;}
static bool disp_http_builder_header(disp_native_http_request *request,const char *name,size_t name_len,const char *value,size_t value_len,disp_native_http_request *output,disp_native_string *error){disp_http_builder_state *state=disp_http_builder_take(request);if(!state){*error=disp_owned_bytes("HTTP request is unavailable",strlen("HTTP request is unavailable"));return false;}if(!disp_http_builder_append_header(state,name,name_len,value,value_len,error)){disp_http_builder_state_free(state);return false;}output->state=state;return true;}
static bool disp_http_builder_body(disp_native_http_request *request,const char *body,size_t body_len,bool text,bool json,disp_native_http_request *output,disp_native_string *error){disp_http_builder_state *state=disp_http_builder_take(request);if(!state){*error=disp_owned_bytes("HTTP request is unavailable",strlen("HTTP request is unavailable"));return false;}if(body_len>DISP_HTTP_BODY_LIMIT){disp_http_builder_state_free(state);*error=disp_owned_bytes("HTTP request body exceeds the 16 MiB limit",strlen("HTTP request body exceeds the 16 MiB limit"));return false;}if((text||json)&&!disp_utf8_valid(body,body_len)){disp_http_builder_state_free(state);*error=disp_owned_bytes("HTTP text body is not valid UTF-8",strlen("HTTP text body is not valid UTF-8"));return false;}if((text||json)&&!state->has_content_type&&!disp_http_builder_append_header(state,"Content-Type",12,json?"application/json":"text/plain; charset=utf-8",json?16:25,error)){disp_http_builder_state_free(state);return false;}disp_string_drop(&state->body);state->body=disp_owned_bytes(body,body_len);output->state=state;return true;}
#ifdef _WIN32
#define disp_http_execute disp_http_execute_legacy
static bool disp_http_execute(disp_http_request_state *state,disp_native_http_response *response,disp_native_string *error){wchar_t *wide_url=NULL,*host=NULL,*target=NULL;HINTERNET session=NULL,connection=NULL,request=NULL;disp_http_response_state *result=NULL;bool ok=false;if(!disp_http_utf8_to_wide(state->url.data,state->url.len,&wide_url,error))goto cleanup;const wchar_t *authority=wcsstr(wide_url,L"://");if(!authority){*error=disp_owned_bytes("invalid HTTP URL",strlen("invalid HTTP URL"));goto cleanup;}authority+=3;for(const wchar_t *cursor=authority;*cursor&&*cursor!=L'/'&&*cursor!=L'?'&&*cursor!=L'#';cursor++)if(*cursor==L'@'){*error=disp_owned_bytes("credentials are not allowed in HTTP URLs",strlen("credentials are not allowed in HTTP URLs"));goto cleanup;}URL_COMPONENTS parts={0};parts.dwStructSize=sizeof(parts);parts.dwHostNameLength=(DWORD)-1;parts.dwUrlPathLength=(DWORD)-1;parts.dwExtraInfoLength=(DWORD)-1;parts.dwUserNameLength=(DWORD)-1;parts.dwPasswordLength=(DWORD)-1;if(!WinHttpCrackUrl(wide_url,0,0,&parts)){*error=disp_http_win_error("HTTP URL parsing");goto cleanup;}if((parts.nScheme!=INTERNET_SCHEME_HTTP&&parts.nScheme!=INTERNET_SCHEME_HTTPS)||!parts.dwHostNameLength){*error=disp_owned_bytes("HTTP URL scheme must be http or https and include a host",strlen("HTTP URL scheme must be http or https and include a host"));goto cleanup;}if(parts.dwUserNameLength||parts.dwPasswordLength){*error=disp_owned_bytes("credentials are not allowed in HTTP URLs",strlen("credentials are not allowed in HTTP URLs"));goto cleanup;}host=(wchar_t*)disp_alloc(((size_t)parts.dwHostNameLength+1)*sizeof(wchar_t),_Alignof(wchar_t));memcpy(host,parts.lpszHostName,(size_t)parts.dwHostNameLength*sizeof(wchar_t));host[parts.dwHostNameLength]=0;size_t path_len=parts.dwUrlPathLength,extra_len=parts.dwExtraInfoLength;if(!path_len)path_len=1;target=(wchar_t*)disp_alloc((path_len+extra_len+1)*sizeof(wchar_t),_Alignof(wchar_t));if(parts.dwUrlPathLength)memcpy(target,parts.lpszUrlPath,(size_t)parts.dwUrlPathLength*sizeof(wchar_t));else target[0]=L'/';if(extra_len)memcpy(target+path_len,parts.lpszExtraInfo,extra_len*sizeof(wchar_t));target[path_len+extra_len]=0;int timeout_ms=0;if(!disp_http_timeout_ms(state,&timeout_ms,error))goto cleanup;
#ifndef WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY
#define WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY 4
#endif
session=WinHttpOpen(L"DISP/0.1",WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY,WINHTTP_NO_PROXY_NAME,WINHTTP_NO_PROXY_BYPASS,0);if(!session){*error=disp_http_win_error("HTTP session creation");goto cleanup;}if(!WinHttpSetTimeouts(session,timeout_ms,timeout_ms,timeout_ms,timeout_ms)){*error=disp_http_win_error("HTTP timeout setup");goto cleanup;}DWORD protocols=WINHTTP_FLAG_SECURE_PROTOCOL_TLS1_2;
#ifdef WINHTTP_FLAG_SECURE_PROTOCOL_TLS1_3
protocols|=WINHTTP_FLAG_SECURE_PROTOCOL_TLS1_3;
#endif
if(!WinHttpSetOption(session,WINHTTP_OPTION_SECURE_PROTOCOLS,&protocols,sizeof(protocols))){protocols=WINHTTP_FLAG_SECURE_PROTOCOL_TLS1_2;if(!WinHttpSetOption(session,WINHTTP_OPTION_SECURE_PROTOCOLS,&protocols,sizeof(protocols))){*error=disp_http_win_error("HTTP secure protocol setup");goto cleanup;}}DWORD redirects=(DWORD)DISP_HTTP_REDIRECT_LIMIT,header_limit=(DWORD)DISP_HTTP_HEADER_LIMIT,redirect_policy=WINHTTP_OPTION_REDIRECT_POLICY_DISALLOW_HTTPS_TO_HTTP;if(!WinHttpSetOption(session,WINHTTP_OPTION_MAX_HTTP_AUTOMATIC_REDIRECTS,&redirects,sizeof(redirects))||!WinHttpSetOption(session,WINHTTP_OPTION_MAX_RESPONSE_HEADER_SIZE,&header_limit,sizeof(header_limit))||!WinHttpSetOption(session,WINHTTP_OPTION_REDIRECT_POLICY,&redirect_policy,sizeof(redirect_policy))){*error=disp_http_win_error("HTTP safety limit setup");goto cleanup;}connection=WinHttpConnect(session,host,parts.nPort,0);if(!connection){*error=disp_http_win_error("HTTP connection creation");goto cleanup;}DWORD flags=parts.nScheme==INTERNET_SCHEME_HTTPS?WINHTTP_FLAG_SECURE:0;request=WinHttpOpenRequest(connection,L"GET",target,NULL,WINHTTP_NO_REFERER,WINHTTP_DEFAULT_ACCEPT_TYPES,flags);if(!request){*error=disp_http_win_error("HTTP request creation");goto cleanup;}if(parts.nScheme==INTERNET_SCHEME_HTTPS){DWORD feature=WINHTTP_ENABLE_SSL_REVOCATION;if(!WinHttpSetOption(request,WINHTTP_OPTION_ENABLE_FEATURE,&feature,sizeof(feature))){*error=disp_http_win_error("HTTP certificate revocation setup");goto cleanup;}}if(disp_http_cancelled(state,error))goto cleanup;if(!WinHttpSendRequest(request,WINHTTP_NO_ADDITIONAL_HEADERS,0,WINHTTP_NO_REQUEST_DATA,0,0,0)){*error=disp_http_win_error("HTTP request send");goto cleanup;}if(!disp_http_timeout_ms(state,&timeout_ms,error))goto cleanup;if(!WinHttpSetOption(request,WINHTTP_OPTION_RECEIVE_TIMEOUT,&timeout_ms,sizeof(timeout_ms))){*error=disp_http_win_error("HTTP receive timeout setup");goto cleanup;}if(!WinHttpReceiveResponse(request,NULL)){*error=disp_http_win_error("HTTP response receive");goto cleanup;}if(disp_http_cancelled(state,error))goto cleanup;DWORD status=0,status_size=sizeof(status);if(!WinHttpQueryHeaders(request,WINHTTP_QUERY_STATUS_CODE|WINHTTP_QUERY_FLAG_NUMBER,WINHTTP_HEADER_NAME_BY_INDEX,&status,&status_size,WINHTTP_NO_HEADER_INDEX)||status>999){*error=disp_http_win_error("HTTP status query");goto cleanup;}result=(disp_http_response_state*)disp_alloc_zeroed(1,sizeof(disp_http_response_state),_Alignof(disp_http_response_state));result->status=(uint16_t)status;if(!disp_http_query_wide_option(request,WINHTTP_OPTION_URL,&result->url,error)||!disp_http_query_headers(request,&result->headers,error))goto cleanup;for(;;){if(disp_http_cancelled(state,error))goto cleanup;if(!disp_http_timeout_ms(state,&timeout_ms,error))goto cleanup;if(!WinHttpSetOption(request,WINHTTP_OPTION_RECEIVE_TIMEOUT,&timeout_ms,sizeof(timeout_ms))){*error=disp_http_win_error("HTTP receive timeout setup");goto cleanup;}DWORD available=0;if(!WinHttpQueryDataAvailable(request,&available)){*error=disp_http_win_error("HTTP body availability query");goto cleanup;}if(!available)break;if((size_t)available>DISP_HTTP_BODY_LIMIT-result->body.len){*error=disp_owned_bytes("HTTP response body exceeds the 16 MiB limit",strlen("HTTP response body exceeds the 16 MiB limit"));goto cleanup;}size_t next=result->body.len+(size_t)available;result->body.data=(char*)disp_realloc(result->body.data,next?next:1,1);DWORD read=0;if(!WinHttpReadData(request,result->body.data+result->body.len,available,&read)){*error=disp_http_win_error("HTTP body read");goto cleanup;}if(!read){*error=disp_owned_bytes("HTTP body read made no progress",strlen("HTTP body read made no progress"));goto cleanup;}result->body.len+=(size_t)read;result->body.cap=next;}response->state=result;result=NULL;ok=true;cleanup:if(request)WinHttpCloseHandle(request);if(connection)WinHttpCloseHandle(connection);if(session)WinHttpCloseHandle(session);disp_dealloc(target);disp_dealloc(host);disp_dealloc(wide_url);disp_http_response_state_free(result);return ok;}
#undef disp_http_execute
static bool disp_http_utf8_wide(const char *text,size_t length,wchar_t **wide,disp_native_string *error){if(length>INT_MAX){*error=disp_owned_bytes("HTTP text is too large",strlen("HTTP text is too large"));return false;}if(!length){*wide=NULL;return true;}int count=MultiByteToWideChar(CP_UTF8,MB_ERR_INVALID_CHARS,text,(int)length,NULL,0);if(count<=0){*error=disp_owned_bytes("HTTP request text is not valid UTF-8",strlen("HTTP request text is not valid UTF-8"));return false;}*wide=(wchar_t*)disp_alloc(((size_t)count+1)*sizeof(wchar_t),_Alignof(wchar_t));if(MultiByteToWideChar(CP_UTF8,MB_ERR_INVALID_CHARS,text,(int)length,*wide,count)!=count){disp_dealloc(*wide);*wide=NULL;*error=disp_http_win_error("HTTP request text conversion");return false;}(*wide)[count]=0;return true;}
static INIT_ONCE disp_http_session_once=INIT_ONCE_STATIC_INIT;
static HINTERNET disp_http_shared_session=NULL;
static DWORD disp_http_session_error=ERROR_SUCCESS;
static void disp_http_session_cleanup(void){if(disp_http_shared_session){WinHttpCloseHandle(disp_http_shared_session);disp_http_shared_session=NULL;}}
static BOOL CALLBACK disp_http_session_initialize(PINIT_ONCE once,PVOID parameter,PVOID *context){(void)once;(void)parameter;(void)context;
#ifndef WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY
#define WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY 4
#endif
disp_http_shared_session=WinHttpOpen(L"DISP/0.1",WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY,WINHTTP_NO_PROXY_NAME,WINHTTP_NO_PROXY_BYPASS,0);if(!disp_http_shared_session){disp_http_session_error=GetLastError();return TRUE;}DWORD protocols=WINHTTP_FLAG_SECURE_PROTOCOL_TLS1_2;
#ifdef WINHTTP_FLAG_SECURE_PROTOCOL_TLS1_3
protocols|=WINHTTP_FLAG_SECURE_PROTOCOL_TLS1_3;
#endif
if(!WinHttpSetOption(disp_http_shared_session,WINHTTP_OPTION_SECURE_PROTOCOLS,&protocols,sizeof(protocols))){protocols=WINHTTP_FLAG_SECURE_PROTOCOL_TLS1_2;if(!WinHttpSetOption(disp_http_shared_session,WINHTTP_OPTION_SECURE_PROTOCOLS,&protocols,sizeof(protocols))){disp_http_session_error=GetLastError();WinHttpCloseHandle(disp_http_shared_session);disp_http_shared_session=NULL;return TRUE;}}atexit(disp_http_session_cleanup);return TRUE;}
static HINTERNET disp_http_session_get(disp_native_string *error){InitOnceExecuteOnce(&disp_http_session_once,disp_http_session_initialize,NULL,NULL);if(!disp_http_shared_session){SetLastError(disp_http_session_error);*error=disp_http_win_error("HTTP shared session creation");return NULL;}return disp_http_shared_session;}
static bool disp_http_execute(disp_http_request_state *state,disp_native_http_response *response,disp_native_string *error){
    wchar_t *wide_url=NULL,*wide_method=NULL,*wide_headers=NULL,*host=NULL,*target=NULL;
    HINTERNET session=NULL,connection=NULL,request=NULL;
    disp_http_response_state *result=NULL;
    bool ok=false;
    if(!disp_http_utf8_to_wide(state->url.data,state->url.len,&wide_url,error))goto cleanup;
    if(!disp_http_utf8_wide(state->method.data,state->method.len,&wide_method,error))goto cleanup;
    if(state->headers.len&&!disp_http_utf8_wide(state->headers.data,state->headers.len,&wide_headers,error))goto cleanup;
    const wchar_t *authority=wcsstr(wide_url,L"://");
    if(!authority){*error=disp_owned_bytes("invalid HTTP URL",strlen("invalid HTTP URL"));goto cleanup;}
    authority+=3;
    for(const wchar_t *cursor=authority;*cursor&&*cursor!=L'/'&&*cursor!=L'?'&&*cursor!=L'#';cursor++)if(*cursor==L'@'){*error=disp_owned_bytes("credentials are not allowed in HTTP URLs",strlen("credentials are not allowed in HTTP URLs"));goto cleanup;}
    URL_COMPONENTS parts={0};
    parts.dwStructSize=sizeof(parts);parts.dwHostNameLength=(DWORD)-1;parts.dwUrlPathLength=(DWORD)-1;parts.dwExtraInfoLength=(DWORD)-1;parts.dwUserNameLength=(DWORD)-1;parts.dwPasswordLength=(DWORD)-1;
    if(!WinHttpCrackUrl(wide_url,0,0,&parts)){*error=disp_http_win_error("HTTP URL parsing");goto cleanup;}
    if((parts.nScheme!=INTERNET_SCHEME_HTTP&&parts.nScheme!=INTERNET_SCHEME_HTTPS)||!parts.dwHostNameLength||parts.dwUserNameLength||parts.dwPasswordLength){*error=disp_owned_bytes("HTTP URL must use http or https, contain a host, and contain no credentials",strlen("HTTP URL must use http or https, contain a host, and contain no credentials"));goto cleanup;}
    host=(wchar_t*)disp_alloc(((size_t)parts.dwHostNameLength+1)*sizeof(wchar_t),_Alignof(wchar_t));
    memcpy(host,parts.lpszHostName,(size_t)parts.dwHostNameLength*sizeof(wchar_t));host[parts.dwHostNameLength]=0;
    size_t path_len=parts.dwUrlPathLength?parts.dwUrlPathLength:1,extra_len=parts.dwExtraInfoLength;
    target=(wchar_t*)disp_alloc((path_len+extra_len+1)*sizeof(wchar_t),_Alignof(wchar_t));
    if(parts.dwUrlPathLength)memcpy(target,parts.lpszUrlPath,(size_t)parts.dwUrlPathLength*sizeof(wchar_t));else target[0]=L'/';
    if(extra_len)memcpy(target+path_len,parts.lpszExtraInfo,extra_len*sizeof(wchar_t));target[path_len+extra_len]=0;
    int timeout_ms=0;if(!disp_http_timeout_ms(state,&timeout_ms,error))goto cleanup;
    session=disp_http_session_get(error);if(!session)goto cleanup;
    bool redirectable=(disp_http_name_equal(state->method.data,state->method.len,"GET",3)||disp_http_name_equal(state->method.data,state->method.len,"HEAD",4))&&!state->headers.len&&!state->body.len;
    DWORD redirects=(DWORD)DISP_HTTP_REDIRECT_LIMIT,header_limit=(DWORD)DISP_HTTP_HEADER_LIMIT,redirect_policy=redirectable?WINHTTP_OPTION_REDIRECT_POLICY_DISALLOW_HTTPS_TO_HTTP:WINHTTP_OPTION_REDIRECT_POLICY_NEVER;
    connection=WinHttpConnect(session,host,parts.nPort,0);if(!connection){*error=disp_http_win_error("HTTP connection creation");goto cleanup;}
    DWORD flags=parts.nScheme==INTERNET_SCHEME_HTTPS?WINHTTP_FLAG_SECURE:0;
    request=WinHttpOpenRequest(connection,wide_method,target,NULL,WINHTTP_NO_REFERER,WINHTTP_DEFAULT_ACCEPT_TYPES,flags);if(!request){*error=disp_http_win_error("HTTP request creation");goto cleanup;}
    if(!WinHttpSetOption(request,WINHTTP_OPTION_MAX_HTTP_AUTOMATIC_REDIRECTS,&redirects,sizeof(redirects))||!WinHttpSetOption(request,WINHTTP_OPTION_MAX_RESPONSE_HEADER_SIZE,&header_limit,sizeof(header_limit))||!WinHttpSetOption(request,WINHTTP_OPTION_REDIRECT_POLICY,&redirect_policy,sizeof(redirect_policy))||!WinHttpSetOption(request,WINHTTP_OPTION_RESOLVE_TIMEOUT,&timeout_ms,sizeof(timeout_ms))||!WinHttpSetOption(request,WINHTTP_OPTION_CONNECT_TIMEOUT,&timeout_ms,sizeof(timeout_ms))||!WinHttpSetOption(request,WINHTTP_OPTION_SEND_TIMEOUT,&timeout_ms,sizeof(timeout_ms))||!WinHttpSetOption(request,WINHTTP_OPTION_RECEIVE_TIMEOUT,&timeout_ms,sizeof(timeout_ms))){*error=disp_http_win_error("HTTP safety and timeout setup");goto cleanup;}
    if(parts.nScheme==INTERNET_SCHEME_HTTPS){DWORD feature=WINHTTP_ENABLE_SSL_REVOCATION;if(!WinHttpSetOption(request,WINHTTP_OPTION_ENABLE_FEATURE,&feature,sizeof(feature))){*error=disp_http_win_error("HTTP certificate revocation setup");goto cleanup;}}
    if(wide_headers&&!WinHttpAddRequestHeaders(request,wide_headers,(DWORD)-1,WINHTTP_ADDREQ_FLAG_ADD)){*error=disp_http_win_error("HTTP request header setup");goto cleanup;}
    if(disp_http_cancelled(state,error))goto cleanup;
    if(!WinHttpSendRequest(request,WINHTTP_NO_ADDITIONAL_HEADERS,0,state->body.len?(LPVOID)state->body.data:WINHTTP_NO_REQUEST_DATA,(DWORD)state->body.len,(DWORD)state->body.len,0)){*error=disp_http_win_error("HTTP request send");goto cleanup;}
    if(!disp_http_timeout_ms(state,&timeout_ms,error))goto cleanup;
    if(!WinHttpSetOption(request,WINHTTP_OPTION_RECEIVE_TIMEOUT,&timeout_ms,sizeof(timeout_ms))){*error=disp_http_win_error("HTTP receive timeout setup");goto cleanup;}
    if(!WinHttpReceiveResponse(request,NULL)){*error=disp_http_win_error("HTTP response receive");goto cleanup;}
    if(disp_http_cancelled(state,error))goto cleanup;
    DWORD status=0,status_size=sizeof(status);
    if(!WinHttpQueryHeaders(request,WINHTTP_QUERY_STATUS_CODE|WINHTTP_QUERY_FLAG_NUMBER,WINHTTP_HEADER_NAME_BY_INDEX,&status,&status_size,WINHTTP_NO_HEADER_INDEX)||status>999){*error=disp_http_win_error("HTTP status query");goto cleanup;}
    result=(disp_http_response_state*)disp_alloc_zeroed(1,sizeof(disp_http_response_state),_Alignof(disp_http_response_state));result->status=(uint16_t)status;
    if(!disp_http_query_wide_option(request,WINHTTP_OPTION_URL,&result->url,error)||!disp_http_query_headers(request,&result->headers,error))goto cleanup;
    for(;;){if(disp_http_cancelled(state,error))goto cleanup;if(!disp_http_timeout_ms(state,&timeout_ms,error))goto cleanup;if(!WinHttpSetOption(request,WINHTTP_OPTION_RECEIVE_TIMEOUT,&timeout_ms,sizeof(timeout_ms))){*error=disp_http_win_error("HTTP receive timeout setup");goto cleanup;}DWORD available=0;if(!WinHttpQueryDataAvailable(request,&available)){*error=disp_http_win_error("HTTP body availability query");goto cleanup;}if(!available)break;if((size_t)available>DISP_HTTP_BODY_LIMIT-result->body.len){*error=disp_owned_bytes("HTTP response body exceeds the 16 MiB limit",strlen("HTTP response body exceeds the 16 MiB limit"));goto cleanup;}size_t next=result->body.len+(size_t)available;result->body.data=(char*)disp_realloc(result->body.data,next?next:1,1);DWORD read=0;if(!WinHttpReadData(request,result->body.data+result->body.len,available,&read)){*error=disp_http_win_error("HTTP body read");goto cleanup;}if(!read){*error=disp_owned_bytes("HTTP body read made no progress",strlen("HTTP body read made no progress"));goto cleanup;}result->body.len+=(size_t)read;result->body.cap=next;}
    response->state=result;result=NULL;ok=true;
cleanup:
    if(request)WinHttpCloseHandle(request);if(connection)WinHttpCloseHandle(connection);
    disp_dealloc(target);disp_dealloc(host);disp_dealloc(wide_headers);disp_dealloc(wide_method);disp_dealloc(wide_url);disp_http_response_state_free(result);return ok;
}
#else
typedef struct {disp_http_request_state *request;disp_http_response_state *response;bool failed;const char *message;} disp_curl_context;
static pthread_once_t disp_curl_once=PTHREAD_ONCE_INIT;
static CURLcode disp_curl_init_code=CURLE_OK;
static CURLSH *disp_curl_share=NULL;
static pthread_mutex_t disp_curl_locks[CURL_LOCK_DATA_LAST];
static void disp_curl_lock(CURL *handle,curl_lock_data data,curl_lock_access access,void *user){(void)handle;(void)access;(void)user;if(data>0&&data<CURL_LOCK_DATA_LAST)pthread_mutex_lock(&disp_curl_locks[data]);}
static void disp_curl_unlock(CURL *handle,curl_lock_data data,void *user){(void)handle;(void)user;if(data>0&&data<CURL_LOCK_DATA_LAST)pthread_mutex_unlock(&disp_curl_locks[data]);}
static void disp_curl_cleanup(void){if(disp_curl_share)curl_share_cleanup(disp_curl_share);for(int i=1;i<CURL_LOCK_DATA_LAST;i++)pthread_mutex_destroy(&disp_curl_locks[i]);curl_global_cleanup();}
static void disp_curl_initialize(void){disp_curl_init_code=curl_global_init(CURL_GLOBAL_DEFAULT);if(disp_curl_init_code!=CURLE_OK)return;for(int i=1;i<CURL_LOCK_DATA_LAST;i++)pthread_mutex_init(&disp_curl_locks[i],NULL);disp_curl_share=curl_share_init();if(!disp_curl_share){disp_curl_init_code=CURLE_OUT_OF_MEMORY;disp_curl_cleanup();return;}curl_share_setopt(disp_curl_share,CURLSHOPT_LOCKFUNC,disp_curl_lock);curl_share_setopt(disp_curl_share,CURLSHOPT_UNLOCKFUNC,disp_curl_unlock);curl_share_setopt(disp_curl_share,CURLSHOPT_SHARE,CURL_LOCK_DATA_CONNECT);curl_share_setopt(disp_curl_share,CURLSHOPT_SHARE,CURL_LOCK_DATA_DNS);curl_share_setopt(disp_curl_share,CURLSHOPT_SHARE,CURL_LOCK_DATA_SSL_SESSION);atexit(disp_curl_cleanup);}
static size_t disp_curl_body(char *data,size_t size,size_t count,void *raw){disp_curl_context *context=(disp_curl_context*)raw;size_t bytes;if(__builtin_mul_overflow(size,count,&bytes)||bytes>DISP_HTTP_BODY_LIMIT-context->response->body.len){context->failed=true;context->message="HTTP response body exceeds the 16 MiB limit";return 0;}size_t next=context->response->body.len+bytes;context->response->body.data=(char*)disp_realloc(context->response->body.data,next?next:1,1);if(bytes)memcpy(context->response->body.data+context->response->body.len,data,bytes);context->response->body.len=context->response->body.cap=next;return bytes;}
static size_t disp_curl_header(char *data,size_t size,size_t count,void *raw){disp_curl_context *context=(disp_curl_context*)raw;size_t bytes;if(__builtin_mul_overflow(size,count,&bytes))return 0;if(bytes>=5&&disp_http_name_equal(data,5,"HTTP/",5)){disp_string_drop(&context->response->headers);disp_string_drop(&context->response->body);}if(bytes>DISP_HTTP_HEADER_LIMIT-context->response->headers.len){context->failed=true;context->message="HTTP response headers exceed the 64 KiB limit";return 0;}size_t next=context->response->headers.len+bytes;context->response->headers.data=(char*)disp_realloc(context->response->headers.data,next?next:1,1);if(bytes)memcpy(context->response->headers.data+context->response->headers.len,data,bytes);context->response->headers.len=context->response->headers.cap=next;return bytes;}
static int disp_curl_progress(void *raw,curl_off_t download_total,curl_off_t download_now,curl_off_t upload_total,curl_off_t upload_now){(void)download_total;(void)download_now;(void)upload_total;(void)upload_now;disp_curl_context *context=(disp_curl_context*)raw;return atomic_load_explicit(&context->request->cancelled,memory_order_acquire)?1:0;}
static bool disp_http_execute(disp_http_request_state *state,disp_native_http_response *response,disp_native_string *error){
    CURL *curl=NULL;struct curl_slist *headers=NULL;disp_http_response_state *result=NULL;char *url=NULL,*method=NULL;bool ok=false;int timeout_ms=0;CURLcode code=CURLE_OK;
    if(!disp_http_url_valid(state->url.data,state->url.len,error)||!disp_http_timeout_ms(state,&timeout_ms,error))goto cleanup;
    pthread_once(&disp_curl_once,disp_curl_initialize);if(disp_curl_init_code!=CURLE_OK){*error=disp_owned_bytes(curl_easy_strerror(disp_curl_init_code),strlen(curl_easy_strerror(disp_curl_init_code)));goto cleanup;}
    curl=curl_easy_init();if(!curl){*error=disp_owned_bytes("HTTP client initialization failed",strlen("HTTP client initialization failed"));goto cleanup;}
    url=(char*)disp_alloc(state->url.len+1,1);memcpy(url,state->url.data,state->url.len);url[state->url.len]=0;
    method=(char*)disp_alloc(state->method.len+1,1);memcpy(method,state->method.data,state->method.len);method[state->method.len]=0;
    for(size_t start=0;start<state->headers.len;){size_t end=start;while(end+1<state->headers.len&&(state->headers.data[end]!='\r'||state->headers.data[end+1]!='\n'))end++;if(end+1>=state->headers.len)break;char *line=(char*)disp_alloc(end-start+1,1);memcpy(line,state->headers.data+start,end-start);line[end-start]=0;struct curl_slist *next=curl_slist_append(headers,line);disp_dealloc(line);if(!next){*error=disp_owned_bytes("HTTP header allocation failed",strlen("HTTP header allocation failed"));goto cleanup;}headers=next;start=end+2;}
    result=(disp_http_response_state*)disp_alloc_zeroed(1,sizeof(disp_http_response_state),_Alignof(disp_http_response_state));disp_curl_context context={.request=state,.response=result};
    bool redirectable=(disp_http_name_equal(state->method.data,state->method.len,"GET",3)||disp_http_name_equal(state->method.data,state->method.len,"HEAD",4))&&!state->headers.len&&!state->body.len;
    #define DISP_CURL_SET(option,value) do{code=curl_easy_setopt(curl,option,value);if(code!=CURLE_OK){const char *message=curl_easy_strerror(code);*error=disp_owned_bytes(message,strlen(message));goto cleanup;}}while(0)
    DISP_CURL_SET(CURLOPT_SHARE,disp_curl_share);DISP_CURL_SET(CURLOPT_URL,url);DISP_CURL_SET(CURLOPT_CUSTOMREQUEST,method);DISP_CURL_SET(CURLOPT_HTTPHEADER,headers);DISP_CURL_SET(CURLOPT_USERAGENT,"DISP/0.1");DISP_CURL_SET(CURLOPT_NOSIGNAL,1L);DISP_CURL_SET(CURLOPT_TIMEOUT_MS,(long)timeout_ms);DISP_CURL_SET(CURLOPT_CONNECTTIMEOUT_MS,(long)timeout_ms);DISP_CURL_SET(CURLOPT_SSL_VERIFYPEER,1L);DISP_CURL_SET(CURLOPT_SSL_VERIFYHOST,2L);DISP_CURL_SET(CURLOPT_SSLVERSION,(long)CURL_SSLVERSION_TLSv1_2);DISP_CURL_SET(CURLOPT_PROTOCOLS_STR,"http,https");DISP_CURL_SET(CURLOPT_FOLLOWLOCATION,redirectable?1L:0L);DISP_CURL_SET(CURLOPT_MAXREDIRS,(long)DISP_HTTP_REDIRECT_LIMIT);DISP_CURL_SET(CURLOPT_REDIR_PROTOCOLS_STR,disp_http_name_equal(url,5,"https",5)?"https":"http,https");DISP_CURL_SET(CURLOPT_WRITEFUNCTION,disp_curl_body);DISP_CURL_SET(CURLOPT_WRITEDATA,&context);DISP_CURL_SET(CURLOPT_HEADERFUNCTION,disp_curl_header);DISP_CURL_SET(CURLOPT_HEADERDATA,&context);DISP_CURL_SET(CURLOPT_XFERINFOFUNCTION,disp_curl_progress);DISP_CURL_SET(CURLOPT_XFERINFODATA,&context);DISP_CURL_SET(CURLOPT_NOPROGRESS,0L);
    if(state->body.len||disp_http_name_equal(method,state->method.len,"POST",4)||disp_http_name_equal(method,state->method.len,"PUT",3)||disp_http_name_equal(method,state->method.len,"PATCH",5)){DISP_CURL_SET(CURLOPT_POSTFIELDS,state->body.data?state->body.data:"");DISP_CURL_SET(CURLOPT_POSTFIELDSIZE_LARGE,(curl_off_t)state->body.len);}
    #undef DISP_CURL_SET
    code=curl_easy_perform(curl);if(code!=CURLE_OK){const char *message=context.failed?context.message:(code==CURLE_ABORTED_BY_CALLBACK?"HTTP request was cancelled":curl_easy_strerror(code));*error=disp_owned_bytes(message,strlen(message));goto cleanup;}
    long status=0;char *effective=NULL;if(curl_easy_getinfo(curl,CURLINFO_RESPONSE_CODE,&status)!=CURLE_OK||status<0||status>999||curl_easy_getinfo(curl,CURLINFO_EFFECTIVE_URL,&effective)!=CURLE_OK||!effective){*error=disp_owned_bytes("HTTP response metadata is invalid",strlen("HTTP response metadata is invalid"));goto cleanup;}
    result->status=(uint16_t)status;result->url=disp_owned_bytes(effective,strlen(effective));response->state=result;result=NULL;ok=true;
cleanup:
    curl_slist_free_all(headers);if(curl)curl_easy_cleanup(curl);disp_dealloc(method);disp_dealloc(url);disp_http_response_state_free(result);return ok;
}
#endif
#define disp_http_request_release disp_http_request_release_legacy
static void disp_http_request_release(disp_http_request_state *state){if(atomic_fetch_sub_explicit(&state->refs,1,memory_order_acq_rel)!=1)return;atomic_thread_fence(memory_order_acquire);disp_string_drop(&state->url);disp_http_response_drop(&state->response);disp_string_drop(&state->error);disp_dealloc(state);}
#undef disp_http_request_release
static void disp_http_request_release(disp_http_request_state *state){if(atomic_fetch_sub_explicit(&state->refs,1,memory_order_acq_rel)!=1)return;atomic_thread_fence(memory_order_acquire);disp_string_drop(&state->method);disp_string_drop(&state->url);disp_string_drop(&state->headers);disp_string_drop(&state->body);disp_http_response_drop(&state->response);disp_string_drop(&state->error);disp_dealloc(state);}
static void disp_http_request_worker(void *raw){disp_http_request_state *state=(disp_http_request_state*)raw;disp_native_http_response response={0};disp_native_string error={0};disp_runtime_acquire_handle();bool ok=disp_http_execute(state,&response,&error);disp_runtime_release_handle();int expected=0;if(atomic_compare_exchange_strong_explicit(&state->owner,&expected,1,memory_order_acq_rel,memory_order_acquire)){state->ok=ok;state->response=response;state->error=error;atomic_store_explicit(&state->done,true,memory_order_release);}else{disp_http_response_drop(&response);disp_string_drop(&error);}disp_http_request_release(state);atomic_fetch_sub_explicit(&disp_async_jobs,1,memory_order_acq_rel);}
static disp_http_request_state *disp_http_request_create(const char *method,size_t method_len,const char *url,size_t url_len,const char *headers,size_t headers_len,const char *body,size_t body_len,uint64_t timeout,int line,int column){disp_http_request_state *state=(disp_http_request_state*)disp_alloc_zeroed(1,sizeof(disp_http_request_state),_Alignof(disp_http_request_state));atomic_init(&state->refs,1);atomic_init(&state->owner,0);atomic_init(&state->done,false);atomic_init(&state->cancelled,false);state->method=disp_owned_bytes(method,method_len);state->url=disp_owned_bytes(url,url_len);state->headers=disp_owned_bytes(headers,headers_len);state->body=disp_owned_bytes(body,body_len);state->timeout=timeout;state->line=line;state->column=column;if(body_len>DISP_HTTP_BODY_LIMIT||headers_len>DISP_HTTP_HEADER_LIMIT){atomic_store_explicit(&state->owner,2,memory_order_release);state->error=disp_owned_bytes(body_len>DISP_HTTP_BODY_LIMIT?"HTTP request body exceeds the 16 MiB limit":"HTTP request headers exceed the 64 KiB limit",body_len>DISP_HTTP_BODY_LIMIT?strlen("HTTP request body exceeds the 16 MiB limit"):strlen("HTTP request headers exceed the 64 KiB limit"));atomic_store_explicit(&state->done,true,memory_order_release);}return state;}
static disp_http_request_state *disp_http_request_from_builder(disp_native_http_request *request,uint64_t timeout,int line,int column){disp_http_builder_state *builder=disp_http_builder_take(request);if(!builder)dv_panic("HTTP request is unavailable",line,column);disp_http_request_state *state=(disp_http_request_state*)disp_alloc_zeroed(1,sizeof(disp_http_request_state),_Alignof(disp_http_request_state));atomic_init(&state->refs,1);atomic_init(&state->owner,0);atomic_init(&state->done,false);atomic_init(&state->cancelled,false);state->method=builder->method;builder->method=(disp_native_string){0};state->url=builder->url;builder->url=(disp_native_string){0};state->headers=builder->headers;builder->headers=(disp_native_string){0};state->body=builder->body;builder->body=(disp_native_string){0};state->timeout=timeout;state->line=line;state->column=column;disp_http_builder_state_free(builder);return state;}
static bool disp_http_request_poll(disp_http_request_state *state){if(!state||state->taken)dv_panic("HTTP future has already completed",0,0);if(atomic_load_explicit(&state->done,memory_order_acquire))return true;if(!state->started){state->started=true;uint64_t now=disp_time_now_nanos();state->deadline=UINT64_MAX-now<state->timeout?UINT64_MAX:now+state->timeout;if(!state->timeout){atomic_store_explicit(&state->owner,2,memory_order_release);state->error=disp_owned_bytes("HTTP request timed out",strlen("HTTP request timed out"));atomic_store_explicit(&state->done,true,memory_order_release);return true;}atomic_fetch_add_explicit(&state->refs,1,memory_order_relaxed);atomic_fetch_add_explicit(&disp_async_jobs,1,memory_order_relaxed);uintptr_t handle=disp_thread_start(disp_http_request_worker,state,state->line,state->column);disp_thread_detach(handle);}if(disp_time_now_nanos()>=state->deadline){int expected=0;if(atomic_compare_exchange_strong_explicit(&state->owner,&expected,2,memory_order_acq_rel,memory_order_acquire)){atomic_store_explicit(&state->cancelled,true,memory_order_release);state->error=disp_owned_bytes("HTTP request timed out",strlen("HTTP request timed out"));atomic_store_explicit(&state->done,true,memory_order_release);return true;}}disp_reactor_offer(1000000ULL);return false;}
static void disp_http_request_take(disp_http_request_state *state,bool *ok,disp_native_http_response *response,disp_native_string *error){if(!atomic_load_explicit(&state->done,memory_order_acquire)||state->taken)dv_panic("HTTP result is not ready",0,0);state->taken=true;*ok=state->ok;*response=state->response;state->response=(disp_native_http_response){0};*error=state->error;state->error=(disp_native_string){0};}
static void disp_http_request_drop(void *raw){disp_http_request_state *state=(disp_http_request_state*)raw;if(!state)return;atomic_store_explicit(&state->cancelled,true,memory_order_release);int expected=0;atomic_compare_exchange_strong_explicit(&state->owner,&expected,2,memory_order_acq_rel,memory_order_acquire);disp_http_request_release(state);}
#endif
typedef enum { DISP_SOCKET_READ,DISP_SOCKET_WRITE } disp_socket_io_operation;
typedef struct { disp_tcp_state *stream;disp_socket_io_operation operation;bool started;bool claimed;bool taken;bool done;bool ok;bool has_deadline;uint64_t timeout;uint64_t deadline;size_t limit;size_t offset;disp_native_string buffer;disp_native_string error; } disp_socket_io_state;
static disp_socket_io_state *disp_socket_io_create(disp_tcp_state *stream,disp_socket_io_operation operation,const char *bytes,size_t length,bool has_timeout,uint64_t timeout,int line,int column){if(!stream)dv_panic("TCP stream is unavailable",line,column);if(operation==DISP_SOCKET_READ&&length>DISP_TCP_READ_LIMIT)dv_panic("TCP read limit exceeds the 16 MiB safety limit",line,column);disp_socket_io_state *state=(disp_socket_io_state*)disp_alloc_zeroed(1,sizeof(disp_socket_io_state),_Alignof(disp_socket_io_state));state->stream=stream;disp_tcp_state_retain(stream);state->operation=operation;state->has_deadline=has_timeout;state->timeout=timeout;state->limit=length;if(operation==DISP_SOCKET_WRITE&&length)state->buffer=disp_owned_bytes(bytes,length);return state;}
static void disp_socket_io_finish(disp_socket_io_state *state,bool ok){state->ok=ok;state->done=true;if(state->claimed){atomic_store_explicit(state->operation==DISP_SOCKET_READ?&state->stream->read_busy:&state->stream->write_busy,false,memory_order_release);state->claimed=false;}}
static bool disp_socket_io_poll(disp_socket_io_state *state){if(!state||state->taken)dv_panic("TCP socket future has already completed",0,0);if(state->done)return true;if(!state->started){state->started=true;uint64_t now=disp_time_now_nanos();state->deadline=UINT64_MAX-now<state->timeout?UINT64_MAX:now+state->timeout;}if(atomic_load_explicit(&state->stream->closed,memory_order_acquire)){state->error=disp_owned_bytes("TCP stream is closed",strlen("TCP stream is closed"));disp_socket_io_finish(state,false);return true;}atomic_bool *busy=state->operation==DISP_SOCKET_READ?&state->stream->read_busy:&state->stream->write_busy;if(!state->claimed){if(!disp_tcp_claim(busy)){uint64_t now=disp_time_now_nanos();if(state->has_deadline&&now>=state->deadline){state->error=disp_owned_bytes("TCP operation timed out",strlen("TCP operation timed out"));disp_socket_io_finish(state,false);return true;}disp_reactor_offer(1000000ULL);return false;}state->claimed=true;}if(state->operation==DISP_SOCKET_READ&&atomic_load_explicit(&state->stream->read_shutdown,memory_order_acquire)){state->error=disp_owned_bytes("TCP read side is shut down",strlen("TCP read side is shut down"));disp_socket_io_finish(state,false);return true;}if(state->operation==DISP_SOCKET_WRITE&&atomic_load_explicit(&state->stream->write_shutdown,memory_order_acquire)){state->error=disp_owned_bytes("TCP write side is shut down",strlen("TCP write side is shut down"));disp_socket_io_finish(state,false);return true;}if(state->operation==DISP_SOCKET_WRITE&&state->offset==state->limit){disp_socket_io_finish(state,true);return true;}if(state->operation==DISP_SOCKET_READ&&!state->limit){disp_socket_io_finish(state,true);return true;}int ready_error=0;int ready=disp_socket_ready(state->stream->socket,state->operation==DISP_SOCKET_READ,&ready_error);if(ready<0){state->error=disp_network_error_code(state->operation==DISP_SOCKET_READ?"TCP read readiness":"TCP write readiness",ready_error);disp_socket_io_finish(state,false);return true;}if(!ready){uint64_t now=disp_time_now_nanos();if(state->has_deadline&&now>=state->deadline){state->error=disp_owned_bytes(state->operation==DISP_SOCKET_READ?"TCP read timed out":"TCP write timed out",state->operation==DISP_SOCKET_READ?strlen("TCP read timed out"):strlen("TCP write timed out"));disp_socket_io_finish(state,false);return true;}uint64_t wait=1000000ULL;if(state->has_deadline&&state->deadline-now<wait)wait=state->deadline-now;disp_reactor_offer(wait);return false;}if(state->operation==DISP_SOCKET_READ){char *data=(char*)disp_alloc(state->limit,1);
#ifdef _WIN32
int count=recv(state->stream->socket,data,(int)state->limit,0);
#else
ssize_t count=recv(state->stream->socket,data,state->limit,0);
#endif
if(count<0){int code=disp_socket_error_code();disp_dealloc(data);if(disp_socket_would_block(code)){disp_reactor_offer(1000000ULL);return false;}state->error=disp_network_error_code("TCP read",code);disp_socket_io_finish(state,false);return true;}if(!count)disp_dealloc(data);else state->buffer=(disp_native_string){.data=data,.len=(size_t)count,.cap=state->limit};disp_socket_io_finish(state,true);return true;}
size_t remaining=state->limit-state->offset;size_t chunk=remaining>65536?65536:remaining;
#ifdef _WIN32
int count=send(state->stream->socket,state->buffer.data+state->offset,(int)chunk,0);
#else
ssize_t count=send(state->stream->socket,state->buffer.data+state->offset,chunk,
#ifdef MSG_NOSIGNAL
MSG_NOSIGNAL
#else
0
#endif
);
#endif
if(count<0){int code=disp_socket_error_code();if(disp_socket_would_block(code)){disp_reactor_offer(1000000ULL);return false;}state->error=disp_network_error_code("TCP write",code);disp_socket_io_finish(state,false);return true;}if(!count){state->error=disp_owned_bytes("TCP write made no progress",strlen("TCP write made no progress"));disp_socket_io_finish(state,false);return true;}state->offset+=(size_t)count;if(state->offset==state->limit){disp_socket_io_finish(state,true);return true;}disp_reactor_offer(0);return false;}
static void disp_socket_io_take(disp_socket_io_state *state,bool *ok,disp_native_string *bytes,size_t *written,disp_native_string *error){if(!state->done||state->taken)dv_panic("TCP socket result is not ready",0,0);state->taken=true;*ok=state->ok;*written=state->offset;*error=state->error;state->error=(disp_native_string){0};if(state->operation==DISP_SOCKET_READ){*bytes=state->buffer;state->buffer=(disp_native_string){0};}else *bytes=(disp_native_string){0};}
static void disp_socket_io_drop(void *raw){disp_socket_io_state *state=(disp_socket_io_state*)raw;if(!state)return;if(state->claimed)atomic_store_explicit(state->operation==DISP_SOCKET_READ?&state->stream->read_busy:&state->stream->write_busy,false,memory_order_release);disp_string_drop(&state->buffer);disp_string_drop(&state->error);disp_tcp_state_release(state->stream);disp_dealloc(state);}
typedef struct { atomic_size_t refs;atomic_bool done;atomic_bool cancelled;bool started;bool taken;bool ok;bool has_deadline;uint64_t timeout;uint64_t deadline;int line;int column;disp_native_socket_address address;disp_native_tcp_stream stream;disp_native_string error; } disp_connect_state;
static void disp_connect_release(disp_connect_state *state){if(atomic_fetch_sub_explicit(&state->refs,1,memory_order_acq_rel)!=1)return;atomic_thread_fence(memory_order_acquire);disp_socket_address_drop(&state->address);disp_tcp_stream_drop(&state->stream);disp_string_drop(&state->error);disp_dealloc(state);}
static void disp_connect_worker(void *raw){disp_connect_state *state=(disp_connect_state*)raw;disp_network_init();char port[6];snprintf(port,sizeof(port),"%u",(unsigned)state->address.port);struct addrinfo hints={0},*addresses=NULL;hints.ai_family=AF_UNSPEC;hints.ai_socktype=SOCK_STREAM;hints.ai_protocol=IPPROTO_TCP;int status=getaddrinfo(state->address.host,port,&hints,&addresses);if(status!=0){
#ifdef _WIN32
const char *message=gai_strerrorA(status);
#else
const char *message=gai_strerror(status);
#endif
state->error=disp_owned_bytes(message?message:"address resolution failed",message?strlen(message):strlen("address resolution failed"));}else{int last_error=0;bool timed_out=false;for(struct addrinfo *address=addresses;address;address=address->ai_next){if(atomic_load_explicit(&state->cancelled,memory_order_acquire))break;if(state->has_deadline&&disp_time_now_nanos()>=state->deadline){timed_out=true;break;}disp_socket_handle socket_handle=socket(address->ai_family,address->ai_socktype,address->ai_protocol);if(socket_handle==DISP_INVALID_SOCKET){last_error=disp_socket_error_code();continue;}int connected=-1;if(!disp_socket_set_blocking(socket_handle,false)){last_error=disp_socket_error_code();}else{connected=connect(socket_handle,address->ai_addr,(int)address->ai_addrlen);if(connected!=0){last_error=disp_socket_error_code();if(disp_socket_connect_pending(last_error)){for(;;){if(atomic_load_explicit(&state->cancelled,memory_order_acquire))break;if(state->has_deadline&&disp_time_now_nanos()>=state->deadline){timed_out=true;break;}int ready_error=0;int ready=disp_socket_connect_ready(socket_handle,&ready_error);if(ready<0){last_error=ready_error;break;}if(ready>0){last_error=disp_socket_connection_error(socket_handle);if(!last_error)connected=0;break;}disp_time_sleep(1000000ULL);}}}}if(connected==0&&!atomic_load_explicit(&state->cancelled,memory_order_acquire)){state->stream=(disp_native_tcp_stream){.state=disp_tcp_state_create(socket_handle)};state->ok=true;break;}disp_socket_close(socket_handle);if(timed_out)break;}if(!state->ok&&!atomic_load_explicit(&state->cancelled,memory_order_acquire)){if(timed_out)state->error=disp_owned_bytes("TCP connect timed out",strlen("TCP connect timed out"));else state->error=disp_network_error_code("TCP connect",last_error);}freeaddrinfo(addresses);}disp_socket_address_drop(&state->address);atomic_store_explicit(&state->done,true,memory_order_release);disp_connect_release(state);atomic_fetch_sub_explicit(&disp_async_jobs,1,memory_order_acq_rel);}
static disp_connect_state *disp_connect_create(disp_native_socket_address address,bool has_timeout,uint64_t timeout,int line,int column){disp_connect_state *state=(disp_connect_state*)disp_alloc_zeroed(1,sizeof(disp_connect_state),_Alignof(disp_connect_state));atomic_init(&state->refs,1);atomic_init(&state->done,false);atomic_init(&state->cancelled,false);state->address=address;state->has_deadline=has_timeout;state->timeout=timeout;state->line=line;state->column=column;return state;}
static bool disp_connect_poll(disp_connect_state *state){if(!state||state->taken)dv_panic("TCP connect future has already completed",0,0);if(!state->started){state->started=true;uint64_t now=disp_time_now_nanos();state->deadline=UINT64_MAX-now<state->timeout?UINT64_MAX:now+state->timeout;atomic_fetch_add_explicit(&state->refs,1,memory_order_relaxed);atomic_fetch_add_explicit(&disp_async_jobs,1,memory_order_relaxed);uintptr_t handle=disp_thread_start(disp_connect_worker,state,state->line,state->column);disp_thread_detach(handle);}if(!atomic_load_explicit(&state->done,memory_order_acquire)){disp_reactor_offer(1000000ULL);return false;}return true;}
static void disp_connect_take(disp_connect_state *state,bool *ok,disp_native_tcp_stream *stream,disp_native_string *error){if(!atomic_load_explicit(&state->done,memory_order_acquire)||state->taken)dv_panic("TCP connect result is not ready",0,0);state->taken=true;*ok=state->ok;*stream=state->stream;*error=state->error;state->stream=(disp_native_tcp_stream){0};state->error=(disp_native_string){0};}
static void disp_connect_drop(void *raw){disp_connect_state *state=(disp_connect_state*)raw;if(!state)return;atomic_store_explicit(&state->cancelled,true,memory_order_release);disp_connect_release(state);}
struct disp_tcp_listener_state { atomic_size_t refs;atomic_bool closed;disp_socket_handle socket; };
static void disp_tcp_listener_retain(disp_tcp_listener_state *state){atomic_fetch_add_explicit(&state->refs,1,memory_order_relaxed);}
static void disp_tcp_listener_close(disp_native_tcp_listener *listener){if(!listener->state)return;if(!atomic_exchange_explicit(&listener->state->closed,true,memory_order_acq_rel)){disp_socket_close(listener->state->socket);disp_runtime_release_handle();}}
static void disp_tcp_listener_release(disp_tcp_listener_state *state){if(atomic_fetch_sub_explicit(&state->refs,1,memory_order_acq_rel)!=1)return;atomic_thread_fence(memory_order_acquire);if(!atomic_load_explicit(&state->closed,memory_order_acquire)){disp_socket_close(state->socket);disp_runtime_release_handle();}disp_dealloc(state);}
static void disp_tcp_listener_drop(disp_native_tcp_listener *listener){if(!listener->state)return;disp_tcp_listener_close(listener);disp_tcp_listener_release(listener->state);listener->state=NULL;}
static bool disp_tcp_listener_bind(disp_native_socket_address address,disp_native_tcp_listener *listener,disp_native_string *error){disp_network_init();char port[6];snprintf(port,sizeof(port),"%u",(unsigned)address.port);struct addrinfo hints={0},*addresses=NULL;hints.ai_family=AF_UNSPEC;hints.ai_socktype=SOCK_STREAM;hints.ai_protocol=IPPROTO_TCP;hints.ai_flags=AI_PASSIVE;int status=getaddrinfo(address.host,port,&hints,&addresses);if(status!=0){
#ifdef _WIN32
const char *message=gai_strerrorA(status);
#else
const char *message=gai_strerror(status);
#endif
*error=disp_owned_bytes(message?message:"address resolution failed",message?strlen(message):25);disp_socket_address_drop(&address);return false;}int last_error=0;disp_socket_handle bound=DISP_INVALID_SOCKET;for(struct addrinfo *candidate=addresses;candidate;candidate=candidate->ai_next){disp_socket_handle handle=socket(candidate->ai_family,candidate->ai_socktype,candidate->ai_protocol);if(handle==DISP_INVALID_SOCKET){last_error=disp_socket_error_code();continue;}int reuse=1;setsockopt(handle,SOL_SOCKET,SO_REUSEADDR,(const char*)&reuse,sizeof(reuse));if(bind(handle,candidate->ai_addr,(int)candidate->ai_addrlen)==0&&listen(handle,SOMAXCONN)==0&&disp_socket_set_blocking(handle,false)){bound=handle;break;}last_error=disp_socket_error_code();disp_socket_close(handle);}freeaddrinfo(addresses);disp_socket_address_drop(&address);if(bound==DISP_INVALID_SOCKET){*error=disp_network_error_code("TCP bind",last_error);return false;}disp_tcp_listener_state *state=(disp_tcp_listener_state*)disp_alloc(sizeof(disp_tcp_listener_state),_Alignof(disp_tcp_listener_state));atomic_init(&state->refs,1);atomic_init(&state->closed,false);state->socket=bound;disp_runtime_acquire_handle();listener->state=state;return true;}
static bool disp_tcp_listener_local_port(const disp_native_tcp_listener *listener,size_t *port,disp_native_string *error){if(!listener->state||atomic_load_explicit(&listener->state->closed,memory_order_acquire)){*error=disp_owned_bytes("TCP listener is closed",22);return false;}struct sockaddr_storage address;socklen_t length=sizeof(address);if(getsockname(listener->state->socket,(struct sockaddr*)&address,&length)!=0){*error=disp_network_error_code("TCP local address",disp_socket_error_code());return false;}if(address.ss_family==AF_INET)*port=(size_t)ntohs(((struct sockaddr_in*)&address)->sin_port);else if(address.ss_family==AF_INET6)*port=(size_t)ntohs(((struct sockaddr_in6*)&address)->sin6_port);else{*error=disp_owned_bytes("TCP listener has an unsupported address family",46);return false;}return true;}
typedef struct { disp_tcp_listener_state *listener;bool started;bool taken;bool done;bool ok;bool has_deadline;uint64_t timeout;uint64_t deadline;int line;int column;disp_native_tcp_stream stream;disp_native_string error; } disp_accept_state;
static disp_accept_state *disp_accept_create(const disp_native_tcp_listener *listener,bool has_timeout,uint64_t timeout,int line,int column){if(!listener||!listener->state)dv_panic("TCP listener is unavailable",line,column);disp_accept_state *state=(disp_accept_state*)disp_alloc_zeroed(1,sizeof(disp_accept_state),_Alignof(disp_accept_state));state->listener=listener->state;disp_tcp_listener_retain(state->listener);state->has_deadline=has_timeout;state->timeout=timeout;state->line=line;state->column=column;return state;}
static bool disp_accept_poll(disp_accept_state *state){if(!state||state->taken)dv_panic("TCP accept future has already completed",0,0);if(state->done)return true;if(!state->started){state->started=true;uint64_t now=disp_time_now_nanos();state->deadline=UINT64_MAX-now<state->timeout?UINT64_MAX:now+state->timeout;}if(atomic_load_explicit(&state->listener->closed,memory_order_acquire)){state->error=disp_owned_bytes("TCP listener is closed",22);state->done=true;return true;}disp_socket_handle accepted=accept(state->listener->socket,NULL,NULL);if(accepted!=DISP_INVALID_SOCKET){if(!disp_socket_set_blocking(accepted,false)){int code=disp_socket_error_code();disp_socket_close(accepted);state->error=disp_network_error_code("TCP accepted socket setup",code);state->done=true;return true;}state->stream=(disp_native_tcp_stream){.state=disp_tcp_state_create(accepted)};state->ok=true;state->done=true;return true;}int code=disp_socket_error_code();if(!disp_socket_would_block(code)){state->error=disp_network_error_code("TCP accept",code);state->done=true;return true;}uint64_t now=disp_time_now_nanos();if(state->has_deadline&&now>=state->deadline){state->error=disp_owned_bytes("TCP accept timed out",20);state->done=true;return true;}uint64_t wait=1000000ULL;if(state->has_deadline&&state->deadline-now<wait)wait=state->deadline-now;disp_reactor_offer(wait);return false;}
static void disp_accept_take(disp_accept_state *state,bool *ok,disp_native_tcp_stream *stream,disp_native_string *error){if(!state->done||state->taken)dv_panic("TCP accept result is not ready",0,0);state->taken=true;*ok=state->ok;*stream=state->stream;*error=state->error;state->stream=(disp_native_tcp_stream){0};state->error=(disp_native_string){0};}
static void disp_accept_drop(void *raw){disp_accept_state *state=(disp_accept_state*)raw;if(!state)return;disp_tcp_stream_drop(&state->stream);disp_string_drop(&state->error);disp_tcp_listener_release(state->listener);disp_dealloc(state);}
struct disp_udp_socket_state { atomic_size_t refs;atomic_bool closed;atomic_bool receive_busy;atomic_bool send_busy;disp_socket_handle socket;int family; };
static disp_udp_socket_state *disp_udp_socket_state_create(disp_socket_handle socket,int family){disp_udp_socket_state *state=(disp_udp_socket_state*)disp_alloc(sizeof(disp_udp_socket_state),_Alignof(disp_udp_socket_state));atomic_init(&state->refs,1);atomic_init(&state->closed,false);atomic_init(&state->receive_busy,false);atomic_init(&state->send_busy,false);state->socket=socket;state->family=family;disp_runtime_acquire_handle();return state;}
static void disp_udp_socket_retain(disp_udp_socket_state *state){atomic_fetch_add_explicit(&state->refs,1,memory_order_relaxed);}
static void disp_udp_socket_close(disp_native_udp_socket *socket){if(!socket||!socket->state)return;if(!atomic_exchange_explicit(&socket->state->closed,true,memory_order_acq_rel)){disp_socket_close(socket->state->socket);disp_runtime_release_handle();}}
static void disp_udp_socket_release(disp_udp_socket_state *state){if(atomic_fetch_sub_explicit(&state->refs,1,memory_order_acq_rel)!=1)return;atomic_thread_fence(memory_order_acquire);if(!atomic_load_explicit(&state->closed,memory_order_acquire)){disp_socket_close(state->socket);disp_runtime_release_handle();}disp_dealloc(state);}
static void disp_udp_socket_drop(disp_native_udp_socket *socket){if(!socket||!socket->state)return;disp_udp_socket_close(socket);disp_udp_socket_release(socket->state);socket->state=NULL;}
static bool disp_udp_resolve(const disp_native_socket_address *address,bool passive,int family,struct sockaddr_storage *resolved,socklen_t *length,disp_native_string *error){char port[6];snprintf(port,sizeof(port),"%u",(unsigned)address->port);struct addrinfo hints={0},*addresses=NULL;hints.ai_family=family;hints.ai_socktype=SOCK_DGRAM;hints.ai_protocol=IPPROTO_UDP;hints.ai_flags=passive?AI_PASSIVE:0;int status=getaddrinfo(address->host,port,&hints,&addresses);if(status!=0){
#ifdef _WIN32
const char *message=gai_strerrorA(status);
#else
const char *message=gai_strerror(status);
#endif
*error=disp_owned_bytes(message?message:"address resolution failed",message?strlen(message):strlen("address resolution failed"));return false;}bool found=false;for(struct addrinfo *candidate=addresses;candidate;candidate=candidate->ai_next){if(candidate->ai_addrlen<=sizeof(*resolved)){memcpy(resolved,candidate->ai_addr,candidate->ai_addrlen);*length=(socklen_t)candidate->ai_addrlen;found=true;break;}}freeaddrinfo(addresses);if(!found)*error=disp_owned_bytes("address resolution returned no usable UDP address",strlen("address resolution returned no usable UDP address"));return found;}
static disp_native_socket_address disp_socket_address_from_sockaddr(const struct sockaddr *source,socklen_t length,disp_native_string *error){char host[NI_MAXHOST];int status=getnameinfo(source,length,host,sizeof(host),NULL,0,NI_NUMERICHOST);if(status!=0){*error=disp_owned_bytes("could not format UDP sender address",strlen("could not format UDP sender address"));return (disp_native_socket_address){0};}uint16_t port=source->sa_family==AF_INET?ntohs(((const struct sockaddr_in*)source)->sin_port):source->sa_family==AF_INET6?ntohs(((const struct sockaddr_in6*)source)->sin6_port):0;return disp_socket_address_create(host,strlen(host),port,0,0);}
static bool disp_udp_socket_bind(disp_native_socket_address address,disp_native_udp_socket *output,disp_native_string *error){disp_network_init();char port[6];snprintf(port,sizeof(port),"%u",(unsigned)address.port);struct addrinfo hints={0},*addresses=NULL;hints.ai_family=AF_UNSPEC;hints.ai_socktype=SOCK_DGRAM;hints.ai_protocol=IPPROTO_UDP;hints.ai_flags=AI_PASSIVE;int status=getaddrinfo(address.host,port,&hints,&addresses);if(status!=0){
#ifdef _WIN32
const char *message=gai_strerrorA(status);
#else
const char *message=gai_strerror(status);
#endif
*error=disp_owned_bytes(message?message:"address resolution failed",message?strlen(message):strlen("address resolution failed"));disp_socket_address_drop(&address);return false;}int last_error=0;disp_socket_handle bound=DISP_INVALID_SOCKET;int family=AF_UNSPEC;for(struct addrinfo *candidate=addresses;candidate;candidate=candidate->ai_next){disp_socket_handle handle=socket(candidate->ai_family,candidate->ai_socktype,candidate->ai_protocol);if(handle==DISP_INVALID_SOCKET){last_error=disp_socket_error_code();continue;}if(bind(handle,candidate->ai_addr,(int)candidate->ai_addrlen)==0&&disp_socket_set_blocking(handle,false)){bound=handle;family=candidate->ai_family;break;}last_error=disp_socket_error_code();disp_socket_close(handle);}freeaddrinfo(addresses);disp_socket_address_drop(&address);if(bound==DISP_INVALID_SOCKET){*error=disp_network_error_code("UDP bind",last_error);return false;}output->state=disp_udp_socket_state_create(bound,family);return true;}
static bool disp_udp_socket_local_port(const disp_native_udp_socket *socket,size_t *port,disp_native_string *error){if(!socket->state||atomic_load_explicit(&socket->state->closed,memory_order_acquire)){*error=disp_owned_bytes("UDP socket is closed",strlen("UDP socket is closed"));return false;}struct sockaddr_storage address;socklen_t length=sizeof(address);if(getsockname(socket->state->socket,(struct sockaddr*)&address,&length)!=0){*error=disp_network_error_code("UDP local address",disp_socket_error_code());return false;}if(address.ss_family==AF_INET)*port=(size_t)ntohs(((struct sockaddr_in*)&address)->sin_port);else if(address.ss_family==AF_INET6)*port=(size_t)ntohs(((struct sockaddr_in6*)&address)->sin6_port);else{*error=disp_owned_bytes("UDP socket has an unsupported address family",strlen("UDP socket has an unsupported address family"));return false;}return true;}
static void disp_udp_datagram_drop(disp_native_udp_datagram *datagram){if(!datagram)return;disp_socket_address_drop(&datagram->source);disp_dealloc(datagram->data);datagram->data=NULL;datagram->len=0;datagram->cap=0;}
static bool disp_udp_socket_receive(disp_native_udp_socket *socket,size_t limit,disp_native_udp_datagram *datagram,disp_native_string *error,int line,int column){if(limit>DISP_UDP_RECEIVE_LIMIT)dv_panic("UDP receive limit exceeds 65535 bytes",line,column);if(!socket->state||atomic_load_explicit(&socket->state->closed,memory_order_acquire)){*error=disp_owned_bytes("UDP socket is closed",strlen("UDP socket is closed"));return false;}if(!disp_tcp_claim(&socket->state->receive_busy)){*error=disp_owned_bytes("UDP receive is already in progress",strlen("UDP receive is already in progress"));return false;}size_t capacity=limit+1;uint8_t *data=(uint8_t*)disp_alloc(capacity,1);for(;;){struct sockaddr_storage source={0};socklen_t source_length=sizeof(source);
#ifdef _WIN32
int count=recvfrom(socket->state->socket,(char*)data,(int)capacity,0,(struct sockaddr*)&source,&source_length);
#else
ssize_t count=recvfrom(socket->state->socket,data,capacity,0,(struct sockaddr*)&source,&source_length);
#endif
if(count<0){int code=disp_socket_error_code();if(disp_socket_would_block(code)){disp_time_sleep(1000000ULL);continue;}disp_dealloc(data);atomic_store_explicit(&socket->state->receive_busy,false,memory_order_release);*error=disp_network_error_code("UDP receive",code);return false;}atomic_store_explicit(&socket->state->receive_busy,false,memory_order_release);if((size_t)count>limit){disp_dealloc(data);*error=disp_owned_bytes("UDP datagram exceeds receive limit",strlen("UDP datagram exceeds receive limit"));return false;}disp_native_socket_address sender=disp_socket_address_from_sockaddr((struct sockaddr*)&source,source_length,error);if(!sender.host){disp_dealloc(data);return false;}datagram->source=sender;datagram->data=data;datagram->len=(size_t)count;datagram->cap=capacity;return true;}}
static bool disp_udp_socket_send(disp_native_udp_socket *socket,const char *bytes,size_t length,const disp_native_socket_address *address,size_t *sent,disp_native_string *error){if(length>DISP_UDP_PAYLOAD_LIMIT){*error=disp_owned_bytes("UDP datagram exceeds the 65507-byte payload limit",strlen("UDP datagram exceeds the 65507-byte payload limit"));return false;}if(!socket->state||atomic_load_explicit(&socket->state->closed,memory_order_acquire)){*error=disp_owned_bytes("UDP socket is closed",strlen("UDP socket is closed"));return false;}if(!disp_tcp_claim(&socket->state->send_busy)){*error=disp_owned_bytes("UDP send is already in progress",strlen("UDP send is already in progress"));return false;}struct sockaddr_storage destination={0};socklen_t destination_length=0;if(!disp_udp_resolve(address,false,socket->state->family,&destination,&destination_length,error)){atomic_store_explicit(&socket->state->send_busy,false,memory_order_release);return false;}for(;;){
#ifdef _WIN32
int count=sendto(socket->state->socket,bytes?bytes:"",(int)length,0,(struct sockaddr*)&destination,destination_length);
#else
ssize_t count=sendto(socket->state->socket,bytes?bytes:"",length,
#ifdef MSG_NOSIGNAL
MSG_NOSIGNAL
#else
0
#endif
,(struct sockaddr*)&destination,destination_length);
#endif
if(count<0){int code=disp_socket_error_code();if(disp_socket_would_block(code)){disp_time_sleep(1000000ULL);continue;}atomic_store_explicit(&socket->state->send_busy,false,memory_order_release);*error=disp_network_error_code("UDP send",code);return false;}atomic_store_explicit(&socket->state->send_busy,false,memory_order_release);*sent=(size_t)count;return true;}}
typedef enum { DISP_UDP_RECEIVE,DISP_UDP_SEND } disp_udp_io_operation;
typedef struct { atomic_size_t refs;atomic_int resolution_owner;atomic_bool resolution_done;atomic_bool cancelled;disp_udp_socket_state *socket;disp_udp_io_operation operation;bool started;bool resolving;bool claimed;bool taken;bool done;bool ok;bool has_deadline;uint64_t timeout;uint64_t deadline;size_t limit;size_t sent;int line;int column;struct sockaddr_storage resolved;socklen_t resolved_length;disp_native_socket_address address;disp_native_udp_datagram datagram;disp_native_string buffer;disp_native_string error; } disp_udp_io_state;
static void disp_udp_io_release(disp_udp_io_state *state){if(atomic_fetch_sub_explicit(&state->refs,1,memory_order_acq_rel)!=1)return;atomic_thread_fence(memory_order_acquire);disp_socket_address_drop(&state->address);disp_udp_datagram_drop(&state->datagram);disp_string_drop(&state->buffer);disp_string_drop(&state->error);disp_udp_socket_release(state->socket);disp_dealloc(state);}
static void disp_udp_resolve_worker(void *raw){disp_udp_io_state *state=(disp_udp_io_state*)raw;struct sockaddr_storage resolved={0};socklen_t length=0;disp_native_string error={0};bool ok=disp_udp_resolve(&state->address,false,state->socket->family,&resolved,&length,&error);int expected=0;if(atomic_compare_exchange_strong_explicit(&state->resolution_owner,&expected,1,memory_order_acq_rel,memory_order_acquire)){state->resolved=resolved;state->resolved_length=length;if(!ok)state->error=error;else disp_string_drop(&error);atomic_store_explicit(&state->resolution_done,true,memory_order_release);}else disp_string_drop(&error);disp_socket_address_drop(&state->address);disp_udp_io_release(state);atomic_fetch_sub_explicit(&disp_async_jobs,1,memory_order_acq_rel);}
static disp_udp_io_state *disp_udp_io_create(disp_udp_socket_state *socket,disp_udp_io_operation operation,const char *bytes,size_t length,const disp_native_socket_address *address,bool has_timeout,uint64_t timeout,int line,int column){if(!socket)dv_panic("UDP socket is unavailable",line,column);if(operation==DISP_UDP_RECEIVE&&length>DISP_UDP_RECEIVE_LIMIT)dv_panic("UDP receive limit exceeds 65535 bytes",line,column);disp_udp_io_state *state=(disp_udp_io_state*)disp_alloc_zeroed(1,sizeof(disp_udp_io_state),_Alignof(disp_udp_io_state));atomic_init(&state->refs,1);atomic_init(&state->resolution_owner,0);atomic_init(&state->resolution_done,operation==DISP_UDP_RECEIVE);atomic_init(&state->cancelled,false);state->socket=socket;disp_udp_socket_retain(socket);state->operation=operation;state->limit=length;state->has_deadline=has_timeout;state->timeout=timeout;state->line=line;state->column=column;if(operation==DISP_UDP_SEND){if(length)state->buffer=disp_owned_bytes(bytes,length);state->address=disp_socket_address_clone(address);}return state;}
static void disp_udp_io_finish(disp_udp_io_state *state,bool ok){state->ok=ok;state->done=true;if(state->claimed){atomic_store_explicit(state->operation==DISP_UDP_RECEIVE?&state->socket->receive_busy:&state->socket->send_busy,false,memory_order_release);state->claimed=false;}}
static bool disp_udp_io_poll(disp_udp_io_state *state){if(!state||state->taken)dv_panic("UDP future has already completed",0,0);if(state->done)return true;if(!state->started){state->started=true;uint64_t now=disp_time_now_nanos();state->deadline=UINT64_MAX-now<state->timeout?UINT64_MAX:now+state->timeout;}if(atomic_load_explicit(&state->socket->closed,memory_order_acquire)){state->error=disp_owned_bytes("UDP socket is closed",strlen("UDP socket is closed"));disp_udp_io_finish(state,false);return true;}atomic_bool *busy=state->operation==DISP_UDP_RECEIVE?&state->socket->receive_busy:&state->socket->send_busy;if(!state->claimed){if(!disp_tcp_claim(busy)){uint64_t now=disp_time_now_nanos();if(state->has_deadline&&now>=state->deadline){state->error=disp_owned_bytes("UDP operation timed out",strlen("UDP operation timed out"));disp_udp_io_finish(state,false);return true;}disp_reactor_offer(1000000ULL);return false;}state->claimed=true;}if(state->operation==DISP_UDP_SEND&&state->limit>DISP_UDP_PAYLOAD_LIMIT){state->error=disp_owned_bytes("UDP datagram exceeds the 65507-byte payload limit",strlen("UDP datagram exceeds the 65507-byte payload limit"));disp_udp_io_finish(state,false);return true;}if(state->operation==DISP_UDP_SEND&&!atomic_load_explicit(&state->resolution_done,memory_order_acquire)){int owner=atomic_load_explicit(&state->resolution_owner,memory_order_acquire);if(owner==0&&!state->resolving){state->resolving=true;atomic_fetch_add_explicit(&state->refs,1,memory_order_relaxed);atomic_fetch_add_explicit(&disp_async_jobs,1,memory_order_relaxed);uintptr_t handle=disp_thread_start(disp_udp_resolve_worker,state,state->line,state->column);disp_thread_detach(handle);}uint64_t now=disp_time_now_nanos();if(state->has_deadline&&now>=state->deadline){int expected=0;if(atomic_compare_exchange_strong_explicit(&state->resolution_owner,&expected,2,memory_order_acq_rel,memory_order_acquire)){atomic_store_explicit(&state->cancelled,true,memory_order_release);state->error=disp_owned_bytes("UDP send timed out",strlen("UDP send timed out"));disp_udp_io_finish(state,false);return true;}}disp_reactor_offer(1000000ULL);return false;}if(state->operation==DISP_UDP_SEND&&state->error.data){disp_udp_io_finish(state,false);return true;}int ready_error=0;int ready=disp_socket_ready(state->socket->socket,state->operation==DISP_UDP_RECEIVE,&ready_error);if(ready<0){state->error=disp_network_error_code(state->operation==DISP_UDP_RECEIVE?"UDP receive readiness":"UDP send readiness",ready_error);disp_udp_io_finish(state,false);return true;}if(!ready){uint64_t now=disp_time_now_nanos();if(state->has_deadline&&now>=state->deadline){state->error=disp_owned_bytes(state->operation==DISP_UDP_RECEIVE?"UDP receive timed out":"UDP send timed out",state->operation==DISP_UDP_RECEIVE?strlen("UDP receive timed out"):strlen("UDP send timed out"));disp_udp_io_finish(state,false);return true;}uint64_t wait=1000000ULL;if(state->has_deadline&&state->deadline-now<wait)wait=state->deadline-now;disp_reactor_offer(wait);return false;}if(state->operation==DISP_UDP_RECEIVE){size_t capacity=state->limit+1;uint8_t *data=(uint8_t*)disp_alloc(capacity,1);struct sockaddr_storage source={0};socklen_t source_length=sizeof(source);
#ifdef _WIN32
int count=recvfrom(state->socket->socket,(char*)data,(int)capacity,0,(struct sockaddr*)&source,&source_length);
#else
ssize_t count=recvfrom(state->socket->socket,data,capacity,0,(struct sockaddr*)&source,&source_length);
#endif
if(count<0){int code=disp_socket_error_code();disp_dealloc(data);if(disp_socket_would_block(code)){disp_reactor_offer(1000000ULL);return false;}state->error=disp_network_error_code("UDP receive",code);disp_udp_io_finish(state,false);return true;}if((size_t)count>state->limit){disp_dealloc(data);state->error=disp_owned_bytes("UDP datagram exceeds receive limit",strlen("UDP datagram exceeds receive limit"));disp_udp_io_finish(state,false);return true;}disp_native_string address_error={0};disp_native_socket_address sender=disp_socket_address_from_sockaddr((struct sockaddr*)&source,source_length,&address_error);if(!sender.host){disp_dealloc(data);state->error=address_error;disp_udp_io_finish(state,false);return true;}state->datagram=(disp_native_udp_datagram){.source=sender,.data=data,.len=(size_t)count,.cap=capacity};disp_udp_io_finish(state,true);return true;}
#ifdef _WIN32
int count=sendto(state->socket->socket,state->buffer.data?state->buffer.data:"",(int)state->buffer.len,0,(struct sockaddr*)&state->resolved,state->resolved_length);
#else
ssize_t count=sendto(state->socket->socket,state->buffer.data?state->buffer.data:"",state->buffer.len,
#ifdef MSG_NOSIGNAL
MSG_NOSIGNAL
#else
0
#endif
,(struct sockaddr*)&state->resolved,state->resolved_length);
#endif
if(count<0){int code=disp_socket_error_code();if(disp_socket_would_block(code)){disp_reactor_offer(1000000ULL);return false;}state->error=disp_network_error_code("UDP send",code);disp_udp_io_finish(state,false);return true;}state->sent=(size_t)count;disp_udp_io_finish(state,true);return true;}
static void disp_udp_io_take(disp_udp_io_state *state,bool *ok,disp_native_udp_datagram *datagram,size_t *sent,disp_native_string *error){if(!state->done||state->taken)dv_panic("UDP result is not ready",0,0);state->taken=true;*ok=state->ok;*datagram=state->datagram;state->datagram=(disp_native_udp_datagram){0};*sent=state->sent;*error=state->error;state->error=(disp_native_string){0};}
static void disp_udp_io_drop(void *raw){disp_udp_io_state *state=(disp_udp_io_state*)raw;if(!state)return;atomic_store_explicit(&state->cancelled,true,memory_order_release);if(state->claimed)atomic_store_explicit(state->operation==DISP_UDP_RECEIVE?&state->socket->receive_busy:&state->socket->send_busy,false,memory_order_release);disp_udp_io_release(state);}
#endif
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
typedef struct { uint64_t duration;uint64_t deadline;bool started; } disp_sleep_future;
static bool disp_sleep_poll(void *raw,void *output){disp_sleep_future *state=(disp_sleep_future*)raw;uint64_t now=disp_time_now_nanos();if(!state->started){state->started=true;state->deadline=UINT64_MAX-now<state->duration?UINT64_MAX:now+state->duration;}if(now>=state->deadline){*(disp_native_unit*)output=(disp_native_unit){0};return true;}disp_reactor_offer(state->deadline-now);return false;}
static void disp_sleep_drop(void *raw){disp_dealloc(raw);}
static disp_native_future disp_future_sleep(disp_native_duration duration){disp_sleep_future *state=(disp_sleep_future*)disp_alloc_zeroed(1,sizeof(disp_sleep_future),_Alignof(disp_sleep_future));state->duration=duration.nanos;return (disp_native_future){.context=state,.poll=disp_sleep_poll,.drop=disp_sleep_drop};}

typedef enum { DV_UNIT, DV_SIGNED, DV_UNSIGNED, DV_FLOAT, DV_BOOL, DV_CHAR, DV_STRING, DV_IP, DV_AGG, DV_REF, DV_RAW } DVTag;
typedef struct DV DV;
typedef struct { size_t refs, count; uint64_t disc; const char *type_name, *variant_name; DV *fields; } DVAgg;
struct DV { DVTag tag; uint16_t width; union { __int128 si; unsigned __int128 ui; double fp; bool boolean; uint32_t ch; struct { const char *data; size_t len; } string; disp_native_ip_address ip; DVAgg *agg; DV *reference; } as; };

static DV dv_unit(void){ DV v={0}; v.tag=DV_UNIT; return v; }
static DV dv_bool(bool x){ DV v=dv_unit(); v.tag=DV_BOOL; v.as.boolean=x; return v; }
static DV dv_i(__int128 x,uint16_t w){ DV v=dv_unit(); v.tag=DV_SIGNED; v.width=w?w:64; v.as.si=x; return v; }
static DV dv_u(unsigned __int128 x,uint16_t w){ DV v=dv_unit(); v.tag=DV_UNSIGNED; v.width=w?w:64; v.as.ui=x; return v; }
static DV dv_u128(uint64_t hi,uint64_t lo,uint16_t w){ return dv_u(((unsigned __int128)hi<<64)|lo,w); }
static DV dv_f(double x,uint16_t w){ DV v=dv_unit(); v.tag=DV_FLOAT; v.width=w; v.as.fp=x; return v; }
static DV dv_char(uint32_t x){ DV v=dv_unit(); v.tag=DV_CHAR; v.as.ch=x; return v; }
static DV dv_string(const char *x,size_t n){ DV v=dv_unit(); v.tag=DV_STRING; v.as.string.data=x; v.as.string.len=n; return v; }
static DV dv_ip(disp_native_ip_address x){ DV v=dv_unit(); v.tag=DV_IP; v.as.ip=x; return v; }
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
static void dv_panic(const char *message,int line,int column){const char *file=disp_source_location(&line);if(disp_ffi_panic_target){if(file)snprintf(disp_ffi_last_error,sizeof(disp_ffi_last_error),"DISP runtime error at %s:%d:%d: %s",file,line,column,message);else snprintf(disp_ffi_last_error,sizeof(disp_ffi_last_error),"DISP runtime error at %d:%d: %s",line,column,message);longjmp(*disp_ffi_panic_target,1);}if(file)fprintf(stderr,"DISP runtime error at %s:%d:%d: %s\n",file,line,column,message);else fprintf(stderr,"DISP runtime error at %d:%d: %s\n",line,column,message);exit(101);}
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
static size_t dv_size_add(size_t left,size_t right){size_t value;if(__builtin_add_overflow(left,right,&value))disp_resource_failure("printed output bytes");return value;}
static size_t dv_u128_size(unsigned __int128 value){size_t size=1;while(value>=10){value/=10;size++;}return size;}
static size_t dv_i128_size(__int128 value){return value<0?dv_size_add(1,dv_u128_size((unsigned __int128)(-(value+1))+1)):dv_u128_size((unsigned __int128)value);}
static size_t dv_print_size(DV value,size_t depth){if(depth>=64)disp_resource_failure("print nesting depth");switch(value.tag){case DV_UNIT:return 2;case DV_SIGNED:return dv_i128_size(value.as.si);case DV_UNSIGNED:return dv_u128_size(value.as.ui);case DV_FLOAT:{char text[128];int count=snprintf(text,sizeof(text),"%.15g",value.as.fp);return count<0?0:(size_t)count;}case DV_BOOL:return value.as.boolean?4:5;case DV_CHAR:return value.as.ch<=0x7F?1:(value.as.ch<=0x7FF?2:(value.as.ch<=0xFFFF?3:4));case DV_STRING:return value.as.string.len;case DV_IP:return 64;case DV_REF:case DV_RAW:return value.as.reference?dv_print_size(*value.as.reference,depth+1):0;case DV_AGG:{if(!value.as.agg)return 0;size_t size=0;if(value.as.agg->variant_name){size=dv_size_add(strlen(value.as.agg->type_name),dv_size_add(1,strlen(value.as.agg->variant_name)));if(value.as.agg->count){size=dv_size_add(size,2);for(size_t i=0;i<value.as.agg->count;i++){if(i)size=dv_size_add(size,2);size=dv_size_add(size,dv_print_size(value.as.agg->fields[i],depth+1));}}}else size=dv_size_add(strlen(value.as.agg->type_name),2);return size;}}return 0;}
static void dv_print_value(DV v){switch(v.tag){case DV_UNIT:fputs("()",stdout);break;case DV_SIGNED:print_i128(v.as.si);break;case DV_UNSIGNED:print_u128(v.as.ui);break;case DV_FLOAT:printf("%.15g",v.as.fp);break;case DV_BOOL:fputs(v.as.boolean?"true":"false",stdout);break;case DV_CHAR:{uint32_t c=v.as.ch;print_char(c);break;}case DV_STRING:fwrite(v.as.string.data,1,v.as.string.len,stdout);break;case DV_IP:{
#ifdef DISP_NETWORKING
disp_native_string text=disp_ip_address_string(&v.as.ip);fwrite(text.data,1,text.len,stdout);disp_string_drop(&text);
#else
fputs("<IpAddress>",stdout);
#endif
break;}case DV_REF:case DV_RAW:dv_print_value(*v.as.reference);break;case DV_AGG:if(v.as.agg->variant_name){fputs(v.as.agg->type_name,stdout);putchar('.');fputs(v.as.agg->variant_name,stdout);if(v.as.agg->count){putchar('(');for(size_t i=0;i<v.as.agg->count;i++){if(i)fputs(", ",stdout);dv_print_value(v.as.agg->fields[i]);}putchar(')');}}else{putchar('<');fputs(v.as.agg->type_name,stdout);putchar('>');}break;}}
static DV dv_print(DV value){disp_runtime_charge_output(dv_size_add(dv_print_size(value,0),1));dv_print_value(value);putchar('\n');dv_drop(&value);return dv_unit();}
"#;
