//! Simulator-hosted conformance runner and reusable actors.
//!
//! The runner is deterministic end to end: fixed seed per case, virtual time
//! only. The wire oracle (the [`RtuContract`]) always judges the DUT
//! direction; the tester direction is judged unless the case deliberately
//! emits malformed stimuli.

use std::cell::RefCell;
use std::rc::Rc;

use stablehal_contract::Nanos;
use stablehal_modbus::contract::{Dir, RtuContract, RtuEvent};
use stablehal_modbus::pdu::{self, Adu};
use stablehal_modbus::{
    MasterConfig, MasterProgress, RegisterBank, RtuMaster, RtuSlave, RtuTiming, SlaveConfig,
    SlaveProgress,
};
use stablehal_sim::{Actor, BusObserver, ContractObserver, NextPoll, Sim, SimConfig, UartPort};
use stablehal_uart::{LineEvent, UartBus, UartConfig, UartError};

use crate::{Case, CaseRequest, Expected, GapKind, Stimulus, DUT_UNIT};

// ---------------------------------------------------------------------------
// Wire oracle plumbing
// ---------------------------------------------------------------------------

/// The RTU contract as a bus observer: line 0 = master→slave, line 1 =
/// slave→master; only transmitter-side events are judged (what each party
/// actually emitted, not what the noisy channel delivered).
pub fn rtu_observer(
    timing: RtuTiming,
    judge_master: bool,
    judge_slave: bool,
) -> Box<dyn BusObserver> {
    let mut contract = RtuContract::new(timing);
    contract.judge_master = judge_master;
    contract.judge_slave = judge_slave;
    Box::new(ContractObserver::new(
        contract,
        |line, ev| {
            let dir = if line == 0 {
                Dir::MasterToSlave
            } else {
                Dir::SlaveToMaster
            };
            match *ev {
                LineEvent::TxStart { byte } => Some(RtuEvent::CharStart { dir, byte }),
                LineEvent::TxEnd { byte } => Some(RtuEvent::CharEnd { dir, byte }),
                _ => None,
            }
        },
        Some(RtuEvent::EndOfObservation),
    ))
}

// ---------------------------------------------------------------------------
// Actor adapters
// ---------------------------------------------------------------------------

/// Adapts the `no_std` [`RtuSlave`] into the simulator poll loop.
pub struct SlaveActor {
    pub slave: RtuSlave,
}

impl SlaveActor {
    pub fn new(cfg: SlaveConfig, bank: RegisterBank) -> Self {
        SlaveActor {
            slave: RtuSlave::new(cfg, bank),
        }
    }
}

impl Actor for SlaveActor {
    fn poll(&mut self, now: Nanos, port: &mut UartPort<'_>) -> NextPoll {
        let SlaveProgress { wake_at } = self.slave.poll(now, port);
        match wake_at {
            Some(t) => NextPoll::At(t),
            None => NextPoll::OnRx,
        }
    }
}

// ---------------------------------------------------------------------------
// Scripted master (for DST sweeps): drives the real RtuMaster driver through
// a transaction list and verifies semantic properties.
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub enum OwnedRequest {
    ReadHolding { addr: u16, count: u16 },
    WriteSingle { addr: u16, value: u16 },
    WriteMultiple { addr: u16, values: Vec<u16> },
}

#[derive(Clone, Debug)]
pub enum Verify {
    None,
    /// Every returned register equals `pattern(addr + i)`.
    Pattern {
        addr: u16,
    },
    /// Returned registers equal exactly these values.
    Values(Vec<u16>),
}

#[derive(Clone, Debug)]
pub struct Txn {
    pub unit: u8,
    pub request: OwnedRequest,
    pub verify: Verify,
    /// Skip verification if the previous transaction errored (e.g. a
    /// readback of a write that may not have applied).
    pub needs_prev_ok: bool,
}

/// Shared outcome sink for a DST run.
#[derive(Default)]
pub struct ScriptOutcome {
    /// Semantic property failures: wrong data returned. Any entry fails a run.
    pub property_failures: Vec<String>,
    /// Transactions that honestly errored after retries (acceptable under
    /// fault injection; counted for reporting).
    pub gave_up: usize,
    pub completed: usize,
}

pub struct ScriptedMaster {
    master: RtuMaster,
    txns: Vec<Txn>,
    idx: usize,
    started: bool,
    prev_ok: bool,
    outcome: Rc<RefCell<ScriptOutcome>>,
    pattern: fn(u16) -> u16,
}

impl ScriptedMaster {
    pub fn new(
        cfg: MasterConfig,
        txns: Vec<Txn>,
        pattern: fn(u16) -> u16,
    ) -> (Self, Rc<RefCell<ScriptOutcome>>) {
        let outcome = Rc::new(RefCell::new(ScriptOutcome::default()));
        (
            ScriptedMaster {
                master: RtuMaster::new(cfg),
                txns,
                idx: 0,
                started: false,
                prev_ok: true,
                outcome: outcome.clone(),
                pattern,
            },
            outcome,
        )
    }

    fn verify(&self, txn: &Txn, resp: &Adu) -> Result<(), String> {
        let fc = match txn.request {
            OwnedRequest::ReadHolding { .. } => 0x03,
            OwnedRequest::WriteSingle { .. } => 0x06,
            OwnedRequest::WriteMultiple { .. } => 0x10,
        };
        let parsed = pdu::parse_response(txn.unit, fc, resp.bytes())
            .map_err(|e| format!("driver accepted unparseable response: {:?}", e))?;
        match (&txn.verify, &parsed) {
            (Verify::None, _) => Ok(()),
            (Verify::Pattern { addr }, pdu::ParsedResponse::ReadHolding { .. }) => {
                for i in 0..parsed.register_count() {
                    let want = (self.pattern)(addr + i as u16);
                    let got = parsed.register(i).unwrap();
                    if got != want {
                        return Err(format!(
                            "register {} = {:#06X}, expected pattern {:#06X}",
                            addr + i as u16,
                            got,
                            want
                        ));
                    }
                }
                Ok(())
            }
            (Verify::Values(vals), pdu::ParsedResponse::ReadHolding { .. }) => {
                if parsed.register_count() != vals.len() {
                    return Err(format!(
                        "expected {} registers, got {}",
                        vals.len(),
                        parsed.register_count()
                    ));
                }
                for (i, want) in vals.iter().enumerate() {
                    let got = parsed.register(i).unwrap();
                    if got != *want {
                        return Err(format!(
                            "readback[{}] = {:#06X}, expected {:#06X}",
                            i, got, want
                        ));
                    }
                }
                Ok(())
            }
            (v, p) => Err(format!("verify {:?} against response {:?}", v, p)),
        }
    }
}

impl Actor for ScriptedMaster {
    fn poll(&mut self, now: Nanos, port: &mut UartPort<'_>) -> NextPoll {
        loop {
            if self.idx >= self.txns.len() {
                return NextPoll::Done;
            }
            if !self.started {
                let txn = &self.txns[self.idx];
                let ok = match &txn.request {
                    OwnedRequest::ReadHolding { addr, count } => self.master.start(
                        txn.unit,
                        &pdu::Request::ReadHoldingRegisters {
                            addr: *addr,
                            count: *count,
                        },
                    ),
                    OwnedRequest::WriteSingle { addr, value } => self.master.start(
                        txn.unit,
                        &pdu::Request::WriteSingleRegister {
                            addr: *addr,
                            value: *value,
                        },
                    ),
                    OwnedRequest::WriteMultiple { addr, values } => self.master.start(
                        txn.unit,
                        &pdu::Request::WriteMultipleRegisters {
                            addr: *addr,
                            values,
                        },
                    ),
                };
                debug_assert!(ok, "master must be idle between transactions");
                self.started = true;
            }
            match self.master.poll(now, port) {
                MasterProgress::Busy { wake_at } => {
                    return match wake_at {
                        Some(t) => NextPoll::At(t),
                        None => NextPoll::OnRx,
                    };
                }
                MasterProgress::Done(result) => {
                    let txn = self.txns[self.idx].clone();
                    let mut out = self.outcome.borrow_mut();
                    match result {
                        Ok(resp) => {
                            out.completed += 1;
                            let check = !txn.needs_prev_ok || self.prev_ok;
                            if check {
                                if let Err(msg) = self.verify(&txn, &resp) {
                                    out.property_failures
                                        .push(format!("txn {}: {}", self.idx, msg));
                                }
                            }
                            self.prev_ok = true;
                        }
                        Err(e) => {
                            // Honest failure after retries — acceptable under
                            // fault injection, never silently wrong data.
                            out.gave_up += 1;
                            self.prev_ok = false;
                            let _ = e;
                        }
                    }
                    self.idx += 1;
                    self.started = false;
                    // Loop to start the next transaction in this poll.
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Conformance tester actor: frame-level scripting with deliberate corruption
// and timing probes. This is NOT the RtuMaster driver — a tester must be
// able to violate the contract on purpose.
// ---------------------------------------------------------------------------

struct StepPlan {
    bytes: Vec<u8>,
    /// Extra silence inserted before byte i (beyond back-to-back timing).
    pre_gap: Vec<Nanos>,
    /// Expected function code (for building expected responses).
    expect_fc: u8,
}

enum TesterState {
    WaitStart(Nanos),
    Sending { i: usize, next_at: Nanos },
    Listening { until: Nanos },
    Done,
}

struct TesterActor {
    plans: Vec<StepPlan>,
    cur: usize,
    state: TesterState,
    char_time: Nanos,
    t35: Nanos,
    window: Nanos,
    /// Per-step received (time, byte-or-fault) pairs.
    #[allow(clippy::type_complexity)]
    rx_log: Rc<RefCell<Vec<Vec<(Nanos, Result<u8, UartError>)>>>>,
}

impl Actor for TesterActor {
    fn poll(&mut self, now: Nanos, port: &mut UartPort<'_>) -> NextPoll {
        while let Some(item) = port.try_read() {
            if self.cur < self.plans.len() {
                self.rx_log.borrow_mut()[self.cur].push((now, item));
            }
        }
        loop {
            match self.state {
                TesterState::WaitStart(t) => {
                    if now < t {
                        return NextPoll::At(t);
                    }
                    self.state = TesterState::Sending { i: 0, next_at: now };
                }
                TesterState::Sending { i, next_at } => {
                    let plan = &self.plans[self.cur];
                    if i == plan.bytes.len() {
                        // Last char completes at next_at; listen from there.
                        self.state = TesterState::Listening {
                            until: next_at + self.window,
                        };
                        continue;
                    }
                    if now < next_at {
                        return NextPoll::At(next_at);
                    }
                    if port.try_write(plan.bytes[i]).is_ok() {
                        let mut nxt = now + self.char_time;
                        if let Some(g) = plan.pre_gap.get(i + 1) {
                            nxt += *g;
                        }
                        self.state = TesterState::Sending {
                            i: i + 1,
                            next_at: nxt,
                        };
                        continue;
                    }
                    return NextPoll::At(now + self.char_time);
                }
                TesterState::Listening { until } => {
                    if now < until {
                        return NextPoll::At(until);
                    }
                    self.cur += 1;
                    if self.cur == self.plans.len() {
                        self.state = TesterState::Done;
                    } else {
                        self.state = TesterState::WaitStart(now + self.t35);
                    }
                }
                TesterState::Done => return NextPoll::Done,
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Suite runner
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct CaseOutcome {
    pub id: &'static str,
    pub pass: bool,
    pub detail: String,
}

#[derive(Clone, Debug, Default)]
pub struct Report {
    pub outcomes: Vec<CaseOutcome>,
}

impl Report {
    pub fn passed(&self) -> usize {
        self.outcomes.iter().filter(|o| o.pass).count()
    }
    pub fn failed(&self) -> Vec<&CaseOutcome> {
        self.outcomes.iter().filter(|o| !o.pass).collect()
    }
    pub fn all_pass(&self) -> bool {
        self.outcomes.iter().all(|o| o.pass)
    }
    /// Concatenated failure details (for asserting which clause was caught).
    pub fn failure_text(&self) -> String {
        self.failed()
            .iter()
            .map(|o| format!("[{}] {}", o.id, o.detail))
            .collect::<Vec<_>>()
            .join("\n")
    }
    /// Clause ids probed by the executed cases (union of case.probes).
    /// WP4 §4.1: every clause id in the contract (except documented
    /// structural-only ones) must appear in some case's probes.
    pub fn probed_clauses(&self, suite: &[Case]) -> Vec<&'static str> {
        let mut ids: Vec<&'static str> = Vec::new();
        for case in suite {
            for p in case.probes {
                if !ids.contains(p) {
                    ids.push(p);
                }
            }
        }
        ids
    }
}

fn build_stimulus(timing: &RtuTiming, stim: &Stimulus) -> StepPlan {
    fn request_bytes(unit: u8, req: &CaseRequest) -> Vec<u8> {
        let mut adu = Adu::new();
        let r = match *req {
            CaseRequest::ReadHolding { addr, count } => {
                pdu::Request::ReadHoldingRegisters { addr, count }
            }
            CaseRequest::WriteSingle { addr, value } => {
                pdu::Request::WriteSingleRegister { addr, value }
            }
            CaseRequest::WriteMultiple { addr, values } => {
                pdu::Request::WriteMultipleRegisters { addr, values }
            }
        };
        pdu::build_request(unit, &r, &mut adu);
        adu.bytes().to_vec()
    }

    let (bytes, expect_fc, gap_at): (Vec<u8>, u8, Option<(usize, Nanos)>) = match stim {
        Stimulus::Valid(req) => (request_bytes(DUT_UNIT, req), req.function_code(), None),
        Stimulus::WrongUnit(req) => (
            request_bytes(DUT_UNIT.wrapping_add(0x22), req),
            req.function_code(),
            None,
        ),
        Stimulus::CorruptCrc(req) => {
            let mut b = request_bytes(DUT_UNIT, req);
            let last = b.len() - 1;
            b[last] ^= 0xFF;
            (b, req.function_code(), None)
        }
        Stimulus::UnknownFc { fc } => {
            let mut adu = Adu::new();
            adu.push(DUT_UNIT);
            adu.push(*fc);
            adu.push(0x00);
            adu.push(0x00);
            adu.append_crc();
            (adu.bytes().to_vec(), *fc, None)
        }
        Stimulus::Runt => (vec![DUT_UNIT, 0x03, 0x00], 0x03, None),
        Stimulus::MidFrameGap {
            req,
            after_byte,
            gap,
        } => {
            let g = match gap {
                GapKind::ExactlyT15 => timing.t15,
                GapKind::WellOverT35 => timing.t35 + Nanos::from_micros(250),
            };
            (
                request_bytes(DUT_UNIT, req),
                req.function_code(),
                Some((*after_byte + 1, g)),
            )
        }
    };

    let mut pre_gap = vec![Nanos::ZERO; bytes.len()];
    if let Some((i, g)) = gap_at {
        if i < pre_gap.len() {
            pre_gap[i] = g;
        }
    }
    StepPlan {
        bytes,
        pre_gap,
        expect_fc,
    }
}

/// Expected response frame bytes, if a response is expected.
fn expected_frame(step_fc: u8, expect: &Expected) -> Option<Vec<u8>> {
    let mut adu = Adu::new();
    match expect {
        Expected::Silence => return None,
        Expected::ReadPattern { addr, count } => {
            let values: Vec<u16> = (0..*count).map(|i| crate::pattern(addr + i)).collect();
            pdu::build_read_response(DUT_UNIT, &values, &mut adu);
        }
        Expected::ReadValues { values } => {
            pdu::build_read_response(DUT_UNIT, values, &mut adu);
        }
        Expected::EchoWriteSingle { addr, value } => {
            pdu::build_write_single_response(DUT_UNIT, *addr, *value, &mut adu);
        }
        Expected::EchoWriteMultiple { addr, count } => {
            pdu::build_write_multiple_response(DUT_UNIT, *addr, *count, &mut adu);
        }
        Expected::Exception { code } => {
            let c = pdu::ExceptionCode::from_u8(*code).expect("suite uses valid codes");
            pdu::build_exception(DUT_UNIT, step_fc, c, &mut adu);
        }
    }
    Some(adu.bytes().to_vec())
}

/// Split a step's RX log into frames by ≥ t3.5 arrival gaps.
fn frames_of(log: &[(Nanos, Result<u8, UartError>)], t35: Nanos) -> Vec<Vec<u8>> {
    let mut frames: Vec<Vec<u8>> = Vec::new();
    let mut last: Option<Nanos> = None;
    for (at, item) in log {
        let new_frame = match last {
            None => true,
            Some(prev) => at.saturating_sub(prev) >= t35,
        };
        if new_frame {
            frames.push(Vec::new());
        }
        if let Ok(b) = item {
            frames.last_mut().unwrap().push(*b);
        }
        last = Some(*at);
    }
    frames
}

pub struct SuiteConfig {
    pub uart: UartConfig,
    pub timing: RtuTiming,
    /// Seed for the (narrow) environment nondeterminism during conformance.
    pub seed: u64,
}

impl SuiteConfig {
    pub fn default_9600() -> Self {
        let uart = UartConfig::rtu_default(9600);
        SuiteConfig {
            uart,
            timing: RtuTiming::from_uart(&uart, Nanos::from_millis(50)),
            seed: 0xC0FFEE,
        }
    }
}

/// Run `suite` against a DUT slave, one fresh sim per case.
/// `make_dut` must yield a freshly initialized DUT actor each call
/// (registers pre-set to [`crate::pattern`]).
pub fn run_suite(
    cfg: &SuiteConfig,
    suite: &[Case],
    mut make_dut: impl FnMut() -> Box<dyn Actor>,
) -> Report {
    let mut report = Report::default();
    for case in suite {
        report.outcomes.push(run_case(cfg, case, make_dut()));
    }
    report
}

fn run_case(cfg: &SuiteConfig, case: &Case, dut: Box<dyn Actor>) -> CaseOutcome {
    let timing = cfg.timing;
    let plans: Vec<StepPlan> = case
        .steps
        .iter()
        .map(|s| build_stimulus(&timing, &s.stimulus))
        .collect();
    let expect_fcs: Vec<u8> = plans.iter().map(|p| p.expect_fc).collect();

    let rx_log = Rc::new(RefCell::new(vec![Vec::new(); plans.len()]));
    let tester = TesterActor {
        plans,
        cur: 0,
        state: TesterState::WaitStart(timing.t35 + timing.t35),
        char_time: timing.char_time,
        t35: timing.t35,
        // The response may *start* as late as response_timeout and a maximal
        // ADU (256 chars) takes 256 char times to transmit.
        window: timing.response_timeout
            + timing.char_time.mul_ratio(260, 1)
            + timing.t35.mul_ratio(4, 1),
        rx_log: rx_log.clone(),
    };

    let mut sim = Sim::new(SimConfig::new(cfg.seed, cfg.uart), Box::new(tester), dut);
    sim.add_observer(rtu_observer(timing, case.judge_master_dir, true));

    let deadline = Nanos::from_secs(120);
    match sim.run_until(deadline) {
        Err(e) => CaseOutcome {
            id: case.id,
            pass: false,
            detail: format!("wire contract violation: {}", e),
        },
        Ok(summary) => {
            // The tester (actor 0) must finish; the DUT listens forever.
            if !summary.done[0] {
                return CaseOutcome {
                    id: case.id,
                    pass: false,
                    detail: "tester did not complete within the virtual deadline".into(),
                };
            }
            // Judge each step's response against the expectation.
            let log = rx_log.borrow();
            for (i, step) in case.steps.iter().enumerate() {
                let corrupted = log[i].iter().any(|(_, r)| r.is_err());
                if corrupted {
                    return CaseOutcome {
                        id: case.id,
                        pass: false,
                        detail: format!("step {}: response corrupted in transit (rerun)", i),
                    };
                }
                let frames = frames_of(&log[i], timing.t35);
                match expected_frame(expect_fcs[i], &step.expect) {
                    None => {
                        if !frames.is_empty() {
                            return CaseOutcome {
                                id: case.id,
                                pass: false,
                                detail: format!(
                                    "step {}: expected silence, DUT transmitted {:02X?}",
                                    i, frames[0]
                                ),
                            };
                        }
                    }
                    Some(want) => {
                        if frames.len() != 1 {
                            return CaseOutcome {
                                id: case.id,
                                pass: false,
                                detail: format!(
                                    "step {}: expected one response frame, saw {}",
                                    i,
                                    frames.len()
                                ),
                            };
                        }
                        if frames[0] != want {
                            return CaseOutcome {
                                id: case.id,
                                pass: false,
                                detail: format!(
                                    "step {}: response {:02X?} != expected {:02X?}",
                                    i, frames[0], want
                                ),
                            };
                        }
                    }
                }
            }
            CaseOutcome {
                id: case.id,
                pass: true,
                detail: String::new(),
            }
        }
    }
}
