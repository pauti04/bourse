//! End-to-end engine: gateway thread → SPSC → matcher thread → SPSC → consumer.
//!
//! `Engine::start` allocates two lock-free SPSC queues (commands in,
//! events out) and spawns the matcher on a dedicated thread. The matcher
//! loop polls the command queue, processes through [`Matcher::accept`]
//! / [`Matcher::cancel`], and pushes emitted events onto the output
//! queue. It busy-spins when both queues are quiet — this is the low-
//! latency configuration; production would park instead.
//!
//! Shutdown is by an `AtomicBool` flag on the engine. The matcher
//! checks it after every empty input poll, drains anything that raced
//! in, and exits returning the final [`Matcher`].

#![allow(
    clippy::expect_used,
    reason = "thread spawn and join failures are unrecoverable; surfacing them as panics matches std::thread's own contract"
)]

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};

use crate::matcher::{Event, Matcher, NewOrder};
use crate::spsc::{self, Consumer, Producer};
use crate::types::OrderId;

/// What the gateway sends to the matcher.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    /// Submit a new order.
    New(NewOrder),
    /// Cancel a resting order by id.
    Cancel(OrderId),
}

/// A running matcher engine. The matcher loop runs on its own OS thread.
pub struct Engine {
    input: Producer<Command>,
    events: Consumer<Event>,
    stop_flag: Arc<AtomicBool>,
    handle: Option<JoinHandle<Matcher>>,
}

/// Stop handle returned alongside the producer/consumer halves by
/// [`Engine::split`].
pub struct EngineHandle {
    stop_flag: Arc<AtomicBool>,
    handle: JoinHandle<Matcher>,
}

impl EngineHandle {
    /// Signal the matcher to stop, join the thread, return the final
    /// [`Matcher`].
    pub fn stop(self) -> Matcher {
        self.stop_flag.store(true, Ordering::Release);
        self.handle.join().expect("matcher thread panicked")
    }
}

impl Engine {
    /// Spawn the matcher thread. `command_capacity` and `event_capacity`
    /// are minimums; both are rounded up to the next power of two.
    pub fn start(command_capacity: usize, event_capacity: usize) -> Self {
        let (cmd_tx, cmd_rx) = spsc::channel::<Command>(command_capacity);
        let (evt_tx, evt_rx) = spsc::channel::<Event>(event_capacity);
        let stop_flag = Arc::new(AtomicBool::new(false));
        let stop_for_thread = Arc::clone(&stop_flag);

        let handle = thread::Builder::new()
            .name("bourse-matcher".into())
            .spawn(move || matcher_loop(cmd_rx, evt_tx, stop_for_thread))
            .expect("spawn matcher thread");

        Self {
            input: cmd_tx,
            events: evt_rx,
            stop_flag,
            handle: Some(handle),
        }
    }

    /// Producer-side handle for the gateway. `try_push` returns `Err` if
    /// the command queue is full.
    pub fn input(&mut self) -> &mut Producer<Command> {
        &mut self.input
    }

    /// Consumer-side handle for the publisher. `try_pop` returns `None`
    /// if the event queue is empty.
    pub fn events(&mut self) -> &mut Consumer<Event> {
        &mut self.events
    }

    /// Move the producer and consumer halves out of the engine. The
    /// returned [`EngineHandle`] keeps ownership of the matcher thread
    /// and is responsible for stopping it. Useful when the producer
    /// and consumer need to live in separate tasks (e.g. tokio reader
    /// / writer halves of a TCP connection).
    #[allow(
        unsafe_code,
        reason = "moving fields out of Self via ManuallyDrop+ptr::read; safe because we never touch `self` after"
    )]
    pub fn split(self) -> (Producer<Command>, Consumer<Event>, EngineHandle) {
        let me = std::mem::ManuallyDrop::new(self);
        // SAFETY: ManuallyDrop blocks the `Drop for Engine`. We `ptr::read`
        // each field exactly once and never touch `me` again afterward,
        // so there's no double-drop and no use-after-move.
        unsafe {
            let input = std::ptr::read(&me.input);
            let events = std::ptr::read(&me.events);
            let stop_flag = std::ptr::read(&me.stop_flag);
            let handle = std::ptr::read(&me.handle).expect("engine handle was already taken");
            (input, events, EngineHandle { stop_flag, handle })
        }
    }

    /// Signal the matcher to stop, join the thread, and return the
    /// final [`Matcher`] for inspection.
    pub fn stop(mut self) -> Matcher {
        self.stop_flag.store(true, Ordering::Release);
        self.handle
            .take()
            .expect("handle already taken")
            .join()
            .expect("matcher thread panicked")
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        if let Some(handle) = self.handle.take() {
            self.stop_flag.store(true, Ordering::Release);
            let _ = handle.join();
        }
    }
}

fn matcher_loop(
    mut commands: Consumer<Command>,
    mut events_out: Producer<Event>,
    stop: Arc<AtomicBool>,
) -> Matcher {
    let mut matcher = Matcher::new();
    let mut events = Vec::with_capacity(16);

    loop {
        let drained = drain_one(&mut commands, &mut matcher, &mut events, &mut events_out);
        if !drained && stop.load(Ordering::Acquire) {
            // Last drain pass for anything that raced in just before stop.
            while drain_one(&mut commands, &mut matcher, &mut events, &mut events_out) {}
            break;
        }
        if !drained {
            std::hint::spin_loop();
        }
    }
    matcher
}

fn drain_one(
    commands: &mut Consumer<Command>,
    matcher: &mut Matcher,
    events: &mut Vec<Event>,
    events_out: &mut Producer<Event>,
) -> bool {
    let Some(cmd) = commands.try_pop() else {
        return false;
    };
    events.clear();
    match cmd {
        Command::New(no) => matcher.accept(no, events),
        Command::Cancel(id) => matcher.cancel(id, events),
    }
    for e in events.drain(..) {
        let mut e = e;
        while let Err(returned) = events_out.try_push(e) {
            e = returned;
            std::hint::spin_loop();
        }
    }
    true
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

    fn market(id: u64, side: Side, qty: u64) -> Command {
        Command::New(NewOrder {
            id: OrderId::new(id),
            side,
            qty: Qty::new(qty),
            kind: OrderKind::Market,
            timestamp: Timestamp::EPOCH,
        })
    }

    fn push_blocking(engine: &mut Engine, c: Command) {
        let mut c = c;
        while let Err(returned) = engine.input().try_push(c) {
            c = returned;
            std::hint::spin_loop();
        }
    }

    fn drain_until<F>(engine: &mut Engine, mut stop: F) -> Vec<Event>
    where
        F: FnMut(&Event) -> bool,
    {
        let mut out = Vec::new();
        loop {
            if let Some(e) = engine.events().try_pop() {
                let done = stop(&e);
                out.push(e);
                if done {
                    return out;
                }
            } else {
                std::hint::spin_loop();
            }
        }
    }

    #[test]
    fn engine_round_trips_a_limit_order() {
        let mut engine = Engine::start(64, 64);
        push_blocking(&mut engine, limit(1, Side::Buy, 100, 5));
        let events = drain_until(&mut engine, |e| matches!(e, Event::Accepted { .. }));
        assert_eq!(events.len(), 1);
        let m = engine.stop();
        assert_eq!(m.book().best_bid(), Some(Price::from_raw(100)));
    }

    #[test]
    fn engine_matches_through_the_pipeline() {
        let mut engine = Engine::start(64, 64);
        // Sell rests, buy crosses.
        push_blocking(&mut engine, limit(1, Side::Sell, 100, 5));
        push_blocking(&mut engine, limit(2, Side::Buy, 100, 5));

        // Wait until id 2 finishes.
        let events = drain_until(
            &mut engine,
            |e| matches!(e, Event::Done { id, reason: DoneReason::Filled, .. } if *id == OrderId::new(2)),
        );
        let trades: Vec<_> = events
            .iter()
            .filter_map(|e| match *e {
                Event::Trade { qty, .. } => Some(qty.get()),
                _ => None,
            })
            .collect();
        assert_eq!(trades, vec![5]);

        let m = engine.stop();
        assert!(m.book().is_empty());
    }

    #[test]
    fn engine_handles_a_thousand_resting_limits() {
        let mut engine = Engine::start(128, 4096);
        for i in 1..=1000u64 {
            push_blocking(&mut engine, limit(i, Side::Buy, 100, 1));
        }
        // Wait until the last order's Accepted event.
        let _ = drain_until(
            &mut engine,
            |e| matches!(e, Event::Accepted { id, .. } if *id == OrderId::new(1000)),
        );
        let m = engine.stop();
        assert_eq!(m.book().len(), 1000);
        assert_eq!(
            m.book().level_qty(Side::Buy, Price::from_raw(100)),
            Qty::new(1000)
        );
    }

    #[test]
    fn market_on_empty_completes_through_pipeline() {
        let mut engine = Engine::start(64, 64);
        push_blocking(&mut engine, market(1, Side::Buy, 5));
        let events = drain_until(&mut engine, |e| {
            matches!(
                e,
                Event::Done {
                    reason: DoneReason::NoLiquidity,
                    ..
                }
            )
        });
        // Accepted + Done(NoLiquidity).
        assert_eq!(events.len(), 2);
        let _ = engine.stop();
    }
}
