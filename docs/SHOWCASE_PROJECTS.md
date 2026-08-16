# DISP showcase projects

Status: architecture and execution roadmap. A project is not implemented until its listed gates
have executable, reproducible evidence.

These projects are integration proofs for DISP's all-purpose goal. They share language-native
security, packages, storage, networking, UI, concurrency, deployment, and observability rather than
building separate incompatible stacks.

## 1. Outernet — first flagship project

Outernet is an independently implemented network ecosystem, not a rebranding of the existing
Internet. Its staged path is:

1. Address, packet, route, checksum, framing, and deterministic simulation libraries in DISP.
2. Capability-gated network-device APIs and safe asynchronous driver callbacks.
3. A versioned authenticated transport with congestion control, multiplexed streams, discovery,
   naming, routing, and explicit resource limits.
4. DISP-native client/server runtimes, package distribution, identity integration, and gateways for
   deliberate interoperability with Internet protocols.
5. Multi-machine and hostile-network conformance, fuzzing, performance, recovery, and upgrade
   evidence.

The first executable gate is a deterministic in-process packet network with loss, duplication,
reordering, bounded queues, authenticated peers, and reproducible traces. The second is a real
two-host exchange without depending on a database, browser engine, or language runtime outside the
documented bootstrap boundary.

## 2. Shared platform proofs

| Project | DISP domains it must prove | First executable gate |
|---|---|---|
| Authentication system | identity, cryptography, secrets, policy, audit, storage, Outernet/Internet interoperability | Two independent services authenticate, rotate credentials, revoke access, and verify an append-only audit trail under fault injection |
| Development platform | compiler, package/build system, editor services, debugger, profiler, CI, deployment | A DISP project is edited, built, tested, debugged, packaged, and reproducibly deployed using the platform |
| AI model and runtime | tensors, autodiff, accelerator/CPU execution, datasets, training, inference, model formats, sandboxing | Train and serve a small reproducible model with bounded memory and numerically checked CPU results |
| Office suite | document, sheet, presentation, database, collaboration, UI, import/export | Create, edit, save, reopen, and collaboratively merge each native document type without data loss |
| Robotics platform | real-time scheduling, sensors, actuators, simulation, safety states, AI inference | The same bounded controller passes simulation and hardware-in-the-loop emergency-stop tests |
| Operating system | boot, memory, processes, drivers, filesystem, networking, graphics, security, updates | Boot on two architectures, run isolated DISP processes, persist data, communicate over Outernet, and recover from an interrupted update |

## Engineering order

Outernet remains first. In parallel with its early simulation layers, DISP continues strengthening
the compiler, freestanding targets, native data engine, cryptography, and UI foundations. The
authentication system becomes Outernet's identity layer; the development platform makes every later
project reproducible; the AI and office workloads stress compute and application capabilities; the
robotics platform stresses deterministic real-time safety; and the operating system ultimately
hosts the complete stack without becoming a prerequisite for early progress.

Every project must publish source, threat model, capability manifest, resource budgets, conformance
tests, reproducible builds, benchmark methodology, recovery tests, and an independence inventory.
