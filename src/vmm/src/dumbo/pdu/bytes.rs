// Copyright 2018 Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

//! Defines traits which allow byte slices to be interpreted as sequences of bytes that stand for
//! different values packed together using network byte ordering (such as network packets).
//!
//! The main use of these traits is reading and writing numerical values at a given offset in the
//! underlying slice. Why are they needed? Given a byte slice, there are two approaches to
//! reading/writing packet data that come to mind:
//!
//! (1) Have structs which represent the potential contents of each packet type, unsafely cast the
//! bytes slice to a struct pointer/reference (after doing the required checks), and then use the
//! newly obtained pointer/reference to access the data.
//!
//! (2) Access fields by reading bytes at the appropriate offset from the original slice.
//!
//! The first solution looks more appealing at first, but it requires some unsafe code. Moreover,
//! de-referencing unaligned pointers or references is considered undefined behaviour in Rust, and
//! it's not clear whether this undermines the approach or not. Until any further developments,
//! the second option is used, based on the `NetworkBytes` implementation.
//!
//! What's with the `T: Deref<Target = [u8]>`? Is there really a need to be that generic?
//! Not really. The logic in this crate currently expects to work with byte slices (`&[u8]` and
//! `&mut [u8]`), but there's a significant inconvenience. Consider `NetworkBytes` is defined as:
//!
//! ```
//! struct NetworkBytes<'a> {
//!     bytes: &'a [u8],
//! }
//! ```
//!
//! This is perfectly fine for reading values from immutable slices, but what about writing values?
//! Implementing methods such as `fn write_something(&mut self)`, is not really possible, because
//! even with a mutable reference to `self`, `self.bytes` is still an immutable slice. On the other
//! hand, `NetworkBytes` can be defined as:
//!
//! ```
//! struct NetworkBytes<'a> {
//!     bytes: &'a mut [u8],
//! }
//! ```
//!
//! This allows both reads and writes, but requires a mutable reference at all times (and it looks
//! weird to use one for immutable operations). This is where one interesting feature of Rust
//! comes in handy; given a type `Something<T>`, it's possible to  implement different features
//! depending on trait bounds on `T`. For `NetworkBytes`, if `T` implements `Deref<Target = [u8]>`
//! (which `&[u8]` does), read operations are possible to define. If `T` implements
//! `DerefMut<Target = [u8]>`, write operations are also a possibility. Since
//! `DerefMut<Target = [u8]>` implies `Deref<Target = [u8]>`, `NetworkBytes<&mut [u8]>` implements
//! both read and write operations.
//!
//! This can theoretically lead to code bloat when using both `&[u8]` and `&mut [u8]` (as opposed
//! to just `&mut [u8]`), but most calls should be inlined anyway, so it probably doesn't matter
//! in the end. `NetworkBytes` itself implements `Deref` (and `DerefMut` when `T: DerefMut`), so
//! this line of reasoning can be extended to structs which represent different kinds of protocol
//! data units (such as IPv4 packets, Ethernet frames, etc.).
//!
//! Finally, why `Deref` and not something like `AsRef`? The answer is `Deref` coercion, which in
//! this case means that a `NetworkBytes` value will automatically coerce to `&[u8]`
//! (or `&mut [u8]`), without having to go through an explicit `as_ref()` call, which makes the
//! code easier to work with.
//!
//! Method names have the **unchecked** suffix as a reminder they do not check whether the
//! read/write goes beyond the boundaries of a slice. Callers must take the necessary precautions
//! to avoid panics.

use std::fmt::Debug;
use std::marker::PhantomData;
use std::ops::{Deref, DerefMut};

use crate::utils::byte_order;

/// Represents an immutable view into a sequence of bytes which stands for different values packed
/// together using network byte ordering.
pub trait NetworkBytes: Deref<Target = [u8]> {
    /// Reads an `u16` value from the specified offset, converting it to host byte ordering.
    ///
    /// # Panics
    ///
    /// This method will panic if `offset` is invalid.
    #[inline]
    fn ntohs_unchecked(&self, offset: usize) -> u16 {
        // The unwrap() can fail when the offset is invalid, or there aren't enough bytes (2 in this
        // case) left until the end of the slice. The caller must ensure this doesn't happen (hence
        // the `unchecked` suffix).
        byte_order::read_be_u16(&self[offset..])
    }

    /// Reads an `u32` value from the specified offset, converting it to host byte ordering.
    ///
    /// # Panics
    ///
    /// This method will panic if `offset` is invalid.
    #[inline]
    fn ntohl_unchecked(&self, offset: usize) -> u32 {
        byte_order::read_be_u32(&self[offset..])
    }

    /// Shrinks the current slice to the given `len`.
    ///
    /// Does not check whether `len` is actually smaller than `self.len()`.
    ///
    /// # Panics
    ///
    /// This method will panic if `len` is greater than `self.len()`.
    fn shrink_unchecked(&mut self, len: usize);
}

/// Offers mutable access to a sequence of bytes which stands for different values packed
/// together using network byte ordering.
pub trait NetworkBytesMut: NetworkBytes + DerefMut<Target = [u8]> {
    /// Writes the given `u16` value at the specified `offset` using network byte ordering.
    ///
    /// # Panics
    ///
    /// If `value` cannot be written into `self` at the given `offset`
    /// (e.g. if `offset > self.len() - size_of::<u16>()`).
    #[inline]
    fn htons_unchecked(&mut self, offset: usize, value: u16) {
        assert!(offset <= self.len() - std::mem::size_of::<u16>());
        byte_order::write_be_u16(&mut self[offset..], value)
    }

    /// Writes the given `u32` value at the specified `offset` using network byte ordering.
    ///
    /// # Panics
    ///
    /// If `value` cannot be written into `self` at the given `offset`
    /// (e.g. if `offset > self.len() - size_of::<u32>()`).
    #[inline]
    fn htonl_unchecked(&mut self, offset: usize, value: u32) {
        assert!(offset <= self.len() - std::mem::size_of::<u32>());
        byte_order::write_be_u32(&mut self[offset..], value)
    }
}

impl NetworkBytes for &[u8] {
    #[inline]
    fn shrink_unchecked(&mut self, len: usize) {
        *self = &self[..len];
    }
}
impl NetworkBytes for &mut [u8] {
    #[inline]
    fn shrink_unchecked(&mut self, len: usize) {
        *self = &mut std::mem::take(self)[..len];
    }
}

impl NetworkBytesMut for &mut [u8] {}

// This struct is used as a convenience for any type which contains a generic member implementing
// NetworkBytes with a lifetime, so we don't have to also add the PhantomData member each time. We
// use pub(super) here because we only want this to be usable by the child modules of `pdu`.
#[derive(Debug)]
pub(super) struct InnerBytes<'a, T: 'a> {
    bytes: T,
    phantom: PhantomData<&'a T>,
}

impl<T: Debug> InnerBytes<'_, T> {
    /// Creates a new instance as a wrapper around `bytes`.
    #[inline]
    pub fn new(bytes: T) -> Self {
        InnerBytes {
            bytes,
            phantom: PhantomData,
        }
    }
}

impl<T: Deref<Target = [u8]> + Debug> Deref for InnerBytes<'_, T> {
    type Target = [u8];

    #[inline]
    fn deref(&self) -> &[u8] {
        self.bytes.deref()
    }
}

impl<T: DerefMut<Target = [u8]> + Debug> DerefMut for InnerBytes<'_, T> {
    #[inline]
    fn deref_mut(&mut self) -> &mut [u8] {
        self.bytes.deref_mut()
    }
}

impl<T: NetworkBytes + Debug> NetworkBytes for InnerBytes<'_, T> {
    #[inline]
    fn shrink_unchecked(&mut self, len: usize) {
        self.bytes.shrink_unchecked(len);
    }
}

impl<T: NetworkBytesMut + Debug> NetworkBytesMut for InnerBytes<'_, T> {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[should_panic]
    fn test_htons_unchecked() {
        let mut buf = [u8::default(); std::mem::size_of::<u16>()];
        let mut a = buf.as_mut();
        a.htons_unchecked(1, u16::default());
    }

    #[test]
    #[should_panic]
    fn test_htonl_unchecked() {
        let mut buf = [u8::default(); std::mem::size_of::<u32>()];
        let mut a = buf.as_mut();
        a.htonl_unchecked(1, u32::default());
    }

    #[test]
    fn test_network_bytes() {
        let mut buf = [0u8; 1000];

        {
            let mut a = buf.as_mut();

            a.htons_unchecked(1, 123);
            a.htonl_unchecked(100, 1234);

            assert_eq!(a.ntohs_unchecked(1), 123);
            assert_eq!(a.ntohl_unchecked(100), 1234);

            a.shrink_unchecked(500);

            assert_eq!(a.len(), 500);
            assert_eq!(a.ntohs_unchecked(1), 123);
            assert_eq!(a.ntohl_unchecked(100), 1234);
        }

        {
            let mut b = buf.as_ref();
            b.shrink_unchecked(500);

            assert_eq!(b.len(), 500);
            assert_eq!(b.ntohs_unchecked(1), 123);
            assert_eq!(b.ntohl_unchecked(100), 1234);
        }
    }
}
