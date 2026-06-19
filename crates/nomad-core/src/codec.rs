//! Codec de trame : `[u32 longueur big-endian][payload bincode]`.
//!
//! Volontairement *synchrone et pur* : `nomad-net` enveloppe ces fonctions dans des
//! I/O tokio, mais la logique d'(en/dé)codage reste testable sans runtime.

use crate::error::{Error, Result};
use crate::protocol::Message;

/// En-tête de longueur : `u32` big-endian.
pub const HEADER_LEN: usize = 4;

/// Garde-fou contre les trames aberrantes (16 Mo).
pub const MAX_FRAME_LEN: usize = 16 * 1024 * 1024;

/// Sérialise un message en une trame complète (en-tête + payload).
pub fn encode_frame(msg: &Message) -> Result<Vec<u8>> {
    let payload = bincode::serialize(msg)?;
    if payload.len() > MAX_FRAME_LEN {
        return Err(Error::FrameTooLarge(payload.len()));
    }
    let mut frame = Vec::with_capacity(HEADER_LEN + payload.len());
    frame.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    frame.extend_from_slice(&payload);
    Ok(frame)
}

/// Décode un message depuis un payload (sans l'en-tête de longueur).
pub fn decode_payload(payload: &[u8]) -> Result<Message> {
    Ok(bincode::deserialize(payload)?)
}

/// Accumulateur de flux : on y pousse les octets reçus, il rend les messages
/// complets au fur et à mesure. Pratique pour les transports orientés flux (TCP).
#[derive(Default)]
pub struct FrameDecoder {
    buf: Vec<u8>,
}

impl FrameDecoder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Ajoute des octets bruts au tampon interne.
    pub fn push(&mut self, bytes: &[u8]) {
        self.buf.extend_from_slice(bytes);
    }

    /// Tente d'extraire le prochain message complet. `Ok(None)` si incomplet.
    pub fn next_message(&mut self) -> Result<Option<Message>> {
        if self.buf.len() < HEADER_LEN {
            return Ok(None);
        }
        let len = u32::from_be_bytes(self.buf[..HEADER_LEN].try_into().unwrap()) as usize;
        if len > MAX_FRAME_LEN {
            return Err(Error::FrameTooLarge(len));
        }
        if self.buf.len() < HEADER_LEN + len {
            return Ok(None);
        }
        let msg = decode_payload(&self.buf[HEADER_LEN..HEADER_LEN + len])?;
        self.buf.drain(..HEADER_LEN + len);
        Ok(Some(msg))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::{Button, InputEvent, Key};
    use crate::layout::Screen;
    use crate::protocol::{NodeId, Os};

    fn samples() -> Vec<Message> {
        vec![
            Message::Hello {
                node_id: NodeId::random(),
                name: "mac-thomas".into(),
                os: Os::MacOs,
                screen: Screen::new(2560, 1440),
            },
            Message::Input {
                event: InputEvent::MouseMove { dx: -3.5, dy: 12.0 },
            },
            Message::Input {
                event: InputEvent::MouseButton {
                    button: Button::Right,
                    pressed: true,
                },
            },
            Message::Input {
                event: InputEvent::Key {
                    key: Key::Raw(0x1234),
                    pressed: false,
                },
            },
            Message::Clipboard {
                text: "héllo 🌍".into(),
            },
            Message::Ping,
        ]
    }

    #[test]
    fn frame_roundtrip() {
        for msg in samples() {
            let frame = encode_frame(&msg).unwrap();
            let decoded = decode_payload(&frame[HEADER_LEN..]).unwrap();
            assert_eq!(msg, decoded);
        }
    }

    #[test]
    fn streaming_decoder_handles_split_and_coalesced_frames() {
        let msgs = samples();
        let mut stream = Vec::new();
        for m in &msgs {
            stream.extend(encode_frame(m).unwrap());
        }

        // Pousse octet par octet : aucun message ne doit apparaître prématurément.
        let mut dec = FrameDecoder::new();
        let mut out = Vec::new();
        for b in &stream {
            dec.push(&[*b]);
            while let Some(m) = dec.next_message().unwrap() {
                out.push(m);
            }
        }
        assert_eq!(out, msgs);
    }

    #[test]
    fn incomplete_frame_yields_none() {
        let frame = encode_frame(&Message::Pong).unwrap();
        let mut dec = FrameDecoder::new();
        dec.push(&frame[..2]); // en-tête partiel
        assert!(dec.next_message().unwrap().is_none());
    }
}
