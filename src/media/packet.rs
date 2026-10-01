use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

use anyhow::{Result, bail};
use bytes::{BufMut, Bytes, BytesMut};

const HEADER: usize = 24;
const MAX_PACKET: usize = 8192;
const MAX_FRAGMENTS: usize = 16;
const MAX_PENDING: usize = 128;
const MAX_AGE: Duration = Duration::from_millis(150);

pub fn generation_bytes(value: &str) -> Result<[u8; 16]> {
    if !crate::protocol::valid_id(value) {
        bail!("媒体 generation 无效");
    }
    let mut result = [0; 16];
    for (index, byte) in result.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&value[index * 2..index * 2 + 2], 16)?;
    }
    Ok(result)
}

pub fn fragment(
    generation: [u8; 16],
    packet_id: u32,
    payload: &Bytes,
    mtu: usize,
) -> Result<Vec<Bytes>> {
    if payload.is_empty() || payload.len() > MAX_PACKET || mtu <= HEADER {
        bail!("媒体包大小无效");
    }
    let size = mtu - HEADER;
    let count = payload.len().div_ceil(size);
    if count > MAX_FRAGMENTS {
        bail!("媒体包分片过多");
    }
    payload
        .chunks(size)
        .enumerate()
        .map(|(index, part)| {
            let mut data = BytesMut::with_capacity(HEADER + part.len());
            data.extend_from_slice(&generation);
            data.put_u32(packet_id);
            data.put_u16(u16::try_from(index)?);
            data.put_u16(u16::try_from(count)?);
            data.extend_from_slice(part);
            Ok(data.freeze())
        })
        .collect()
}

struct Pending {
    created: Instant,
    parts: Vec<Option<Bytes>>,
    size: usize,
}

#[derive(Default)]
pub struct Reassembler {
    pending: HashMap<u32, Pending>,
}

impl Reassembler {
    pub fn push(&mut self, data: Bytes, generation: [u8; 16], now: Instant) -> Option<Bytes> {
        self.pending
            .retain(|_, entry| now.duration_since(entry.created) < MAX_AGE);
        if data.len() <= HEADER || data[..16] != generation {
            return None;
        }
        let id = u32::from_be_bytes(data[16..20].try_into().ok()?);
        let index = usize::from(u16::from_be_bytes(data[20..22].try_into().ok()?));
        let count = usize::from(u16::from_be_bytes(data[22..24].try_into().ok()?));
        if count == 0 || count > MAX_FRAGMENTS || index >= count || data.len() - HEADER > MAX_PACKET
        {
            return None;
        }
        if !self.pending.contains_key(&id) && self.pending.len() >= MAX_PENDING {
            return None;
        }
        let entry = self.pending.entry(id).or_insert_with(|| Pending {
            created: now,
            parts: vec![None; count],
            size: 0,
        });
        if entry.parts.len() != count {
            return None;
        }
        if entry.parts[index].is_none() {
            entry.size += data.len() - HEADER;
            if entry.size > MAX_PACKET {
                self.pending.remove(&id);
                return None;
            }
            entry.parts[index] = Some(data.slice(HEADER..));
        }
        if entry.parts.iter().any(Option::is_none) {
            return None;
        }
        let entry = self.pending.remove(&id)?;
        let mut result = BytesMut::with_capacity(entry.size);
        for part in entry.parts.into_iter().flatten() {
            result.extend_from_slice(&part);
        }
        Some(result.freeze())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reassembles_out_of_order_fragments() -> Result<()> {
        let data = Bytes::from(
            (0..3000)
                .map(|index| (index % 251) as u8)
                .collect::<Vec<_>>(),
        );
        let fragments = fragment([1; 16], 4, &data, 1000)?;
        let mut receiver = Reassembler::default();
        let mut result = None;
        let count = fragments.len();
        for (index, part) in fragments.into_iter().rev().enumerate() {
            result = receiver.push(part, [1; 16], Instant::now());
            if index + 1 < count {
                assert!(result.is_none());
            }
        }
        assert_eq!(result, Some(data));
        Ok(())
    }

    #[test]
    fn rejects_wrong_generation_and_invalid_headers() -> Result<()> {
        let mut receiver = Reassembler::default();
        let part = fragment([1; 16], 0, &Bytes::from_static(b"test"), 1000)?.remove(0);
        assert!(receiver.push(part, [2; 16], Instant::now()).is_none());
        assert!(
            receiver
                .push(Bytes::from_static(b"bad"), [1; 16], Instant::now())
                .is_none()
        );
        Ok(())
    }

    #[test]
    fn incomplete_fragments_expire() -> Result<()> {
        let parts = fragment([1; 16], 0, &Bytes::from(vec![0; 2000]), 1100)?;
        let now = Instant::now();
        let mut receiver = Reassembler::default();
        assert!(receiver.push(parts[0].clone(), [1; 16], now).is_none());
        assert!(
            receiver
                .push(parts[1].clone(), [1; 16], now + MAX_AGE)
                .is_none()
        );
        Ok(())
    }
}
