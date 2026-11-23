//! Poll a real device-start acknowledgement without waiting on the frame thread.
use std::{sync::mpsc, time::Instant};

pub(crate) struct StartAcknowledgement<E> {
    receiver: mpsc::Receiver<Result<(), E>>,
    deadline: Instant,
}

impl<E> StartAcknowledgement<E> {
    pub(crate) fn new(receiver: mpsc::Receiver<Result<(), E>>, deadline: Instant) -> Self {
        Self { receiver, deadline }
    }

    pub(crate) fn poll(&mut self, now: Instant, unavailable: impl FnOnce() -> E) -> Option<Result<(), E>> {
        match self.receiver.try_recv() {
            Ok(result) => Some(result),
            Err(mpsc::TryRecvError::Empty) if now < self.deadline => None,
            Err(_) => Some(Err(unavailable())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[derive(Debug, PartialEq)]
    enum Failure { DeviceStartTimeout, WorkerUnavailable }

    #[test]
    fn pending_driver_start_does_not_hold_the_caller_until_acknowledgement() {
        let (sender, receiver) = mpsc::sync_channel(1);
        let now = Instant::now();
        let mut start = StartAcknowledgement::new(receiver, now + Duration::from_secs(10));
        assert_eq!(start.poll(now, || Failure::WorkerUnavailable), None);
        sender.send(Ok(())).unwrap();
        assert_eq!(start.poll(now, || Failure::WorkerUnavailable), Some(Ok(())));
    }

    #[test]
    fn device_start_timeout_keeps_its_error_kind() {
        let (sender, receiver) = mpsc::sync_channel(1);
        sender.send(Err(Failure::DeviceStartTimeout)).unwrap();
        let now = Instant::now();
        let mut start = StartAcknowledgement::new(receiver, now + Duration::from_secs(10));
        assert_eq!(start.poll(now, || Failure::WorkerUnavailable), Some(Err(Failure::DeviceStartTimeout)));
    }

    #[test]
    fn expiry_does_not_wait_for_a_driver_still_holding_the_sender() {
        let (_sender, receiver) = mpsc::sync_channel::<Result<(), Failure>>(1);
        let deadline = Instant::now();
        let mut start = StartAcknowledgement::new(receiver, deadline);
        assert_eq!(start.poll(deadline, || Failure::WorkerUnavailable), Some(Err(Failure::WorkerUnavailable)));
    }

    #[test]
    fn an_acknowledged_start_wins_over_the_expired_poll_deadline() {
        let (sender, receiver) = mpsc::sync_channel(1);
        sender.send(Ok(())).unwrap();
        let deadline = Instant::now();
        let mut start = StartAcknowledgement::new(receiver, deadline);
        assert_eq!(start.poll(deadline, || Failure::WorkerUnavailable), Some(Ok(())));
    }

    #[test]
    fn a_worker_that_exits_without_acknowledgement_is_an_error() {
        let (sender, receiver) = mpsc::sync_channel::<Result<(), Failure>>(1);
        drop(sender);
        let now = Instant::now();
        let mut start = StartAcknowledgement::new(receiver, now + Duration::from_secs(10));
        assert_eq!(start.poll(now, || Failure::WorkerUnavailable), Some(Err(Failure::WorkerUnavailable)));
    }
}
