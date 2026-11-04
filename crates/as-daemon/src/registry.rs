//! Connected clients and event fan-out.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};

use as_proto::{Event, ServerMsg};
use parking_lot::Mutex;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// Pending outgoing messages per client before it is considered stuck and disconnected
/// (a client that reconnects resyncs through `Subscribe`, which is better than a corrupt stream).
pub const OUTGOING_CAPACITY: usize = 8192;

struct Entry {
    tx: mpsc::Sender<ServerMsg>,
    subs: HashSet<String>,
    kick: CancellationToken,
    focused: bool,
}

#[derive(Default)]
pub struct Registry {
    next_id: AtomicU64,
    clients: Mutex<HashMap<u64, Entry>>,
}

impl Registry {
    pub fn add(&self, tx: mpsc::Sender<ServerMsg>, kick: CancellationToken) -> u64 {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed) + 1;
        self.clients.lock().insert(
            id,
            Entry {
                tx,
                subs: HashSet::new(),
                kick,
                focused: false,
            },
        );
        id
    }

    pub fn remove(&self, id: u64) {
        self.clients.lock().remove(&id);
    }

    pub fn len(&self) -> usize {
        self.clients.lock().len()
    }

    pub fn set_focus(&self, client: u64, focused: bool) {
        if let Some(e) = self.clients.lock().get_mut(&client) {
            e.focused = focused;
        }
    }

    /// True while some client reports its terminal as focused.
    pub fn any_focused(&self) -> bool {
        self.clients.lock().values().any(|e| e.focused)
    }

    pub fn subscribe(&self, client: u64, agent_id: &str) {
        if let Some(e) = self.clients.lock().get_mut(&client) {
            e.subs.insert(agent_id.to_string());
        }
    }

    pub fn unsubscribe(&self, client: u64, agent_id: &str) {
        if let Some(e) = self.clients.lock().get_mut(&client) {
            e.subs.remove(agent_id);
        }
    }

    /// Forget a removed agent's subscriptions.
    pub fn drop_agent(&self, agent_id: &str) {
        for e in self.clients.lock().values_mut() {
            e.subs.remove(agent_id);
        }
    }

    /// Send to every client.
    pub fn broadcast(&self, event: &Event) {
        let mut clients = self.clients.lock();
        for (id, entry) in clients.iter_mut() {
            push(*id, entry, event.clone());
        }
    }

    /// Send PTY output to the clients subscribed to `agent_id`.
    pub fn output(&self, agent_id: &str, data: &[u8]) {
        let mut clients = self.clients.lock();
        for (id, entry) in clients
            .iter_mut()
            .filter(|(_, e)| e.subs.contains(agent_id))
        {
            push(
                *id,
                entry,
                Event::AgentOutput {
                    agent_id: agent_id.to_string(),
                    data: data.to_vec(),
                },
            );
        }
    }
}

fn push(id: u64, entry: &mut Entry, event: Event) {
    if entry.tx.try_send(ServerMsg::Event(event)).is_err() && !entry.kick.is_cancelled() {
        // Full (stuck client) or closed (going away).
        if !entry.tx.is_closed() {
            tracing::warn!(client = id, "client too slow, disconnecting");
        }
        entry.kick.cancel();
    }
}
