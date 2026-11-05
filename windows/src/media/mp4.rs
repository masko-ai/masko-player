//! Reads what the player needs from an MP4 file held in memory: each track's
//! samples (where their bytes are and when they show), its sample
//! description and its edit list. Masko's clips and sounds are plain MP4s,
//! so fragmented files are refused.

use super::MediaError;

/// What a track holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    Video,
    Audio,
    Other,
}

/// A track's first sample description, for the codecs the player decodes.
#[derive(Clone, Debug)]
pub(crate) enum Entry {
    /// `av01`, with its coded size.
    Av1 { width: u32, height: u32 },
    /// `mp4a` with AAC: its AudioSpecificConfig, from `esds`.
    Aac { sample_rate: u32, config: Vec<u8> },
    /// Anything else.
    Other,
}

/// One sample: its bytes in the file, and its decode and presentation time
/// in the track's timescale.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Sample {
    pub offset: usize,
    pub size: usize,
    pub dts: u64,
    pub cts_offset: i64,
}

impl Sample {
    pub fn pts(&self) -> i64 {
        self.dts as i64 + self.cts_offset
    }
}

/// One edit of the edit list: `segment_duration` in the movie's timescale,
/// `media_time` in the track's, -1 for an empty edit (a pause before the
/// track starts).
#[derive(Clone, Copy, Debug)]
pub(crate) struct Edit {
    pub segment_duration: u64,
    pub media_time: i64,
}

#[derive(Clone, Debug)]
pub(crate) struct Track {
    pub kind: Kind,
    pub timescale: u32,
    /// The media's length in its timescale, from `mdhd`.
    pub media_duration: u64,
    pub edits: Vec<Edit>,
    pub entry: Entry,
    pub samples: Vec<Sample>,
}

impl Track {
    /// When the track's first presented sample shows on the movie's
    /// timeline, in seconds: the empty edits before it.
    pub fn start_seconds(&self, movie_timescale: u32) -> f64 {
        let empty: u64 = self
            .edits
            .iter()
            .take_while(|edit| edit.media_time < 0)
            .map(|edit| edit.segment_duration)
            .sum();
        empty as f64 / movie_timescale.max(1) as f64
    }

    /// The first edit that plays media: where in the media it starts (the
    /// samples before it are not shown, an AAC encoder's priming for one),
    /// and how long it plays, in seconds.
    pub fn played_edit(&self, movie_timescale: u32) -> Option<(i64, f64)> {
        self.edits
            .iter()
            .find(|edit| edit.media_time >= 0)
            .map(|edit| {
                (
                    edit.media_time,
                    edit.segment_duration as f64 / movie_timescale.max(1) as f64,
                )
            })
    }

    /// How long the track plays, in seconds: its edits when it has some, its
    /// media's length otherwise (what AVFoundation calls its time range).
    pub fn duration_seconds(&self, movie_timescale: u32) -> f64 {
        let played: u64 = self
            .edits
            .iter()
            .filter(|edit| edit.media_time >= 0)
            .map(|edit| edit.segment_duration)
            .sum();
        if played > 0 {
            played as f64 / movie_timescale.max(1) as f64
        } else {
            self.media_duration as f64 / self.timescale.max(1) as f64
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct Movie {
    pub timescale: u32,
    pub tracks: Vec<Track>,
}

impl Movie {
    pub fn first(&self, kind: Kind) -> Option<&Track> {
        self.tracks.iter().find(|track| track.kind == kind)
    }
}

/// Whether these bytes start like an MP4 or QuickTime file.
pub(crate) fn looks_like_mp4(bytes: &[u8]) -> bool {
    bytes.len() >= 12
        && matches!(
            &bytes[4..8],
            b"ftyp" | b"moov" | b"free" | b"mdat" | b"wide"
        )
}

/// Reads the movie from a whole file.
pub(crate) fn read(file: &[u8]) -> Result<Movie, MediaError> {
    let mut moov = None;
    for b in boxes(file) {
        let b = b?;
        match &b.kind {
            b"moov" => moov = Some(b.body),
            b"moof" => return Err(format("fragmented MP4 files are not supported")),
            _ => {}
        }
    }
    let moov = moov.ok_or_else(|| format("no moov box: not an MP4 file"))?;
    let mut timescale = 0;
    let mut tracks = Vec::new();
    for b in boxes(moov) {
        let b = b?;
        match &b.kind {
            b"mvhd" => {
                let mut r = Reader::new(b.body);
                let version = r.u8()?;
                r.skip(3)?;
                r.skip(if version == 1 { 16 } else { 8 })?;
                timescale = r.u32()?;
            }
            b"trak" => {
                if let Some(track) = read_track(b.body, file)? {
                    tracks.push(track);
                }
            }
            _ => {}
        }
    }
    if timescale == 0 {
        return Err(format("the movie has no timescale"));
    }
    Ok(Movie { timescale, tracks })
}

fn format(detail: &str) -> MediaError {
    MediaError::Format(detail.to_string())
}

/// One box: its four-letter type and its body.
struct Mp4Box<'a> {
    kind: [u8; 4],
    body: &'a [u8],
}

/// The boxes laid one after the other in `data`.
fn boxes(data: &[u8]) -> impl Iterator<Item = Result<Mp4Box<'_>, MediaError>> {
    let mut at = 0usize;
    std::iter::from_fn(move || {
        if at + 8 > data.len() {
            return None;
        }
        let size32 = u32::from_be_bytes(data[at..at + 4].try_into().unwrap()) as u64;
        let kind: [u8; 4] = data[at + 4..at + 8].try_into().unwrap();
        let (size, header) = match size32 {
            0 => ((data.len() - at) as u64, 8),
            1 => {
                if at + 16 > data.len() {
                    at = data.len();
                    return Some(Err(format("a box's 64-bit size is cut")));
                }
                (
                    u64::from_be_bytes(data[at + 8..at + 16].try_into().unwrap()),
                    16,
                )
            }
            size => (size, 8),
        };
        if size < header as u64 || size > (data.len() - at) as u64 {
            at = data.len();
            return Some(Err(format("a box runs past the end of its parent")));
        }
        let body = &data[at + header..at + size as usize];
        at += size as usize;
        Some(Ok(Mp4Box { kind, body }))
    })
}

/// The first child box of this type.
fn child<'a>(data: &'a [u8], kind: &[u8; 4]) -> Result<Option<&'a [u8]>, MediaError> {
    for b in boxes(data) {
        let b = b?;
        if &b.kind == kind {
            return Ok(Some(b.body));
        }
    }
    Ok(None)
}

fn read_track(trak: &[u8], file: &[u8]) -> Result<Option<Track>, MediaError> {
    let Some(mdia) = child(trak, b"mdia")? else {
        return Ok(None);
    };
    let mut edits = Vec::new();
    if let Some(elst) = child(trak, b"edts")?
        .map(|edts| child(edts, b"elst"))
        .transpose()?
        .flatten()
    {
        let mut r = Reader::new(elst);
        let version = r.u8()?;
        r.skip(3)?;
        let count = r.u32()?;
        for _ in 0..count.min(1024) {
            let (segment_duration, media_time) = if version == 1 {
                (r.u64()?, r.u64()? as i64)
            } else {
                (r.u32()? as u64, r.u32()? as i32 as i64)
            };
            r.skip(4)?; // media rate
            edits.push(Edit {
                segment_duration,
                media_time,
            });
        }
    }

    let mdhd = child(mdia, b"mdhd")?.ok_or_else(|| format("a track has no mdhd"))?;
    let mut r = Reader::new(mdhd);
    let version = r.u8()?;
    r.skip(3)?;
    let (timescale, media_duration) = if version == 1 {
        r.skip(16)?;
        (r.u32()?, r.u64()?)
    } else {
        r.skip(8)?;
        (r.u32()?, r.u32()? as u64)
    };
    if timescale == 0 {
        return Err(format("a track has no timescale"));
    }
    let kind = match child(mdia, b"hdlr")? {
        Some(hdlr) if hdlr.len() >= 12 => match &hdlr[8..12] {
            b"vide" => Kind::Video,
            b"soun" => Kind::Audio,
            _ => Kind::Other,
        },
        _ => Kind::Other,
    };
    let stbl = child(mdia, b"minf")?
        .map(|minf| child(minf, b"stbl"))
        .transpose()?
        .flatten()
        .ok_or_else(|| format("a track has no sample table"))?;
    let entry = match child(stbl, b"stsd")? {
        Some(stsd) => read_entry(stsd)?,
        None => return Err(format("a track has no sample description")),
    };
    let samples = read_samples(stbl, file)?;
    Ok(Some(Track {
        kind,
        timescale,
        media_duration,
        edits,
        entry,
        samples,
    }))
}

fn read_entry(stsd: &[u8]) -> Result<Entry, MediaError> {
    let mut r = Reader::new(stsd);
    r.skip(4)?; // version and flags
    if r.u32()? == 0 {
        return Err(format("a track has no sample description"));
    }
    let entry = boxes(r.rest())
        .next()
        .ok_or_else(|| format("a sample description is cut"))??;
    let body = entry.body;
    match &entry.kind {
        b"av01" => {
            // A visual sample entry: 8 bytes of sample entry, then 16 before
            // the width and height, then 50 more before its child boxes.
            let mut r = Reader::new(body);
            r.skip(24)?;
            let width = r.u16()? as u32;
            let height = r.u16()? as u32;
            Ok(Entry::Av1 { width, height })
        }
        b"mp4a" => {
            let mut r = Reader::new(body);
            r.skip(8)?;
            let version = r.u16()?;
            r.skip(14)?;
            let sample_rate = r.u32()? >> 16;
            // QuickTime sound descriptions version 1 and 2 are longer.
            r.skip(match version {
                1 => 16,
                2 => 36,
                _ => 0,
            })?;
            let children = r.rest();
            let esds = match child(children, b"esds")? {
                Some(esds) => Some(esds),
                // QuickTime puts it in a `wave` box.
                None => child(children, b"wave")?
                    .map(|wave| child(wave, b"esds"))
                    .transpose()?
                    .flatten(),
            };
            let esds = esds.ok_or_else(|| format("an mp4a track has no esds"))?;
            let (object_type, config) = read_esds(esds)?;
            // 0x40 is MPEG-4 audio, 0x66 to 0x68 MPEG-2 AAC.
            if !matches!(object_type, 0x40 | 0x66..=0x68) {
                return Err(MediaError::Format(format!(
                    "an mp4a track with object type 0x{object_type:02x}, not AAC"
                )));
            }
            Ok(Entry::Aac {
                sample_rate,
                config,
            })
        }
        _ => Ok(Entry::Other),
    }
}

/// The decoder's object type and specific info (for AAC, the
/// AudioSpecificConfig) from an `esds` box's descriptors.
fn read_esds(esds: &[u8]) -> Result<(u8, Vec<u8>), MediaError> {
    let mut r = Reader::new(esds);
    r.skip(4)?;
    let mut object_type = 0;
    // Descriptors nest: ES (3) holds DecoderConfig (4), which holds
    // DecoderSpecificInfo (5). Walk into each in turn.
    while r.left() > 0 {
        let tag = r.u8()?;
        let mut size = 0usize;
        for _ in 0..4 {
            let byte = r.u8()?;
            size = (size << 7) | (byte & 0x7f) as usize;
            if byte & 0x80 == 0 {
                break;
            }
        }
        match tag {
            0x03 => {
                r.skip(2)?;
                let flags = r.u8()?;
                if flags & 0x80 != 0 {
                    r.skip(2)?;
                }
                if flags & 0x40 != 0 {
                    let len = r.u8()? as usize;
                    r.skip(len)?;
                }
                if flags & 0x20 != 0 {
                    r.skip(2)?;
                }
            }
            0x04 => {
                object_type = r.u8()?;
                r.skip(12)?;
            }
            0x05 => return Ok((object_type, r.take(size)?.to_vec())),
            _ => r.skip(size)?,
        }
    }
    Err(format("an esds box has no decoder specific info"))
}

fn read_samples(stbl: &[u8], file: &[u8]) -> Result<Vec<Sample>, MediaError> {
    // Sizes.
    let sizes: Vec<u32> = if let Some(stsz) = child(stbl, b"stsz")? {
        let mut r = Reader::new(stsz);
        r.skip(4)?;
        let fixed = r.u32()?;
        let count = r.u32()? as usize;
        if fixed != 0 {
            vec![fixed; count.min(file.len())]
        } else {
            if count > r.left() / 4 {
                return Err(format("the sample size table is cut"));
            }
            (0..count).map(|_| r.u32()).collect::<Result<_, _>>()?
        }
    } else if let Some(stz2) = child(stbl, b"stz2")? {
        let mut r = Reader::new(stz2);
        r.skip(7)?;
        let field = r.u8()?;
        let count = r.u32()? as usize;
        let mut sizes = Vec::with_capacity(count.min(r.left() * 2));
        match field {
            4 => {
                for i in 0..count {
                    let byte = *r.rest().get(i / 2).ok_or_else(|| format("stz2 is cut"))?;
                    sizes.push(if i % 2 == 0 { byte >> 4 } else { byte & 15 } as u32);
                }
            }
            8 => {
                for _ in 0..count {
                    sizes.push(r.u8()? as u32);
                }
            }
            16 => {
                for _ in 0..count {
                    sizes.push(r.u16()? as u32);
                }
            }
            _ => return Err(format("stz2 has an unknown field size")),
        }
        sizes
    } else {
        return Err(format("a track has no sample sizes"));
    };
    let count = sizes.len();

    // Chunk offsets.
    let chunks: Vec<u64> = if let Some(stco) = child(stbl, b"stco")? {
        let mut r = Reader::new(stco);
        r.skip(4)?;
        let n = r.u32()? as usize;
        if n > r.left() / 4 {
            return Err(format("the chunk offset table is cut"));
        }
        (0..n)
            .map(|_| r.u32().map(u64::from))
            .collect::<Result<_, _>>()?
    } else if let Some(co64) = child(stbl, b"co64")? {
        let mut r = Reader::new(co64);
        r.skip(4)?;
        let n = r.u32()? as usize;
        if n > r.left() / 8 {
            return Err(format("the chunk offset table is cut"));
        }
        (0..n).map(|_| r.u64()).collect::<Result<_, _>>()?
    } else {
        return Err(format("a track has no chunk offsets"));
    };

    // Samples per chunk: runs of (first chunk, samples per chunk).
    let stsc = child(stbl, b"stsc")?.ok_or_else(|| format("a track has no stsc"))?;
    let mut r = Reader::new(stsc);
    r.skip(4)?;
    let runs = r.u32()? as usize;
    if runs > r.left() / 12 {
        return Err(format("the sample-to-chunk table is cut"));
    }
    let mut stsc_runs = Vec::with_capacity(runs);
    for _ in 0..runs {
        let first = r.u32()? as usize;
        let per_chunk = r.u32()? as usize;
        r.skip(4)?;
        stsc_runs.push((first.max(1), per_chunk));
    }

    let mut samples = Vec::with_capacity(count);
    let mut index = 0usize;
    'chunks: for (run, &(first, per_chunk)) in stsc_runs.iter().enumerate() {
        let last = stsc_runs
            .get(run + 1)
            .map(|&(next, _)| next.saturating_sub(1))
            .unwrap_or(chunks.len());
        for chunk in first..=last.min(chunks.len()) {
            let mut offset = chunks[chunk - 1] as usize;
            for _ in 0..per_chunk {
                if index == count {
                    break 'chunks;
                }
                let size = sizes[index] as usize;
                if offset.checked_add(size).is_none_or(|end| end > file.len()) {
                    return Err(format("a sample lies past the end of the file"));
                }
                samples.push(Sample {
                    offset,
                    size,
                    dts: 0,
                    cts_offset: 0,
                });
                offset += size;
                index += 1;
            }
        }
    }
    if samples.len() != count {
        return Err(format("the sample tables do not agree"));
    }

    // Decode times.
    let stts = child(stbl, b"stts")?.ok_or_else(|| format("a track has no stts"))?;
    let mut r = Reader::new(stts);
    r.skip(4)?;
    let runs = r.u32()?;
    let mut dts = 0u64;
    let mut at = 0usize;
    for _ in 0..runs {
        let n = r.u32()? as usize;
        let delta = r.u32()? as u64;
        for _ in 0..n {
            if at == count {
                break;
            }
            samples[at].dts = dts;
            dts += delta;
            at += 1;
        }
    }
    // Presentation offsets.
    if let Some(ctts) = child(stbl, b"ctts")? {
        let mut r = Reader::new(ctts);
        let version = r.u8()?;
        r.skip(3)?;
        let runs = r.u32()?;
        let mut at = 0usize;
        for _ in 0..runs {
            let n = r.u32()? as usize;
            let raw = r.u32()?;
            let offset = if version == 1 {
                raw as i32 as i64
            } else {
                raw as i64
            };
            for _ in 0..n {
                if at == count {
                    break;
                }
                samples[at].cts_offset = offset;
                at += 1;
            }
        }
    }
    Ok(samples)
}

/// Reads big-endian numbers from a byte slice, refusing to run past its end.
struct Reader<'a> {
    data: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Reader { data, at: 0 }
    }

    fn left(&self) -> usize {
        self.data.len() - self.at
    }

    fn rest(&self) -> &'a [u8] {
        &self.data[self.at..]
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8], MediaError> {
        if n > self.left() {
            return Err(format("a box is cut"));
        }
        let bytes = &self.data[self.at..self.at + n];
        self.at += n;
        Ok(bytes)
    }

    fn skip(&mut self, n: usize) -> Result<(), MediaError> {
        self.take(n).map(|_| ())
    }

    fn u8(&mut self) -> Result<u8, MediaError> {
        Ok(self.take(1)?[0])
    }

    fn u16(&mut self) -> Result<u16, MediaError> {
        Ok(u16::from_be_bytes(self.take(2)?.try_into().unwrap()))
    }

    fn u32(&mut self) -> Result<u32, MediaError> {
        Ok(u32::from_be_bytes(self.take(4)?.try_into().unwrap()))
    }

    fn u64(&mut self) -> Result<u64, MediaError> {
        Ok(u64::from_be_bytes(self.take(8)?.try_into().unwrap()))
    }
}
