//! World-facing CAN node model (risk T8): unlike MODBUS/I2C/LIN/SPI's
//! master + N-addressed-slaves shape, CAN is a broadcast bus where any node
//! can transmit and every other node ACKs a correctly received frame. This
//! module models that directly rather than mirroring the other protocols'
//! actor/device split.

use crate::{CanFrame, CanFrameDataBuf, CanId};
use stablehal_contract::Nanos;

/// A node on the bus, as seen by a `stablehal-sim`-hosted world. Symmetric
/// by design: every node can offer a frame to transmit and every other node
/// gets a chance to ACK it — there is no privileged "master".
pub trait CanNodeActor {
    /// Offer a frame to transmit this round, if any. The actor retains its
    /// own pending-frame state and keeps offering the same frame until
    /// [`CanNodeActor::on_sent`] confirms it won arbitration — called every
    /// bus-idle tick.
    fn poll_tx(&mut self, now: Nanos) -> Option<CanFrame>;
    /// Called on the node whose offered frame won arbitration this round,
    /// once it has actually been transmitted (so it can advance past the
    /// frame it was offering).
    fn on_sent(&mut self) {}
    /// A frame from another node was observed on the bus; return whether
    /// this node acks it. Real CAN acks any correctly received frame
    /// (dominant ACK bit) regardless of ID interest — a node that withholds
    /// this is a planted bug, not a legal "not for me" response.
    fn on_frame(&mut self, frame: &CanFrame) -> bool;
    /// Whether this node has no more scripted work. Default `false`:
    /// passive/responder nodes (like [`CanRegisterNode`]) listen forever;
    /// only a driving/tester node overrides this. `CanWorld::run_until`
    /// stops once *any* node reports `true` — correct for the reference
    /// topology of one tester + one passive responder; a bus with more than
    /// one driving node would need a richer termination rule.
    fn done(&self) -> bool {
        false
    }
}

/// All-`false` is a conforming device — mirrors `I2cSlaveBugs`/
/// `LinSlaveBugs`/`SpiSlaveBugs`.
#[derive(Copy, Clone, Debug, Default)]
pub struct CanNodeBugs {
    /// Reply with corrupted data instead of the requested register bytes (a
    /// data-correctness bug; not itself a wire-contract breach — mirrors
    /// `I2cSlaveBugs::wrong_read_data`).
    pub wrong_response_data: bool,
    /// Withhold the dominant ACK bit on a correctly received frame. With
    /// only one other node on the reference bus, this node is the *sole*
    /// possible acknowledger, so this breaches `CAN-ACK` — unlike the other
    /// two bugs here, this one is wire-contract-visible.
    pub silent_no_ack: bool,
    /// Accept and ACK the request but never queue a response (the
    /// request/response convention is application-level, not part of
    /// `CanContract` — see the crate-level doc comment — so this is not a
    /// wire-contract breach either, only an application-level silence a
    /// tester can observe directly).
    pub silent_no_response: bool,
}

/// One scripted step: a request frame to offer. `CanFrame` isn't `Copy`
/// (owns a data buffer), so the script carries raw parts and the node
/// builds the frame on demand — mirrors `SpiStep`'s `&'static [u8]` field.
#[derive(Copy, Clone, Debug)]
pub struct CanScriptStep {
    pub id: CanId,
    pub rtr: bool,
    pub data: &'static [u8],
}

/// Outcome of one scripted step, recorded for assertions.
#[derive(Clone, Debug)]
pub enum CanStepOutcome {
    /// A response frame was observed (its data, verbatim).
    Responded(CanFrameDataBuf),
    /// No response frame arrived (legal for the request/response
    /// convention only when the responder is deliberately silent — see
    /// `CanNodeBugs::silent_no_response`).
    NoResponse,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Phase {
    /// Offering `script[next]`, not yet sent.
    Offering,
    /// Sent; waiting for the response frame (or the world to give up).
    AwaitingResponse,
}

const MAX_SCRIPT: usize = 16;

/// Reference tester: offers a fixed script of request frames, one at a
/// time, and records whatever response frame (if any) is observed on the
/// configured `response_id` after each. Mirrors `ScriptedMaster`/
/// `ScheduleMaster`/`ScriptedSpiMaster`.
pub struct CanScriptedNode {
    script: &'static [CanScriptStep],
    response_id: CanId,
    next: usize,
    phase: Phase,
    outcomes: [Option<CanStepOutcome>; MAX_SCRIPT],
    outcomes_len: usize,
}

impl CanScriptedNode {
    pub fn new(script: &'static [CanScriptStep], response_id: CanId) -> Self {
        CanScriptedNode {
            script,
            response_id,
            next: 0,
            phase: Phase::Offering,
            outcomes: [const { None }; MAX_SCRIPT],
            outcomes_len: 0,
        }
    }

    pub fn outcomes(&self) -> &[Option<CanStepOutcome>] {
        &self.outcomes[..self.outcomes_len]
    }

    fn push_outcome(&mut self, o: CanStepOutcome) {
        if self.outcomes_len < MAX_SCRIPT {
            self.outcomes[self.outcomes_len] = Some(o);
            self.outcomes_len += 1;
        }
    }
}

impl CanNodeActor for CanScriptedNode {
    fn poll_tx(&mut self, _now: Nanos) -> Option<CanFrame> {
        if self.phase != Phase::Offering || self.next >= self.script.len() {
            return None;
        }
        let step = self.script[self.next];
        Some(CanFrame {
            id: step.id,
            rtr: step.rtr,
            data: CanFrameDataBuf::from_slice(step.data),
            dlc: step.data.len() as u8,
        })
    }

    fn on_sent(&mut self) {
        self.phase = Phase::AwaitingResponse;
    }

    fn on_frame(&mut self, frame: &CanFrame) -> bool {
        if self.phase == Phase::AwaitingResponse && frame.id == self.response_id {
            self.push_outcome(CanStepOutcome::Responded(frame.data.clone()));
            self.next += 1;
            self.phase = Phase::Offering;
        }
        true // always ack — a correctly received frame is always ACKed.
    }

    fn done(&self) -> bool {
        self.next >= self.script.len()
    }
}

/// Reference responder: on receiving a frame addressed to `request_id`,
/// replies (once selected next) with `response_id` carrying register bytes
/// — first data byte is a pointer, matching `RegisterPointerDevice`/
/// `SpiRegisterDevice`'s convention. This request/response shape is an
/// application-level convention this reference node imposes on top of the
/// raw CAN contract, not part of `CanContract` itself.
pub struct CanRegisterNode {
    request_id: CanId,
    response_id: CanId,
    regs: [u8; 256],
    pending: Option<[u8; 8]>, // (ptr, data...) up to 7 data bytes
    bugs: CanNodeBugs,
}

impl CanRegisterNode {
    pub fn new(request_id: CanId, response_id: CanId) -> Self {
        CanRegisterNode {
            request_id,
            response_id,
            regs: [0u8; 256],
            pending: None,
            bugs: CanNodeBugs::default(),
        }
    }

    pub fn with_bugs(request_id: CanId, response_id: CanId, bugs: CanNodeBugs) -> Self {
        CanRegisterNode {
            bugs,
            ..CanRegisterNode::new(request_id, response_id)
        }
    }

    pub fn read_reg(&self, offset: u8) -> u8 {
        self.regs[offset as usize]
    }
    pub fn write_reg(&mut self, offset: u8, val: u8) {
        self.regs[offset as usize] = val;
    }
}

impl CanNodeActor for CanRegisterNode {
    fn poll_tx(&mut self, _now: Nanos) -> Option<CanFrame> {
        let payload = self.pending?;
        let ptr = payload[0];
        let n = 4usize;
        let mut data = [0u8; 8];
        for (i, slot) in data.iter_mut().take(n).enumerate() {
            let val = self.regs[ptr.wrapping_add(i as u8) as usize];
            *slot = if self.bugs.wrong_response_data {
                !val
            } else {
                val
            };
        }
        Some(CanFrame {
            id: self.response_id,
            rtr: false,
            data: CanFrameDataBuf::from_slice(&data[..n]),
            dlc: n as u8,
        })
    }

    fn on_sent(&mut self) {
        self.pending = None;
    }

    fn on_frame(&mut self, frame: &CanFrame) -> bool {
        if frame.id != self.request_id {
            return true; // ack any correctly received frame regardless of interest
        }
        // Application convention: first data byte is a register pointer;
        // remaining bytes (if any) are written starting there.
        let bytes = frame.data.as_slice();
        if let Some((&ptr, writes)) = bytes.split_first() {
            for (i, &b) in writes.iter().enumerate() {
                self.regs[ptr.wrapping_add(i as u8) as usize] = b;
            }
            if !self.bugs.silent_no_response {
                self.pending = Some([ptr, 0, 0, 0, 0, 0, 0, 0]);
            }
        }
        !self.bugs.silent_no_ack
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const REQ: CanId = CanId::Standard(0x100);
    const RESP: CanId = CanId::Standard(0x101);

    #[test]
    fn register_node_replies_with_written_data_after_a_request() {
        let mut node = CanRegisterNode::new(REQ, RESP);
        node.write_reg(0x10, 0xAB);
        node.write_reg(0x11, 0xCD);

        let acked = node.on_frame(&CanFrame {
            id: REQ,
            rtr: false,
            data: CanFrameDataBuf::from_slice(&[0x10]),
            dlc: 1,
        });
        assert!(acked);
        let response = node.poll_tx(Nanos::ZERO).expect("pending response");
        assert_eq!(response.id, RESP);
        assert_eq!(response.data.as_slice(), &[0xAB, 0xCD, 0x00, 0x00]);
    }

    #[test]
    fn silent_no_ack_bug_withholds_the_ack() {
        let mut node = CanRegisterNode::with_bugs(
            REQ,
            RESP,
            CanNodeBugs {
                silent_no_ack: true,
                ..CanNodeBugs::default()
            },
        );
        let acked = node.on_frame(&CanFrame {
            id: REQ,
            rtr: false,
            data: CanFrameDataBuf::from_slice(&[0x00]),
            dlc: 1,
        });
        assert!(!acked);
    }

    #[test]
    fn scripted_node_records_the_response_and_advances() {
        static SCRIPT: &[CanScriptStep] = &[CanScriptStep {
            id: REQ,
            rtr: false,
            data: &[0x00],
        }];
        let mut tester = CanScriptedNode::new(SCRIPT, RESP);
        let frame = tester.poll_tx(Nanos::ZERO).expect("offers step 0");
        assert_eq!(frame.id, REQ);
        tester.on_sent();
        assert!(tester.poll_tx(Nanos::ZERO).is_none()); // awaiting response
        tester.on_frame(&CanFrame {
            id: RESP,
            rtr: false,
            data: CanFrameDataBuf::from_slice(&[0xAA]),
            dlc: 1,
        });
        assert!(tester.done());
        assert_eq!(tester.outcomes().len(), 1);
    }
}
