# DISP security policy

DISP is a developer preview and is not yet security-certified. Security defects are nevertheless
treated as release-blocking engineering issues, not ordinary feature requests.

## Reporting a vulnerability

Do not open a public issue for a suspected vulnerability. Submit a private report through
[GitHub Security Advisories](https://github.com/muhammadibrahimdev79/DISP/security/advisories/new).
Include the affected revision, platform, reproduction, expected impact, and whether exploitation
has been observed. Never include real credentials, production secrets, or unrelated personal data.

If private advisory submission is unavailable, disclose only that the private channel is
unavailable in a public issue; do not post exploit details. Maintainers must establish a private
channel before requesting the reproducer.

## Supported versions

Until DISP 1.0, only the current default branch is supported. Preview tags receive security fixes
only when explicitly stated in their release notes. A security fix is not complete until affected
artifacts, documentation, tests, advisories, and any rotated signing material are addressed.

## Response targets

| Severity | Initial acknowledgement | Containment or decision | Target remediation |
|---|---:|---:|---:|
| Critical | 24 hours | 72 hours | 7 days |
| High | 2 business days | 7 days | 30 days |
| Medium | 5 business days | 30 days | 90 days |
| Low | 10 business days | 90 days | Next planned release |

Targets begin when the private report is received. They are objectives, not a promise that an
unsafe release will be rushed: missed targets require a documented revised date and containment.

## Process

1. Preserve the original private report and assign a coordinator.
2. Reproduce without exposing reporter data; determine affected revisions, targets, and artifacts.
3. Classify severity from demonstrated confidentiality, integrity, availability, sandbox, and
   supply-chain impact. Treat uncertainty conservatively.
4. Contain active exploitation, revoke or rotate affected keys, and halt vulnerable releases.
5. Develop the smallest complete fix plus a regression, adversarial, and cross-backend test.
6. Re-run specification, fuzzing, sanitizer, dependency, artifact-provenance, and release gates.
7. Coordinate disclosure and credit with the reporter; publish affected/fixed versions,
   mitigations, and upgrade instructions without unnecessary exploit enablement.
8. Record root cause and preventive work in the threat model and pass ledger.

## Release blockers

Known exploitable memory unsafety in safe DISP, sandbox escape, capability bypass, unauthenticated
plaintext release, signing-key disclosure, signature-verification bypass, arbitrary in-process
package code execution, or an unmitigated critical/high dependency advisory blocks release.
Suppressing a failing security gate or silently ignoring an advisory is not an accepted mitigation.

The maintained threat model is [docs/security/THREAT_MODEL_0.1.md](docs/security/THREAT_MODEL_0.1.md).
