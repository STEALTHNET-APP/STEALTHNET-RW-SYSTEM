pub const MP4_TYPE: &str = "video/mp4";

const BOX_LIMIT: usize = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VideoInfo {
    pub width: u32,
    pub height: u32,
    pub duration: u32,
    pub streaming: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VideoProblem {
    NotMp4,
    QuickTime,
    Damaged,
    NoPicture,
}

pub fn sniff(bytes: &[u8]) -> bool {
    bytes.get(4..8) == Some(b"ftyp".as_slice())
}

pub fn inspect(bytes: &[u8]) -> Result<VideoInfo, VideoProblem> {
    if !sniff(bytes) {
        return Err(VideoProblem::NotMp4);
    }
    let mut boxes = Boxes::new(bytes);
    let mut moov = None;
    let mut media_first = false;
    let mut media = false;
    for (index, (kind, payload)) in boxes.by_ref().enumerate() {
        if index >= BOX_LIMIT {
            return Err(VideoProblem::Damaged);
        }
        if index == 0 && payload.get(0..4) == Some(b"qt  ".as_slice()) {
            return Err(VideoProblem::QuickTime);
        }
        match &kind {
            b"moov" if moov.is_none() => {
                moov = Some(payload);
                media_first = media;
            }
            b"mdat" => media = true,
            _ => {}
        }
    }
    if boxes.broken || !media {
        return Err(VideoProblem::Damaged);
    }
    let moov = moov.ok_or(VideoProblem::Damaged)?;
    let (width, height) = Boxes::new(moov)
        .take(BOX_LIMIT)
        .filter(|(kind, _)| kind == b"trak")
        .find_map(|(_, trak)| picture(trak))
        .ok_or(VideoProblem::NoPicture)?;
    Ok(VideoInfo {
        width,
        height,
        duration: child(moov, b"mvhd").and_then(seconds).unwrap_or(0),
        streaming: !media_first,
    })
}

struct Boxes<'a> {
    data: &'a [u8],
    at: usize,
    broken: bool,
}

impl<'a> Boxes<'a> {
    fn new(data: &'a [u8]) -> Self {
        Boxes {
            data,
            at: 0,
            broken: false,
        }
    }
}

impl<'a> Iterator for Boxes<'a> {
    type Item = ([u8; 4], &'a [u8]);

    fn next(&mut self) -> Option<Self::Item> {
        let rest = self.data.get(self.at..).filter(|rest| !rest.is_empty())?;
        let Some((kind, start, end)) = header(rest) else {
            self.broken = true;
            return None;
        };
        self.at += end;
        Some((kind, rest.get(start..end)?))
    }
}

fn header(rest: &[u8]) -> Option<([u8; 4], usize, usize)> {
    let size = be32(rest, 0)?;
    let kind: [u8; 4] = rest.get(4..8)?.try_into().ok()?;
    let (start, end) = match size {
        0 => (8, rest.len()),
        1 => (16, usize::try_from(be64(rest, 8)?).ok()?),
        size => (8, usize::try_from(size).ok()?),
    };
    (start <= end && end <= rest.len()).then_some((kind, start, end))
}

fn be16(bytes: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_be_bytes(bytes.get(at..at + 2)?.try_into().ok()?))
}

fn be32(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(bytes.get(at..at + 4)?.try_into().ok()?))
}

fn be64(bytes: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_be_bytes(bytes.get(at..at + 8)?.try_into().ok()?))
}

fn child<'a>(data: &'a [u8], kind: &[u8; 4]) -> Option<&'a [u8]> {
    Boxes::new(data)
        .take(BOX_LIMIT)
        .find(|(found, _)| found == kind)
        .map(|(_, payload)| payload)
}

fn seconds(mvhd: &[u8]) -> Option<u32> {
    let (scale, length) = match mvhd.first()? {
        0 => (
            be32(mvhd, 12)?,
            be32(mvhd, 16).filter(|v| *v != u32::MAX)?.into(),
        ),
        1 => (be32(mvhd, 20)?, be64(mvhd, 24).filter(|v| *v != u64::MAX)?),
        _ => return None,
    };
    let scale = u64::from(scale);
    if scale == 0 || length == 0 {
        return None;
    }
    let whole = length / scale + u64::from((length % scale) * 2 >= scale);
    u32::try_from(whole.max(1)).ok()
}

fn picture(trak: &[u8]) -> Option<(u32, u32)> {
    let mdia = child(trak, b"mdia")?;
    if child(mdia, b"hdlr")?.get(8..12)? != b"vide" {
        return None;
    }
    let tkhd = child(trak, b"tkhd")?;
    let matrix = match tkhd.first()? {
        0 => 40,
        1 => 52,
        _ => return None,
    };
    let (mut width, mut height) = (
        be32(tkhd, matrix + 36)? >> 16,
        be32(tkhd, matrix + 40)? >> 16,
    );
    if width == 0 || height == 0 {
        (width, height) = entry_size(mdia)?;
    }
    if width == 0 || height == 0 {
        return None;
    }
    if be32(tkhd, matrix)? == 0 && be32(tkhd, matrix + 16)? == 0 {
        (width, height) = (height, width);
    }
    Some((width, height))
}

fn entry_size(mdia: &[u8]) -> Option<(u32, u32)> {
    let stbl = child(child(mdia, b"minf")?, b"stbl")?;
    let entries = child(stbl, b"stsd")?.get(8..)?;
    let (_, entry) = Boxes::new(entries).next()?;
    Some((be16(entry, 24)?.into(), be16(entry, 26)?.into()))
}

#[cfg(any(test, feature = "test-util"))]
pub mod sample {
    const IDENTITY: [u32; 9] = [0x0001_0000, 0, 0, 0, 0x0001_0000, 0, 0, 0, 0x4000_0000];
    const QUARTER_TURN: [u32; 9] = [0, 0x0001_0000, 0, 0xFFFF_0000, 0, 0, 0, 0, 0x4000_0000];

    pub fn boxed(kind: &[u8; 4], payload: &[u8]) -> Vec<u8> {
        let size = u32::try_from(payload.len() + 8).unwrap_or(u32::MAX);
        let mut out = size.to_be_bytes().to_vec();
        out.extend_from_slice(kind);
        out.extend_from_slice(payload);
        out
    }

    fn movie_header(seconds: u32) -> Vec<u8> {
        let mut mvhd = vec![0u8; 100];
        mvhd[12..16].copy_from_slice(&1000u32.to_be_bytes());
        mvhd[16..20].copy_from_slice(&seconds.saturating_mul(1000).to_be_bytes());
        boxed(b"mvhd", &mvhd)
    }

    fn track(width: u32, height: u32, handler: &[u8; 4], matrix: [u32; 9]) -> Vec<u8> {
        let mut tkhd = vec![0u8; 84];
        tkhd[3] = 3;
        for (index, value) in matrix.iter().enumerate() {
            tkhd[40 + index * 4..44 + index * 4].copy_from_slice(&value.to_be_bytes());
        }
        tkhd[76..80].copy_from_slice(&(width << 16).to_be_bytes());
        tkhd[80..84].copy_from_slice(&(height << 16).to_be_bytes());
        let mut hdlr = vec![0u8; 8];
        hdlr.extend_from_slice(handler);
        hdlr.extend_from_slice(&[0u8; 12]);
        hdlr.extend_from_slice(b"Handler\0");
        let mut entry = vec![0u8; 78];
        entry[7] = 1;
        entry[24..26].copy_from_slice(&u16::try_from(width).unwrap_or(0).to_be_bytes());
        entry[26..28].copy_from_slice(&u16::try_from(height).unwrap_or(0).to_be_bytes());
        let mut stsd = vec![0, 0, 0, 0, 0, 0, 0, 1];
        stsd.extend(boxed(b"avc1", &entry));
        let stbl = boxed(b"stbl", &boxed(b"stsd", &stsd));
        let minf = boxed(b"minf", &stbl);
        let mdia = boxed(b"mdia", &[boxed(b"hdlr", &hdlr), minf].concat());
        boxed(b"trak", &[boxed(b"tkhd", &tkhd), mdia].concat())
    }

    fn file(brand: &[u8; 4], moov: Vec<u8>, faststart: bool) -> Vec<u8> {
        let ftyp = boxed(
            b"ftyp",
            &[brand.as_slice(), &[0, 0, 2, 0], b"isommp41"].concat(),
        );
        let mdat = boxed(b"mdat", &[0x5Au8; 256]);
        if faststart {
            [ftyp, moov, mdat].concat()
        } else {
            [ftyp, mdat, moov].concat()
        }
    }

    pub fn mp4(width: u32, height: u32, seconds: u32, faststart: bool) -> Vec<u8> {
        let moov = boxed(
            b"moov",
            &[
                movie_header(seconds),
                track(0, 0, b"soun", IDENTITY),
                track(width, height, b"vide", IDENTITY),
            ]
            .concat(),
        );
        file(b"isom", moov, faststart)
    }

    pub fn turned(width: u32, height: u32, seconds: u32) -> Vec<u8> {
        let moov = boxed(
            b"moov",
            &[
                movie_header(seconds),
                track(width, height, b"vide", QUARTER_TURN),
            ]
            .concat(),
        );
        file(b"mp42", moov, true)
    }

    pub fn sound_only(seconds: u32) -> Vec<u8> {
        let moov = boxed(
            b"moov",
            &[movie_header(seconds), track(0, 0, b"soun", IDENTITY)].concat(),
        );
        file(b"M4A ", moov, true)
    }

    pub fn quicktime(width: u32, height: u32, seconds: u32) -> Vec<u8> {
        let moov = boxed(
            b"moov",
            &[
                movie_header(seconds),
                track(width, height, b"vide", IDENTITY),
            ]
            .concat(),
        );
        file(b"qt  ", moov, true)
    }
}

#[cfg(test)]
mod tests {
    use super::sample::*;
    use super::*;

    #[test]
    fn размер_длительность_и_порядок_частей_читаются_из_mp4() {
        assert_eq!(
            inspect(&mp4(1280, 720, 34, true)),
            Ok(VideoInfo {
                width: 1280,
                height: 720,
                duration: 34,
                streaming: true,
            })
        );
        assert_eq!(
            inspect(&mp4(640, 360, 5, false)),
            Ok(VideoInfo {
                width: 640,
                height: 360,
                duration: 5,
                streaming: false,
            })
        );
    }

    #[test]
    fn повёрнутое_видео_с_телефона_отдаёт_стороны_как_на_экране() {
        let info = inspect(&turned(1920, 1080, 12)).unwrap();
        assert_eq!((info.width, info.height), (1080, 1920));
    }

    #[test]
    fn не_mp4_mov_без_картинки_и_обрезанный_файл_не_проходят() {
        assert_eq!(inspect(b"\x89PNG\r\n\x1a\n"), Err(VideoProblem::NotMp4));
        assert_eq!(inspect(&[]), Err(VideoProblem::NotMp4));
        assert_eq!(
            inspect(&quicktime(1280, 720, 3)),
            Err(VideoProblem::QuickTime)
        );
        assert_eq!(inspect(&sound_only(60)), Err(VideoProblem::NoPicture));
        let whole = mp4(1280, 720, 10, false);
        assert_eq!(
            inspect(&whole[..whole.len() - 40]),
            Err(VideoProblem::Damaged)
        );
        let head = boxed(b"ftyp", b"isom\0\0\0\0isom");
        assert_eq!(inspect(&head), Err(VideoProblem::Damaged));
        let mut liar = mp4(1280, 720, 10, true);
        liar[0..4].copy_from_slice(&u32::MAX.to_be_bytes());
        assert_eq!(inspect(&liar), Err(VideoProblem::Damaged));
    }

    #[test]
    fn секунды_округляются_а_короткое_видео_длится_секунду() {
        let mut mvhd = vec![0u8; 100];
        mvhd[12..16].copy_from_slice(&600u32.to_be_bytes());
        mvhd[16..20].copy_from_slice(&20_700u32.to_be_bytes());
        assert_eq!(seconds(&mvhd), Some(35));
        mvhd[16..20].copy_from_slice(&120u32.to_be_bytes());
        assert_eq!(seconds(&mvhd), Some(1));
        mvhd[16..20].copy_from_slice(&u32::MAX.to_be_bytes());
        assert_eq!(seconds(&mvhd), None);
        mvhd[12..16].copy_from_slice(&0u32.to_be_bytes());
        mvhd[16..20].copy_from_slice(&50u32.to_be_bytes());
        assert_eq!(seconds(&mvhd), None);
    }

    #[test]
    fn пустые_размеры_в_заголовке_дорожки_берутся_из_описания_кадра() {
        let mut bytes = mp4(854, 480, 7, true);
        let tkhd = bytes
            .windows(4)
            .rposition(|w| w == b"tkhd")
            .expect("заголовок дорожки");
        let at = tkhd + 4 + 76;
        bytes[at..at + 8].fill(0);
        let info = inspect(&bytes).unwrap();
        assert_eq!((info.width, info.height), (854, 480));
    }
}
