//! Contract clauses. Every checkable obligation carries an identifier and a
//! citation into the governing specification, so a violation verdict always
//! points back to prose the counterparty signed up to.

/// One obligation of a contract party.
#[derive(Debug, PartialEq, Eq)]
pub struct Clause {
    /// Stable identifier, e.g. `"RTU-T35-FRAME"`. Used in verdicts, reports
    /// and the traceability table.
    pub id: &'static str,
    /// Citation into the governing spec, e.g.
    /// `"MODBUS over Serial Line V1.02 §2.5.1.1"`.
    pub citation: &'static str,
    /// The requirement, in one human-readable sentence.
    pub text: &'static str,
}
