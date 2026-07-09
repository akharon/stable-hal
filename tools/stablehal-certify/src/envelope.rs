//! `stablehal-envelope-v1`: the per-clause timing hull format shared by the
//! simulator-explored envelope and hardware-measured envelope, so
//! `certify contain` can compare them structurally instead of re-deriving
//! bounds from two different code paths. Hand-rolled JSON (G1: zero
//! third-party deps).

use std::fs;

use stablehal_contract::{ClauseHull, EnvelopeAccumulator};

pub struct Envelope {
    pub contract: String,
    pub source: String,
    pub traces: u64,
    pub clauses: Vec<ClauseHull>,
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

impl Envelope {
    pub fn from_accumulator<const N: usize>(
        contract: &str,
        source: &str,
        traces: u64,
        acc: &EnvelopeAccumulator<N>,
    ) -> Self {
        Envelope {
            contract: contract.to_string(),
            source: source.to_string(),
            traces,
            clauses: acc.hulls().copied().collect(),
        }
    }

    pub fn to_json(&self) -> String {
        let mut clauses_json = String::new();
        for (i, c) in self.clauses.iter().enumerate() {
            if i > 0 {
                clauses_json.push_str(",\n");
            }
            clauses_json.push_str(&format!(
                "    {{ \"id\": \"{}\", \"min_ns\": {}, \"max_ns\": {}, \"samples\": {} }}",
                json_escape(c.clause_id),
                c.min.as_nanos(),
                c.max.as_nanos(),
                c.samples,
            ));
        }
        format!(
            "{{\n  \"format\": \"stablehal-envelope-v1\",\n  \"contract\": \"{}\",\n  \"source\": \"{}\",\n  \"traces\": {},\n  \"clauses\": [\n{}\n  ]\n}}\n",
            json_escape(&self.contract),
            json_escape(&self.source),
            self.traces,
            clauses_json,
        )
    }

    /// Parse an envelope JSON file written by [`Envelope::to_json`]. Minimal
    /// hand-rolled parser: it only needs to round-trip our own writer's
    /// output, not arbitrary JSON (G1).
    pub fn read(path: &str) -> Result<Self, String> {
        let text = fs::read_to_string(path).map_err(|e| format!("failed to read {path}: {e}"))?;
        Self::parse(&text)
    }

    pub fn parse(text: &str) -> Result<Self, String> {
        let contract = extract_string(text, "\"contract\"")
            .ok_or_else(|| "missing \"contract\"".to_string())?;
        let source =
            extract_string(text, "\"source\"").ok_or_else(|| "missing \"source\"".to_string())?;
        let traces =
            extract_number(text, "\"traces\"").ok_or_else(|| "missing \"traces\"".to_string())?;

        let mut clauses = Vec::new();
        let clauses_start = text
            .find("\"clauses\"")
            .ok_or_else(|| "missing \"clauses\"".to_string())?;
        let body = &text[clauses_start..];
        // Each clause object is `{ ... }`; scan them one at a time.
        let mut rest = body;
        while let Some(open) = rest.find('{') {
            let close = rest[open..]
                .find('}')
                .ok_or_else(|| "unterminated clause object".to_string())?
                + open;
            let obj = &rest[open..=close];
            let id =
                extract_string(obj, "\"id\"").ok_or_else(|| "clause missing \"id\"".to_string())?;
            let min_ns = extract_number(obj, "\"min_ns\"")
                .ok_or_else(|| "clause missing \"min_ns\"".to_string())?;
            let max_ns = extract_number(obj, "\"max_ns\"")
                .ok_or_else(|| "clause missing \"max_ns\"".to_string())?;
            let samples = extract_number(obj, "\"samples\"")
                .ok_or_else(|| "clause missing \"samples\"".to_string())?;
            clauses.push(ClauseHull {
                // Leak: clause ids are used as `&'static str` throughout the
                // contract kernel; a parsed envelope file's ids only need to
                // live for the process lifetime, same tradeoff as any other
                // long-lived host-side interning.
                clause_id: Box::leak(id.into_boxed_str()),
                min: stablehal_contract::Nanos::from_nanos(min_ns),
                max: stablehal_contract::Nanos::from_nanos(max_ns),
                samples,
            });
            rest = &rest[close + 1..];
            // Stop once we've left the `"clauses": [ ... ]` array — the
            // next top-level key (if any) would otherwise be misread as
            // another clause object.
            if rest.trim_start().starts_with(']') {
                break;
            }
        }

        Ok(Envelope {
            contract,
            source,
            traces,
            clauses,
        })
    }
}

fn extract_string(text: &str, key: &str) -> Option<String> {
    let pos = text.find(key)?;
    let after_key = &text[pos + key.len()..];
    let colon = after_key.find(':')?;
    let after_colon = after_key[colon + 1..].trim_start();
    let rest = after_colon.strip_prefix('"')?;
    let end = rest.find('"')?;
    Some(rest[..end].replace("\\\"", "\"").replace("\\\\", "\\"))
}

fn extract_number(text: &str, key: &str) -> Option<u64> {
    let pos = text.find(key)?;
    let after_key = &text[pos + key.len()..];
    let colon = after_key.find(':')?;
    let after_colon = after_key[colon + 1..].trim_start();
    let end = after_colon
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(after_colon.len());
    after_colon[..end].parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use stablehal_contract::{Measurement, Nanos};

    #[test]
    fn round_trips_through_json() {
        let mut acc: EnvelopeAccumulator<4> = EnvelopeAccumulator::new();
        acc.observe(Measurement {
            clause_id: "RTU-T35-FRAME",
            value: Nanos::from_nanos(100),
        });
        acc.observe(Measurement {
            clause_id: "RTU-T35-FRAME",
            value: Nanos::from_nanos(200),
        });
        acc.observe(Measurement {
            clause_id: "RTU-T15-INTRA",
            value: Nanos::from_nanos(50),
        });
        let env = Envelope::from_accumulator("modbus-rtu", "sim", 5, &acc);
        let json = env.to_json();
        let parsed = Envelope::parse(&json).unwrap();

        assert_eq!(parsed.contract, "modbus-rtu");
        assert_eq!(parsed.source, "sim");
        assert_eq!(parsed.traces, 5);
        assert_eq!(parsed.clauses.len(), 2);
        let t35 = parsed
            .clauses
            .iter()
            .find(|c| c.clause_id == "RTU-T35-FRAME")
            .unwrap();
        assert_eq!(t35.min, Nanos::from_nanos(100));
        assert_eq!(t35.max, Nanos::from_nanos(200));
        assert_eq!(t35.samples, 2);
    }
}
