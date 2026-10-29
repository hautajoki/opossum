//! Audio inspection against Roblox's limits, so a file it would refuse fails before upload with a reason instead of
//! as a transcoding error after it. Tessera's parsers, ported line for line: headers only, page CRCs unchecked, and no
//! input of any length or content panics (every read is bounds-checked; the no-panic loop below holds it to that).

/// Roblox's limit: 420 s passes, anything longer fails.
const MAX_SECONDS: f64 = 7.0 * 60.0;

/// What a file's headers say. `seconds` is 0 when the format leaves the length unknown (a FLAC without a sample count).
struct Info {
    seconds: f64,
    rate: u64,
    channels: u64,
}

/// Inspects audio by extension (`.ogg`, `.mp3`, `.wav`, `.flac`) and checks it against Roblox's limits: at most 7
/// minutes, a sample rate of at most 48 kHz, and mono, stereo, 3.0 or 5.1.
pub fn inspect_audio(ext: &str, bytes: &[u8]) -> Result<(), String> {
    let a = info(ext, bytes)?;
    if a.seconds > MAX_SECONDS {
        return Err(format!("{:.1} s long; Roblox takes audio up to 7 minutes", a.seconds));
    }
    if a.rate == 0 {
        return Err("sample rate could not be determined".into());
    }
    if a.rate > 48_000 {
        return Err(format!("sample rate {} Hz exceeds Roblox's 48 kHz limit", a.rate));
    }
    if !matches!(a.channels, 1 | 2 | 3 | 6) {
        return Err(format!(
            "channel count {} is unsupported; Roblox accepts mono, stereo 2.0, 3.0, or 5.1 audio",
            a.channels
        ));
    }
    Ok(())
}

fn info(ext: &str, bytes: &[u8]) -> Result<Info, String> {
    match ext {
        ".ogg" => ogg(bytes),
        ".mp3" => mp3(bytes),
        ".wav" => wav(bytes),
        ".flac" => flac(bytes),
        _ => Err(format!("{ext} is not audio opossum inspects")),
    }
}

/// `n` bytes at `at`, little-endian; 0 when they run past the end (callers check lengths first, this only makes a
/// missed check wrong instead of a panic).
fn le(b: &[u8], at: usize, n: usize) -> u64 {
    b.get(at..at.saturating_add(n)).map_or(0, |s| s.iter().rev().fold(0, |v, &x| v << 8 | u64::from(x)))
}

fn be(b: &[u8], at: usize, n: usize) -> u64 {
    b.get(at..at.saturating_add(n)).map_or(0, |s| s.iter().fold(0, |v, &x| v << 8 | u64::from(x)))
}

fn wav(b: &[u8]) -> Result<Info, String> {
    if b.len() < 12 || &b[..4] != b"RIFF" || &b[8..12] != b"WAVE" {
        return Err("WAV header must start with RIFF/WAVE".into());
    }
    // Bytes past the RIFF payload are not chunks.
    let end = le(b, 4, 4) as usize + 8;
    if end < 12 || end > b.len() {
        return Err("WAV RIFF payload is truncated".into());
    }
    let b = &b[..end];
    let (mut format, mut channels, mut rate, mut byte_rate, mut align) = (0, 0, 0, 0, 0);
    let (mut fmt, mut data) = (false, 0u64);
    let mut at = 12;
    while at < b.len() {
        if at + 8 > b.len() {
            return Err("WAV contains a truncated chunk header".into());
        }
        let size = le(b, at + 4, 4) as usize;
        let (start, end) = (at + 8, at + 8 + size);
        if end > b.len() {
            return Err("WAV contains a truncated chunk".into());
        }
        match &b[at..at + 4] {
            b"fmt " => {
                if fmt {
                    return Err("WAV contains multiple fmt chunks".into());
                }
                if size < 16 {
                    return Err("WAV fmt chunk is too short".into());
                }
                format = le(b, start, 2);
                channels = le(b, start + 2, 2);
                rate = le(b, start + 4, 4);
                byte_rate = le(b, start + 8, 4);
                align = le(b, start + 12, 2);
                fmt = true;
            }
            b"data" => {
                if !fmt {
                    return Err("WAV data chunk precedes fmt metadata".into());
                }
                data += size as u64;
            }
            _ => {}
        }
        at = end + size % 2;
        if at > b.len() {
            return Err("WAV chunk is missing its alignment byte".into());
        }
    }
    if !fmt || format == 0 || rate == 0 || channels == 0 || byte_rate == 0 || align == 0 {
        return Err("WAV is missing valid fmt metadata".into());
    }
    if data == 0 {
        return Err("WAV has no audio data".into());
    }
    if data % align != 0 {
        return Err("WAV audio data does not contain complete sample blocks".into());
    }
    // PCM, IEEE float and extensible.
    if matches!(format, 1 | 3 | 0xFFFE) && byte_rate != rate * align {
        return Err("WAV PCM byte rate is inconsistent with its sample rate and block alignment".into());
    }
    Ok(Info { seconds: data as f64 / byte_rate as f64, rate, channels })
}

fn flac(b: &[u8]) -> Result<Info, String> {
    let b = &b[id3(b).map_err(|e| format!("FLAC {e}"))?..];
    if b.len() < 8 || &b[..4] != b"fLaC" {
        return Err("FLAC header must start with fLaC".into());
    }
    let mut info = Info { seconds: 0.0, rate: 0, channels: 0 };
    let mut at = 4;
    loop {
        if at + 4 > b.len() {
            return Err("FLAC metadata chain is truncated".into());
        }
        let (header, size) = (b[at], be(b, at + 1, 3) as usize);
        let kind = header & 0x7F;
        let (start, end) = (at + 4, at + 4 + size);
        if end > b.len() {
            return Err("FLAC contains a truncated metadata block".into());
        }
        if at == 4 {
            if kind != 0 {
                return Err("FLAC STREAMINFO must be the first metadata block".into());
            }
            if size != 34 {
                return Err("FLAC STREAMINFO block must be 34 bytes".into());
            }
            let (min_block, max_block) = (be(b, start, 2), be(b, start + 2, 2));
            if min_block < 16 || max_block < min_block {
                return Err("FLAC STREAMINFO has invalid block-size bounds".into());
            }
            let packed = be(b, start + 10, 8);
            let rate = packed >> 44 & 0xF_FFFF;
            let bits = (packed >> 36 & 0x1F) + 1;
            let samples = packed & 0xF_FFFF_FFFF;
            if rate == 0 {
                return Err("FLAC STREAMINFO has no sample rate".into());
            }
            if bits < 4 {
                return Err("FLAC STREAMINFO has an invalid bit depth".into());
            }
            info = Info { seconds: samples as f64 / rate as f64, rate, channels: (packed >> 41 & 7) + 1 };
        } else if kind == 0 {
            return Err("FLAC contains multiple STREAMINFO blocks".into());
        } else if kind == 127 {
            return Err("FLAC contains a forbidden metadata block type".into());
        }
        at = end;
        if header & 0x80 != 0 {
            return Ok(info);
        }
    }
}

fn ogg(b: &[u8]) -> Result<Info, String> {
    let mut serial = None;
    let (mut first, mut complete) = (Vec::new(), false);
    let mut granule = 0;
    let mut at = 0;
    while at < b.len() {
        if at + 27 > b.len() || &b[at..at + 4] != b"OggS" || b[at + 4] != 0 {
            return Err("OGG contains an invalid or truncated page header".into());
        }
        let segments = usize::from(b[at + 26]);
        let payload = at + 27 + segments;
        if payload > b.len() {
            return Err("OGG contains a truncated segment table".into());
        }
        let page_serial = le(b, at + 14, 4);
        if *serial.get_or_insert(page_serial) != page_serial {
            return Err("OGG contains multiple logical streams; Roblox requires one track/stream".into());
        }
        let g = le(b, at + 6, 8);
        if g != u64::MAX {
            granule = g;
        }
        let lacing = &b[at + 27..payload];
        let end = payload + lacing.iter().map(|&s| usize::from(s)).sum::<usize>();
        if end > b.len() {
            return Err("OGG contains truncated page data".into());
        }
        // The first packet: segments until one shorter than 255, across pages if need be.
        if !complete {
            let mut p = payload;
            for &s in lacing {
                first.extend_from_slice(&b[p..p + usize::from(s)]);
                p += usize::from(s);
                if s < 255 {
                    complete = true;
                    break;
                }
            }
        }
        at = end;
    }
    if !complete {
        return Err("OGG is missing codec metadata".into());
    }
    let p = &first[..];
    if p.starts_with(b"\x01vorbis") {
        if p.len() < 30 {
            return Err("OGG Vorbis identification header is truncated".into());
        }
        if le(p, 7, 4) != 0 {
            return Err("OGG Vorbis identification header has an unsupported version".into());
        }
        let rate = le(p, 12, 4);
        if rate == 0 {
            return Err("OGG Vorbis header has no sample rate".into());
        }
        let (small, large) = (p[28] & 0x0F, p[28] >> 4);
        if small < 6 || large > 13 || small > large || p[29] & 1 == 0 {
            return Err("OGG Vorbis identification header has invalid block sizes or framing".into());
        }
        return Ok(Info { seconds: granule as f64 / rate as f64, rate, channels: u64::from(p[11]) });
    }
    if p.starts_with(b"OpusHead") {
        if p.len() < 19 {
            return Err("OGG Opus identification header is truncated".into());
        }
        if p[8] >= 16 {
            return Err("OGG Opus identification header has an unsupported version".into());
        }
        let channels = usize::from(p[9]);
        let pre_skip = le(p, 10, 2);
        match p[18] {
            0 => {
                if !(1..=2).contains(&channels) || p.len() != 19 {
                    return Err("OGG Opus mapping family 0 requires a mono or stereo 19-byte header".into());
                }
            }
            1 => {
                if !(1..=8).contains(&channels) || p.len() < 21 + channels {
                    return Err("OGG Opus surround mapping header is truncated".into());
                }
                let (streams, coupled) = (usize::from(p[19]), usize::from(p[20]));
                if streams == 0 || coupled > streams || streams + coupled != channels {
                    return Err("OGG Opus surround mapping has inconsistent stream counts".into());
                }
                if p[21..21 + channels].iter().any(|&m| usize::from(m) >= streams + coupled) {
                    return Err("OGG Opus surround mapping contains an invalid channel index".into());
                }
            }
            family => {
                return Err(format!(
                    "OGG Opus channel mapping family {family} cannot guarantee a Roblox-supported layout"
                ));
            }
        }
        if granule > 0 && granule < pre_skip {
            return Err("OGG Opus final granule precedes its pre-skip".into());
        }
        let seconds = granule.saturating_sub(pre_skip) as f64 / 48_000.0;
        return Ok(Info { seconds, rate: 48_000, channels: channels as u64 });
    }
    Err("OGG codec is not Vorbis or Opus".into())
}

/// The first valid Layer III frame starts the stream; after it, frames run back to back until a tag or zero padding.
/// Rate and channels are the largest any frame has (a VBR info frame counts as audio, as in tessera).
fn mp3(b: &[u8]) -> Result<Info, String> {
    let start = id3(b).map_err(|e| format!("MP3 {e}"))?;
    let Some(mut at) =
        (start..b.len().saturating_sub(3)).find(|&i| mp3_frame(&b[i..]).is_some_and(|f| i + f.len <= b.len()))
    else {
        return Err("MP3 contains no MPEG Layer III frames".into());
    };
    let (mut seconds, mut rate, mut channels) = (0.0, 0, 0);
    while at + 4 <= b.len() && !tail(&b[at..]) {
        let Some(f) = mp3_frame(&b[at..]) else {
            return Err(format!("MP3 frame at byte {at} has an invalid header"));
        };
        if at + f.len > b.len() {
            return Err(format!("MP3 frame at byte {at} is truncated"));
        }
        rate = rate.max(f.rate);
        channels = channels.max(f.channels);
        seconds += f.samples as f64 / f.rate as f64;
        at += f.len;
    }
    if at < b.len() && !tail(&b[at..]) {
        return Err(format!("MP3 has invalid trailing data at byte {at}"));
    }
    if seconds == 0.0 {
        return Err("MP3 contains no complete audio frames".into());
    }
    Ok(Info { seconds, rate, channels })
}

struct Mp3Frame {
    len: usize,
    samples: u64,
    rate: u64,
    channels: u64,
}

fn mp3_frame(b: &[u8]) -> Option<Mp3Frame> {
    let h = be(b, 0, 4);
    if b.len() < 4 || h >> 21 != 0x7FF {
        return None;
    }
    // Version 1 is reserved; layer 1 is Layer III.
    let version = h >> 19 & 3;
    if version == 1 || h >> 17 & 3 != 1 {
        return None;
    }
    let (bitrate, rate) = ((h >> 12 & 0xF) as usize, (h >> 10 & 3) as usize);
    if bitrate == 0 || bitrate == 15 || rate == 3 {
        return None;
    }
    const MPEG1: [u64; 15] = [0, 32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320];
    const MPEG2: [u64; 15] = [0, 8, 16, 24, 32, 40, 48, 56, 64, 80, 96, 112, 128, 144, 160];
    let rate = [44_100, 48_000, 32_000][rate];
    let (kbps, coefficient, samples, rate) = match version {
        3 => (MPEG1[bitrate], 144, 1152, rate),
        2 => (MPEG2[bitrate], 72, 576, rate / 2),
        _ => (MPEG2[bitrate], 72, 576, rate / 4),
    };
    let len = (coefficient * kbps * 1000 / rate + (h >> 9 & 1)) as usize;
    let channels = if h >> 6 & 3 == 3 { 1 } else { 2 };
    (len >= 4).then_some(Mp3Frame { len, samples, rate, channels })
}

/// What may follow the last frame: an ID3v1, APE or ID3v2 tag, or zero padding.
fn tail(b: &[u8]) -> bool {
    b.starts_with(b"TAG") || b.starts_with(b"APETAGEX") || b.starts_with(b"ID3") || b.iter().all(|&x| x == 0)
}

/// The offset past a leading ID3v2 tag (its footer too), or 0 without one.
fn id3(b: &[u8]) -> Result<usize, String> {
    if !b.starts_with(b"ID3") {
        return Ok(0);
    }
    if b.len() < 10 {
        return Err("has a truncated ID3v2 header".into());
    }
    if b[6..10].iter().any(|&x| x & 0x80 != 0) {
        return Err("ID3v2 header has an invalid synchsafe size".into());
    }
    let size = b[6..10].iter().fold(0, |v, &x| v << 7 | usize::from(x));
    let end = 10 + size + if b[3] == 4 && b[5] & 0x10 != 0 { 10 } else { 0 };
    if end > b.len() {
        return Err("has a truncated ID3v2 tag".into());
    }
    Ok(end)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Tessera's builders: the smallest headers each parser accepts, with lengths in seconds or milliseconds.

    fn wav(rate: u32, channels: u16, ms: u64) -> Vec<u8> {
        let align = channels * 2;
        let byte_rate = rate * u32::from(align);
        let size = (ms * u64::from(byte_rate) / 1000) as u32;
        let mut b = Vec::new();
        b.extend_from_slice(b"RIFF");
        b.extend_from_slice(&(36 + size).to_le_bytes());
        b.extend_from_slice(b"WAVEfmt ");
        b.extend_from_slice(&16u32.to_le_bytes());
        b.extend_from_slice(&1u16.to_le_bytes());
        b.extend_from_slice(&channels.to_le_bytes());
        b.extend_from_slice(&rate.to_le_bytes());
        b.extend_from_slice(&byte_rate.to_le_bytes());
        b.extend_from_slice(&align.to_le_bytes());
        b.extend_from_slice(&16u16.to_le_bytes());
        b.extend_from_slice(b"data");
        b.extend_from_slice(&size.to_le_bytes());
        b.resize(b.len() + size as usize, 0);
        b
    }

    fn flac(rate: u64, channels: u64, secs: u64) -> Vec<u8> {
        let mut b = b"fLaC\x80\x00\x00\x22".to_vec();
        b.extend_from_slice(&[0x10, 0x00, 0x10, 0x00, 0, 0, 0, 0, 0, 0]);
        b.extend_from_slice(&(rate << 44 | (channels - 1) << 41 | 15 << 36 | (secs * rate)).to_be_bytes());
        b.resize(b.len() + 16, 0);
        b
    }

    fn page(serial: u32, granule: u64, packet: &[u8]) -> Vec<u8> {
        let mut b = b"OggS\x00\x02".to_vec();
        b.extend_from_slice(&granule.to_le_bytes());
        b.extend_from_slice(&serial.to_le_bytes());
        b.extend_from_slice(&[0; 8]);
        b.extend_from_slice(&[1, packet.len() as u8]);
        b.extend_from_slice(packet);
        b
    }

    fn vorbis_head(rate: u32, channels: u8) -> Vec<u8> {
        let mut p = b"\x01vorbis".to_vec();
        p.resize(30, 0);
        p[11] = channels;
        p[12..16].copy_from_slice(&rate.to_le_bytes());
        p[28] = 0xB8; // 256- and 2048-sample blocks
        p[29] = 1; // framing
        p
    }

    fn opus_head(channels: u8, family: u8) -> Vec<u8> {
        let mut p = b"OpusHead\x01".to_vec();
        p.push(channels);
        p.extend_from_slice(&312u16.to_le_bytes());
        p.extend_from_slice(&48_000u32.to_le_bytes());
        p.extend_from_slice(&[0, 0, family]);
        if family == 1 {
            let (streams, coupled, map): (u8, u8, &[u8]) =
                if channels == 3 { (2, 1, &[0, 2, 1]) } else { (4, 2, &[0, 4, 1, 2, 3, 5]) };
            p.extend_from_slice(&[streams, coupled]);
            p.extend_from_slice(map);
        }
        p
    }

    fn ogg_of(head: &[u8], granule: u64) -> Vec<u8> {
        [page(7, 0, head), page(7, granule, &[])].concat()
    }

    /// An MPEG-1 Layer III frame at 44.1 kHz, stereo, of bitrate index `index`.
    fn mp3_frame(index: u32) -> Vec<u8> {
        let kbps = [0, 32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320][index as usize];
        let mut f = vec![0; 144 * kbps * 1000 / 44_100];
        f[..4].copy_from_slice(&(0xFFFB_0000 | index << 12).to_be_bytes());
        f
    }

    fn err(ext: &str, b: &[u8]) -> String {
        inspect_audio(ext, b).err().unwrap_or_else(|| panic!("{ext} was accepted"))
    }

    #[test]
    fn each_format_reports_rate_channels_and_length() {
        let a = |ext, b: &[u8]| {
            let a = info(ext, b).unwrap();
            (a.rate, a.channels, (a.seconds * 1000.0).round() as u64)
        };
        assert_eq!(a(".mp3", &mp3_frame(9)), (44_100, 2, 26));
        assert_eq!(a(".ogg", &ogg_of(&vorbis_head(48_000, 2), 3 * 48_000)), (48_000, 2, 3000));
        assert_eq!(a(".ogg", &ogg_of(&opus_head(2, 0), 3 * 48_000 + 312)), (48_000, 2, 3000));
        assert_eq!(a(".ogg", &ogg_of(&opus_head(6, 1), 48_000 + 312)), (48_000, 6, 1000));
        assert_eq!(a(".wav", &wav(24_000, 1, 1000)), (24_000, 1, 1000));
        assert_eq!(a(".flac", &flac(48_000, 6, 2)), (48_000, 6, 2000));
        // ID3v2.3 ahead, two frames of different bitrates, an ID3v1 tag behind.
        let mut vbr = b"ID3\x03\x00\x00\x00\x00\x00\x04\x00\x00\x00\x00".to_vec();
        vbr.extend([mp3_frame(9), mp3_frame(11), b"TAG".to_vec(), vec![0; 125]].concat());
        assert_eq!(a(".mp3", &vbr), (44_100, 2, 52));
        // An ID3v2.4 tag ahead of a FLAC whose sample count is unknown: length 0, which passes.
        let tagged = [b"ID3\x04\x00\x00\x00\x00\x00\x03\x00\x00\x00".as_slice(), &flac(48_000, 2, 0)].concat();
        assert_eq!(a(".flac", &tagged), (48_000, 2, 0));
    }

    #[test]
    fn roblox_limits_are_checked() {
        assert!(inspect_audio(".flac", &flac(48_000, 2, 420)).is_ok());
        assert_eq!(err(".flac", &flac(48_000, 2, 421)), "421.0 s long; Roblox takes audio up to 7 minutes");
        assert!(err(".flac", &flac(96_000, 2, 1)).contains("48 kHz limit"));
        assert!(err(".flac", &flac(48_000, 4, 1)).contains("channel count 4"));
        assert!(err(".mp3", b"not an mp3").contains("contains no MPEG Layer III frames"));
    }

    #[test]
    fn malformed_headers_are_refused() {
        let mut framing = vorbis_head(48_000, 2);
        framing[29] = 0;
        let cases: [(&[u8], &str); 5] = [
            (b"\x01vorbis", "identification header is truncated"),
            (&framing, "invalid block sizes or framing"),
            (&opus_head(6, 0), "mapping family 0"),
            (&opus_head(6, 1)[..21], "surround mapping header is truncated"),
            (&opus_head(3, 2), "mapping family 2"),
        ];
        for (head, want) in cases {
            assert!(err(".ogg", &page(1, 0, head)).contains(want), "{want}");
        }
        let two = [page(1, 0, &vorbis_head(48_000, 2)), page(2, 48_000, &[])].concat();
        assert!(err(".ogg", &two).contains("multiple logical streams"));

        let not_first = [b"fLaC\x81\x00\x00\x00".as_slice(), &flac(48_000, 2, 1)[8..]].concat();
        assert!(err(".flac", &not_first).contains("must be the first"));
        let mut open = flac(48_000, 2, 1);
        open[4] &= 0x7F;
        assert!(err(".flac", &open).contains("metadata chain is truncated"));

        // A RIFF size of the whole file (8 too many) is truncated; bytes past the RIFF payload are ignored.
        let mut riff = wav(24_000, 1, 1000);
        let len = riff.len() as u32;
        riff[4..8].copy_from_slice(&len.to_le_bytes());
        assert!(err(".wav", &riff).contains("RIFF payload is truncated"));
        let trailing = [wav(24_000, 1, 1000), b"data\x04\x00\x00\x00junk".to_vec()].concat();
        assert_eq!(info(".wav", &trailing).unwrap().seconds, 1.0);
    }

    #[test]
    fn no_input_panics() {
        // Deterministic xorshift: random bytes, and the valid builders truncated and with bytes overwritten.
        let mut seed = 0x2545_F491_4F6C_DD1Du64;
        let mut next = || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        let seeds = [
            mp3_frame(9),
            ogg_of(&vorbis_head(48_000, 2), 48_000),
            ogg_of(&opus_head(6, 1), 48_000),
            wav(48_000, 2, 1),
            flac(48_000, 2, 1),
            b"ID3\x04\x00\x10\x00\x00\x00\x00".to_vec(),
        ];
        let exts = [".mp3", ".ogg", ".wav", ".flac"];
        for i in 0..100_000 {
            let mut b = if i % 4 == 0 {
                (0..next() % 96).map(|_| next() as u8).collect()
            } else {
                let s = &seeds[next() as usize % seeds.len()];
                s[..next() as usize % (s.len() + 1)].to_vec()
            };
            for _ in 0..next() % 4 {
                let at = next() as usize % b.len().max(1);
                if let Some(x) = b.get_mut(at) {
                    *x = if next() % 2 == 0 { 0xFF } else { next() as u8 };
                }
            }
            let _ = inspect_audio(exts[next() as usize % exts.len()], &b);
        }
    }
}
