//! Columns of plain data that are either owned (`Vec`, while a catalog is being built) or
//! read straight out of a mapped file (a kept snapshot, see `snapshot`). Both read as a
//! slice, so the catalog's code doesn't care which it has.
//!
//! A mapped column costs no parsing and no heap: the system reads its pages from the file
//! when they're touched, and may drop them again when memory is short.

use std::ops::Deref;
use std::sync::Arc;

use memmap2::Mmap;
use uuid::Uuid;

use super::text::Str;

/// Plain data: no pointers, nothing to drop, and its bytes are what it is. Only for types
/// with a fixed layout and no padding (`#[repr(C)]`, padding written as fields).
///
/// # Safety
/// Every bit pattern the file's writer produces for the type is a valid value, and the type
/// has no padding bytes.
pub unsafe trait Flat: Copy + 'static {}

// SAFETY: integers have no padding and no invalid values.
unsafe impl Flat for u8 {}
unsafe impl Flat for u32 {}
// SAFETY: a `Uuid` is 16 bytes of any value.
unsafe impl Flat for Uuid {}
// SAFETY: two `u32`s.
unsafe impl Flat for Str {}

/// A mapped file, kept alive by the columns that point into it.
pub struct Mapping {
    pub(super) map: Mmap,
}

pub enum Col<T: Flat> {
    Owned(Vec<T>),
    Mapped { mapping: Arc<Mapping>, offset: usize, len: usize },
}

impl<T: Flat> Col<T> {
    /// `len` items of `T` at byte `offset` of the mapping. The caller checked that they
    /// are within it and aligned.
    pub(super) fn mapped(mapping: &Arc<Mapping>, offset: usize, len: usize) -> Col<T> {
        debug_assert!(offset.is_multiple_of(std::mem::align_of::<T>()));
        debug_assert!(offset + len * std::mem::size_of::<T>() <= mapping.map.len());
        Col::Mapped { mapping: mapping.clone(), offset, len }
    }

    /// To change it: an owned copy first, if it's mapped (only while building).
    pub fn make_mut(&mut self) -> &mut Vec<T> {
        if let Col::Mapped { .. } = self {
            *self = Col::Owned(self.to_vec());
        }
        match self {
            Col::Owned(v) => v,
            Col::Mapped { .. } => unreachable!(),
        }
    }

    /// Its bytes (what a snapshot writes).
    pub fn as_bytes(&self) -> &[u8] {
        let s: &[T] = self;
        // SAFETY: `Flat` has no padding, so every byte is initialized.
        unsafe { std::slice::from_raw_parts(s.as_ptr().cast::<u8>(), std::mem::size_of_val(s)) }
    }

    pub fn is_mapped(&self) -> bool {
        matches!(self, Col::Mapped { .. })
    }
}

impl<T: Flat> Deref for Col<T> {
    type Target = [T];

    fn deref(&self) -> &[T] {
        match self {
            Col::Owned(v) => v,
            Col::Mapped { mapping, offset, len } => {
                // SAFETY: `mapped` checked the range and alignment; the mapping lives as
                // long as `self` holds it, and a snapshot file is never written to.
                unsafe { std::slice::from_raw_parts(mapping.map.as_ptr().add(*offset).cast::<T>(), *len) }
            }
        }
    }
}

impl<T: Flat> Default for Col<T> {
    fn default() -> Self {
        Col::Owned(Vec::new())
    }
}

impl<T: Flat> From<Vec<T>> for Col<T> {
    fn from(v: Vec<T>) -> Self {
        Col::Owned(v)
    }
}
