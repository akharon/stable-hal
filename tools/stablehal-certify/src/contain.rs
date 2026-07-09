//! Envelope containment: does the hardware-measured hull sit inside the
//! sim-explored hull, for every clause? This is the check that closes the
//! gap between "the simulator conforms" and "the hardware conforms" being
//! proven separately — see docs/user-guide-containment.md.

use stablehal_contract::ClauseHull;

use crate::envelope::Envelope;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClauseVerdict {
    /// Hardware hull fully inside the sim-explored hull.
    Covered,
    /// Hardware operated somewhere the simulator never exercised.
    Uncovered { gap_low_ns: u64, gap_high_ns: u64 },
    /// Hardware has samples for this clause but the sim envelope has none.
    Unprobed,
}

pub struct ClauseReport {
    pub clause_id: String,
    pub hw: Option<ClauseHull>,
    pub sim: Option<ClauseHull>,
    pub verdict: ClauseVerdict,
}

pub struct ContainmentReport {
    pub clauses: Vec<ClauseReport>,
}

impl ContainmentReport {
    pub fn all_covered(&self) -> bool {
        self.clauses
            .iter()
            .all(|c| c.verdict == ClauseVerdict::Covered)
    }
}

/// Compare a hardware-measured envelope against a sim-explored envelope,
/// clause by clause. Every clause the hardware envelope reports is judged;
/// a clause present only in the sim envelope is not reported (nothing to
/// contain).
pub fn contain(hw: &Envelope, sim: &Envelope) -> ContainmentReport {
    let mut clauses = Vec::new();
    for hw_hull in &hw.clauses {
        let sim_hull = sim
            .clauses
            .iter()
            .find(|c| c.clause_id == hw_hull.clause_id);
        let verdict = match sim_hull {
            None => ClauseVerdict::Unprobed,
            Some(s) => {
                if hw_hull.contained_in(s) {
                    ClauseVerdict::Covered
                } else {
                    let gap_low_ns = if hw_hull.min < s.min {
                        (s.min.as_nanos()).saturating_sub(hw_hull.min.as_nanos())
                    } else {
                        0
                    };
                    let gap_high_ns = if hw_hull.max > s.max {
                        hw_hull.max.as_nanos().saturating_sub(s.max.as_nanos())
                    } else {
                        0
                    };
                    ClauseVerdict::Uncovered {
                        gap_low_ns,
                        gap_high_ns,
                    }
                }
            }
        };
        clauses.push(ClauseReport {
            clause_id: hw_hull.clause_id.to_string(),
            hw: Some(*hw_hull),
            sim: sim_hull.copied(),
            verdict,
        });
    }
    ContainmentReport { clauses }
}

fn json_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            c => out.push(c),
        }
    }
    out
}

fn hull_json(h: &Option<ClauseHull>) -> String {
    match h {
        None => "null".to_string(),
        Some(h) => format!(
            "{{ \"min_ns\": {}, \"max_ns\": {}, \"samples\": {} }}",
            h.min.as_nanos(),
            h.max.as_nanos(),
            h.samples
        ),
    }
}

fn verdict_json(v: ClauseVerdict) -> String {
    match v {
        ClauseVerdict::Covered => "{ \"status\": \"COVERED\" }".to_string(),
        ClauseVerdict::Uncovered {
            gap_low_ns,
            gap_high_ns,
        } => format!(
            "{{ \"status\": \"UNCOVERED\", \"gap_low_ns\": {gap_low_ns}, \"gap_high_ns\": {gap_high_ns} }}"
        ),
        ClauseVerdict::Unprobed => "{ \"status\": \"UNPROBED\" }".to_string(),
    }
}

pub fn certificate_json(hw: &Envelope, sim: &Envelope, report: &ContainmentReport) -> String {
    let overall = if report.all_covered() {
        "COVERED"
    } else {
        "UNCOVERED"
    };
    let mut clauses_json = String::new();
    for (i, c) in report.clauses.iter().enumerate() {
        if i > 0 {
            clauses_json.push_str(",\n");
        }
        clauses_json.push_str(&format!(
            "    {{ \"id\": \"{}\", \"hw\": {}, \"sim\": {}, \"verdict\": {} }}",
            json_escape(&c.clause_id),
            hull_json(&c.hw),
            hull_json(&c.sim),
            verdict_json(c.verdict),
        ));
    }
    format!(
        "{{\n  \"format\": \"stablehal-certificate-v2\",\n  \"contract\": \"{}\",\n  \"hw_traces\": {},\n  \"sim_traces\": {},\n  \"verdict\": \"{}\",\n  \"clauses\": [\n{}\n  ]\n}}\n",
        json_escape(&hw.contract),
        hw.traces,
        sim.traces,
        overall,
        clauses_json,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use stablehal_contract::Nanos;

    fn hull(id: &'static str, min: u64, max: u64, samples: u64) -> ClauseHull {
        ClauseHull {
            clause_id: id,
            min: Nanos::from_nanos(min),
            max: Nanos::from_nanos(max),
            samples,
        }
    }

    fn envelope(source: &str, clauses: Vec<ClauseHull>) -> Envelope {
        Envelope {
            contract: "modbus-rtu".to_string(),
            source: source.to_string(),
            traces: 1,
            clauses,
        }
    }

    #[test]
    fn hw_hull_inside_sim_hull_is_covered() {
        let hw = envelope("capture", vec![hull("A", 100, 200, 5)]);
        let sim = envelope("sim", vec![hull("A", 0, 1000, 50)]);
        let report = contain(&hw, &sim);
        assert!(report.all_covered());
        assert_eq!(report.clauses[0].verdict, ClauseVerdict::Covered);
    }

    #[test]
    fn hw_hull_wider_than_sim_hull_is_uncovered_with_gaps() {
        let hw = envelope("capture", vec![hull("A", 50, 1500, 5)]);
        let sim = envelope("sim", vec![hull("A", 100, 1000, 50)]);
        let report = contain(&hw, &sim);
        assert!(!report.all_covered());
        match report.clauses[0].verdict {
            ClauseVerdict::Uncovered {
                gap_low_ns,
                gap_high_ns,
            } => {
                assert_eq!(gap_low_ns, 50);
                assert_eq!(gap_high_ns, 500);
            }
            other => panic!("expected Uncovered, got {other:?}"),
        }
    }

    #[test]
    fn clause_absent_from_sim_is_unprobed() {
        let hw = envelope("capture", vec![hull("B", 10, 20, 1)]);
        let sim = envelope("sim", vec![hull("A", 0, 1000, 50)]);
        let report = contain(&hw, &sim);
        assert!(!report.all_covered());
        assert_eq!(report.clauses[0].verdict, ClauseVerdict::Unprobed);
    }
}
