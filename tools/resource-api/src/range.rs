//! A seekable view confined to a range of an original source.
use std::io::{self, Read, Seek, SeekFrom};

pub trait ReadSeek: Read + Seek {}
impl<T: Read + Seek> ReadSeek for T {}

pub struct RangeReader<R> {
    inner: R,
    start: u64,
    len: u64,
    position: u64,
}

impl<R: Read + Seek> RangeReader<R> {
    pub fn new(inner: R, start: u64, len: u64) -> io::Result<Self> {
        start.checked_add(len).ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "range overflow"))?;
        Ok(Self { inner, start, len, position: 0 })
    }
}

impl<R: Read + Seek> Read for RangeReader<R> {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        let count = (self.len - self.position).min(out.len() as u64) as usize;
        if count == 0 { return Ok(0); }
        self.inner.seek(SeekFrom::Start(self.start + self.position))?;
        let read = self.inner.read(&mut out[..count])?;
        self.position += read as u64;
        Ok(read)
    }
}

impl<R: Read + Seek> Seek for RangeReader<R> {
    fn seek(&mut self, from: SeekFrom) -> io::Result<u64> {
        let target = match from {
            SeekFrom::Start(n) => n as i128,
            SeekFrom::Current(n) => self.position as i128 + n as i128,
            SeekFrom::End(n) => self.len as i128 + n as i128,
        };
        if target < 0 || target > self.len as i128 {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "seek outside source range"));
        }
        self.position = target as u64;
        Ok(self.position)
    }
}
