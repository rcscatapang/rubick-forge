//! The event bus: one publish path that both persists and fans out.
//!
//! Nothing else may write to the `events` table, because an event that reaches
//! history without reaching subscribers is a bug the UI shows as a stall.

use forge_core::{EventRecord, ForgeEvent};
use tokio::sync::broadcast;

use crate::store::{append_event_on, Store, StoreError};

/// Room for a burst of events before a slow subscriber starts missing them.
/// A subscriber that lags is expected to refill from `GET /events`.
const CAPACITY: usize = 1024;

#[derive(Clone)]
pub struct Bus {
    store: Store,
    sender: broadcast::Sender<EventRecord>,
}

impl Bus {
    pub fn new(store: Store) -> Self {
        Self {
            store,
            sender: broadcast::Sender::new(CAPACITY),
        }
    }

    /// Persist `event`, then hand it to every live subscriber.
    ///
    /// Both halves happen under the store lock, so subscribers see ids in the
    /// same order the database assigned them.
    pub fn publish(&self, event: ForgeEvent) -> Result<EventRecord, StoreError> {
        self.store.with(|conn| {
            let record = append_event_on(conn, &event)?;
            // An error here only means nobody is listening.
            let _ = self.sender.send(record.clone());
            Ok(record)
        })
    }

    /// Receive every event published from now on.
    pub fn subscribe(&self) -> broadcast::Receiver<EventRecord> {
        self.sender.subscribe()
    }

    /// How many live subscribers there are, so a caller can wait until a
    /// subscription really is in place rather than guessing.
    pub fn subscriber_count(&self) -> usize {
        self.sender.receiver_count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::broadcast::error::TryRecvError;

    use crate::store::Query;

    /// A bus and a second handle on the same database, for asserting that
    /// what was broadcast also landed in history.
    fn bus() -> (Bus, Store) {
        let store = Store::open_in_memory().unwrap();
        (Bus::new(store.clone()), store)
    }

    fn history(store: &Store) -> Vec<forge_core::EventRecord> {
        store.events(Query::default()).unwrap().events
    }

    fn event(project_id: i64) -> ForgeEvent {
        ForgeEvent::ProjectRegistered {
            project_id,
            name: format!("p{project_id}"),
            path: format!("/repos/p{project_id}"),
        }
    }

    #[test]
    fn publishing_both_persists_and_broadcasts() {
        let (bus, store) = bus();
        let mut subscriber = bus.subscribe();

        let published = bus.publish(event(1)).unwrap();

        assert_eq!(subscriber.try_recv().unwrap(), published);
        assert_eq!(history(&store), [published]);
    }

    #[test]
    fn subscribers_only_see_what_follows_them() {
        let (bus, _store) = bus();
        bus.publish(event(1)).unwrap();

        let mut subscriber = bus.subscribe();
        assert!(matches!(subscriber.try_recv(), Err(TryRecvError::Empty)));

        let later = bus.publish(event(2)).unwrap();
        assert_eq!(subscriber.try_recv().unwrap(), later);
    }

    #[test]
    fn every_subscriber_gets_every_event() {
        let (bus, _store) = bus();
        let mut first = bus.subscribe();
        let mut second = bus.subscribe();

        let published = bus.publish(event(1)).unwrap();

        assert_eq!(first.try_recv().unwrap(), published);
        assert_eq!(second.try_recv().unwrap(), published);
    }

    #[test]
    fn broadcast_ids_arrive_in_the_order_the_database_assigned() {
        let (bus, _store) = bus();
        let mut subscriber = bus.subscribe();

        for id in 1..=10 {
            bus.publish(event(id)).unwrap();
        }

        let received: Vec<i64> = (0..10).map(|_| subscriber.try_recv().unwrap().id).collect();
        let mut sorted = received.clone();
        sorted.sort_unstable();
        assert_eq!(received, sorted);
    }

    #[test]
    fn publishing_with_nobody_listening_still_records_history() {
        let (bus, store) = bus();
        bus.publish(event(1)).unwrap();

        assert_eq!(history(&store).len(), 1);
    }
}
