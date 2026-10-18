//! 購読モードの最新値の表と、購読の失敗の知らせ（ADR-0006）。
//!
//! 通知のコールバックは async-opcua のイベントループの中で、購読の状態のロックを
//! 持ったまま呼ばれる。そこでは表を短いロックで書き換えるだけにし、待つ処理や
//! `Session` の呼び出しはしない。

use std::sync::atomic::{AtomicBool, Ordering};

use banto_opcua_core::StatusCode;
use opcua::types::{DataValue, StatusCode as UaStatusCode};

use crate::NodeReading;

/// 登録したノード 1 つぶんの行。
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Slot {
    pub(crate) reading: NodeReading,
    /// MonitoredItem の作成に成功したか（成功を見込んで作る前から真にする）。偽の行は
    /// 通知を受けず、登録し直すまで作成の StatusCode のまま。
    pub(crate) created: bool,
}

impl Slot {
    /// MonitoredItem を作る行。`carried` があればそれまでの値を引き継ぎ、無ければ
    /// 最初の通知待ち。
    pub(crate) fn monitoring(carried: Option<NodeReading>) -> Self {
        Self {
            reading: carried.unwrap_or(NodeReading::without_value(
                StatusCode::BAD_WAITING_FOR_INITIAL_DATA,
            )),
            created: true,
        }
    }

    /// MonitoredItem を作らなかった、または作成に失敗した行。
    pub(crate) fn failed(status: StatusCode) -> Self {
        Self {
            reading: NodeReading::without_value(status),
            created: false,
        }
    }
}

/// 登録した並びの位置で引ける最新値の表。
#[derive(Debug, Default)]
pub(crate) struct Table {
    /// いまの登録の番号。購読を作り直すたびに変わり、古い購読の通知はこれで捨てる。
    pub(crate) registration: u64,
    pub(crate) slots: Vec<Slot>,
    /// いまの登録の購読をサーバーが捨てたと知らせた（StatusChangeNotification が Bad）。
    pub(crate) lost: bool,
}

impl Table {
    /// 新しい登録に入れ替える。
    pub(crate) fn replace(&mut self, registration: u64, slots: Vec<Slot>) {
        self.registration = registration;
        self.slots = slots;
        self.lost = false;
    }

    /// 通知を表へ入れる。古い登録の通知、知らない `client_handle`、作成に失敗した行への
    /// 通知は捨てる。`client_handle` は「位置 + 1」。
    pub(crate) fn apply(&mut self, registration: u64, client_handle: u32, value: DataValue) {
        if registration != self.registration {
            return;
        }
        let Some(position) = usize::try_from(client_handle)
            .ok()
            .and_then(|handle| handle.checked_sub(1))
        else {
            return;
        };
        if let Some(slot) = self.slots.get_mut(position)
            && slot.created
        {
            slot.reading = NodeReading::from_data_value(value);
        }
    }

    /// 購読の失効の知らせ。古い登録の知らせは捨てる。
    pub(crate) fn mark_lost(&mut self, registration: u64) {
        if registration == self.registration {
            self.lost = true;
        }
    }

    /// 作成の結果を行へ入れる。Bad の行は通知を受けなくなる。
    pub(crate) fn set_created(&mut self, registration: u64, position: usize, status: StatusCode) {
        if registration != self.registration || !status.is_bad() {
            return;
        }
        if let Some(slot) = self.slots.get_mut(position) {
            *slot = Slot::failed(status);
        }
    }

    pub(crate) fn readings(&self) -> Vec<NodeReading> {
        self.slots.iter().map(|slot| slot.reading.clone()).collect()
    }
}

/// イベントループから見た Publish の失敗。どの購読の失敗かは分からないので、
/// 購読を作り直すときに消す。
#[derive(Debug, Default)]
pub(crate) struct PublishSignals {
    /// 通知を落としたかもしれない。購読を作り直して最新にする。
    failed: AtomicBool,
    /// 購読がサーバーに無い（`BadNoSubscription` / `BadSubscriptionIdInvalid`）。
    lost: AtomicBool,
}

impl PublishSignals {
    /// `PublishFailed` を分ける。`BadTooManyPublishRequests` は、サーバーが Publish の数を
    /// 抑えただけなので数えない。
    pub(crate) fn on_publish_failed(&self, status: UaStatusCode) {
        match status {
            UaStatusCode::BadNoSubscription | UaStatusCode::BadSubscriptionIdInvalid => {
                self.lost.store(true, Ordering::SeqCst);
            }
            UaStatusCode::BadTooManyPublishRequests => {}
            _ => self.failed.store(true, Ordering::SeqCst),
        }
    }

    pub(crate) fn failed(&self) -> bool {
        self.failed.load(Ordering::SeqCst)
    }

    pub(crate) fn lost(&self) -> bool {
        self.lost.load(Ordering::SeqCst)
    }

    /// 購読を作り直す前に消す。
    pub(crate) fn clear(&self) {
        self.failed.store(false, Ordering::SeqCst);
        self.lost.store(false, Ordering::SeqCst);
    }
}

#[cfg(test)]
mod tests {
    use opcua::types::Variant;

    use super::*;
    use banto_opcua_core::ScalarValue;

    fn value(number: f64) -> DataValue {
        DataValue {
            value: Some(Variant::Double(number)),
            ..Default::default()
        }
    }

    fn table(registration: u64) -> Table {
        let mut table = Table::default();
        table.replace(
            registration,
            vec![
                Slot::monitoring(None),
                Slot::failed(StatusCode::BAD_NODE_ID_UNKNOWN),
                Slot::monitoring(None),
            ],
        );
        table
    }

    #[test]
    fn new_rows_wait_for_the_initial_notification() {
        let table = table(1);
        let readings = table.readings();
        assert_eq!(readings[0].status, StatusCode::BAD_WAITING_FOR_INITIAL_DATA);
        assert_eq!(readings[0].value, None);
        assert_eq!(readings[1].status, StatusCode::BAD_NODE_ID_UNKNOWN);
    }

    #[test]
    fn notifications_update_the_row_at_handle_minus_one() {
        let mut table = table(1);
        table.apply(1, 3, value(2.5));
        assert_eq!(table.readings()[2].value, Some(ScalarValue::Double(2.5)));
        assert_eq!(table.readings()[2].status, StatusCode::GOOD);
    }

    #[test]
    fn stale_registration_unknown_handle_and_failed_rows_are_ignored() {
        let mut table = table(2);
        let before = table.readings();
        table.apply(1, 1, value(1.0));
        table.apply(2, 0, value(1.0));
        table.apply(2, 9, value(1.0));
        table.apply(2, 2, value(1.0));
        assert_eq!(table.readings(), before);
    }

    #[test]
    fn failed_creation_keeps_its_status_against_later_notifications() {
        let mut table = table(1);
        table.set_created(1, 2, StatusCode::BAD_TOO_MANY_MONITORED_ITEMS);
        table.set_created(1, 0, StatusCode::GOOD);
        table.apply(1, 3, value(1.0));
        assert_eq!(
            table.readings()[2].status,
            StatusCode::BAD_TOO_MANY_MONITORED_ITEMS
        );
        assert_eq!(
            table.readings()[0].status,
            StatusCode::BAD_WAITING_FOR_INITIAL_DATA
        );
    }

    #[test]
    fn carried_values_stay_until_the_new_registration_notifies() {
        let carried = NodeReading::new(Some(ScalarValue::Double(9.0)), StatusCode::GOOD, None);
        let mut table = Table::default();
        table.replace(5, vec![Slot::monitoring(Some(carried.clone()))]);
        table.apply(4, 1, value(1.0));
        assert_eq!(table.readings(), vec![carried]);
        table.apply(5, 1, value(3.0));
        assert_eq!(table.readings()[0].value, Some(ScalarValue::Double(3.0)));
    }

    #[test]
    fn lost_is_only_for_the_current_registration_and_cleared_on_replace() {
        let mut table = table(3);
        table.mark_lost(2);
        assert!(!table.lost);
        table.mark_lost(3);
        assert!(table.lost);
        table.replace(4, Vec::new());
        assert!(!table.lost);
    }

    #[test]
    fn publish_failures_are_classified() {
        let signals = PublishSignals::default();
        signals.on_publish_failed(UaStatusCode::BadTooManyPublishRequests);
        assert!(!signals.failed() && !signals.lost());
        signals.on_publish_failed(UaStatusCode::BadTimeout);
        assert!(signals.failed() && !signals.lost());
        signals.on_publish_failed(UaStatusCode::BadNoSubscription);
        assert!(signals.lost());
        signals.clear();
        signals.on_publish_failed(UaStatusCode::BadSubscriptionIdInvalid);
        assert!(signals.lost() && !signals.failed());
    }
}
