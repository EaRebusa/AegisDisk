use aligned_vec::{AVec, ConstAlign};
use crate::common::error::{AegisError, Result};

pub type SectorAlign4096 = ConstAlign<4096>;

/// Sector-aligned buffer guaranteed to meet Windows Direct I/O (FILE_FLAG_NO_BUFFERING) requirements.
pub struct AlignedBuffer {
    inner: AVec<u8, SectorAlign4096>,
    alignment: usize,
}

#[allow(dead_code)]
impl AlignedBuffer {
    pub fn new(size: usize, alignment: usize) -> Self {
        let aligned_size = (size + alignment - 1) & !(alignment - 1);
        let mut inner = AVec::new(alignment);
        inner.resize(aligned_size, 0);
        Self { inner, alignment }
    }

    pub fn as_slice(&self) -> &[u8] {
        &self.inner
    }

    pub fn as_mut_slice(&mut self) -> &mut [u8] {
        &mut self.inner
    }

    pub fn as_ptr(&self) -> *const u8 {
        self.inner.as_ptr()
    }

    pub fn as_mut_ptr(&mut self) -> *mut u8 {
        self.inner.as_mut_ptr()
    }

    pub fn len(&self) -> usize {
        self.inner.len()
    }

    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    pub fn verify_alignment(&self) -> Result<()> {
        let addr = self.as_ptr() as usize;
        if addr % self.alignment != 0 {
            return Err(AegisError::AlignmentViolation(addr, self.alignment));
        }
        Ok(())
    }
}
