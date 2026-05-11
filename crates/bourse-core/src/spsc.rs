//! Lock-free single-producer single-consumer ring buffer.
//!
//! Bounded capacity (rounded up to a power of two for index masking),
//! no allocation on the hot path, no waiting. The producer and consumer
//! each own one half of the queue and never block on each other.
//!
//! ## Memory ordering
//!
//! The synchronisation is the standard Acquire/Release pair:
//!
//! - The producer writes the slot, then publishes the new tail with
//!   `Release`.
//! - The consumer reads the new tail with `Acquire` (which establishes
//!   a happens-before with the slot write), then reads the slot.
//! - Symmetric on the head index for the consumer publishing
//!   "this slot is now free".
//!
//! Each side caches the last-observed value of the other side's index
//! and only re-reads from the atomic when the cached value indicates
//! not-empty (consumer) or not-full (producer). That keeps the cache
//! line for the other side's atomic out of the hot path most of the
//! time — without this, every `try_push` and `try_pop` would round-trip
//! the producer's tail and the consumer's head between cores.
//!
//! ## Correctness
//!
//! Validated under Miri in CI; see the threaded test below. Miri's
//! Stacked Borrows / Tree Borrows checker exercises the `unsafe` blocks
//! and the data-race detector exercises the atomic ordering choices.

#![allow(
    unsafe_code,
    reason = "lock-free ring buffer; SAFETY proofs annotate each unsafe block"
)]

use std::cell::{Cell, UnsafeCell};
use std::marker::PhantomData;
use std::mem::MaybeUninit;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Padded to a typical cache line so producer and consumer atomics don't
/// share a line and bounce on every write.
#[repr(align(64))]
struct CachePadded<T>(T);

impl<T> std::ops::Deref for CachePadded<T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.0
    }
}

struct Slot<T> {
    cell: UnsafeCell<MaybeUninit<T>>,
}

struct Inner<T> {
    /// Producer writes; consumer reads. On its own cache line.
    tail: CachePadded<AtomicUsize>,
    /// Consumer writes; producer reads. On its own cache line.
    head: CachePadded<AtomicUsize>,
    /// Ring storage. Length is `capacity`, a power of two.
    buffer: Box<[Slot<T>]>,
    mask: usize,
    capacity: usize,
}

// SAFETY: Every access to `buffer` slots is gated by the head/tail
// atomics. Producer writes a slot before storing the new tail with
// `Release`; consumer loads tail with `Acquire` before reading the slot.
// Symmetric on the consumer side for "slot is now free". `T: Send` is
// enough because each slot is owned by exactly one side at a time.
unsafe impl<T: Send> Sync for Inner<T> {}

/// Producer half. Single-thread use; can be moved between threads but
/// not shared between them.
pub struct Producer<T> {
    inner: Arc<Inner<T>>,
    cached_head: usize,
    /// Make the type `!Sync` without affecting `Send`.
    _not_sync: PhantomData<Cell<()>>,
}

/// Consumer half. Single-thread use; can be moved between threads but
/// not shared between them.
pub struct Consumer<T> {
    inner: Arc<Inner<T>>,
    cached_tail: usize,
    /// Make the type `!Sync` without affecting `Send`.
    _not_sync: PhantomData<Cell<()>>,
}

/// Create an SPSC queue with capacity at least `min_capacity`. The
/// allocated capacity is the next power of two ≥ `min_capacity` (and
/// ≥ 2). Returns the producer and consumer halves.
pub fn channel<T>(min_capacity: usize) -> (Producer<T>, Consumer<T>) {
    let capacity = min_capacity.max(2).next_power_of_two();
    let mask = capacity - 1;
    let mut buffer = Vec::with_capacity(capacity);
    for _ in 0..capacity {
        buffer.push(Slot {
            cell: UnsafeCell::new(MaybeUninit::uninit()),
        });
    }
    let inner = Arc::new(Inner {
        tail: CachePadded(AtomicUsize::new(0)),
        head: CachePadded(AtomicUsize::new(0)),
        buffer: buffer.into_boxed_slice(),
        mask,
        capacity,
    });
    (
        Producer {
            inner: Arc::clone(&inner),
            cached_head: 0,
            _not_sync: PhantomData,
        },
        Consumer {
            inner,
            cached_tail: 0,
            _not_sync: PhantomData,
        },
    )
}

impl<T> Producer<T> {
    /// Allocated capacity (always a power of two ≥ requested).
    pub fn capacity(&self) -> usize {
        self.inner.capacity
    }

    /// Push a value. Returns `Err(value)` if the queue is full.
    pub fn try_push(&mut self, value: T) -> Result<(), T> {
        // We're the only writer of `tail`, so a Relaxed load is fine.
        let tail = self.inner.tail.load(Ordering::Relaxed);

        // Quick path: cached head says we have room. Refresh from the
        // atomic only when the cache says full — that keeps the
        // consumer's cache line out of the hot path most of the time.
        if tail.wrapping_sub(self.cached_head) >= self.inner.capacity {
            self.cached_head = self.inner.head.load(Ordering::Acquire);
            if tail.wrapping_sub(self.cached_head) >= self.inner.capacity {
                return Err(value);
            }
        }

        let slot = &self.inner.buffer[tail & self.inner.mask];
        // SAFETY: `tail - head < capacity` here, so this slot is
        // logically free. The consumer reads slot[i] only after seeing
        // tail >= i+1 via an Acquire load, so it cannot be touching
        // this slot until we publish the new tail below.
        unsafe {
            slot.cell.get().write(MaybeUninit::new(value));
        }
        // Release publishes the slot write to any consumer that does an
        // Acquire load on tail.
        self.inner
            .tail
            .store(tail.wrapping_add(1), Ordering::Release);
        Ok(())
    }
}

impl<T> Consumer<T> {
    /// Allocated capacity (always a power of two ≥ requested).
    pub fn capacity(&self) -> usize {
        self.inner.capacity
    }

    /// Pop a value. Returns `None` if the queue is empty.
    pub fn try_pop(&mut self) -> Option<T> {
        // We're the only writer of `head`, so a Relaxed load is fine.
        let head = self.inner.head.load(Ordering::Relaxed);

        if head == self.cached_tail {
            self.cached_tail = self.inner.tail.load(Ordering::Acquire);
            if head == self.cached_tail {
                return None;
            }
        }

        let slot = &self.inner.buffer[head & self.inner.mask];
        // SAFETY: `head < cached_tail ≤ tail`, so this slot was
        // published by the producer (Release on tail; we did Acquire on
        // tail). We're the sole reader of slot[head] until we publish
        // the new head below.
        let value = unsafe { slot.cell.get().read().assume_init() };

        // Release publishes "this slot is free" to the producer.
        self.inner
            .head
            .store(head.wrapping_add(1), Ordering::Release);
        Some(value)
    }
}

impl<T> Drop for Inner<T> {
    fn drop(&mut self) {
        // Drop any items that were left in the queue when both halves
        // were dropped. `&mut self` here means there are no other
        // accessors, so we can use Relaxed loads.
        let head = self.head.load(Ordering::Relaxed);
        let tail = self.tail.load(Ordering::Relaxed);
        let mut h = head;
        while h != tail {
            let slot = &self.buffer[h & self.mask];
            // SAFETY: indices in [head, tail) are valid published slots
            // and we are the unique owner of `Inner` here.
            unsafe {
                slot.cell.get().read().assume_init_drop();
            }
            h = h.wrapping_add(1);
        }
    }
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
    use std::sync::Arc;
    use std::sync::atomic::AtomicUsize;
    use std::thread;

    #[test]
    fn empty_pop_is_none() {
        let (_p, mut c) = channel::<u32>(4);
        assert_eq!(c.try_pop(), None);
    }

    #[test]
    fn push_then_pop_round_trip() {
        let (mut p, mut c) = channel::<u32>(4);
        assert!(p.try_push(1).is_ok());
        assert!(p.try_push(2).is_ok());
        assert_eq!(c.try_pop(), Some(1));
        assert_eq!(c.try_pop(), Some(2));
        assert_eq!(c.try_pop(), None);
    }

    #[test]
    fn full_returns_err_with_value() {
        let (mut p, _c) = channel::<u32>(4);
        for i in 0..4 {
            assert!(p.try_push(i).is_ok());
        }
        assert_eq!(p.try_push(99), Err(99));
    }

    #[test]
    fn wraps_around_many_times() {
        let (mut p, mut c) = channel::<u32>(4);
        for i in 0..1000u32 {
            assert!(p.try_push(i).is_ok());
            assert_eq!(c.try_pop(), Some(i));
        }
    }

    #[test]
    fn capacity_rounds_up_to_power_of_two() {
        let (p, _c) = channel::<u32>(5);
        assert_eq!(p.capacity(), 8);
        let (p, _c) = channel::<u32>(1);
        assert_eq!(p.capacity(), 2);
        let (p, _c) = channel::<u32>(0);
        assert_eq!(p.capacity(), 2);
    }

    #[test]
    fn drop_runs_for_remaining_items() {
        #[derive(Debug)]
        struct Counter(Arc<AtomicUsize>);
        impl Drop for Counter {
            fn drop(&mut self) {
                self.0.fetch_add(1, Ordering::SeqCst);
            }
        }
        let counter = Arc::new(AtomicUsize::new(0));
        {
            let (mut p, _c) = channel::<Counter>(4);
            p.try_push(Counter(Arc::clone(&counter))).unwrap();
            p.try_push(Counter(Arc::clone(&counter))).unwrap();
            p.try_push(Counter(Arc::clone(&counter))).unwrap();
        }
        assert_eq!(counter.load(Ordering::SeqCst), 3);
    }

    // Miri runs single-instruction at a time, so dial back the threaded
    // workload massively when running under it.
    #[cfg(not(miri))]
    const N: u32 = 100_000;
    #[cfg(miri)]
    const N: u32 = 200;

    #[test]
    fn threaded_in_order_no_loss() {
        let (mut p, mut c) = channel::<u32>(64);
        let producer = thread::spawn(move || {
            for i in 0..N {
                while p.try_push(i).is_err() {
                    std::hint::spin_loop();
                }
            }
        });
        let consumer = thread::spawn(move || {
            let mut next = 0u32;
            while next < N {
                if let Some(v) = c.try_pop() {
                    assert_eq!(v, next);
                    next += 1;
                } else {
                    std::hint::spin_loop();
                }
            }
        });
        producer.join().unwrap();
        consumer.join().unwrap();
    }
}
