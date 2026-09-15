# Data Files & Fixtures

Guide to test data, golden values, fixtures, and reference data in StableHAL.

---

## Test Fixtures

| File | Purpose | Format | Usage |
|------|---------|--------|-------|
| `crates/*/tests/fixtures/*.ron` | Hardware capture dumps | RON (Rust Object Notation) | Deterministic replay tests |
| `crates/*/tests/fixtures/golden_*.json` | Expected golden outputs | JSON (hash, length, data) | Comparison for captured hardware |
| `crates/*/benches/*.rs` | Benchmark definitions | Rust code | Performance regression detection |

---

## Golden Values

**Protocol Conformance (G3 Rule: Positive + Negative Cases):**

**MODBUS RTU:**
- **Valid write (Function 03)**: `01 03 0000 0002 ...` → response contains data
- **Invalid CRC**: `01 03 0000 0002 BAAD` → rejected, no response (timeout)
- **Invalid function**: `01 FF ...` → exception response (exception code 1)
- **Timeout threshold**: 500ms (defined in `crates/modbus/src/constants.rs`)

**I2C:**
- **Valid address scan**: 7-bit address 0x50 → ACK if slave present
- **Invalid address**: 0x7F (reserved) → NACK
- **Clock cycle**: 10µs @ 100 kHz (defined in `crates/i2c/src/constants.rs`)
- **SDA hold time**: 0µs (I2C spec minimum)

**CAN:**
- **Standard frame**: ID 0x123, 8 bytes data → accepted
- **Extended frame**: ID 0x18DA1234, 64 bytes (CAN FD) → accepted if FD enabled
- **Invalid DLC**: Data length > frame type allows → rejected

**LIN:**
- **Master frame spacing**: Min 40ms between frames (baud-dependent)
- **Slave response delay**: 0–10ms after header (configurable)
- **Break field**: 13 bits @ configured baud rate

**SPI:**
- **Clock polarity (CPOL)**: 0 (default), clock low at idle
- **Clock phase (CPHA)**: 0 (default), sample on leading edge
- **Chip select timing**: Min 100ns after clock stop

---

## How to Use Test Fixtures

### Capture Hardware Response (New Conformance Case)
```bash
# 1. Connect real device/protocol analyzer
# 2. Run capture script
./scripts/generate-golden.sh modbus

# 3. Script captures response, saves to crates/modbus/tests/fixtures/capture_*.ron
# 4. Manually verify output looks correct
# 5. Convert to golden value
python3 scripts/capture_to_golden.py crates/modbus/tests/fixtures/capture_*.ron

# 6. Golden is now in crates/modbus/tests/fixtures/golden_*.json
```

### Run Deterministic Replay Test
```bash
# Uses captured .ron file to replay exactly
cargo test --test conformance_replay

# Each test runs simulation with captured trace, compares outputs
```

### Add New Protocol Conformance Clause
```rust
// In crates/modbus/tests/conformance_test.rs
#[test]
fn test_modbus_function_03_positive() {
    // Positive case: valid read holding registers
    let request = ModbusRequest::read_holding_registers(0x01, 0x0000, 0x0002);
    let expected = load_golden_value("golden_modbus_fn03.json");
    assert_eq!(execute(&request), expected);
}

#[test]
fn test_modbus_function_03_negative() {
    // Negative case: invalid CRC
    let request = ModbusRequest::read_holding_registers_bad_crc(0x01);
    let result = execute(&request);
    assert!(result.is_err());  // Should fail
}
```

---

## Benchmark Data

**Hardware Baseline Timings:**
- **MODBUS RTU command-response**: ~50ms (typical ELM327)
- **I2C address scan**: ~10ms (8 bits × ~1.25µs per bit @ 100 kHz)
- **CAN frame reception**: ~0.2ms (1 Mbps baud rate)

**Simulation Target Accuracy:**
- Simulation must match hardware ±10% for timing-sensitive tests
- See `crates/*/benches/compare_sim_vs_hw.rs` for regression detection

---

## Test Data Structure

### Conformance Test Format
```rust
pub struct ConformanceCase {
    pub name: &str,
    pub description: &str,
    pub positive: bool,  // G3 rule: both required
    pub input: Vec<u8>,
    pub expected_output: Vec<u8>,
    pub timeout_ms: u32,
}
```

### Golden Value Format (JSON)
```json
{
  "protocol": "modbus",
  "function_code": 3,
  "slave_id": 1,
  "register_address": 0,
  "register_count": 2,
  "positive": true,
  "captured_at": "2026-09-15T10:30:00Z",
  "hardware_info": "ELM327 v1.5",
  "response_bytes": "01 03 04 00 50 00 60 ...",
  "response_hash": "abc123..."
}
```

---

## Reference Protocols

**MODBUS RTU (crates/modbus/):**
- Baud rate: 9600 typical
- Data format: 8N1 (8 bits, no parity, 1 stop bit)
- Frame format: Slave ID + Function + Data + CRC

**I2C (crates/i2c/):**
- Clock speed: 100 kHz, 400 kHz, 1 MHz
- Addressing: 7-bit or 10-bit slave addresses
- Protocol: Start bit, address, R/W, ACK, data, ACK, stop

**CAN (crates/can/):**
- Standard: 11-bit ID, 8 bytes data
- Extended (CAN FD): 29-bit ID, up to 64 bytes
- Baud rates: 250 kbps, 500 kbps, 1 Mbps typical

**LIN (crates/lin/):**
- Single-wire protocol
- Master/slave architecture
- Baud rate: 9.6 kbps typical

**SPI (crates/spi/):**
- Full-duplex synchronous
- Clock polarity (CPOL), clock phase (CPHA)
- Chip select (CS) active low by default

---

**Last updated:** 2026-09-15
