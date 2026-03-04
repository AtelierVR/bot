use bytes::{Buf, BufMut, BytesMut};
use std::io;

#[derive(Debug, Clone)]
pub struct Buffer {
    data: BytesMut,
}

impl Buffer {
    pub fn new() -> Self {
        Self {
            data: BytesMut::new(),
        }
    }

    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            data: BytesMut::with_capacity(capacity),
        }
    }

    pub fn from_vec(vec: Vec<u8>) -> Self {
        Self {
            data: BytesMut::from(&vec[..]),
        }
    }

    pub fn write_u8(&mut self, value: u8) {
        self.data.put_u8(value);
    }

    pub fn write_u16(&mut self, value: u16) {
        self.data.put_u16(value); // Uses network byte order (big-endian) by default
    }

    pub fn write_u32(&mut self, value: u32) {
        self.data.put_u32(value); // Uses network byte order (big-endian) by default
    }

    pub fn write_u64(&mut self, value: u64) {
        self.data.put_u64(value); // Uses network byte order (big-endian) by default
    }

    pub fn write_f32(&mut self, value: f32) {
        self.data.put_f32(value); // Uses network byte order (big-endian) by default
    }

    pub fn write_bytes(&mut self, bytes: &[u8]) {
        self.data.put_slice(bytes);
    }

    pub fn write_string(&mut self, s: &str) {
        let bytes = s.as_bytes();
        self.write_u16(bytes.len() as u16);
        self.write_bytes(bytes);
    }

    pub fn read_u8(&mut self) -> io::Result<u8> {
        if self.data.remaining() < 1 {
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "Not enough data"));
        }
        Ok(self.data.get_u8())
    }

    pub fn read_u16(&mut self) -> io::Result<u16> {
        if self.data.remaining() < 2 {
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "Not enough data"));
        }
        Ok(self.data.get_u16())
    }

    pub fn read_u32(&mut self) -> io::Result<u32> {
        if self.data.remaining() < 4 {
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "Not enough data"));
        }
        Ok(self.data.get_u32())
    }

    pub fn read_u64(&mut self) -> io::Result<u64> {
        if self.data.remaining() < 8 {
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "Not enough data"));
        }
        Ok(self.data.get_u64())
    }

    pub fn read_i64(&mut self) -> io::Result<i64> {
        if self.data.remaining() < 8 {
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "Not enough data"));
        }
        Ok(self.data.get_i64())
    }

    pub fn read_f32(&mut self) -> io::Result<f32> {
        if self.data.remaining() < 4 {
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "Not enough data"));
        }
        Ok(self.data.get_f32())
    }

    pub fn read_bytes(&mut self, len: usize) -> io::Result<Vec<u8>> {
        if self.data.remaining() < len {
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "Not enough data"));
        }
        let mut buf = vec![0u8; len];
        self.data.copy_to_slice(&mut buf);
        Ok(buf)
    }

    pub fn read_string(&mut self) -> io::Result<String> {
        let len = self.read_u16()? as usize;
        let bytes = self.read_bytes(len)?;
        String::from_utf8(bytes).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
    }

    pub fn as_slice(&self) -> &[u8] {
        &self.data
    }

    pub fn to_vec(&self) -> Vec<u8> {
        self.data.to_vec()
    }

    pub fn len(&self) -> usize {
        self.data.len()
    }

    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    pub fn remaining(&self) -> usize {
        self.data.remaining()
    }
}

impl Default for Buffer {
    fn default() -> Self {
        Self::new()
    }
}

impl From<Vec<u8>> for Buffer {
    fn from(vec: Vec<u8>) -> Self {
        Self::from_vec(vec)
    }
}

impl From<&[u8]> for Buffer {
    fn from(slice: &[u8]) -> Self {
        Self {
            data: BytesMut::from(slice),
        }
    }
}
