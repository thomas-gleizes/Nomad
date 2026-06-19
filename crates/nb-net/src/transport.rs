//! Lecture/écriture asynchrone de [`Message`] sur un flux tokio, en s'appuyant
//! sur le codec de trame de `nb-core`.

use nb_core::codec::{encode_frame, decode_payload, HEADER_LEN, MAX_FRAME_LEN};
use nb_core::Message;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// Lit une trame complète et la désérialise.
pub async fn read_message<R>(r: &mut R) -> anyhow::Result<Message>
where
    R: AsyncRead + Unpin,
{
    let mut header = [0u8; HEADER_LEN];
    r.read_exact(&mut header).await?;
    let len = u32::from_be_bytes(header) as usize;
    if len > MAX_FRAME_LEN {
        anyhow::bail!("trame trop grande: {len} octets");
    }
    let mut payload = vec![0u8; len];
    r.read_exact(&mut payload).await?;
    Ok(decode_payload(&payload)?)
}

/// Sérialise et écrit une trame complète, puis flush.
pub async fn write_message<W>(w: &mut W, msg: &Message) -> anyhow::Result<()>
where
    W: AsyncWrite + Unpin,
{
    let frame = encode_frame(msg)?;
    w.write_all(&frame).await?;
    w.flush().await?;
    Ok(())
}
