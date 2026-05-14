use std::collections::VecDeque;
use std::sync::{Arc, Mutex, MutexGuard};

use star9_core::{Error, Result};

pub trait MessagePort: Clone + Send + Sync + 'static {
    fn post_message(&self, message: &[u8]) -> Result<()>;

    fn try_recv_message(&self) -> Result<Option<Vec<u8>>>;

    fn drain_messages(&self) -> Result<Vec<Vec<u8>>> {
        let mut messages = Vec::new();
        while let Some(message) = self.try_recv_message()? {
            messages.push(message);
        }
        Ok(messages)
    }
}

#[derive(Clone, Debug)]
pub struct InMemoryMessagePort {
    state: Arc<Mutex<InMemoryMessagePortState>>,
    endpoint: EndpointId,
}

#[derive(Clone, Copy, Debug)]
enum EndpointId {
    A,
    B,
}

impl EndpointId {
    fn index(self) -> usize {
        match self {
            Self::A => 0,
            Self::B => 1,
        }
    }

    fn peer_index(self) -> usize {
        match self {
            Self::A => 1,
            Self::B => 0,
        }
    }
}

#[derive(Debug)]
struct InMemoryMessagePortState {
    queues: [VecDeque<Vec<u8>>; 2],
}

impl InMemoryMessagePort {
    pub fn channel() -> (Self, Self) {
        let state = Arc::new(Mutex::new(InMemoryMessagePortState {
            queues: std::array::from_fn(|_| VecDeque::new()),
        }));
        (
            Self {
                state: state.clone(),
                endpoint: EndpointId::A,
            },
            Self {
                state,
                endpoint: EndpointId::B,
            },
        )
    }

    pub fn queued_message_count(&self) -> Result<usize> {
        Ok(self.lock_state()?.queues[self.endpoint.index()].len())
    }

    fn lock_state(&self) -> Result<MutexGuard<'_, InMemoryMessagePortState>> {
        self.state
            .lock()
            .map_err(|_| Error::Message("message port state poisoned".into()))
    }
}

impl MessagePort for InMemoryMessagePort {
    fn post_message(&self, message: &[u8]) -> Result<()> {
        self.lock_state()?.queues[self.endpoint.peer_index()].push_back(message.to_vec());
        Ok(())
    }

    fn try_recv_message(&self) -> Result<Option<Vec<u8>>> {
        Ok(self.lock_state()?.queues[self.endpoint.index()].pop_front())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::p9_transport::FrameBuffer;
    use star9_protocol::p9::{
        decode_request, encode_request, NinePRequest, DEFAULT_MSIZE, VERSION,
    };

    #[test]
    fn in_memory_message_port_preserves_message_boundaries() {
        let (left, right) = InMemoryMessagePort::channel();

        left.post_message(b"alpha").unwrap();
        left.post_message(b"beta").unwrap();

        assert_eq!(right.queued_message_count().unwrap(), 2);
        assert_eq!(right.try_recv_message().unwrap(), Some(b"alpha".to_vec()));
        assert_eq!(right.try_recv_message().unwrap(), Some(b"beta".to_vec()));
        assert_eq!(right.try_recv_message().unwrap(), None);
    }

    #[test]
    fn in_memory_message_port_transfers_9p_frames_losslessly() {
        let (left, right) = InMemoryMessagePort::channel();
        let frame = encode_request(
            7,
            &NinePRequest::Version {
                msize: DEFAULT_MSIZE,
                version: VERSION.to_string(),
            },
        )
        .unwrap();

        left.post_message(&frame).unwrap();

        let delivered = right.try_recv_message().unwrap().unwrap();
        assert_eq!(delivered, frame);

        let mut buffer = FrameBuffer::new();
        let frames = buffer.push(&delivered).unwrap();
        assert_eq!(frames, vec![frame.clone()]);
        assert_eq!(
            decode_request(&frames[0]).unwrap().1,
            NinePRequest::Version {
                msize: DEFAULT_MSIZE,
                version: VERSION.to_string(),
            }
        );
    }
}
