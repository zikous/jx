//! Runs a function over chunks on all cores and hands the results back in the
//! order the chunks came in.

use std::collections::BTreeMap;
use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;

use crate::stack;
use crossbeam_channel::{bounded, unbounded};

pub enum Flow {
    Continue,
    Stop,
}

/// Runs the first chunk on the calling thread, so small inputs never pay for a
/// thread pool; any further chunks go to worker threads, each of which calls
/// `init` once to build its worker. Results reach `sink` in input order, and the
/// run stops early when `sink` says so.
pub fn run_ordered<C, T, W>(
    mut chunks: impl Iterator<Item = io::Result<C>>,
    init: impl Fn() -> W + Sync,
    mut sink: impl FnMut(io::Result<T>) -> Flow,
) where
    C: Send,
    T: Send,
    W: FnMut(C) -> T,
{
    let mut inline = init();
    let Some(first) = chunks.next() else { return };
    if matches!(sink(first.map(&mut inline)), Flow::Stop) {
        return;
    }
    let Some(second) = chunks.next() else { return };

    let workers = thread::available_parallelism().map_or(2, usize::from);
    let (jobs, queue) = bounded::<(usize, C)>(workers * 4);
    let (done, results) = unbounded::<(usize, T)>();
    let stopped = AtomicBool::new(false);
    thread::scope(|scope| {
        for _ in 0..workers {
            let (queue, done, init, stopped) = (queue.clone(), done.clone(), &init, &stopped);
            scope.spawn(move || {
                stacker::maybe_grow(stack::RED_ZONE, stack::GROWTH, || {
                    let mut work = init();
                    for (seq, chunk) in queue {
                        if !stopped.load(Ordering::Relaxed)
                            && done.send((seq, work(chunk))).is_err()
                        {
                            break;
                        }
                    }
                });
            });
        }
        drop((queue, done));

        let mut delivery = Delivery {
            ready: BTreeMap::new(),
            next: 1,
            sink: &mut sink,
        };
        let mut sent = 1;
        let mut halted = false;
        for item in std::iter::once(second).chain(chunks) {
            match item {
                Ok(chunk) => {
                    if jobs.send((sent, chunk)).is_err() {
                        break;
                    }
                }
                Err(err) => {
                    delivery.ready.insert(sent, Err(err));
                }
            }
            sent += 1;
            while let Ok((seq, result)) = results.try_recv() {
                delivery.ready.insert(seq, Ok(result));
            }
            if delivery.flush() {
                halted = true;
                break;
            }
        }
        drop(jobs);
        while !halted && delivery.next < sent {
            match results.recv() {
                Ok((seq, result)) => {
                    delivery.ready.insert(seq, Ok(result));
                    halted = delivery.flush();
                }
                Err(_) => break,
            }
        }
        stopped.store(true, Ordering::Relaxed);
    });
}

/// Results that arrived out of order, waiting for their turn.
struct Delivery<'s, T> {
    ready: BTreeMap<usize, io::Result<T>>,
    next: usize,
    sink: &'s mut dyn FnMut(io::Result<T>) -> Flow,
}

impl<T> Delivery<'_, T> {
    /// Hands over every result that is next in line; true if the sink asked to stop.
    fn flush(&mut self) -> bool {
        while let Some(result) = self.ready.remove(&self.next) {
            self.next += 1;
            if matches!((self.sink)(result), Flow::Stop) {
                return true;
            }
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn results_come_back_in_input_order_whatever_the_workers_do() {
        let chunks = (0..200u32).map(Ok);
        let mut seen = Vec::new();
        run_ordered(
            chunks,
            || {
                |n: u32| {
                    if n.is_multiple_of(7) {
                        std::thread::sleep(std::time::Duration::from_millis(2));
                    }
                    n
                }
            },
            |result| {
                seen.push(result.unwrap());
                Flow::Continue
            },
        );
        assert_eq!(seen, (0..200).collect::<Vec<_>>());
    }

    #[test]
    fn the_sink_can_stop_the_run() {
        let mut seen = 0;
        run_ordered(
            (0..1000u32).map(Ok),
            || |n: u32| n,
            |_| {
                seen += 1;
                if seen == 3 {
                    Flow::Stop
                } else {
                    Flow::Continue
                }
            },
        );
        assert_eq!(seen, 3);
    }
}
