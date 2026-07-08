# StableHAL — Competitive Analysis, Positioning & Strategy

> Generated 2026-07-08 after a full codebase and market review.
> See [paper/whitepaper.md](../paper/whitepaper.md) for the scientific argument,
> [docs/architecture.md](architecture.md) for the technical design,
> [next_steps.md](../next_steps.md) for the implementation backlog, and
> [ChangeLog.txt](../ChangeLog.txt) for the project log.

## Table of Contents

1. [What StableHAL Is](#1-what-stablehal-is)
2. [Comparable Tools & Frameworks](#2-comparable-tools--frameworks)
3. [Competitive Positioning](#3-competitive-positioning)
4. [What to Add](#4-what-to-add)
5. [What to Keep & Strengthen](#5-what-to-keep--strengthen)
6. [What to Remove or Deprioritise](#6-what-to-remove-or-deprioritise)
7. [How to Increase Impact](#7-how-to-increase-impact)
8. [Target Markets / Customers](#8-target-markets--customers)
9. [Recommendations — Immediate Next Steps](#9-recommendations--immediate-next-steps)

---

## 1. What StableHAL Is

StableHAL solves a specific problem: embedded software tested against
simulation still fails on real hardware because HALs are only interface
*signatures* — they bind neither side to timing, ordering, or fault
behaviour. StableHAL replaces signatures with **executable contracts**.

**Unique value proposition (no other tool does this combination):**

| Dimension | StableHAL |
|---|---|
| Contract basis | Executable state machine, compiled into `no_std` |
| Timing terms | Envelopes (`min..=max`), never point values |
| Fault vocabulary | Closed, exhaustive per contract |
| Spec traceability | Every clause has a stable ID + spec citation |
| Artifacts from one contract | Reference sim (conforming by construction), `no_std` trace oracle, conformance suite |
| Sim vs. HW judgement | Identical oracle, identical clauses |
| Determinism | Virtual time, seeded PRNG, total event order, FNV-1a trace hash |

**Current state (verified 2026-07-08):** All M0–M4 + T1–T8 complete:

| Milestone | Status |
|---|---|
| M0 — Contract kernel (`stablehal-contract`) | ✓ |
| M1 — Deterministic simulator (`stablehal-sim`) | ✓ |
| M2 — MODBUS RTU contract, drivers, DST sweep | ✓ |
| M3 — Conformance suite, closed loop | ✓ |
| M4 — Whitepaper v0.1 | ✓ |
| T5 — I2C contract (`stablehal-i2c`, 7 clauses) | ✓ |
| T6 — SHC1 capture format + `stablehal-certify` | ✓ |
| T7 — On-target RP2040 tester | **deferred (needs hardware)** |
| T8 — Whitepaper v0.2 | ✓ |

---

## 2. Comparable Tools & Frameworks

### 2.1 Full-system embedded emulation

| Tool | Approach | Deterministic? | Contract-based? | Maturity |
|---|---|---|---|---|
| **Renode** (Antmicro) | CPU-level emulation + peripheral models; shared virtual time | ✅ | ❌ — models concrete chips, not agreements | Production (Google, Zephyr, Microsemi) |
| **QEMU** | Binary translation + device models | Partial (`-icount`) | ❌ | Production |

**Key difference from StableHAL:** Renode models *what the chip does*, not
*what it must do*. It cannot name the spec clause a device violates.

### 2.2 Deterministic simulation testing platforms

| Tool | Approach | Deterministic? | Embedded? | Contract-based? | Open source? |
|---|---|---|---|---|---|
| **Antithesis** | Custom hypervisor, fault injection, automated RCA | ✅ | ❌ — distributed systems | ❌ | ❌ (commercial SaaS, $$$) |
| **FoundationDB** | Actor-based DST built into the database | ✅ | ❌ | ❌ | ✅ |
| **WarpStream** | DST applied to streaming platform | ✅ | ❌ | ❌ | ❌ |

**Key difference:** Antithesis et al. prove that DST is commercially viable
at scale. StableHAL adapts the *same idea* to the embedded HAL boundary, a
domain none of them target. They are complementary, not competitive.

### 2.3 Formal verification & model checking

| Tool | Approach | Contract-based? | Embedded? | Key differentiator from StableHAL |
|---|---|---|---|---|
| **TLA+** (Lamport/MSR) | Formal spec language + TLC model checker (exhaustive) | ✅ | ❌ | Writes specs in a separate mathematical language; StableHAL contracts *are* Rust code |
| **Kani** (AWS) | Bit-precise model checker for Rust | ✅ (function contracts) | ✅ | Proves internal correctness (no panics); StableHAL proves protocol conformance. **Complementary** |
| **Microsoft P** | State-machine modelling + model checking; shipped Windows USB 3.0 stack | ✅ | ✅ (drivers) | Full formal model + code generation; higher adoption barrier |
| **Alloy** | Lightweight formal spec + SAT analysis | ✅ | ❌ | Design-time only |
| **CBMC / KLEE** | Bounded model checking / symbolic execution | ❌ | Partial | General C verification |

**Key insight:** StableHAL is lighter-weight than TLA+/P (no separate
modelling language), while being protocol-domain-specific in a way that Kani
is not. The ideal stack: Kani proves contract `step()` functions are
internally correct; StableHAL proves the driver conforms to the wire
protocol.

### 2.4 Property-based testing

| Tool | Approach | Key difference |
|---|---|---|
| **proptest** (Rust) | Random input generation + shrinking | General-purpose, not protocol-aware. Could *generate* StableHAL stimulus sequences. |
| **Hypothesis** (Python) | Adaptive generation | Same role as proptest. |

### 2.5 Embedded HAL & testing ecosystem

| Tool | Approach | Key difference |
|---|---|---|
| **embedded-hal** (Rust) | Trait-based HAL abstraction | Deliberately excludes behaviour enforcement. StableHAL provides the *contract* it omits. **Complementary** |
| **PHiLIP** (RIOT OS) | Reference instrument device for conformance | Hardware-only. StableHAL's deferred T7 follows this pattern. |
| **OPC UA CTT** | Commercial conformance certification suite | Closed, protocol-specific. Proves the *market* for contract-based conformance. |
| **MODBUS conformance tools** (TMG, ModbusConform) | Commercial MODBUS certification | Point solutions. StableHAL would be the open-source alternative. |

### 2.6 Academic related work

- **UniTESK**: Contract-based hardware test oracle generation (cited in
  whitepaper §2)
- **arXiv 2512.16897**: Runtime checking of STM32/PSoC HAL usage (software
  side only)

Both validate the approach but are narrower in scope.

---

## 3. Competitive Positioning

### 3.1 Positioning matrix

| Tool | Deterministic | Contract-based | Embedded | Open source | Protocol-agnostic |
|---|---|---|---|---|---|
| **StableHAL** | ✅ | ✅ | ✅ | ✅ | ✅ |
| Renode | ✅ | ❌ | ✅ | ✅ | ✅ |
| Antithesis | ✅ | ❌ | ❌ | ❌ | ✅ |
| TLA+ | N/A | ✅ | ❌ | ✅ | ✅ |
| Kani | N/A | ✅ | ✅ | ✅ | ✅ |
| OPC UA CTT | ❌ | ✅ | ✅ | ❌ | ❌ |
| `embedded-hal` | ❌ | ❌ | ✅ | ✅ | ✅ |

**StableHAL is the only tool that checks all five boxes.** The space between
"formal specification" (TLA+) and "full-system emulation" (Renode) is
empty, and StableHAL occupies it.

### 3.2 SWOT

| Strengths | Weaknesses |
|---|---|
| Novel, well-articulated idea (whitepaper + arc42) | Only 2 protocols demonstrated — generalisation claim is thin |
| Working implementation across UART, MODBUS, I2C | No real hardware certification done (T7) |
| Strong determinism guarantees (seed-replay, FNV-1a) | No CI, no crates.io, no community |
| `no_std` contract kernel — portable to any MCU | Single author — bus factor = 1 |
| Zero deps in `crates/*` — auditable determinism | Whitepaper not yet peer-reviewed |
| Clause-level spec traceability | No third-party adoption or case studies |
| Clean separation: kernel → protocol → sim → runner | No integration with Rust embedded ecosystem (embassy, RTIC) |
| | Terminology is academic — may deter practitioners |

| Opportunities | Threats |
|---|---|
| Embedded Rust is growing rapidly (embassy, RTIC, probe-rs) | Antithesis could expand into embedded |
| MODBUS certification market is underserved for open source | Renode + Robot Framework could add contract checking |
| I2C, SPI, CAN, USB are all protocol contracts waiting to be written | Industry may not value the approach ("we've always done integration tests") |
| AI-assisted firmware needs stronger verification | Embedded Rust adoption is still small |
| Functional safety standards (IEC 61508, ISO 26262) need evidence | Competing formal methods (Kani, CBMC) are better funded |
| `embedded-hal` ecosystem has no contract layer — gap to fill | |

---

## 4. What to Add

### 4.1 Priority 0 — High impact, low effort

| Item | Effort | Rationale |
|---|---|---|
| **GitHub Actions CI** | 1 day | Blocking contributor adoption. Wire `scripts/verify.sh` into CI. |
| **crates.io publishing** | 1 hour | Visibility; enables use as a dependency. Publish all `crates/*`. |
| **CONTRIBUTING.md + protocol contract template** | 0.5 day | Enables community to add new contracts. Show the 1500-LoC pattern. |

### 4.2 Priority 1 — High impact, moderate effort

| Item | Effort | Rationale |
|---|---|---|
| **SPI protocol contract** | 2–3 days | Third protocol cheaply proves generalisation. Adds multi-slave bus. |
| **`embedded-hal` companion crate** | 2 days | Bridges to the Rust embedded ecosystem. Implement `UartBus` over `embedded-io`. |
| **arXiv preprint** | 1 hour | Priority claim; starts citation graph. |
| **CAN protocol contract** | 5 days | Automotive market signal; functional safety relevance. |

### 4.3 Priority 2 — High impact, high effort

| Item | Effort | Rationale |
|---|---|---|
| **RP2040 hardware certification (T7)** | 5–10 days | Closes the whitepaper's central claim — without it the loop is academic. |
| **Whitepaper → peer-reviewed venue** | 1 week polish | Academic credibility. Target: EMSOFT, RTSS, or IEEE ESL. |
| **Industry partner case study** | depends | Certify a real MODBUS device with the conformance suite. |

### 4.4 Priority 3 — Medium impact

| Item | Effort | Rationale |
|---|---|---|
| **Kani integration** | 3–5 days | Formally verify contract `step()` functions. |
| **Web demo / interactive clause explorer** | 3–5 days | Marketing: "show me what each clause catches". |
| **VSCode extension for `.shc1` files** | 2–3 days | Developer experience for capture-file inspection. |

---

## 5. What to Keep & Strengthen

These design decisions are core differentiators — document them prominently
and defend them in reviews:

| Decision | Why it matters |
|---|---|
| **Zero external dependencies in `crates/*` (G1)** | Auditable determinism; every dependency is a potential entropy source. |
| **Clause-level traceability** | The whitepaper's strongest evidence — every violation names a spec paragraph. |
| **`no_std` oracle** | The architectural load-bearing insight — same code on host and target. |
| **Arc42 architecture documentation** | Unusually thorough for an OSS project — keep current. |
| **Per-direction judgement with waiver semantics** | Enables conformance testers to deliberately violate their own side. |
| **Deterministic scheduler with `(at, seq)` ordering** | Simpler than a full actor framework and provably reproducible. |

---

## 6. What to Remove or Deprioritise

| Item | Action | Rationale |
|---|---|---|
| Multi-master I2C, 10-bit addressing, SMBus PEC | Keep deferred | Already documented as v2+ in design-spec-next.md |
| "What NOT to do" lists in design docs | Migrate to GitHub issues | Don't maintain in prose alongside evolving code |
| Workstation-specific notes (rustup PATH) | Move from next_steps.md to CONTRIBUTING.md | Doesn't belong in the project backlog |
| Full-system emulation ambitions | Explicitly out of scope | Don't compete with Renode — stay at the contract layer |

---

## 7. How to Increase Impact

### 7.1 Academic track

1. **arXiv preprint now** — establishes priority, starts citation graph.
2. **Submit to EMSOFT** — best fit (embedded software, HW/SW contracts).
3. Alternative: **RustIC** (Rust in IoT/C embedded workshop) — community
   building, faster review.

### 7.2 Ecosystem embedding

1. Create an **`embedded-hal-contract` companion crate** — adapts
   `embedded-hal` traits so any existing driver can be verified.
2. Blog post series:
   - "Classical integration test → contract-based certification"
   - "Adding a new protocol contract in 1500 lines of Rust"
   - "What I learned porting MODBUS rmodbus (10/13 conform)"
3. Propose a session at the **Rust Embedded Working Group** meetings.

### 7.3 Industry engagement

1. **MODBUS community**: Publish the rmodbus experiment (10/13 pass, 3
   documented measurement differences).
2. **Automotive**: CAN protocol contract → approach Volvo Cars, Bosch,
   Veecle for collaboration.
3. **Functional safety**: Contract artifacts fit IEC 61508 / ISO 26262
   evidence chains. Approach Ferrocene (qualified Rust toolchain).

### 7.4 Community building

1. Good-first-issue labels: "Add a protocol contract for [SPI/CAN/1-Wire]".
2. Template repository: `protocol-contract-template` — clone, fill in
   clauses, open PR.
3. Monthly release cadence after crates.io publishing.

### 7.5 Commercial pathways (not mutually exclusive)

| Option | Description |
|---|---|
| **Open-source core, paid certifier** | `stablehal-certify` as a SaaS: upload captures → signed certificate. Free for OSS, paid for commercial certification. |
| **Consulting** | Help device vendors certify MODBUS/I2C implementations. |
| **Antithesis partnership** | StableHAL becomes the "embedded contract module" within Antithesis's platform. |
| **Ferrocene partnership** | Qualified Rust toolchain + qualified HAL contracts = end-to-end qualified embedded development. |

---

## 8. Target Markets / Customers

### 8.1 Primary — Early adopters (6–12 months)

| Segment | Need | Go-to-market |
|---|---|---|
| **Industrial IoT device vendors** | MODBUS device certification; reduce per-release integration testing | Whitepaper + rmodbus experiment |
| **Rust embedded consultancies** (Ferrocene, Technokrat, Tweedegolf) | Differentiated offering for clients | Partnership + case study |
| **Embedded Rust OSS projects** (embassy, RTIC) | Better driver testing | Integration PRs + blog posts |

### 8.2 Secondary — Growth (12–36 months)

| Segment | Need |
|---|---|
| **Automotive Tier 1/2 suppliers** | ISO 26262 evidence for HAL correctness — clause traceability + determinism |
| **Medical device manufacturers** | IEC 62304 verification evidence — deterministic replay = audit trail |
| **Aerospace & defence** | DO-178C evidence — contract artifacts + seed-replay |
| **Chip vendors** (STM32, NXP, Renesas) | HAL quality assurance — conformance suite for peripheral HALs |
| **Cloud IoT platforms** (AWS FreeRTOS, Azure IoT) | Device certification at scale — automated conformance pipeline |

### 8.3 Tertiary — Long-term (36+ months)

| Segment | Need |
|---|---|
| **Academic research groups** | Platform for HW/SW contract research |
| **Standard bodies** (MODBUS.org, USB-IF) | Open-source reference conformance tool |
| **Functional safety auditors** (TÜV SÜD, SGS-TÜV) | Independent contract-based evidence |

---

## 9. Recommendations — Immediate Next Steps

Ordered for maximum impact per unit effort:

1. **Publish to crates.io + GitHub Actions CI** (1 day) — removes the
   biggest barrier to external contribution.
2. **Write CONTRIBUTING.md** with a "Adding a New Protocol Contract"
   template (0.5 day) — enables community-written contracts.
3. **Create an `embedded-hal-contract` companion crate** (2 days) —
   bridges StableHAL with the Rust embedded ecosystem.
4. **Publish whitepaper on arXiv** (1 hour) — establishes priority,
   starts citation graph.
5. **SPI protocol contract** (2–3 days) — third protocol strengthens the
   generalisation claim at low cost.
6. **Submit whitepaper to EMSOFT or RustIC** (1 week polishing) —
   academic validation.
7. **RP2040 hardware certification (T7)** (5–10 days, with hardware) —
   closes the loop the whitepaper claims. This is the #1 credibility gap.

**Do NOT raise VC funding** until hardware certification + one industry
case study is complete. The project needs technical validation first.
