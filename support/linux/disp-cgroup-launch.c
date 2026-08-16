#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <limits.h>
#include <linux/audit.h>
#include <linux/filter.h>
#include <linux/seccomp.h>
#include <sched.h>
#include <signal.h>
#include <stddef.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/prctl.h>
#include <sys/resource.h>
#include <sys/stat.h>
#include <sys/syscall.h>
#include <sys/types.h>
#include <sys/wait.h>
#include <time.h>
#include <unistd.h>

#ifndef DISP_CGROUP_ROOT
#define DISP_CGROUP_ROOT "/sys/fs/cgroup/disp"
#endif
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
#define DISP_CLONE_FLAGS_OFFSET offsetof(struct seccomp_data, args[1])
#elif defined(__powerpc64__) && __BYTE_ORDER__ == __ORDER_LITTLE_ENDIAN__
#define DISP_AUDIT_ARCH AUDIT_ARCH_PPC64LE
#elif defined(__powerpc64__)
#define DISP_AUDIT_ARCH AUDIT_ARCH_PPC64
#else
#error "DISP has no verified seccomp audit architecture for this Linux target"
#endif
#ifndef DISP_CLONE_FLAGS_OFFSET
#define DISP_CLONE_FLAGS_OFFSET offsetof(struct seccomp_data, args[0])
#endif

static void report(const char *operation) {
    int code = errno;
    dprintf(STDERR_FILENO, "disp-cgroup-launch: %s: %s\n", operation, strerror(code));
}

static int parse_positive(const char *text, unsigned long long *value) {
    if (!text || !*text || *text == '+' || *text == '-') return -1;
    errno = 0;
    char *end = NULL;
    unsigned long long parsed = strtoull(text, &end, 10);
    if (errno || !end || *end || !parsed) return -1;
    *value = parsed;
    return 0;
}

static int write_all(int fd, const char *text) {
    size_t length = strlen(text), written = 0;
    while (written < length) {
        ssize_t count = write(fd, text + written, length - written);
        if (count < 0) {
            if (errno == EINTR) continue;
            return -1;
        }
        if (!count) {
            errno = EIO;
            return -1;
        }
        written += (size_t)count;
    }
    return 0;
}

static int write_control(int directory, const char *name, const char *value) {
    int fd = openat(directory, name, O_WRONLY | O_CLOEXEC | O_NOFOLLOW);
    if (fd < 0) return -1;
    int result = write_all(fd, value);
    int saved = errno;
    if (close(fd) != 0 && result == 0) {
        saved = errno;
        result = -1;
    }
    errno = saved;
    return result;
}

static int install_escape_filter(void) {
    const uint32_t denied = SECCOMP_RET_ERRNO | (uint32_t)EPERM;
    const uint32_t unsupported = SECCOMP_RET_ERRNO | (uint32_t)ENOSYS;
    const uint32_t namespace_flags = (uint32_t)(
        CLONE_NEWCGROUP | CLONE_NEWIPC | CLONE_NEWNET | CLONE_NEWNS |
        CLONE_NEWPID | CLONE_NEWTIME | CLONE_NEWUSER | CLONE_NEWUTS);
    struct sock_filter filter[] = {
        BPF_STMT(BPF_LD | BPF_W | BPF_ABS, (uint32_t)offsetof(struct seccomp_data, arch)),
        BPF_JUMP(BPF_JMP | BPF_JEQ | BPF_K, DISP_AUDIT_ARCH, 1, 0),
        BPF_STMT(BPF_RET | BPF_K, SECCOMP_RET_KILL_PROCESS),
        BPF_STMT(BPF_LD | BPF_W | BPF_ABS, (uint32_t)offsetof(struct seccomp_data, nr)),
        BPF_JUMP(BPF_JMP | BPF_JEQ | BPF_K, __NR_setpgid, 0, 1),
        BPF_STMT(BPF_RET | BPF_K, denied),
        BPF_JUMP(BPF_JMP | BPF_JEQ | BPF_K, __NR_setsid, 0, 1),
        BPF_STMT(BPF_RET | BPF_K, denied),
        BPF_JUMP(BPF_JMP | BPF_JEQ | BPF_K, __NR_unshare, 0, 1),
        BPF_STMT(BPF_RET | BPF_K, denied),
        BPF_JUMP(BPF_JMP | BPF_JEQ | BPF_K, __NR_setns, 0, 1),
        BPF_STMT(BPF_RET | BPF_K, denied),
        BPF_JUMP(BPF_JMP | BPF_JEQ | BPF_K, __NR_ptrace, 0, 1),
        BPF_STMT(BPF_RET | BPF_K, denied),
        BPF_JUMP(BPF_JMP | BPF_JEQ | BPF_K, __NR_process_vm_readv, 0, 1),
        BPF_STMT(BPF_RET | BPF_K, denied),
        BPF_JUMP(BPF_JMP | BPF_JEQ | BPF_K, __NR_process_vm_writev, 0, 1),
        BPF_STMT(BPF_RET | BPF_K, denied),
#ifdef __NR_pidfd_getfd
        BPF_JUMP(BPF_JMP | BPF_JEQ | BPF_K, __NR_pidfd_getfd, 0, 1),
        BPF_STMT(BPF_RET | BPF_K, denied),
#endif
#ifdef __NR_clone3
        BPF_JUMP(BPF_JMP | BPF_JEQ | BPF_K, __NR_clone3, 0, 1),
        BPF_STMT(BPF_RET | BPF_K, unsupported),
#endif
        BPF_JUMP(BPF_JMP | BPF_JEQ | BPF_K, __NR_clone, 0, 4),
        BPF_STMT(BPF_LD | BPF_W | BPF_ABS, (uint32_t)DISP_CLONE_FLAGS_OFFSET),
        BPF_STMT(BPF_ALU | BPF_AND | BPF_K, namespace_flags),
        BPF_JUMP(BPF_JMP | BPF_JEQ | BPF_K, 0, 1, 0),
        BPF_STMT(BPF_RET | BPF_K, denied),
        BPF_STMT(BPF_RET | BPF_K, SECCOMP_RET_ALLOW),
    };
    struct sock_fprog program = {
        .len = (unsigned short)(sizeof(filter) / sizeof(filter[0])),
        .filter = filter,
    };
    if (prctl(PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) != 0) return -1;
    return prctl(PR_SET_SECCOMP, SECCOMP_MODE_FILTER, &program);
}

static int install_network_deny_filter(void) {
    const uint32_t denied = SECCOMP_RET_ERRNO | (uint32_t)EPERM;
    struct sock_filter filter[] = {
        BPF_STMT(BPF_LD | BPF_W | BPF_ABS, (uint32_t)offsetof(struct seccomp_data, arch)),
        BPF_JUMP(BPF_JMP | BPF_JEQ | BPF_K, DISP_AUDIT_ARCH, 1, 0),
        BPF_STMT(BPF_RET | BPF_K, SECCOMP_RET_KILL_PROCESS),
        BPF_STMT(BPF_LD | BPF_W | BPF_ABS, (uint32_t)offsetof(struct seccomp_data, nr)),
#ifdef __NR_socket
        BPF_JUMP(BPF_JMP | BPF_JEQ | BPF_K, __NR_socket, 0, 1),
        BPF_STMT(BPF_RET | BPF_K, denied),
#endif
#ifdef __NR_socketpair
        BPF_JUMP(BPF_JMP | BPF_JEQ | BPF_K, __NR_socketpair, 0, 1),
        BPF_STMT(BPF_RET | BPF_K, denied),
#endif
#ifdef __NR_connect
        BPF_JUMP(BPF_JMP | BPF_JEQ | BPF_K, __NR_connect, 0, 1),
        BPF_STMT(BPF_RET | BPF_K, denied),
#endif
#ifdef __NR_bind
        BPF_JUMP(BPF_JMP | BPF_JEQ | BPF_K, __NR_bind, 0, 1),
        BPF_STMT(BPF_RET | BPF_K, denied),
#endif
#ifdef __NR_listen
        BPF_JUMP(BPF_JMP | BPF_JEQ | BPF_K, __NR_listen, 0, 1),
        BPF_STMT(BPF_RET | BPF_K, denied),
#endif
#ifdef __NR_accept
        BPF_JUMP(BPF_JMP | BPF_JEQ | BPF_K, __NR_accept, 0, 1),
        BPF_STMT(BPF_RET | BPF_K, denied),
#endif
#ifdef __NR_accept4
        BPF_JUMP(BPF_JMP | BPF_JEQ | BPF_K, __NR_accept4, 0, 1),
        BPF_STMT(BPF_RET | BPF_K, denied),
#endif
#ifdef __NR_sendto
        BPF_JUMP(BPF_JMP | BPF_JEQ | BPF_K, __NR_sendto, 0, 1),
        BPF_STMT(BPF_RET | BPF_K, denied),
#endif
#ifdef __NR_recvfrom
        BPF_JUMP(BPF_JMP | BPF_JEQ | BPF_K, __NR_recvfrom, 0, 1),
        BPF_STMT(BPF_RET | BPF_K, denied),
#endif
#ifdef __NR_sendmsg
        BPF_JUMP(BPF_JMP | BPF_JEQ | BPF_K, __NR_sendmsg, 0, 1),
        BPF_STMT(BPF_RET | BPF_K, denied),
#endif
#ifdef __NR_recvmsg
        BPF_JUMP(BPF_JMP | BPF_JEQ | BPF_K, __NR_recvmsg, 0, 1),
        BPF_STMT(BPF_RET | BPF_K, denied),
#endif
#ifdef __NR_sendmmsg
        BPF_JUMP(BPF_JMP | BPF_JEQ | BPF_K, __NR_sendmmsg, 0, 1),
        BPF_STMT(BPF_RET | BPF_K, denied),
#endif
#ifdef __NR_recvmmsg
        BPF_JUMP(BPF_JMP | BPF_JEQ | BPF_K, __NR_recvmmsg, 0, 1),
        BPF_STMT(BPF_RET | BPF_K, denied),
#endif
#ifdef __NR_shutdown
        BPF_JUMP(BPF_JMP | BPF_JEQ | BPF_K, __NR_shutdown, 0, 1),
        BPF_STMT(BPF_RET | BPF_K, denied),
#endif
#ifdef __NR_getsockname
        BPF_JUMP(BPF_JMP | BPF_JEQ | BPF_K, __NR_getsockname, 0, 1),
        BPF_STMT(BPF_RET | BPF_K, denied),
#endif
#ifdef __NR_getpeername
        BPF_JUMP(BPF_JMP | BPF_JEQ | BPF_K, __NR_getpeername, 0, 1),
        BPF_STMT(BPF_RET | BPF_K, denied),
#endif
#ifdef __NR_setsockopt
        BPF_JUMP(BPF_JMP | BPF_JEQ | BPF_K, __NR_setsockopt, 0, 1),
        BPF_STMT(BPF_RET | BPF_K, denied),
#endif
#ifdef __NR_getsockopt
        BPF_JUMP(BPF_JMP | BPF_JEQ | BPF_K, __NR_getsockopt, 0, 1),
        BPF_STMT(BPF_RET | BPF_K, denied),
#endif
#ifdef __NR_io_uring_setup
        BPF_JUMP(BPF_JMP | BPF_JEQ | BPF_K, __NR_io_uring_setup, 0, 1),
        BPF_STMT(BPF_RET | BPF_K, denied),
#endif
#ifdef __NR_socketcall
        BPF_JUMP(BPF_JMP | BPF_JEQ | BPF_K, __NR_socketcall, 0, 1),
        BPF_STMT(BPF_RET | BPF_K, denied),
#endif
        BPF_STMT(BPF_RET | BPF_K, SECCOMP_RET_ALLOW),
    };
    struct sock_fprog program = {
        .len = (unsigned short)(sizeof(filter) / sizeof(filter[0])),
        .filter = filter,
    };
    return prctl(PR_SET_SECCOMP, SECCOMP_MODE_FILTER, &program);
}

static int close_private_fds(void) {
#ifdef __NR_close_range
    if (syscall(__NR_close_range, 3U, ~0U, 0U) == 0) return 0;
    if (errno != ENOSYS && errno != EINVAL) return -1;
#endif
    long maximum = sysconf(_SC_OPEN_MAX);
    if (maximum < 0) maximum = 65536;
    for (int fd = 3; fd < maximum && fd < INT_MAX; fd++) close(fd);
    return 0;
}

static int configure_worker(
    int job,
    pid_t supervisor,
    uid_t user,
    gid_t group,
    unsigned long long memory,
    unsigned long long cpu_millis,
    int deny_network
) {
    char pid_text[32];
    int pid_length = snprintf(pid_text, sizeof(pid_text), "%ld", (long)getpid());
    if (pid_length <= 0 || (size_t)pid_length >= sizeof(pid_text) ||
        write_control(job, "cgroup.procs", pid_text) != 0) return -1;

    rlim_t memory_limit = (rlim_t)memory;
    rlim_t cpu_seconds = (rlim_t)(cpu_millis / 1000 + (cpu_millis % 1000 != 0));
    if ((unsigned long long)memory_limit != memory ||
        (unsigned long long)cpu_seconds != cpu_millis / 1000 + (cpu_millis % 1000 != 0)) {
        errno = EOVERFLOW;
        return -1;
    }
    struct rlimit address = { memory_limit, memory_limit };
    struct rlimit cpu = { cpu_seconds, cpu_seconds };
    if (setrlimit(RLIMIT_AS, &address) != 0 || setrlimit(RLIMIT_CPU, &cpu) != 0) return -1;

    if (prctl(PR_CAP_AMBIENT, PR_CAP_AMBIENT_CLEAR_ALL, 0, 0, 0) != 0) return -1;
    if (setresgid(group, group, group) != 0 || setresuid(user, user, user) != 0) return -1;
    uid_t real, effective, saved;
    gid_t real_group, effective_group, saved_group;
    if (getresuid(&real, &effective, &saved) != 0 ||
        getresgid(&real_group, &effective_group, &saved_group) != 0 ||
        real != user || effective != user || saved != user ||
        real_group != group || effective_group != group || saved_group != group) {
        errno = EPERM;
        return -1;
    }
    if (prctl(PR_SET_PDEATHSIG, SIGKILL, 0, 0, 0) != 0) return -1;
    if (getppid() != supervisor) {
        errno = ESRCH;
        return -1;
    }
    if (install_escape_filter() != 0 ||
        (deny_network && install_network_deny_filter() != 0) ||
        close_private_fds() != 0) return -1;
    return 0;
}

static int read_cpu_usage(int job, unsigned long long *usage) {
    int fd = openat(job, "cpu.stat", O_RDONLY | O_CLOEXEC | O_NOFOLLOW);
    if (fd < 0) return -1;
    char buffer[1024];
    ssize_t count;
    do {
        count = read(fd, buffer, sizeof(buffer) - 1);
    } while (count < 0 && errno == EINTR);
    int saved = errno;
    close(fd);
    errno = saved;
    if (count <= 0) return -1;
    buffer[count] = 0;
    const char *line = buffer;
    while (*line) {
        unsigned long long value;
        if (sscanf(line, "usage_usec %llu", &value) == 1) {
            *usage = value;
            return 0;
        }
        const char *next = strchr(line, '\n');
        if (!next) break;
        line = next + 1;
    }
    errno = EPROTO;
    return -1;
}

static unsigned long long monotonic_millis(void) {
    struct timespec now;
    if (clock_gettime(CLOCK_MONOTONIC, &now) != 0) return ULLONG_MAX;
    if ((unsigned long long)now.tv_sec > ULLONG_MAX / 1000ULL) return ULLONG_MAX;
    return (unsigned long long)now.tv_sec * 1000ULL +
        (unsigned long long)now.tv_nsec / 1000000ULL;
}

static int cleanup_job(int base, int job, const char *name) {
    int result = 0;
    if (write_control(job, "cgroup.kill", "1") != 0 && errno != ENOENT) result = -1;
    if (close(job) != 0) result = -1;
    struct timespec pause = { .tv_sec = 0, .tv_nsec = 10000000 };
    for (int attempt = 0; attempt < 100; attempt++) {
        if (unlinkat(base, name, AT_REMOVEDIR) == 0 || errno == ENOENT) return result;
        if (errno != EBUSY && errno != ENOTEMPTY) return -1;
        nanosleep(&pause, NULL);
    }
    errno = EBUSY;
    return -1;
}

int main(int argc, char **argv) {
    if (argc < 6) {
        dprintf(STDERR_FILENO,
            "usage: disp-cgroup-launch MEMORY_BYTES CPU_MILLIS PROCESSES WALL_MILLIS [--component-networkless] /absolute/program [args...]\n");
        return 125;
    }
    int target_index = 5;
    int deny_network = 0;
    if (!strcmp(argv[5], "--component-networkless")) {
        if (argc < 7) {
            dprintf(STDERR_FILENO, "disp-cgroup-launch: component target is missing\n");
            return 125;
        }
        target_index = 6;
        deny_network = 1;
    }
    unsigned long long memory, cpu_millis, processes, wall_millis;
    if (parse_positive(argv[1], &memory) != 0 ||
        parse_positive(argv[2], &cpu_millis) != 0 ||
        parse_positive(argv[3], &processes) != 0 ||
        parse_positive(argv[4], &wall_millis) != 0 || processes > 1048576ULL ||
        cpu_millis > ULLONG_MAX / 1000ULL ||
        argv[target_index][0] != '/') {
        dprintf(STDERR_FILENO, "disp-cgroup-launch: invalid limits or non-absolute target\n");
        return 125;
    }
    uid_t user = getuid();
    gid_t group = getgid();
    if (geteuid() != 0 || getegid() != 0) {
        dprintf(STDERR_FILENO, "disp-cgroup-launch: helper is not running with root ownership\n");
        return 125;
    }

    char target[PATH_MAX];
    if (!realpath(argv[target_index], target)) {
        report("realpath target");
        return 125;
    }
    struct stat target_info;
    if (stat(target, &target_info) != 0 || !S_ISREG(target_info.st_mode)) {
        report("validate target");
        return 125;
    }
    argv[target_index] = target;

    int base = open(DISP_CGROUP_ROOT, O_RDONLY | O_DIRECTORY | O_CLOEXEC | O_NOFOLLOW);
    if (base < 0) {
        report("open trusted cgroup root");
        return 125;
    }
    struct stat base_info;
    if (fstat(base, &base_info) != 0 || base_info.st_uid != 0 ||
        (base_info.st_mode & (S_IWGRP | S_IWOTH)) != 0) {
        errno = EPERM;
        report("validate trusted cgroup root ownership");
        close(base);
        return 125;
    }

    struct timespec now;
    if (clock_gettime(CLOCK_MONOTONIC, &now) != 0) {
        report("read monotonic clock");
        close(base);
        return 125;
    }
    char name[96];
    int name_length = snprintf(name, sizeof(name), "uid-%lu-pid-%ld-%lld",
        (unsigned long)user, (long)getpid(),
        (long long)now.tv_sec * 1000000000LL + now.tv_nsec);
    if (name_length <= 0 || (size_t)name_length >= sizeof(name) ||
        mkdirat(base, name, 0755) != 0) {
        report("create job cgroup");
        close(base);
        return 125;
    }
    int job = openat(base, name, O_RDONLY | O_DIRECTORY | O_CLOEXEC | O_NOFOLLOW);
    if (job < 0) {
        report("open job cgroup");
        unlinkat(base, name, AT_REMOVEDIR);
        close(base);
        return 125;
    }

    char memory_text[32], process_text[32];
    snprintf(memory_text, sizeof(memory_text), "%llu", memory);
    snprintf(process_text, sizeof(process_text), "%llu", processes);
    if (write_control(job, "memory.max", memory_text) != 0 ||
        write_control(job, "memory.oom.group", "1") != 0 ||
        write_control(job, "pids.max", process_text) != 0) {
        report("configure cgroup limits");
        cleanup_job(base, job, name);
        close(base);
        return 125;
    }
    unsigned long long initial_cpu;
    if (read_cpu_usage(job, &initial_cpu) != 0 || initial_cpu != 0) {
        report("validate cgroup CPU accounting");
        cleanup_job(base, job, name);
        close(base);
        return 125;
    }

    unsigned long long started = monotonic_millis();
    if (started == ULLONG_MAX) {
        report("read launch clock");
        cleanup_job(base, job, name);
        close(base);
        return 125;
    }
    pid_t supervisor = getpid();
    pid_t child = fork();
    if (child < 0) {
        report("fork worker");
        cleanup_job(base, job, name);
        close(base);
        return 125;
    }
    if (child == 0) {
        if (configure_worker(job, supervisor, user, group, memory, cpu_millis,
            deny_network) != 0) {
            report("configure worker boundary");
            _exit(126);
        }
        execv(target, &argv[target_index]);
        report("execute target");
        _exit(127);
    }

    int status = 0;
    pid_t waited = 0;
    int boundary_exit = 0;
    struct timespec pause = { .tv_sec = 0, .tv_nsec = 2000000 };
    while (!waited) {
        waited = waitpid(child, &status, WNOHANG);
        if (waited < 0 && errno == EINTR) {
            waited = 0;
            continue;
        }
        if (waited) break;
        unsigned long long usage, now = monotonic_millis();
        if (read_cpu_usage(job, &usage) != 0 || now == ULLONG_MAX) {
            report("monitor cgroup boundary");
            boundary_exit = 125;
        } else if (usage >= cpu_millis * 1000ULL ||
            now - started >= wall_millis) {
            boundary_exit = 124;
        }
        if (boundary_exit) {
            if (write_control(job, "cgroup.kill", "1") != 0) report("terminate cgroup");
            do {
                waited = waitpid(child, &status, 0);
            } while (waited < 0 && errno == EINTR);
            break;
        }
        nanosleep(&pause, NULL);
    }
    if (waited < 0) report("wait for worker");
    if (cleanup_job(base, job, name) != 0) report("destroy job cgroup");
    close(base);
    if (waited < 0 || boundary_exit == 125) return 125;
    if (boundary_exit) return boundary_exit;
    if (WIFEXITED(status)) return WEXITSTATUS(status);
    if (WIFSIGNALED(status)) return 128 + WTERMSIG(status);
    return 125;
}
