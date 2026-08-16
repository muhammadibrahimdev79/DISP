# Trusted Linux cgroup launcher

`disp-cgroup-launch.c` is the narrow privileged boundary for DISP's hard Linux process profile.
It is separate from the compiler and generated runtime so those larger components never run with
elevated authority.

The launcher creates one root-owned cgroup v2 leaf per execution, applies `memory.max`,
`memory.oom.group`, and `pids.max`, and monitors aggregate `cpu.stat` plus the wall deadline. Its
worker joins the leaf while still privileged, permanently restores the invoking user's real,
effective, and saved IDs, sets `PR_SET_NO_NEW_PRIVS`, installs the same escape-focused seccomp
filter as the runtime, closes private descriptors, and only then calls `execve`. The root
supervisor remains outside the limited leaf, kills the complete leaf on limit violation or normal
root completion, and removes it.

When invoked with the internal `--component-networkless` profile marker, the worker stacks a second
seccomp filter that rejects socket creation and use, including io_uring setup and legacy
`socketcall`. The Rust component host passes this marker only through the fixed verified helper;
an older helper fails on the marker instead of launching a component without the promised network
boundary.

## Required installation contract

The helper is not enabled merely because a binary with this name exists. Packaging must:

1. compile the exact reviewed source with stack protection, fortified libc calls, PIE, immediate
   binding, and full RELRO;
2. install it at a system path owned by UID/GID 0, with no group/other write bit and both the
   set-user-ID and set-group-ID bits enabled;
3. create `/sys/fs/cgroup/disp` as a root-owned, non-group/other-writable cgroup v2 directory at
   boot;
4. enable the `cpu`, `memory`, and `pids` controllers at both the parent and DISP subtree levels;
5. ensure unprivileged users cannot write the helper's cgroup root or any parent `cgroup.procs`;
6. keep the filesystem containing the helper free of the `nosuid` option; and
7. run the installation and hostile-helper test matrix on every supported Linux distribution.

Example compiler flags for a distribution package:

```sh
cc -std=c11 -O2 -D_FORTIFY_SOURCE=3 -fstack-protector-strong -fPIE \
  -Wall -Wextra -Werror disp-cgroup-launch.c -Wl,-z,relro,-z,now -pie \
  -o disp-cgroup-launch
```

`install-cgroup-helper.sh` implements the hardened installation and boot service above. Until it
and the privileged hostile test matrix have executed successfully on a supported distribution,
DISP must continue reporting the default Linux profile as `resource-contained`, not fully
`isolated`.
