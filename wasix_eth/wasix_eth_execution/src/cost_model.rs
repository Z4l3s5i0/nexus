use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

const EWMA_OLD_WEIGHT: u128 = 7;
const EWMA_NEW_WEIGHT: u128 = 3;
const EWMA_DENOMINATOR: u128 = EWMA_OLD_WEIGHT + EWMA_NEW_WEIGHT;
const DEFAULT_US_PER_TX: u64 = 10_000;
const DEFAULT_FIXED_US: u64 = 100_000;

pub struct BuildCostModel {
    us_per_tx: AtomicU64,
    fixed_us: AtomicU64,
}

impl Default for BuildCostModel {
    fn default() -> Self {
        Self {
            us_per_tx: AtomicU64::new(DEFAULT_US_PER_TX),
            fixed_us: AtomicU64::new(DEFAULT_FIXED_US),
        }
    }
}

impl BuildCostModel {
    pub fn record(&self, tx_count: usize, finalize: Duration) {
        let finalize_us = finalize.as_micros().min(u64::MAX as u128) as u64;
        if tx_count == 0 {
            update_ewma(&self.fixed_us, finalize_us);
            return;
        }

        let fixed_us = self.fixed_us.load(Ordering::Relaxed);
        let per_tx_us = finalize_us.saturating_sub(fixed_us) / tx_count.min(u64::MAX as usize) as u64;
        update_ewma(&self.us_per_tx, per_tx_us.max(1));
    }

    pub fn estimate_finalize(&self, tx_count: usize) -> Duration {
        let fixed_us = self.fixed_us.load(Ordering::Relaxed) as u128;
        let us_per_tx = self.us_per_tx.load(Ordering::Relaxed) as u128;
        let total_us = fixed_us.saturating_add(us_per_tx.saturating_mul(tx_count as u128));
        Duration::from_micros(total_us.min(u64::MAX as u128) as u64)
    }

    pub fn max_txs_for(&self, budget: Duration) -> usize {
        let budget_us = budget.as_micros();
        let fixed_us = self.fixed_us.load(Ordering::Relaxed) as u128;
        if budget_us <= fixed_us {
            return 0;
        }

        let us_per_tx = self.us_per_tx.load(Ordering::Relaxed).max(1) as u128;
        let max_txs = ((budget_us - fixed_us) / us_per_tx).min(usize::MAX as u128) as usize;
        max_txs.max(1)
    }
}

fn update_ewma(value: &AtomicU64, sample: u64) {
    let mut previous = value.load(Ordering::Relaxed);
    loop {
        let next = if previous == 0 {
            sample
        } else {
            ((previous as u128 * EWMA_OLD_WEIGHT + sample as u128 * EWMA_NEW_WEIGHT)
                / EWMA_DENOMINATOR) as u64
        };

        match value.compare_exchange_weak(previous, next, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => return,
            Err(actual) => previous = actual,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::BuildCostModel;
    use std::time::Duration;

    #[test]
    fn ewma_converges_toward_measured_finalize_cost() {
        let model = BuildCostModel::default();
        for _ in 0..20 {
            model.record(100, Duration::from_millis(2100));
        }

        let estimate = model.estimate_finalize(100);
        assert!(estimate >= Duration::from_millis(2000));
        assert!(estimate <= Duration::from_millis(2200));
    }

    #[test]
    fn max_txs_is_monotonic_and_keeps_a_one_transaction_floor() {
        let model = BuildCostModel::default();
        let small = model.max_txs_for(Duration::from_secs(1));
        let large = model.max_txs_for(Duration::from_secs(2));

        assert!(small >= 1);
        assert!(large >= small);
        assert_eq!(model.max_txs_for(Duration::ZERO), 0);
    }
}