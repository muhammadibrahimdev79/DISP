#!/bin/sh
set -eu

if [ "$(id -u)" -ne 0 ]; then
    echo "install-cgroup-helper: run as root" >&2
    exit 1
fi
for command in cc findmnt install stat systemctl; do
    if ! command -v "${command}" >/dev/null 2>&1; then
        echo "install-cgroup-helper: required command is missing: ${command}" >&2
        exit 1
    fi
done

script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd -P)
temporary=$(mktemp "${TMPDIR:-/tmp}/disp-cgroup-launch.XXXXXX")
trap 'rm -f -- "${temporary}"' EXIT HUP INT TERM

cc -std=c11 -O2 -D_FORTIFY_SOURCE=3 -fstack-protector-strong -fPIE \
    -Wall -Wextra -Werror "${script_dir}/disp-cgroup-launch.c" \
    -Wl,-z,relro,-z,now -pie -o "${temporary}"

install -d -o root -g root -m 0755 /usr/libexec
mount_options=$(findmnt -no OPTIONS --target /usr/libexec)
case ",${mount_options}," in
    *,nosuid,*)
        echo "install-cgroup-helper: helper filesystem is mounted nosuid" >&2
        exit 1
        ;;
esac
install -o root -g root -m 6755 "${temporary}" /usr/libexec/disp-cgroup-launch
install -o root -g root -m 0755 "${script_dir}/disp-cgroup-setup" \
    /usr/libexec/disp-cgroup-setup
install -o root -g root -m 0644 "${script_dir}/disp-cgroup-setup.service" \
    /etc/systemd/system/disp-cgroup-setup.service
if [ "$(stat -c '%u:%g:%a' /usr/libexec/disp-cgroup-launch)" != "0:0:6755" ]; then
    echo "install-cgroup-helper: trusted helper identity was not established" >&2
    exit 1
fi

systemctl daemon-reload
systemctl enable --now disp-cgroup-setup.service

/usr/libexec/disp-cgroup-launch 67108864 10000 4 5000 /bin/true
echo "DISP hard cgroup helper installed and verified"
