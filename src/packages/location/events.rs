//! Bounded callback mailbox. Native callbacks never wait for a consumer.
use futures::{task::AtomicWaker, Stream};
use std::{
    collections::{HashMap, VecDeque},
    pin::Pin,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex, OnceLock,
    },
    task::{Context, Poll},
};

const CAPACITY: usize = 64;
struct State {
    queue: VecDeque<String>,
    closed: bool,
}
struct Mailbox {
    state: Mutex<State>,
    waker: AtomicWaker,
    #[cfg(test)]
    pending_polls: AtomicU64,
}
fn registry() -> &'static Mutex<HashMap<u64, Arc<Mailbox>>> {
    static REGISTRY: OnceLock<Mutex<HashMap<u64, Arc<Mailbox>>>> = OnceLock::new();
    REGISTRY.get_or_init(Default::default)
}
pub(super) struct Receiver {
    pub id: u64,
    mailbox: Arc<Mailbox>,
}
impl Receiver {
    pub fn is_closed(&self) -> bool {
        self.mailbox.state.lock().unwrap().closed
    }
    pub fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        let mailbox = Arc::new(Mailbox {
            state: Mutex::new(State {
                queue: VecDeque::new(),
                closed: false,
            }),
            waker: AtomicWaker::new(),
            #[cfg(test)]
            pending_polls: AtomicU64::new(0),
        });
        registry().lock().unwrap().insert(id, mailbox.clone());
        Self { id, mailbox }
    }
    pub fn close(&self) {
        registry().lock().unwrap().remove(&self.id);
        let mut state = self.mailbox.state.lock().unwrap();
        state.closed = true;
        state.queue.clear();
        drop(state);
        self.mailbox.waker.wake();
    }
}
impl Drop for Receiver {
    fn drop(&mut self) {
        self.close();
    }
}
impl Stream for Receiver {
    type Item = String;
    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<String>> {
        self.mailbox.waker.register(cx.waker());
        let mut state = self.mailbox.state.lock().unwrap();
        if let Some(event) = state.queue.pop_front() {
            Poll::Ready(Some(event))
        } else if state.closed {
            Poll::Ready(None)
        } else {
            #[cfg(test)]
            self.mailbox.pending_polls.fetch_add(1, Ordering::SeqCst);
            Poll::Pending
        }
    }
}
#[cfg(test)]
pub(super) fn pending_polls(id: u64) -> u64 {
    registry().lock().unwrap().get(&id)
        .map_or(0, |mailbox| mailbox.pending_polls.load(Ordering::SeqCst))
}
/// False means the native producer must stop, even if the consumer never polls again.
pub(super) fn send(id: u64, event: String) -> bool {
    let mailbox = registry().lock().unwrap().get(&id).cloned();
    if let Some(mailbox) = mailbox {
        let mut state = mailbox.state.lock().unwrap();
        if state.closed {
            return false;
        }
        if state.queue.len() == CAPACITY {
            state.queue.clear();
            state
                .queue
                .push_back("error|overflow|event consumer too slow; restart subscription".into());
            state.closed = true;
        } else {
            let terminal = event.starts_with("end|");
            state.queue.push_back(event);
            state.closed = terminal;
        }
        let accepting = !state.closed;
        drop(state);
        mailbox.waker.wake();
        accepting
    } else {
        false
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use futures::{executor::block_on, StreamExt};
    #[test]
    fn cancellation_rejects_late_callbacks() {
        let mut receiver = Receiver::new();
        receiver.close();
        assert!(!send(receiver.id, "late".into()));
        assert_eq!(block_on(receiver.next()), None);
    }
    #[test]
    fn overflow_is_bounded_and_reported() {
        let mut receiver = Receiver::new();
        for _ in 0..CAPACITY {
            assert!(send(receiver.id, "sample".into()));
        }
        assert!(!send(receiver.id, "overflow".into()));
        assert!(!send(receiver.id, "late".into()));
        assert!(block_on(receiver.next())
            .unwrap()
            .starts_with("error|overflow|"));
        assert_eq!(block_on(receiver.next()), None);
    }
    #[test]
    fn denied_then_granted_is_not_a_terminal_stream() {
        let mut receiver = Receiver::new();
        send(receiver.id, "permission|denied".into());
        send(receiver.id, "permission|precise".into());
        assert_eq!(
            block_on(receiver.next()).as_deref(),
            Some("permission|denied")
        );
        assert_eq!(
            block_on(receiver.next()).as_deref(),
            Some("permission|precise")
        );
    }
    #[test]
    fn native_owner_end_drains_then_terminates_and_ignores_late_callbacks() {
        let mut receiver = Receiver::new();
        send(
            receiver.id,
            "error|configuration|missing permission description".into(),
        );
        send(receiver.id, "end|owner_destroyed".into());
        send(receiver.id, "position|late".into());
        assert!(receiver.is_closed());
        assert!(block_on(receiver.next())
            .unwrap()
            .starts_with("error|configuration|"));
        assert_eq!(
            block_on(receiver.next()).as_deref(),
            Some("end|owner_destroyed")
        );
        assert_eq!(block_on(receiver.next()), None);
    }
}
