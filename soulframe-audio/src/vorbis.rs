//! Restore the engine's raw Vorbis packets using the matching decoder headers
//! embedded in the installed game executable. No game codebooks are bundled.

use anyhow::{anyhow, bail, ensure, Context, Result};
use std::fs;
use std::path::Path;

use crate::format::{ogg_page, AudioFile};

struct Profile {
    identification: Vec<u8>,
    comment: Vec<u8>,
    setup: Vec<u8>,
    rate: u32,
    channels: u16,
    nominal_bitrate: u32,
}

pub struct VorbisHeaders {
    profiles: Vec<Profile>,
}

fn bytes<const N: usize>(raw: &[u8], offset: usize) -> Result<[u8; N]> {
    raw.get(offset..offset.checked_add(N).context("Vorbis offset overflow")?)
        .context("truncated Vorbis data")?
        .try_into()
        .map_err(Into::into)
}

fn u16_at(raw: &[u8], offset: usize) -> Result<u16> {
    Ok(u16::from_le_bytes(bytes(raw, offset)?))
}

fn u32_at(raw: &[u8], offset: usize) -> Result<u32> {
    Ok(u32::from_le_bytes(bytes(raw, offset)?))
}

fn u64_at(raw: &[u8], offset: usize) -> Result<u64> {
    Ok(u64::from_le_bytes(bytes(raw, offset)?))
}

struct Section {
    address: u32,
    offset: usize,
    length: usize,
}

fn find(raw: &[u8], start: usize, end: usize, needle: &[u8]) -> Option<usize> {
    let search = raw.get(start..end.min(raw.len()))?;
    search
        .windows(needle.len())
        .position(|w| w == needle)
        .map(|p| start + p)
}

impl VorbisHeaders {
    /// Read Vorbis identification/comment/setup packets from the client's PE
    /// data. Each packet's own `ogg_packet` descriptor supplies its exact size;
    /// sections map its file position to the descriptor's virtual address.
    pub fn from_exe(path: &Path) -> Result<Self> {
        let raw = fs::read(path)
            .with_context(|| format!("read Vorbis headers from {}", path.display()))?;
        Self::from_pe(&raw)
    }

    fn from_pe(raw: &[u8]) -> Result<Self> {
        ensure!(
            raw.starts_with(b"MZ"),
            "Vorbis header source is not a PE executable"
        );
        let pe = u32_at(raw, 0x3c)? as usize;
        ensure!(
            raw.get(pe..pe + 4) == Some(b"PE\0\0"),
            "invalid PE signature"
        );
        let section_count = u16_at(raw, pe + 6)? as usize;
        let optional_len = u16_at(raw, pe + 20)? as usize;
        ensure!(
            u16_at(raw, pe + 24)? == 0x20b,
            "Vorbis source needs a PE32+ client"
        );
        let image_base = u64_at(raw, pe + 48)?;
        let table = pe
            .checked_add(24 + optional_len)
            .context("PE table overflow")?;
        ensure!(section_count <= 128, "invalid PE section count");
        let mut sections = Vec::new();
        for i in 0..section_count {
            let offset = table + i * 40;
            let section = Section {
                address: u32_at(raw, offset + 12)?,
                length: u32_at(raw, offset + 16)? as usize,
                offset: u32_at(raw, offset + 20)? as usize,
            };
            ensure!(
                section
                    .offset
                    .checked_add(section.length)
                    .is_some_and(|end| end <= raw.len()),
                "PE section exceeds executable"
            );
            sections.push(section);
        }

        let mut profiles = Vec::new();
        let mut next = 0;
        while let Some(start) = find(raw, next, raw.len(), b"\x01vorbis") {
            next = start + 7;
            let result = (|| -> Result<Profile> {
                let identification = raw
                    .get(start..start + 30)
                    .context("truncated Vorbis identification")?;
                ensure!(
                    u32_at(identification, 7)? == 0 && identification[29] == 1,
                    "invalid Vorbis identification"
                );
                let channels = identification[11] as u16;
                let rate = u32_at(identification, 12)?;
                ensure!(
                    (1..=8).contains(&channels) && (8_000..=192_000).contains(&rate),
                    "invalid Vorbis profile"
                );
                let comment_start = find(raw, start + 30, start + 256, b"\x03vorbis")
                    .context("missing embedded Vorbis comment")?;
                let vendor_size = u32_at(raw, comment_start + 7)? as usize;
                ensure!(vendor_size < 65536, "oversized Vorbis vendor");
                let mut comment_end = comment_start + 11 + vendor_size;
                let comments = u32_at(raw, comment_end)? as usize;
                ensure!(comments < 1024, "oversized Vorbis comment list");
                comment_end += 4;
                for _ in 0..comments {
                    let size = u32_at(raw, comment_end)? as usize;
                    comment_end = comment_end
                        .checked_add(4)
                        .and_then(|p| p.checked_add(size))
                        .context("Vorbis comment overflow")?;
                }
                ensure!(
                    raw.get(comment_end) == Some(&1),
                    "missing Vorbis comment framing bit"
                );
                comment_end += 1;
                let setup_start = find(raw, comment_end, comment_end + 256, b"\x05vorbis")
                    .context("missing embedded Vorbis setup")?;
                let section = sections
                    .iter()
                    .find(|s| setup_start >= s.offset && setup_start - s.offset < s.length)
                    .context("Vorbis setup has no PE section")?;
                let address = image_base
                    .checked_add(section.address as u64)
                    .and_then(|a| a.checked_add((setup_start - section.offset) as u64))
                    .context("Vorbis setup address overflow")?;
                let descriptor = find(
                    raw,
                    setup_start + 7,
                    setup_start + 65_536,
                    &address.to_le_bytes(),
                )
                .context("missing Vorbis packet descriptor")?;
                let length = u32_at(raw, descriptor + 8)? as usize;
                ensure!(
                    (8..=65_024).contains(&length)
                        && setup_start
                            .checked_add(length)
                            .is_some_and(|end| end <= descriptor),
                    "invalid embedded Vorbis setup size"
                );
                let setup = raw
                    .get(setup_start..setup_start + length)
                    .context("truncated Vorbis setup")?;
                Ok(Profile {
                    identification: identification.to_vec(),
                    comment: raw
                        .get(comment_start..comment_end)
                        .context("truncated Vorbis comments")?
                        .to_vec(),
                    setup: setup.to_vec(),
                    channels,
                    rate,
                    nominal_bitrate: u32_at(identification, 20)?,
                })
            })();
            if let Ok(profile) = result {
                profiles.push(profile);
            }
        }
        ensure!(
            !profiles.is_empty(),
            "no supported embedded Vorbis header sets in executable"
        );
        Ok(Self { profiles })
    }

    fn profile(&self, rate: u32, channels: u16, quality: u32) -> Result<&Profile> {
        // Format 8 stores encoder quality in hundred-thousandths in the field
        // used for byte rate by PCM. These are the client's shipped presets.
        // The nominal bitrate identifies the exact matching template; choosing
        // an arbitrary profile can decode with incorrect codebooks.
        let nominal = match (rate, channels, quality) {
            (32_000, 1, 50_000) => 82_000,
            (32_000, 2, 50_000) => 135_000,
            (44_100, 1, 75_000) => 125_000,
            (44_100, 2, 75_000) => 232_000,
            (44_100, 1, 80_000) => 136_000,
            (44_100, 2, 80_000) => 249_600,
            (44_100, 1, 85_000) => 147_000,
            (44_100, 2, 85_000) => 278_400,
            (44_100, 1, 90_000) => 158_000,
            (44_100, 2, 90_000) => 313_600,
            _ => bail!(
                "unsupported Vorbis preset: {rate} Hz, {channels} channels, quality {quality}"
            ),
        };
        self.profiles
            .iter()
            .find(|p| p.rate == rate && p.channels == channels && p.nominal_bitrate == nominal)
            .ok_or_else(|| anyhow!("installed client lacks matching Vorbis header preset"))
    }
}

fn audio_header(raw: &[u8]) -> Result<&[u8]> {
    let count = u32_at(raw, 16)? as usize;
    ensure!(count <= raw.len() / 4, "invalid audio references");
    let mut pos = 20;
    for _ in 0..count {
        let length = u32_at(raw, pos)? as usize;
        pos = pos
            .checked_add(4)
            .and_then(|p| p.checked_add(length))
            .context("audio reference overflow")?;
        ensure!(pos <= raw.len(), "truncated audio reference");
    }
    let attributes = u32_at(raw, pos)? as usize;
    pos = pos
        .checked_add(4)
        .and_then(|p| p.checked_add(attributes))
        .and_then(|p| p.checked_add(usize::from(attributes > 0)))
        .context("audio attributes overflow")?;
    ensure!(u32_at(raw, pos)? == 139, "unsupported Vorbis resource type");
    raw.get(pos + 4..).context("missing Vorbis header")
}

pub(crate) fn reconstruct(
    header: &[u8],
    body: Option<&[u8]>,
    stream: Option<&[u8]>,
    headers: &VorbisHeaders,
) -> Result<AudioFile> {
    let h = audio_header(header)?;
    ensure!(u32_at(h, 0)? == 8, "resource is not Evolution Vorbis");
    let rate = u32_at(h, 32)?;
    let channels = *h.get(37).context("missing Vorbis channels")? as u16;
    let samples = u32_at(h, 38)?;
    let quality = u32_at(h, 42)?;
    let size = u32_at(h, 62)? as usize;
    let profile = headers.profile(rate, channels, quality)?;
    let b = body.context("Vorbis packet seek table is missing from B cache")?;
    let table_len = (u32_at(h, 58)? as usize)
        .checked_mul(4)
        .context("Vorbis seek table size overflow")?;
    // Waveform previews, localized lip-sync and other auxiliary vectors precede
    // the table. The table itself is immediately before the B audio suffix;
    // streamed sounds can hold their entire payload in F.
    let streamed = stream.map_or(0, <[u8]>::len);
    let buffered = size
        .checked_sub(streamed)
        .context("Vorbis stream exceeds declared audio size")?;
    let end = b
        .len()
        .checked_sub(buffered)
        .context("truncated Vorbis buffered audio")?;
    let start = end
        .checked_sub(table_len)
        .context("truncated Vorbis seek table")?;
    ensure!(
        start >= u32_at(h, 8)? as usize,
        "Vorbis table overlaps auxiliary waveform data"
    );
    let table = b.get(start..end).context("truncated Vorbis seek table")?;
    let count = u16_at(table, 0)? as usize;
    let envelope = u16_at(table, 2)? as usize;
    ensure!(
        count > 0 && 4 + 4 * envelope + 8 * count == table.len(),
        "invalid Vorbis seek table counts"
    );
    let offsets_start = 4 + 4 * envelope;
    let granules_start = offsets_start + 4 * count;
    let mut offsets = Vec::with_capacity(count + 1);
    let mut granules = Vec::with_capacity(count);
    for i in 0..count {
        offsets.push(u32_at(table, offsets_start + i * 4)? as usize);
        granules.push(u32_at(table, granules_start + i * 4)? as u64);
    }
    ensure!(
        offsets.first() == Some(&0)
            && offsets.windows(2).all(|w| w[0] < w[1])
            && offsets[count - 1] < size,
        "invalid Vorbis packet boundaries"
    );
    ensure!(
        granules.windows(2).all(|w| w[0] <= w[1]) && granules[count - 1] == samples as u64,
        "invalid Vorbis sample boundaries"
    );
    offsets.push(size);
    let mut payload = b.get(end..).context("missing Vorbis payload")?.to_vec();
    payload.extend_from_slice(stream.unwrap_or_default());
    ensure!(
        payload.len() == size,
        "Vorbis payload length mismatch: expected {size}, got {}",
        payload.len()
    );
    let packets: Vec<&[u8]> = offsets.windows(2).map(|w| &payload[w[0]..w[1]]).collect();
    ensure!(
        packets.iter().all(|p| !p.is_empty() && p[0] & 1 == 0),
        "invalid Vorbis audio packet type"
    );
    let mut bytes = Vec::with_capacity(size + table_len / 8 + 8192);
    let serial = 0x5346_5652;
    ogg_page(&mut bytes, 2, 0, serial, 0, &[&profile.identification])?;
    ogg_page(&mut bytes, 0, 0, serial, 1, &[&profile.comment])?;
    ogg_page(&mut bytes, 0, 0, serial, 2, &[&profile.setup])?;
    let mut first = 0;
    let mut sequence = 3;
    while first < packets.len() {
        let mut next = first;
        let mut laces = 0;
        while next < packets.len() && next - first < 50 {
            let needed = packets[next].len() / 255 + 1;
            if laces + needed > 255 {
                break;
            }
            laces += needed;
            next += 1;
        }
        ensure!(next > first, "Vorbis packet exceeds Ogg page limits");
        ogg_page(
            &mut bytes,
            if next == packets.len() { 4 } else { 0 },
            granules[next - 1],
            serial,
            sequence,
            &packets[first..next],
        )?;
        sequence += 1;
        first = next;
    }
    Ok(AudioFile {
        bytes,
        extension: "ogg",
        codec: "vorbis",
        sample_rate: Some(rate),
        channels: Some(channels),
        duration_seconds: Some(samples as f64 / rate as f64),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn malformed_executables_fail_without_panicking() {
        for raw in [&[][..], b"MZ", b"MZ\0\0", b"not a client"] {
            assert!(VorbisHeaders::from_pe(raw).is_err());
        }
    }

    #[test]
    fn unknown_presets_do_not_select_incompatible_codebooks() {
        let headers = VorbisHeaders {
            profiles: Vec::new(),
        };
        assert!(headers.profile(48_000, 2, 85_000).is_err());
        assert!(headers.profile(44_100, 2, 85_000).is_err());
    }

    #[test]
    fn localized_auxiliary_vectors_are_kept_outside_the_packet_table_and_audio() {
        // Speech can have waveform previews AND lip-sync vectors before the
        // seek table. Its waveform-length field alone does not locate it.
        let mut raw = vec![0; 16];
        raw.extend_from_slice(&0u32.to_le_bytes()); // references
        raw.extend_from_slice(&0u32.to_le_bytes()); // attributes
        raw.extend_from_slice(&139u32.to_le_bytes());
        let mut h = vec![0; 94];
        h[0..4].copy_from_slice(&8u32.to_le_bytes());
        h[8..12].copy_from_slice(&3u32.to_le_bytes()); // waveform only
        h[32..36].copy_from_slice(&44_100u32.to_le_bytes());
        h[37] = 2;
        h[38..42].copy_from_slice(&64u32.to_le_bytes());
        h[42..46].copy_from_slice(&85_000u32.to_le_bytes());
        h[58..62].copy_from_slice(&5u32.to_le_bytes());
        h[62..66].copy_from_slice(&2u32.to_le_bytes());
        raw.extend_from_slice(&h);
        let mut body = vec![0xff; 14]; // three waveform + eleven lip-sync bytes
        body.extend_from_slice(&2u16.to_le_bytes()); // two packet starts
        body.extend_from_slice(&0u16.to_le_bytes()); // no envelope entries
        for value in [0u32, 1, 0, 64] {
            body.extend_from_slice(&value.to_le_bytes());
        }
        // The header bytes are opaque to the remuxer. These synthetic packets
        // test framing and preservation without including client codebooks.
        let headers = VorbisHeaders {
            profiles: vec![Profile {
                identification: vec![1],
                comment: vec![3],
                setup: vec![5],
                rate: 44_100,
                channels: 2,
                nominal_bitrate: 278_400,
            }],
        };
        let result = reconstruct(&raw, Some(&body), Some(&[0, 2]), &headers).unwrap();
        assert_eq!(result.codec, "vorbis");
        let page = result.bytes.windows(4).rposition(|w| w == b"OggS").unwrap();
        assert_eq!(u64_at(&result.bytes, page + 6).unwrap(), 64);
        assert_eq!(result.bytes[page + 26], 2);
        assert_eq!(&result.bytes[page + 27..], &[1, 1, 0, 2]);
        assert!(reconstruct(&raw, Some(&body[..body.len() - 1]), Some(&[0, 2]), &headers).is_err());
    }
}
