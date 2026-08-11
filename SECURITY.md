# Security Policy

StableHAL is a research project (deterministic, contract-enforcing HAL +
simulation). It is a host-side testing and certification toolkit plus `no_std`
libraries, not a network-facing service — but it does parse untrusted binary
input (SHC1 hardware captures) and is intended to be trusted as a *judge* of
conformance, so correctness and input-handling bugs are treated as security
relevant.

## Reporting a vulnerability

Please report suspected vulnerabilities privately rather than opening a public
issue:

- Use GitHub's **"Report a vulnerability"** (Security → Advisories) on the
  repository, or
- email the maintainer at **mustafakyr@gmail.com**.

Include a description, affected component/version, and a reproduction (ideally a
seed or a minimal `.shc1` capture). We aim to acknowledge within a reasonable
time and will coordinate a fix and disclosure with you.

## What we consider in scope

- Memory-unsafety or panics reachable from parsing an untrusted **SHC1 capture**
  (`stablehal-shc1`, `stablehal-certify`).
- A path that makes the oracle render a **wrong verdict** (a nonconforming
  trace judged conforming, or vice versa) — this is the load-bearing property of
  the whole tool.
- Any breach of a determinism guarantee (a run that is not a pure function of
  its seed) — see G2 in [CONTRIBUTING.md](CONTRIBUTING.md).

## Posture

- **Zero external dependencies** in `crates/*` (rule G1) — the auditable core
  has no third-party supply-chain surface. Third-party code is confined to
  `adapters/`, `targets/`, and `tools/`.
- **`#![deny(unsafe_code)]`** on every `crates/*` library. The single
  exception is `stablehal-embedded-hal`, which uses one justified `unsafe` call
  to construct `embedded-can` identifiers when bridging to that ecosystem.
- No wall clock, no OS entropy, no threads in the engine — reducing
  nondeterministic and platform-dependent behavior.

## Supported versions

The project is pre-1.0 (`0.1.x`); fixes land on the default branch. There is no
long-term support branch yet.
