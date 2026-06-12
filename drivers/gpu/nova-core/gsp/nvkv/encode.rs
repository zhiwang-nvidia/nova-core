use kernel::prelude::*;

use super::types::{
    Array,
    Index,
    Key,
    KeyId,
    Op,
    Opcode, //
};

pub(crate) trait Encodeable {
    fn encode(&self, encoder: &mut Encoder) -> Result;
}

/// TODO: docs.
macro_rules! nvkv_encode {
    (
        $(#[$attr:meta])*
        $vis:vis struct $name:ident {
            $(
                $(#[$field_attr:meta])*
                $field_vis:vis $field:ident : $ty:ty
            ),* $(,)?
        }
    ) => {
        $(#[$attr])*
        $vis struct $name {
            $(
                $(#[$field_attr])*
                $field_vis $field: $ty,
            )*
        }

        impl $crate::gsp::nvkv::Encodeable for $name {
            #[inline(always)]
            fn encode(&self, encoder: &mut $crate::gsp::nvkv::Encoder) -> ::kernel::error::Result {
                $( $crate::gsp::nvkv::Encodeable::encode(&self.$field, encoder)?; )*
                Ok(())
            }
        }
    };
}

struct IndexedKey<T, const KEY_ID: KeyId> {
    index: Index,
    value: T,
}

impl<T, const KEY_ID: KeyId> IndexedKey<T, KEY_ID> {
    pub(crate) fn new(index: Index, value: T) -> Self {
        Self { index, value }
    }
}

impl<const KEY_ID: KeyId> Encodeable for IndexedKey<u32, KEY_ID> {
    #[inline(always)]
    fn encode(&self, encoder: &mut Encoder) -> Result {
        encoder.encode_u32(KEY_ID, self.index, self.value)
    }
}

impl<const KEY_ID: KeyId> Encodeable for IndexedKey<u64, KEY_ID> {
    #[inline(always)]
    fn encode(&self, encoder: &mut Encoder) -> Result {
        encoder.encode_u64(KEY_ID, self.index, self.value)
    }
}

impl<const KEY_ID: KeyId> Encodeable for IndexedKey<&[u8], KEY_ID> {
    #[inline(always)]
    fn encode(&self, encoder: &mut Encoder) -> Result {
        encoder.encode_array8(KEY_ID, self.index, self.value)
    }
}

impl<const KEY_ID: KeyId> Encodeable for IndexedKey<&[u32], KEY_ID> {
    #[inline(always)]
    fn encode(&self, encoder: &mut Encoder) -> Result {
        encoder.encode_array32(KEY_ID, self.index, self.value)
    }
}

impl<const KEY_ID: KeyId> Encodeable for IndexedKey<&[u64], KEY_ID> {
    #[inline(always)]
    fn encode(&self, encoder: &mut Encoder) -> Result {
        encoder.encode_array64(KEY_ID, self.index, self.value)
    }
}

impl<const N: usize, const KEY_ID: KeyId> Encodeable for IndexedKey<[u8; N], KEY_ID> {
    #[inline(always)]
    fn encode(&self, encoder: &mut Encoder) -> Result {
        encoder.encode_array8(KEY_ID, self.index, &self.value)
    }
}

impl<const N: usize, const KEY_ID: KeyId> Encodeable for IndexedKey<[u32; N], KEY_ID> {
    #[inline(always)]
    fn encode(&self, encoder: &mut Encoder) -> Result {
        encoder.encode_array32(KEY_ID, self.index, &self.value)
    }
}

impl<const N: usize, const KEY_ID: KeyId> Encodeable for IndexedKey<[u64; N], KEY_ID> {
    #[inline(always)]
    fn encode(&self, encoder: &mut Encoder) -> Result {
        encoder.encode_array64(KEY_ID, self.index, &self.value)
    }
}

impl<T, const KEY_ID: KeyId, As> Encodeable for Key<T, KEY_ID, As>
where
    IndexedKey<As, KEY_ID>: Encodeable,
    As: From<T>,
    T: Copy,
{
    #[inline(always)]
    fn encode(&self, encoder: &mut Encoder) -> Result {
        IndexedKey::new(Index::new::<0>(), As::from(self.0)).encode(encoder)
    }
}

impl<T, const N: usize, const KEY_ID: KeyId> Encodeable for Array<T, N, KEY_ID>
where
    T: Default + Copy,
    for<'a> IndexedKey<&'a [T], KEY_ID>: Encodeable,
{
    #[inline(always)]
    fn encode(&self, encoder: &mut Encoder) -> Result {
        IndexedKey::<&[T], KEY_ID>::new(Index::new::<0>(), self.0.as_slice()).encode(encoder)
    }
}

impl<T: Encodeable> Encodeable for Option<T> {
    #[inline(always)]
    fn encode(&self, encoder: &mut Encoder) -> Result {
        if let Some(value) = self {
            value.encode(encoder)?;
        }
        Ok(())
    }
}

#[derive(Copy, Clone)]
#[repr(C)]
union EncoderValue {
    op: Op,
    data: u64,
}

static_assert!(size_of::<EncoderValue>() == size_of::<u64>());

pub(crate) struct Encoder {
    backing: KVVec<EncoderValue>,
}

impl Encoder {
    pub(crate) fn new() -> Self {
        Self {
            backing: KVVec::new(),
        }
    }

    fn push_bytes_with_padding(&mut self, bytes: &[u8]) -> Result {
        let num_entries = bytes.len().div_ceil(size_of::<EncoderValue>());
        self.backing.reserve(num_entries, GFP_KERNEL)?;

        let spare = self.backing.spare_capacity_mut();
        let dst = spare.as_mut_ptr().cast::<u8>();

        // SAFETY: We are guaranteed at least `bytes.len()` bytes of space since we just reserved
        // `num_entries` worth of space.
        unsafe { core::ptr::copy_nonoverlapping(bytes.as_ptr(), dst, bytes.len()) };

        let padding = num_entries * size_of::<EncoderValue>() - bytes.len();
        if padding > 0 {
            // SAFETY: We are guaranteed at least `num_entries * size_of::<EncoderValue>()` bytes of space.
            unsafe { core::ptr::write_bytes(dst.add(bytes.len()), 0, padding) };
        }

        // SAFETY: We just initialized these bytes and every bit pattern is valid for `EncoderValue`.
        unsafe { self.backing.inc_len(num_entries) };

        Ok(())
    }

    pub(crate) fn finish(self) -> KVVec<u8> {
        let (backing, length, capacity) = self.backing.into_raw_parts();

        // SAFETY: All bit patterns are valid for `u8`. `backing` is also from a [`KVVec`] so the
        // allocator is the same.
        unsafe {
            KVVec::from_raw_parts(
                backing.cast::<u8>(),
                length * size_of::<EncoderValue>(),
                capacity * size_of::<EncoderValue>(),
            )
        }
    }

    #[inline(always)]
    fn encode_op(&mut self, op: Op) -> Result {
        self.backing.push(EncoderValue { op }, GFP_KERNEL)?;
        Ok(())
    }

    #[inline(always)]
    pub(crate) fn encode_u32(&mut self, key: KeyId, index: Index, value: u32) -> Result {
        // TODO: Consider automatically merging sequential keys.
        self.encode_op(
            Op::zeroed()
                .with_key(key)
                .with_index(index)
                .with_opcode(Opcode::Imm32)
                .with_value(value),
        )?;
        Ok(())
    }

    #[inline(always)]
    pub(crate) fn encode_u64(&mut self, key: KeyId, index: Index, value: u64) -> Result {
        // TODO: Consider automatically merging sequential keys.
        const KEY_COUNT: u32 = 1;
        self.backing.reserve(2, GFP_KERNEL)?;
        self.encode_op(
            Op::zeroed()
                .with_key(key)
                .with_index(index)
                .with_opcode(Opcode::Seq64)
                .with_value(KEY_COUNT),
        )?;
        self.backing
            .push_within_capacity(EncoderValue { data: value })?;
        Ok(())
    }

    #[inline(always)]
    pub(crate) fn encode_array8(&mut self, key: KeyId, index: Index, array: &[u8]) -> Result {
        let value_count = u32::try_from(array.len())?;
        let num_entries = array.len().div_ceil(size_of::<u64>());
        self.backing.reserve(num_entries + 1, GFP_KERNEL)?;
        self.encode_op(
            Op::zeroed()
                .with_key(key)
                .with_index(index)
                .with_opcode(Opcode::Array8)
                .with_value(value_count),
        )?;
        self.push_bytes_with_padding(<[u8] as IntoBytes>::as_bytes(array))?;
        Ok(())
    }

    #[inline(always)]
    pub(crate) fn encode_array32(&mut self, key: KeyId, index: Index, array: &[u32]) -> Result {
        let value_count = u32::try_from(array.len())?;
        let num_entries = array.len().div_ceil(2);
        self.backing.reserve(num_entries + 1, GFP_KERNEL)?;
        self.encode_op(
            Op::zeroed()
                .with_key(key)
                .with_index(index)
                .with_opcode(Opcode::Array32)
                .with_value(value_count),
        )?;
        self.push_bytes_with_padding(<[u32] as IntoBytes>::as_bytes(array))?;
        Ok(())
    }

    #[inline(always)]
    pub(crate) fn encode_array64(&mut self, key: KeyId, index: Index, array: &[u64]) -> Result {
        let value_count = u32::try_from(array.len())?;
        self.backing.reserve(array.len() + 1, GFP_KERNEL)?;
        self.encode_op(
            Op::zeroed()
                .with_key(key)
                .with_index(index)
                .with_opcode(Opcode::Array64)
                .with_value(value_count),
        )?;
        self.push_bytes_with_padding(<[u64] as IntoBytes>::as_bytes(array))?;
        Ok(())
    }
}
