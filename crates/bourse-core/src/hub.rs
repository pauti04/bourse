//! Multi-tenant matching hub.
//!
//! Many concurrent gateways (typically TCP connections) feed a *single*
//! matcher thread through one shared lock-free MPSC queue
//! (`crossbeam_queue::ArrayQueue`). Each gateway has its own outbound
//! [`tokio::sync::mpsc::UnboundedSender<Event>`] for event delivery.
//! The matcher thread maintains the connection-id → sender map locally,
//! so the per-event dispatch is a single `HashMap` lookup with no
//! cross-thread sync.
//!
//! Compared to the per-connection [`crate::engine::Engine`] this lifts
//! the v1 "one connection per matcher" limit. The trade-off: producers
//! contend on a single MPSC tail (CAS rather than the SPSC's
//! single-writer write), so high-fanout setups will see slightly worse
//! per-message overhead than the single-connection case.

#![allow(
    clippy::expect_used,
    reason = "thread spawn / join failures are unrecoverable; matches std::thread"
)]

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread::{self, JoinHandle};

use crossbeam_queue::ArrayQueue;
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};

use crate::matcher::{Event, Matcher, NewOrder};
use crate::types::OrderId;

/// Unique connection id assigned by the hub at registration time.
pub type ConnId = u64;

/// What a gateway sends through the hub.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    /// Submit a new order on a connection.
    New(NewOrder),
    /// Cancel a resting order by id.
    Cancel(OrderId),
}

/// Internal multiplex over the shared MPSC.
#[derive(Debug)]
enum Msg {
    Connect(ConnId, UnboundedSender<Event>),
    Disconnect(ConnId),
    Cmd(ConnId, Command),
}

/// Multi-tenant matching hub. One matcher thread, many producers.
#[derive(Debug)]
pub struct Hub {
    inbox: Arc<ArrayQueue<Msg>>,
    next_conn: AtomicU64,
    stop_flag: Arc<AtomicBool>,
    handle: Option<JoinHandle<Matcher>>,
}

/// Submit handle for a single gateway / TCP connection. Cheap to
/// clone — multiple tasks on the same connection (e.g. reader and a
/// keepalive ticker) can share one. Drop sends a `Disconnect` to the
/// matcher.
#[derive(Debug, Clone)]
pub struct Submitter {
    inner: Arc<SubmitterInner>,
}

#[derive(Debug)]
struct SubmitterInner {
    conn_id: ConnId,
    inbox: Arc<ArrayQueue<Msg>>,
}

impl Submitter {
    /// This tenant's id.
    pub fn conn_id(&self) -> ConnId {
        self.inner.conn_id
    }

    /// Submit a command. Spins on `inbox` full.
    pub fn submit(&self, cmd: Command) {
        let mut msg = Msg::Cmd(self.inner.conn_id, cmd);
        while let Err(returned) = self.inner.inbox.push(msg) {
            msg = returned;
            std::hint::spin_loop();
        }
    }
}

impl Drop for SubmitterInner {
    fn drop(&mut self) {
        // Best-effort: tell the matcher we're gone so it drops the
        // sender entry. If the inbox is full the matcher will notice
        // the closed sender on the next dispatch attempt anyway.
        let _ = self.inbox.push(Msg::Disconnect(self.conn_id));
    }
}

impl Hub {
    /// Spawn the matcher thread. `inbox_capacity` bounds the shared
    /// MPSC; producers will spin on full.
    pub fn start(inbox_capacity: usize) -> Self {
        let inbox = Arc::new(ArrayQueue::new(inbox_capacity.max(8)));
        let stop_flag = Arc::new(AtomicBool::new(false));

        let inbox_for_thread = Arc::clone(&inbox);
        let stop_for_thread = Arc::clone(&stop_flag);
        let handle = thread::Builder::new()
            .name("bourse-matcher".into())
            .spawn(move || matcher_loop(inbox_for_thread, stop_for_thread))
            .expect("spawn matcher thread");

        Self {
            inbox,
            next_conn: AtomicU64::new(1),
            stop_flag,
            handle: Some(handle),
        }
    }

    /// Register a new tenant. Returns `(submitter, events)`. The
    /// reader task on the gateway drives `submitter.submit(cmd)`; the
    /// writer task drains `events`. Drop of the submitter (after both
    /// tasks exit) tells the matcher to forget this tenant.
    pub fn register(&self) -> (Submitter, UnboundedReceiver<Event>) {
        let conn_id = self.next_conn.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = mpsc::unbounded_channel::<Event>();
        let mut msg = Msg::Connect(conn_id, tx);
        while let Err(returned) = self.inbox.push(msg) {
            msg = returned;
            std::hint::spin_loop();
        }
        let submitter = Submitter {
            inner: Arc::new(SubmitterInner {
                conn_id,
                inbox: Arc::clone(&self.inbox),
            }),
        };
        (submitter, rx)
    }

    /// Stop the matcher thread, return the final [`Matcher`].
    pub fn stop(mut self) -> Matcher {
        self.stop_flag.store(true, Ordering::Release);
        self.handle
            .take()
            .expect("hub stopped twice")
            .join()
            .expect("matcher thread panicked")
    }
}

impl Drop for Hub {
    fn drop(&mut self) {
        if let Some(handle) = self.handle.take() {
            self.stop_flag.store(true, Ordering::Release);
            let _ = handle.join();
        }
    }
}

fn matcher_loop(inbox: Arc<ArrayQueue<Msg>>, stop: Arc<AtomicBool>) -> Matcher {
    let mut matcher = Matcher::new();
    let mut routes: HashMap<ConnId, UnboundedSender<Event>> = HashMap::new();
    let mut events: Vec<Event> = Vec::with_capacity(16);
    // Track which conn owns which order id, so cancel events route home.
    let mut order_owner: HashMap<OrderId, ConnId> = HashMap::new();

    loop {
        let mut did_work = false;
        while let Some(msg) = inbox.pop() {
            did_work = true;
            match msg {
                Msg::Connect(id, tx) => {
                    routes.insert(id, tx);
                }
                Msg::Disconnect(id) => {
                    routes.remove(&id);
                    // Don't bother purging order_owner — pending events
                    // for the dropped sender will fail to send and be
                    // ignored on the next pass, see below.
                }
                Msg::Cmd(conn_id, cmd) => {
                    events.clear();
                    match cmd {
                        Command::New(no) => {
                            order_owner.insert(no.id, conn_id);
                            matcher.accept(no, &mut events);
                        }
                        Command::Cancel(id) => matcher.cancel(id, &mut events),
                    }
                    for e in events.drain(..) {
                        match e {
                            Event::Accepted { id, .. } | Event::Done { id, .. } => {
                                let target = order_owner.get(&id).copied().unwrap_or(conn_id);
                                if let Some(tx) = routes.get(&target) {
                                    let _ = tx.send(e);
                                }
                            }
                            Event::Trade { taker, maker, .. } => {
                                // Trade goes to both sides. Dedupe if
                                // the same connection owns both.
                                let taker_conn =
                                    order_owner.get(&taker).copied().unwrap_or(conn_id);
                                let maker_conn =
                                    order_owner.get(&maker).copied().unwrap_or(conn_id);
                                if let Some(tx) = routes.get(&taker_conn) {
                                    let _ = tx.send(e);
                                }
                                if maker_conn != taker_conn
                                    && let Some(tx) = routes.get(&maker_conn)
                                {
                                    let _ = tx.send(e);
                                }
                            }
                        }
                        if let Event::Done { id, .. } = e {
                            order_owner.remove(&id);
                        }
                    }
                }
            }
        }
        if !did_work {
            if stop.load(Ordering::Acquire) {
                break;
            }
            std::hint::spin_loop();
        }
    }
    matcher
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::panic,
        clippy::unwrap_used,
        clippy::expect_used,
        reason = "test setup"
    )]

    use super::*;
    use crate::matcher::{DoneReason, OrderKind};
    use crate::types::{Price, Qty, Side, Timestamp};

    fn limit(id: u64, side: Side, price: i64, qty: u64) -> Command {
        Command::New(NewOrder {
            id: OrderId::new(id),
            side,
            qty: Qty::new(qty),
            kind: OrderKind::Limit {
                price: Price::from_raw(price),
            },
            timestamp: Timestamp::EPOCH,
        })
    }

    async fn wait_for<F>(rx: &mut UnboundedReceiver<Event>, mut pred: F) -> Event
    where
        F: FnMut(&Event) -> bool,
    {
        loop {
            let e = rx.recv().await.expect("tenant disconnected");
            if pred(&e) {
                return e;
            }
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn two_tenants_each_get_their_own_events() {
        let hub = Hub::start(64);
        let (a_sub, mut a_evt) = hub.register();
        let (b_sub, mut b_evt) = hub.register();
        assert_ne!(a_sub.conn_id(), b_sub.conn_id());

        // A rests a Sell.
        a_sub.submit(limit(1, Side::Sell, 100, 1));
        // B crosses with a Buy → trade.
        b_sub.submit(limit(2, Side::Buy, 100, 1));

        // A sees Accepted(1), Trade, Done(1, Filled).
        let _ = wait_for(&mut a_evt, |e| matches!(e, Event::Accepted { .. })).await;
        let _ = wait_for(&mut a_evt, |e| matches!(e, Event::Trade { .. })).await;
        let _ = wait_for(
            &mut a_evt,
            |e| matches!(e, Event::Done { id, reason: DoneReason::Filled, .. } if *id == OrderId::new(1)),
        )
        .await;

        // B sees Accepted(2), Trade, Done(2, Filled).
        let _ = wait_for(&mut b_evt, |e| matches!(e, Event::Accepted { .. })).await;
        let _ = wait_for(&mut b_evt, |e| matches!(e, Event::Trade { .. })).await;
        let _ = wait_for(
            &mut b_evt,
            |e| matches!(e, Event::Done { id, reason: DoneReason::Filled, .. } if *id == OrderId::new(2)),
        )
        .await;

        drop(a_sub);
        drop(b_sub);
        let m = hub.stop();
        assert!(m.book().is_empty());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn three_tenants_concurrent_load() {
        let hub = Hub::start(1024);
        let mut subs = Vec::new();
        let mut rxs = Vec::new();
        for _ in 0..3 {
            let (s, r) = hub.register();
            subs.push(s);
            rxs.push(r);
        }

        // Each tenant submits 100 sells at distinct prices that won't
        // cross each other.
        for (i, s) in subs.iter().enumerate() {
            for j in 0..100u64 {
                let price = 100 + (i as i64) * 100 + (j as i64 % 50);
                let id = (i as u64) * 1_000 + j + 1;
                s.submit(limit(id, Side::Sell, price, 1));
            }
        }

        // Each tenant should see exactly 100 Accepted events.
        for rx in rxs.iter_mut() {
            let mut seen = 0;
            while seen < 100 {
                let e = rx.recv().await.expect("recv");
                if matches!(e, Event::Accepted { .. }) {
                    seen += 1;
                }
            }
        }

        drop(subs);
        let m = hub.stop();
        assert_eq!(m.book().len(), 300);
    }
}
