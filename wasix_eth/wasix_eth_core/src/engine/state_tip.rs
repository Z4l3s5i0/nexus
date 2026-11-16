use alloy_primitives::B256;
use tokio::sync::{Mutex, MutexGuard};

pub struct StateTip {
    inner: Mutex<(B256, u64)>,
}

impl StateTip {
    pub fn new(hash: B256, number: u64) -> Self {
        Self {
            inner: Mutex::new((hash, number)),
        }
    }

    pub async fn lock(&self) -> MutexGuard<'_, (B256, u64)> {
        self.inner.lock().await
    }

    pub async fn get(&self) -> (B256, u64) {
        *self.inner.lock().await
    }
}

#[cfg(test)]
mod tests {
    use super::StateTip;
    use alloy_primitives::B256;

    #[tokio::test]
    async fn state_tip_can_be_read_and_advanced_under_lock() {
        let tip = StateTip::new(B256::repeat_byte(1), 7);
        assert_eq!(tip.get().await, (B256::repeat_byte(1), 7));

        *tip.lock().await = (B256::repeat_byte(2), 8);
        assert_eq!(tip.get().await, (B256::repeat_byte(2), 8));
    }
}