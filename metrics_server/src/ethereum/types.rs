use anyhow::{Result, anyhow};
use serde_json::Value;

pub fn parse_hex_u64(value: &Value) -> Result<u64> {
    match value {
        Value::String(s) => {
            let clean = s.strip_prefix("0x").unwrap_or(s);
            u64::from_str_radix(clean, 16).map_err(|e| anyhow!("failed to parse hex u64: {}", e))
        }
        Value::Number(n) => {
            n.as_u64().ok_or_else(|| anyhow!("failed to parse number as u64"))
        }
        _ => Err(anyhow!("expected string or number for hex u64, got {:?}", value))
    }
}

pub fn parse_hex_f64(value: &Value) -> Result<f64> {
    match value {
        Value::String(s) => {
            let clean = s.strip_prefix("0x").unwrap_or(s);
            u64::from_str_radix(clean, 16).map(|v| v as f64).map_err(|e| anyhow!("failed to parse hex f64: {}", e))
        }
        Value::Number(n) => {
            n.as_f64().ok_or_else(|| anyhow!("failed to parse number as f64"))
        }
        _ => Err(anyhow!("expected string or number for hex f64, got {:?}", value))
    }
}

pub fn syncing_to_numeric(value: &Value) -> Result<f64> {
    match value {
        Value::Bool(b) => Ok(if *b { 1.0 } else { 0.0 }),
        Value::Object(_) => Ok(1.0),
        _ => Err(anyhow!("unexpected eth_syncing result: {:?}", value)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn syncing_results_do_not_treat_missing_data_as_synced() {
        assert_eq!(syncing_to_numeric(&json!(false)).unwrap(), 0.0);
        assert_eq!(syncing_to_numeric(&json!(true)).unwrap(), 1.0);
        assert_eq!(syncing_to_numeric(&json!({"currentBlock": "0x1"})).unwrap(), 1.0);
        assert!(syncing_to_numeric(&json!(null)).is_err());
    }
}
