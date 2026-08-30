use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReconciliationRecord {
    pub operation_id: u64,
    pub failure: String,
    pub rollback_failures: Vec<String>,
}

#[derive(Default)]
pub struct PlayerReconciliations {
    next_operation_id: AtomicU64,
    required: Mutex<HashMap<String, ReconciliationRecord>>,
}

pub fn mark_reconciliation_required(
    reconciliations: &PlayerReconciliations,
    player_id: &str,
    failure: String,
    rollback_failures: Vec<String>,
) -> ReconciliationRecord {
    let operation_id = reconciliations
        .next_operation_id
        .fetch_add(1, Ordering::Relaxed)
        .saturating_add(1);
    let record = ReconciliationRecord {
        operation_id,
        failure,
        rollback_failures,
    };
    reconciliations
        .required
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .entry(player_id.to_owned())
        .or_insert_with(|| record.clone())
        .clone()
}

pub fn reconciliation_required(
    reconciliations: &PlayerReconciliations,
    player_id: &str,
) -> Option<ReconciliationRecord> {
    reconciliations
        .required
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get(player_id)
        .cloned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quarantine_is_scoped_and_preserves_the_first_failure() {
        let reconciliations = PlayerReconciliations::default();
        let first = mark_reconciliation_required(
            &reconciliations,
            "first",
            "commit failed".to_owned(),
            vec!["rollback failed".to_owned()],
        );
        let repeated = mark_reconciliation_required(
            &reconciliations,
            "first",
            "later failure".to_owned(),
            Vec::new(),
        );

        assert_eq!(repeated, first);
        assert_eq!(
            reconciliation_required(&reconciliations, "first"),
            Some(first)
        );
        assert_eq!(reconciliation_required(&reconciliations, "second"), None);
    }
}
