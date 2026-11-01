//! Reconstruct Evolution audio without re-encoding its samples.
//!
//! Field layout corroborated with the installed Soulframe cache and the
//! AudioReader139 implementation in Puxtril/Warframe-Exporter. Ogg framing
//! follows RFC 3533 and RFC 7845; Opus packet duration follows RFC 6716.

use anyhow::{bail, ensure, Context, Result};

pub struct AudioFile {
    pub bytes: Vec<u8>,
    pub extension: &'static str,
    pub codec: &'static str,
    pub sample_rate: Option<u32>,
    pub channels: Option<u16>,
    pub duration_seconds: Option<f64>,
}

#[derive(Debug)]
struct Header {
    compression: u32,
    sample_rate: u32,
    bits: u16,
    channels: u16,
    samples: u32,
    byte_rate: u32,
    resident_size: usize,
    block_align: u16,
    samples_per_block: u16,
    size: usize,
}

fn field<const N: usize>(data: &[u8], offset: usize) -> Result<[u8; N]> {
    data.get(offset..offset.checked_add(N).context("audio offset overflow")?)
        .context("truncated audio header")?
        .try_into()
        .map_err(Into::into)
}

fn u32_at(data: &[u8], offset: usize) -> Result<u32> {
    Ok(u32::from_le_bytes(field(data, offset)?))
}

fn u16_at(data: &[u8], offset: usize) -> Result<u16> {
    Ok(u16::from_le_bytes(field(data, offset)?))
}

fn parse_header(raw: &[u8]) -> Result<Header> {
    let path_count = u32_at(raw, 16)? as usize;
    ensure!(path_count <= raw.len() / 4, "invalid audio reference count");
    let mut pos = 20usize;
    for _ in 0..path_count {
        let length = u32_at(raw, pos)? as usize;
        pos = pos
            .checked_add(4)
            .and_then(|p| p.checked_add(length))
            .context("audio reference length overflow")?;
        ensure!(pos <= raw.len(), "truncated audio reference");
    }
    let attributes = u32_at(raw, pos)? as usize;
    pos = pos
        .checked_add(4)
        .and_then(|p| p.checked_add(attributes))
        .and_then(|p| p.checked_add(usize::from(attributes > 0)))
        .context("audio attributes length overflow")?;
    let kind = u32_at(raw, pos)?;
    ensure!(
        kind == 139 || kind == 6,
        "unsupported audio resource type {kind}"
    );
    let data = raw.get(pos + 4..).context("missing audio format header")?;
    let header = Header {
        compression: u32_at(data, 0)?,
        sample_rate: u32_at(data, 32)?,
        bits: *data.get(36).context("missing audio bit depth")? as u16,
        channels: *data.get(37).context("missing audio channel count")? as u16,
        samples: u32_at(data, 38)?,
        byte_rate: u32_at(data, 42)?,
        resident_size: u32_at(data, 28)? as usize,
        block_align: u16_at(data, 46)?,
        samples_per_block: u16_at(data, 48)?,
        size: u32_at(data, 62)? as usize,
    };
    ensure!(
        (1..=32).contains(&header.channels),
        "invalid audio channel count"
    );
    ensure!(
        (1..=768_000).contains(&header.sample_rate),
        "invalid audio sample rate"
    );
    ensure!(header.size > 0, "empty audio payload");
    Ok(header)
}

/// `header`, `body`, and `stream` are the decompressed H, B, and F records.
/// Unknown formats fail explicitly so the caller can preserve their raw data.
pub fn reconstruct(
    header: Option<&[u8]>,
    body: Option<&[u8]>,
    stream: Option<&[u8]>,
) -> Result<AudioFile> {
    reconstruct_with_vorbis(header, body, stream, None)
}

/// As [`reconstruct`], with the local game's shared Vorbis header library.
pub fn reconstruct_with_vorbis(
    header: Option<&[u8]>,
    body: Option<&[u8]>,
    stream: Option<&[u8]>,
    vorbis: Option<&crate::vorbis::VorbisHeaders>,
) -> Result<AudioFile> {
    for bytes in [header, stream, body].into_iter().flatten() {
        if bytes.starts_with(b"RIFF") {
            return standard_wave(bytes);
        }
        if bytes.starts_with(b"OggS") {
            return standard_ogg(bytes);
        }
    }
    let h = parse_header(header.context("audio resource has no H header")?)?;
    match h.compression {
        0 | 5 => wave(&h, body, stream),
        7 => opus(&h, body, stream),
        8 => crate::vorbis::reconstruct(header.unwrap(), body, stream, vorbis.context("Evolution Vorbis format 8 needs the matching Soulframe executable's shared headers")?),
        n => bail!("unsupported Evolution audio compression {n}"),
    }
}

fn joined(first: Option<&[u8]>, second: Option<&[u8]>) -> Vec<u8> {
    let mut bytes = first.unwrap_or_default().to_vec();
    bytes.extend_from_slice(second.unwrap_or_default());
    bytes
}

fn wave(h: &Header, body: Option<&[u8]>, stream: Option<&[u8]>) -> Result<AudioFile> {
    let combined = joined(body, stream);
    ensure!(
        combined.len() >= h.size,
        "truncated waveform: expected {} bytes, found {}",
        h.size,
        combined.len()
    );
    // Some B records begin with auxiliary data; the waveform occupies the
    // final declared number of bytes across the B+F records.
    let samples = &combined[combined.len() - h.size..];
    let mut fmt = Vec::new();
    let codec;
    let duration;
    if h.compression == 0 {
        ensure!(
            [8, 16, 24, 32].contains(&h.bits),
            "unsupported PCM bit depth {}",
            h.bits
        );
        let align = h
            .channels
            .checked_mul(h.bits / 8)
            .context("PCM block alignment overflow")?;
        ensure!(
            samples.len() % align as usize == 0,
            "partial PCM sample frame"
        );
        fmt.extend_from_slice(&1u16.to_le_bytes());
        fmt.extend_from_slice(&h.channels.to_le_bytes());
        fmt.extend_from_slice(&h.sample_rate.to_le_bytes());
        fmt.extend_from_slice(
            &h.sample_rate
                .checked_mul(align as u32)
                .context("PCM byte rate overflow")?
                .to_le_bytes(),
        );
        fmt.extend_from_slice(&align.to_le_bytes());
        fmt.extend_from_slice(&h.bits.to_le_bytes());
        codec = "pcm";
        duration = samples.len() as f64 / (align as f64 * h.sample_rate as f64);
    } else {
        ensure!(
            h.channels <= 2 && h.bits == 4,
            "unsupported MS ADPCM channel/bit layout"
        );
        ensure!(
            h.block_align as usize >= 7 * h.channels as usize,
            "invalid MS ADPCM block size"
        );
        let expected = ((h.block_align as u32 - 7 * h.channels as u32) * 2 / h.channels as u32) + 2;
        ensure!(
            h.samples_per_block as u32 == expected,
            "inconsistent MS ADPCM samples per block"
        );
        ensure!(
            samples.len() % h.block_align as usize == 0,
            "partial MS ADPCM block"
        );
        for block in samples.chunks_exact(h.block_align as usize) {
            ensure!(
                block[..h.channels as usize].iter().all(|p| *p < 7),
                "invalid MS ADPCM predictor"
            );
        }
        fmt.extend_from_slice(&2u16.to_le_bytes());
        fmt.extend_from_slice(&h.channels.to_le_bytes());
        fmt.extend_from_slice(&h.sample_rate.to_le_bytes());
        fmt.extend_from_slice(&h.byte_rate.to_le_bytes());
        fmt.extend_from_slice(&h.block_align.to_le_bytes());
        fmt.extend_from_slice(&4u16.to_le_bytes());
        fmt.extend_from_slice(&32u16.to_le_bytes());
        fmt.extend_from_slice(&h.samples_per_block.to_le_bytes());
        fmt.extend_from_slice(&7u16.to_le_bytes());
        for (a, b) in [
            (256i16, 0i16),
            (512, -256),
            (0, 0),
            (192, 64),
            (240, 0),
            (460, -208),
            (392, -232),
        ] {
            fmt.extend_from_slice(&a.to_le_bytes());
            fmt.extend_from_slice(&b.to_le_bytes());
        }
        codec = "ms_adpcm";
        let capacity = (samples.len() / h.block_align as usize) as u64 * h.samples_per_block as u64;
        let frames = if h.samples > 0 {
            h.samples as u64
        } else {
            capacity
        };
        ensure!(
            frames <= capacity,
            "MS ADPCM sample count exceeds encoded blocks"
        );
        duration = frames as f64 / h.sample_rate as f64;
    }
    let mut bytes = b"RIFF\0\0\0\0WAVE".to_vec();
    riff_chunk(&mut bytes, b"fmt ", &fmt)?;
    if h.compression == 5 {
        let frames = if h.samples > 0 {
            h.samples as u64
        } else {
            (samples.len() / h.block_align as usize) as u64 * h.samples_per_block as u64
        };
        let frames = u32::try_from(frames).context("MS ADPCM sample count exceeds WAVE limits")?;
        riff_chunk(&mut bytes, b"fact", &frames.to_le_bytes())?;
    }
    riff_chunk(&mut bytes, b"data", samples)?;
    let size = u32::try_from(bytes.len() - 8).context("waveform exceeds RIFF size limits")?;
    bytes[4..8].copy_from_slice(&size.to_le_bytes());
    Ok(AudioFile {
        bytes,
        extension: "wav",
        codec,
        sample_rate: Some(h.sample_rate),
        channels: Some(h.channels),
        duration_seconds: Some(duration),
    })
}

fn riff_chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) -> Result<()> {
    out.extend_from_slice(kind);
    out.extend_from_slice(
        &u32::try_from(data.len())
            .context("RIFF chunk too large")?
            .to_le_bytes(),
    );
    out.extend_from_slice(data);
    if data.len() % 2 != 0 {
        out.push(0);
    }
    Ok(())
}

fn opus(h: &Header, body: Option<&[u8]>, stream: Option<&[u8]>) -> Result<AudioFile> {
    ensure!(
        h.channels <= 2,
        "multichannel Opus needs an explicit mapping"
    );
    ensure!(h.block_align > 0, "zero Opus packet length");
    let stream = stream.unwrap_or_default();
    ensure!(
        stream.len() <= h.size,
        "Opus stream exceeds declared audio size"
    );
    let resident_size = h.size - stream.len();
    let body = body.unwrap_or_default();
    ensure!(
        body.len() >= resident_size,
        "truncated Opus body: need {resident_size} audio bytes, found {}",
        body.len()
    );
    if !stream.is_empty() && resident_size > 0 && h.resident_size > 0 {
        ensure!(
            resident_size == h.resident_size,
            "Opus resident prefix length disagrees with header"
        );
    }
    // B ends with the resident beginning of the stream; F continues it.
    // Waveform envelopes and localized phoneme data may precede B's audio.
    // Taking the declared suffix excludes all such auxiliary records. If F
    // holds the complete stream, B contains metadata only and is ignored.
    // Ordering was verified against decoded sample continuity in the current
    // client: F+B would move the opening prefetch segment to the end.
    let combined = joined(Some(&body[body.len() - resident_size..]), Some(stream));
    ensure!(
        combined.len() % h.block_align as usize == 0,
        "partial Opus packet"
    );
    let packets: Vec<&[u8]> = combined.chunks_exact(h.block_align as usize).collect();
    let durations = packets
        .iter()
        .map(|p| opus_packet_samples(p))
        .collect::<Result<Vec<_>>>()?;
    let total: u64 = durations.iter().map(|n| *n as u64).sum();
    // For Opus this field stores encoder lookahead at the input sample rate,
    // rather than the byte rate used by PCM/ADPCM. The current client uses
    // CELT low-delay encoding (120 samples at 48 kHz, 60 at 24 kHz). Ogg uses
    // a fixed 48 kHz granule clock. Original counts trim final encoder padding.
    let pre_skip = u16::try_from(h.byte_rate as u64 * 48_000 / h.sample_rate as u64)
        .context("Opus encoder lookahead exceeds Ogg header limits")?;
    let wanted = h.samples as u64 * 48_000 / h.sample_rate as u64;
    let end = if wanted > 0 {
        wanted + pre_skip as u64
    } else {
        total
    };
    ensure!(
        end >= pre_skip as u64,
        "Opus stream is shorter than its encoder lookahead"
    );
    ensure!(
        end <= total
            && end > total.saturating_sub(*durations.last().context("no Opus packets")? as u64),
        "Opus sample count exceeds encoded packet duration"
    );
    let serial = 0x5346_4155;
    let mut bytes = Vec::new();
    let mut head = b"OpusHead".to_vec();
    head.extend_from_slice(&[1, h.channels as u8]);
    head.extend_from_slice(&pre_skip.to_le_bytes());
    head.extend_from_slice(&h.sample_rate.to_le_bytes());
    head.extend_from_slice(&0u16.to_le_bytes());
    head.push(0);
    ogg_page(&mut bytes, 2, 0, serial, 0, &[&head])?;
    let vendor = b"soulframe-audio";
    let mut tags = b"OpusTags".to_vec();
    tags.extend_from_slice(&(vendor.len() as u32).to_le_bytes());
    tags.extend_from_slice(vendor);
    tags.extend_from_slice(&0u32.to_le_bytes());
    ogg_page(&mut bytes, 0, 0, serial, 1, &[&tags])?;
    let mut first = 0;
    let mut sequence = 2;
    let mut granule = 0u64;
    while first < packets.len() {
        let mut next = first;
        let mut laces = 0;
        while next < packets.len() && next - first < 50 {
            let needed = packets[next].len() / 255 + 1;
            if laces + needed > 255 {
                break;
            }
            laces += needed;
            granule += durations[next] as u64;
            next += 1;
        }
        ensure!(next > first, "Opus packet exceeds Ogg page limits");
        let last = next == packets.len();
        ogg_page(
            &mut bytes,
            if last { 4 } else { 0 },
            if last { end } else { granule },
            serial,
            sequence,
            &packets[first..next],
        )?;
        first = next;
        sequence += 1;
    }
    Ok(AudioFile {
        bytes,
        extension: "ogg",
        codec: "opus",
        sample_rate: Some(h.sample_rate),
        channels: Some(h.channels),
        duration_seconds: Some((end - pre_skip as u64) as f64 / 48_000.0),
    })
}

fn opus_packet_samples(packet: &[u8]) -> Result<u32> {
    let toc = *packet.first().context("empty Opus packet")?;
    let per_frame = if toc & 0x80 != 0 {
        120 << ((toc >> 3) & 3)
    } else if toc & 0x60 == 0x60 {
        if toc & 8 != 0 {
            960
        } else {
            480
        }
    } else {
        match (toc >> 3) & 3 {
            0 => 480,
            1 => 960,
            2 => 1920,
            _ => 2880,
        }
    };
    let frames = match toc & 3 {
        0 => 1,
        1 | 2 => 2,
        _ => (packet.get(1).context("truncated Opus frame count")? & 63) as u32,
    };
    ensure!(
        frames > 0 && per_frame * frames <= 5760,
        "invalid Opus packet duration"
    );
    Ok(per_frame * frames)
}

pub(crate) fn ogg_page(
    out: &mut Vec<u8>,
    flags: u8,
    granule: u64,
    serial: u32,
    sequence: u32,
    packets: &[&[u8]],
) -> Result<()> {
    let mut laces = Vec::new();
    for p in packets {
        laces.extend(std::iter::repeat_n(255, p.len() / 255));
        laces.push((p.len() % 255) as u8);
    }
    ensure!(laces.len() <= 255, "too many Ogg page segments");
    let start = out.len();
    out.extend_from_slice(b"OggS");
    out.extend_from_slice(&[0, flags]);
    out.extend_from_slice(&granule.to_le_bytes());
    out.extend_from_slice(&serial.to_le_bytes());
    out.extend_from_slice(&sequence.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.push(laces.len() as u8);
    out.extend_from_slice(&laces);
    for p in packets {
        out.extend_from_slice(p);
    }
    let checksum = ogg_crc(&out[start..]);
    out[start + 22..start + 26].copy_from_slice(&checksum.to_le_bytes());
    Ok(())
}

const fn crc_table() -> [u32; 256] {
    let mut table = [0; 256];
    let mut i = 0;
    while i < 256 {
        let mut value = (i as u32) << 24;
        let mut bit = 0;
        while bit < 8 {
            value = (value << 1)
                ^ if value & 0x8000_0000 != 0 {
                    0x04c1_1db7
                } else {
                    0
                };
            bit += 1;
        }
        table[i] = value;
        i += 1;
    }
    table
}

fn ogg_crc(bytes: &[u8]) -> u32 {
    const TABLE: [u32; 256] = crc_table();
    bytes.iter().fold(0u32, |crc, byte| {
        (crc << 8) ^ TABLE[((crc >> 24) as u8 ^ byte) as usize]
    })
}

fn standard_wave(raw: &[u8]) -> Result<AudioFile> {
    ensure!(
        raw.get(8..12) == Some(b"WAVE"),
        "RIFF resource is not WAVE audio"
    );
    let end = (u32_at(raw, 4)? as usize)
        .checked_add(8)
        .context("RIFF size overflow")?;
    ensure!(end == raw.len(), "truncated or trailing WAVE data");
    let mut pos = 12;
    let mut format = None;
    let mut data_bytes = None;
    while pos < end {
        let kind = field::<4>(raw, pos)?;
        let length = u32_at(raw, pos + 4)? as usize;
        let start = pos + 8;
        let next = start
            .checked_add(length)
            .context("WAVE chunk size overflow")?;
        let bytes = raw.get(start..next).context("truncated WAVE chunk")?;
        if &kind == b"fmt " {
            ensure!(length >= 16, "short WAVE format chunk");
            format = Some((
                u16_at(bytes, 0)?,
                u16_at(bytes, 2)?,
                u32_at(bytes, 4)?,
                u32_at(bytes, 8)?,
            ));
        }
        if &kind == b"data" {
            data_bytes = Some(length);
        }
        pos = next
            .checked_add(length % 2)
            .context("WAVE chunk padding overflow")?;
        ensure!(pos <= end, "missing WAVE chunk padding");
    }
    let (tag, channels, rate, byte_rate) = format.context("WAVE has no format chunk")?;
    let data_bytes = data_bytes.context("WAVE has no data chunk")?;
    ensure!(
        channels > 0 && rate > 0 && byte_rate > 0,
        "invalid WAVE format"
    );
    let codec = match tag {
        1 => "pcm",
        2 => "ms_adpcm",
        3 => "ieee_float",
        17 => "ima_adpcm",
        _ => bail!("unsupported WAVE codec {tag}"),
    };
    Ok(AudioFile {
        bytes: raw.to_vec(),
        extension: "wav",
        codec,
        sample_rate: Some(rate),
        channels: Some(channels),
        duration_seconds: Some(data_bytes as f64 / byte_rate as f64),
    })
}

fn standard_ogg(raw: &[u8]) -> Result<AudioFile> {
    let mut pos = 0;
    let mut sequence = 0;
    let mut serial = None;
    let mut end = false;
    let mut first_packet = Vec::new();
    let mut first_done = false;
    let mut last_granule = 0;
    while pos < raw.len() {
        ensure!(
            raw.get(pos..pos + 4) == Some(b"OggS"),
            "invalid Ogg capture pattern"
        );
        ensure!(raw.get(pos + 4) == Some(&0), "unsupported Ogg version");
        let count = *raw.get(pos + 26).context("truncated Ogg header")? as usize;
        let laces = raw
            .get(pos + 27..pos + 27 + count)
            .context("truncated Ogg lacing table")?;
        let payload: usize = laces.iter().map(|n| *n as usize).sum();
        let next = pos
            .checked_add(27 + count + payload)
            .context("Ogg page size overflow")?;
        let page = raw.get(pos..next).context("truncated Ogg page")?;
        let mut checked = page.to_vec();
        checked[22..26].fill(0);
        ensure!(
            ogg_crc(&checked) == u32_at(page, 22)?,
            "Ogg checksum mismatch"
        );
        let page_serial = u32_at(page, 14)?;
        ensure!(
            serial.get_or_insert(page_serial) == &page_serial,
            "chained Ogg streams require separate extraction"
        );
        ensure!(
            u32_at(page, 18)? == sequence,
            "Ogg page sequence discontinuity"
        );
        sequence += 1;
        if pos == 0 {
            ensure!(page[5] & 2 != 0, "Ogg stream lacks beginning page");
        }
        ensure!(!end, "data after Ogg end page");
        end = page[5] & 4 != 0;
        last_granule = u64::from_le_bytes(field(page, 6)?);
        if !first_done {
            let mut offset = 27 + count;
            for &lace in laces {
                first_packet.extend_from_slice(&page[offset..offset + lace as usize]);
                offset += lace as usize;
                if lace < 255 {
                    first_done = true;
                    break;
                }
            }
        }
        pos = next;
    }
    ensure!(end && first_done, "incomplete Ogg logical stream");
    let (codec, rate, channels, duration) = if first_packet.starts_with(b"OpusHead") {
        ensure!(first_packet.len() >= 19, "short Opus identification packet");
        let pre_skip = u16_at(&first_packet, 10)? as u64;
        (
            "opus",
            48_000,
            first_packet[9] as u16,
            last_granule.saturating_sub(pre_skip) as f64 / 48_000.0,
        )
    } else if first_packet.starts_with(b"\x01vorbis") {
        ensure!(
            first_packet.len() >= 30,
            "short Vorbis identification packet"
        );
        let rate = u32_at(&first_packet, 12)?;
        ensure!(rate > 0, "invalid Vorbis sample rate");
        (
            "vorbis",
            rate,
            first_packet[11] as u16,
            last_granule as f64 / rate as f64,
        )
    } else {
        bail!("unsupported Ogg audio codec");
    };
    ensure!(channels > 0, "invalid Ogg channel count");
    Ok(AudioFile {
        bytes: raw.to_vec(),
        extension: "ogg",
        codec,
        sample_rate: Some(rate),
        channels: Some(channels),
        duration_seconds: Some(duration),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header(
        compression: u32,
        bits: u8,
        channels: u8,
        samples: u32,
        align: u16,
        size: u32,
    ) -> Vec<u8> {
        let mut raw = vec![0; 28 + 94];
        raw[24..28].copy_from_slice(&139u32.to_le_bytes());
        let data = &mut raw[28..];
        data[0..4].copy_from_slice(&compression.to_le_bytes());
        data[32..36].copy_from_slice(&48_000u32.to_le_bytes());
        data[36] = bits;
        data[37] = channels;
        data[38..42].copy_from_slice(&samples.to_le_bytes());
        data[42..46].copy_from_slice(&120u32.to_le_bytes());
        data[46..48].copy_from_slice(&align.to_le_bytes());
        data[48..50].copy_from_slice(&960u16.to_le_bytes());
        data[62..66].copy_from_slice(&size.to_le_bytes());
        raw
    }

    #[test]
    fn pcm_joins_body_then_stream_and_skips_auxiliary_prefix() {
        let h = header(0, 16, 1, 3, 2, 6);
        let audio = reconstruct(Some(&h), Some(&[99, 98, 1, 2]), Some(&[3, 4, 5, 6])).unwrap();
        assert_eq!(&audio.bytes[44..], &[1, 2, 3, 4, 5, 6]);
        assert_eq!(
            u32_at(&audio.bytes, 4).unwrap() as usize + 8,
            audio.bytes.len()
        );
        assert_eq!(audio.duration_seconds, Some(3.0 / 48_000.0));
        assert!(standard_wave(&audio.bytes).is_ok());
        assert!(reconstruct(Some(&h), Some(&[1]), None).is_err());
    }

    #[test]
    fn ms_adpcm_writes_coefficients_fact_and_correct_riff_size() {
        let mut h = header(5, 4, 1, 4, 8, 8);
        h[28 + 48..28 + 50].copy_from_slice(&4u16.to_le_bytes());
        let audio = reconstruct(Some(&h), Some(&[0; 8]), None).unwrap();
        assert_eq!(u16_at(&audio.bytes, 20).unwrap(), 2);
        assert_eq!(&audio.bytes[70..74], b"fact");
        assert_eq!(u32_at(&audio.bytes, 78).unwrap(), 4);
        assert_eq!(
            u32_at(&audio.bytes, 4).unwrap() as usize + 8,
            audio.bytes.len()
        );
        assert!(standard_wave(&audio.bytes).is_ok());
    }

    #[test]
    fn opus_body_prefix_then_stream_and_complete_stream_ignores_metadata_body() {
        // Distinct 20 ms packet payloads expose an accidental B/F rotation.
        let h = header(7, 16, 2, 1608, 3, 6);
        let first = [0xf8, 0xff, 0xfe];
        let last = [0xf8, 0xff, 0xfd];
        let audio = reconstruct(Some(&h), Some(&first), Some(&last)).unwrap();
        assert_eq!(audio.codec, "opus");
        assert_eq!(audio.duration_seconds, Some(1608.0 / 48_000.0));
        assert!(standard_ogg(&audio.bytes).is_ok());
        let full = [first, last].concat();
        assert!(audio.bytes.ends_with(&full));
        assert_eq!(
            audio.bytes,
            reconstruct(Some(&h), Some(&[1; 64]), Some(&full))
                .unwrap()
                .bytes
        );
        // Both split and resident-only assets can have envelope/phoneme data
        // whose length is unrelated to the encoded packet size.
        let prefixed = [&[19, 23, 41, 57][..], &first].concat();
        assert_eq!(
            audio.bytes,
            reconstruct(Some(&h), Some(&prefixed), Some(&last))
                .unwrap()
                .bytes
        );
        let prefixed = [&[19, 23, 41, 57][..], &full].concat();
        assert_eq!(
            audio.bytes,
            reconstruct(Some(&h), Some(&prefixed), None).unwrap().bytes
        );
        assert!(reconstruct(Some(&h), Some(&first[..2]), Some(&last)).is_err());
        let mut corrupted = audio.bytes.clone();
        *corrupted.last_mut().unwrap() ^= 1;
        assert!(standard_ogg(&corrupted).is_err());
        assert!(standard_ogg(&audio.bytes[..audio.bytes.len() - 1]).is_err());
    }

    #[test]
    fn opus_24khz_scales_lookahead_and_trims_final_packet_at_48khz() {
        let mut h = header(7, 16, 1, 900, 3, 6);
        h[28 + 32..28 + 36].copy_from_slice(&24_000u32.to_le_bytes());
        h[28 + 42..28 + 46].copy_from_slice(&60u32.to_le_bytes());
        let packet = [0xf8, 0xff, 0xfe];
        let full = [packet, packet].concat();
        let audio = reconstruct(Some(&h), None, Some(&full)).unwrap();
        // Two 20 ms packets contain 1,920 samples at Ogg's 48 kHz clock.
        // The 60 input-rate lookahead samples become 120 granule samples.
        assert_eq!(u16_at(&audio.bytes, 28 + 10).unwrap(), 120);
        assert_eq!(u32_at(&audio.bytes, 28 + 12).unwrap(), 24_000);
        assert_eq!(audio.duration_seconds, Some(900.0 / 24_000.0));
        let last = audio.bytes.windows(4).rposition(|w| w == b"OggS").unwrap();
        assert_eq!(
            u64::from_le_bytes(field(&audio.bytes, last + 6).unwrap()),
            1920
        );

        // Preserve the original shorter clip rather than exposing the final
        // packet's remaining encoder padding as additional audio.
        h[28 + 38..28 + 42].copy_from_slice(&877u32.to_le_bytes());
        let audio = reconstruct(Some(&h), None, Some(&full)).unwrap();
        let last = audio.bytes.windows(4).rposition(|w| w == b"OggS").unwrap();
        assert_eq!(
            u64::from_le_bytes(field(&audio.bytes, last + 6).unwrap()),
            1874
        );
        assert_eq!(audio.duration_seconds, Some(877.0 / 24_000.0));
        assert!(standard_ogg(&audio.bytes).is_ok());
    }

    #[test]
    fn ogg_lacing_terminates_exact_multiple_of_255() {
        let mut bytes = Vec::new();
        ogg_page(&mut bytes, 4, 960, 7, 0, &[&[1; 255]]).unwrap();
        assert_eq!(&bytes[26..29], &[2, 255, 0]);
        let expected = u32_at(&bytes, 22).unwrap();
        bytes[22..26].fill(0);
        assert_eq!(ogg_crc(&bytes), expected);
        assert_eq!(ogg_crc(b"123456789"), 0x89a1_897f);
    }

    #[test]
    fn opus_packet_durations_cover_modes_and_multiple_frames() {
        assert_eq!(opus_packet_samples(&[0x80]).unwrap(), 120);
        assert_eq!(opus_packet_samples(&[0xf8]).unwrap(), 960);
        assert_eq!(opus_packet_samples(&[0x60]).unwrap(), 480);
        assert_eq!(opus_packet_samples(&[0x18]).unwrap(), 2880);
        assert_eq!(opus_packet_samples(&[0x19]).unwrap(), 5760);
        assert_eq!(opus_packet_samples(&[0x83, 3]).unwrap(), 360);
        assert!(opus_packet_samples(&[0x83]).is_err());
        assert!(opus_packet_samples(&[0x83, 0]).is_err());
        assert!(opus_packet_samples(&[0x83, 63]).is_err());
    }

    #[test]
    fn malformed_and_unknown_resources_fail_without_panicking() {
        for length in 0..122 {
            assert!(reconstruct(Some(&vec![0; length]), None, None).is_err());
        }
        let h = header(8, 16, 2, 960, 320, 320);
        assert!(format!(
            "{:#}",
            reconstruct(Some(&h), Some(&[0; 320]), None).err().unwrap()
        )
        .contains("Vorbis"));
        let mut h = header(0, 16, 2, 1, 4, 4);
        h[16..20].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(reconstruct(Some(&h), None, None).is_err());
    }
}
