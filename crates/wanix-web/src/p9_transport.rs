use wanix_core::{Error, ErrorKind, Result};

#[derive(Clone, Debug, Default)]
pub struct FrameBuffer {
    pending: Vec<u8>,
}

impl FrameBuffer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, bytes: &[u8]) -> Result<Vec<Vec<u8>>> {
        self.pending.extend_from_slice(bytes);
        let mut frames = Vec::new();
        loop {
            if self.pending.len() < 4 {
                break;
            }
            let size = u32::from_le_bytes(
                self.pending[..4]
                    .try_into()
                    .map_err(|_| ErrorKind::Invalid)?,
            ) as usize;
            if size < 7 {
                return Err(Error::Message(format!("invalid 9P frame size {size}")));
            }
            if self.pending.len() < size {
                break;
            }
            frames.push(self.pending.drain(..size).collect());
        }
        Ok(frames)
    }

    pub fn pending_len(&self) -> usize {
        self.pending.len()
    }
}

#[cfg(target_arch = "wasm32")]
pub fn post_frame(
    port: &web_sys::MessagePort,
    frame: &[u8],
) -> std::result::Result<(), wasm_bindgen::JsValue> {
    let data = js_sys::Uint8Array::from(frame);
    port.post_message(&data)
}

#[cfg(test)]
mod tests {
    use super::*;
    use wanix_protocol::p9::{
        decode_request, encode_request, NinePRequest, DEFAULT_MSIZE, VERSION,
    };

    #[test]
    fn frame_buffer_reassembles_split_9p_messages() {
        let frame = encode_request(
            7,
            &NinePRequest::Version {
                msize: DEFAULT_MSIZE,
                version: VERSION.to_string(),
            },
        )
        .unwrap();
        let mut buffer = FrameBuffer::new();
        assert!(buffer.push(&frame[..3]).unwrap().is_empty());
        assert_eq!(buffer.pending_len(), 3);
        let frames = buffer.push(&frame[3..]).unwrap();
        assert_eq!(frames, vec![frame.clone()]);
        assert!(buffer.pending_len() == 0);
        assert_eq!(
            decode_request(&frames[0]).unwrap().1,
            NinePRequest::Version {
                msize: DEFAULT_MSIZE,
                version: VERSION.to_string()
            }
        );
    }
}
