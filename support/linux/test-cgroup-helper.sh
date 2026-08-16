#!/bin/sh
set -eu

helper=/usr/libexec/disp-cgroup-launch
if [ "$(id -u)" -eq 0 ]; then
    echo "test-cgroup-helper: run hostile probes as an unprivileged user" >&2
    exit 1
fi
if [ ! -u "${helper}" ] || [ ! -x "${helper}" ]; then
    echo "test-cgroup-helper: trusted installed helper is unavailable" >&2
    exit 1
fi

temporary=$(mktemp -d "${TMPDIR:-/tmp}/disp-cgroup-test.XXXXXX")
trap 'rm -rf -- "${temporary}"' EXIT HUP INT TERM
probe=${temporary}/probe
marker=${temporary}/escaped

cat > "${temporary}/probe.c" <<'EOF'
#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <sched.h>
#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/types.h>
#include <sys/socket.h>
#include <sys/wait.h>
#include <unistd.h>

static int identity(const char *expected) {
    uid_t wanted = (uid_t)strtoul(expected, NULL, 10);
    return getuid() == wanted && geteuid() == wanted ? 0 : 10;
}

static int escape(void) {
    errno = 0;
    if (setpgid(0, 0) != -1 || errno != EPERM) return 20;
    errno = 0;
    if (setsid() != -1 || errno != EPERM) return 21;
    errno = 0;
    if (unshare(0) != -1 || errno != EPERM) return 22;
    errno = 0;
    int fd = open("/sys/fs/cgroup/cgroup.procs", O_WRONLY | O_CLOEXEC);
    if (fd >= 0) {
        close(fd);
        return 23;
    }
    if (errno != EACCES && errno != EPERM && errno != EROFS) return 24;
    return 0;
}

static int fork_denied(void) {
    errno = 0;
    pid_t child = fork();
    if (child < 0) return errno == EAGAIN ? 0 : 30;
    if (child == 0) _exit(0);
    waitpid(child, NULL, 0);
    return 31;
}

static int network_denied(void) {
    errno = 0;
    int fd = socket(AF_INET, SOCK_STREAM, 0);
    if (fd >= 0) {
        close(fd);
        return 35;
    }
    return errno == EPERM ? 0 : 36;
}

static int exhaust_memory(void) {
    const size_t bytes = 256u * 1024u * 1024u;
    volatile unsigned char *memory = malloc(bytes);
    if (!memory) return 40;
    for (size_t offset = 0; offset < bytes; offset += 4096) memory[offset] = 1;
    free((void *)memory);
    return 0;
}

static int delayed_descendant(const char *marker) {
    pid_t child = fork();
    if (child < 0) return 50;
    if (child > 0) return 0;
    usleep(800000);
    int fd = open(marker, O_WRONLY | O_CREAT | O_EXCL | O_CLOEXEC, 0600);
    if (fd >= 0) {
        write(fd, "escaped", 7);
        close(fd);
    }
    _exit(0);
}

int main(int argc, char **argv) {
    if (argc < 2) return 2;
    if (!strcmp(argv[1], "identity") && argc == 3) return identity(argv[2]);
    if (!strcmp(argv[1], "escape") && argc == 2) return escape();
    if (!strcmp(argv[1], "fork") && argc == 2) return fork_denied();
    if (!strcmp(argv[1], "network") && argc == 2) return network_denied();
    if (!strcmp(argv[1], "memory") && argc == 2) return exhaust_memory();
    if (!strcmp(argv[1], "tree") && argc == 3) return delayed_descendant(argv[2]);
    return 3;
}
EOF

cc -std=c11 -O2 -D_FORTIFY_SOURCE=3 -fstack-protector-strong -fPIE \
    -Wall -Wextra -Werror "${temporary}/probe.c" -Wl,-z,relro,-z,now -pie -o "${probe}"

"${helper}" 134217728 5000 8 5000 "${probe}" identity "$(id -u)"
"${helper}" 134217728 5000 8 5000 "${probe}" escape
"${helper}" 134217728 5000 1 5000 "${probe}" fork
"${helper}" 134217728 5000 8 5000 --component-networkless "${probe}" network

set +e
"${helper}" 67108864 5000 8 5000 "${probe}" memory
memory_status=$?
"${helper}" 134217728 100 8 5000 /bin/sh -c 'while :; do :; done'
cpu_status=$?
"${helper}" 134217728 5000 8 100 /bin/sleep 2
wall_status=$?
set -e
if [ "${memory_status}" -eq 0 ]; then
    echo "test-cgroup-helper: memory.max did not stop the allocation" >&2
    exit 1
fi
if [ "${cpu_status}" -ne 124 ]; then
    echo "test-cgroup-helper: aggregate CPU limit returned ${cpu_status}, expected 124" >&2
    exit 1
fi
if [ "${wall_status}" -ne 124 ]; then
    echo "test-cgroup-helper: wall limit returned ${wall_status}, expected 124" >&2
    exit 1
fi

"${helper}" 134217728 5000 8 5000 "${probe}" tree "${marker}"
sleep 1
if [ -e "${marker}" ]; then
    echo "test-cgroup-helper: delayed descendant escaped cgroup cleanup" >&2
    exit 1
fi

echo "DISP privileged cgroup helper hostile probes passed"
