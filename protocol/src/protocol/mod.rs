// Copyright 2016 Matthew Collins
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

#![allow(dead_code)]
#![allow(non_camel_case_types)]

use aes::Aes128;
use cfb8::stream_cipher::{NewStreamCipher, StreamCipher};
use cfb8::Cfb8;
use std_or_web::fs;

use regex::Regex;
use lazy_static::lazy_static;

pub mod forge;
pub mod mojang;

extern crate regex;
extern crate lazy_static;
use trust_dns_resolver::config::ResolverConfig;
use trust_dns_resolver::config::ResolverOpts;
use crate::format;
use crate::nbt;
use crate::shared::Position;
use byteorder::{BigEndian, ReadBytesExt, WriteBytesExt};
use flate2::read::{ZlibDecoder, ZlibEncoder};
use flate2::Compression;
use instant::{Duration, Instant};
use log::{debug, warn};
use std::convert;
use std::default;
use std::fmt;
use std::io;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::mpsc::Sender;

pub const SUPPORTED_PROTOCOLS: [i32; 24] = [
    754, 753, 751, 736, 735, 578, 575, 498, 490, 485, 480, 477, 452, 451, 404, 340, 316, 315, 210,
    109, 107, 74, 47, 5,
];

static CURRENT_PROTOCOL_VERSION: AtomicI32 = AtomicI32::new(SUPPORTED_PROTOCOLS[0]);
static NETWORK_DEBUG: AtomicBool = AtomicBool::new(false);

pub fn current_protocol_version() -> i32 {
    CURRENT_PROTOCOL_VERSION.load(Ordering::Relaxed)
}

pub fn enable_network_debug() {
    NETWORK_DEBUG.store(true, Ordering::Relaxed);
}

pub fn is_network_debug() -> bool {
    NETWORK_DEBUG.load(Ordering::Relaxed)
}

/// Helper macro for defining packets
#[macro_export]
macro_rules! state_packets {
     ($($state:ident $stateName:ident {
        $($dir:ident $dirName:ident {
            $(
                $(#[$attr:meta])*
                packet $name:ident {
                    $($(#[$fattr:meta])*field $field:ident: $field_type:ty = $(when ($cond:expr))*, )+
                }
            )*
        })+
    })+) => {
        use crate::protocol::*;
        use std::io;

        #[derive(Debug)]
        pub enum Packet {
        $(
            $(
                $(
        $name($state::$dir::$name),
                )*
            )+
        )+
        }

        $(
        pub mod $state {

            $(
            pub mod $dir {
                #![allow(unused_imports)]
                use crate::protocol::*;
                use std::io;
                use crate::format;
                use crate::nbt;
                use crate::types;
                use crate::item;
                use crate::shared::Position;


                #[allow(non_upper_case_globals)]
                pub mod internal_ids {
                    create_ids!(i32, $($name),*);
                }

                $(
                    #[derive(Default, Debug)]
                    $(#[$attr])* pub struct $name {
                        $($(#[$fattr])* pub $field: $field_type),+,
                    }

                    impl PacketType for $name {

                        fn packet_id(&self, version: i32) -> i32 {
                            packet::versions::translate_internal_packet_id_for_version(version, State::$stateName, Direction::$dirName, internal_ids::$name, false)
                        }

                        fn write<W: io::Write>(&self, buf: &mut W) -> Result<(), Error> {
                            $(
                                if true $(&& ($cond(&self)))* {
                                    self.$field.write_to(buf)?;
                                }
                            )+

                            Result::Ok(())
                        }
                    }
                )*
            }
            )+
        }
        )+

        /// Returns the packet for the given state, direction and id after parsing the fields
        /// from the buffer.
        pub fn packet_by_id<R: io::Read>(version: i32, state: State, dir: Direction, id: i32, mut buf: &mut R) -> Result<Option<Packet>, Error> {
            match state {
                $(
                    State::$stateName => {
                        match dir {
                            $(
                                Direction::$dirName => {
                                    let internal_id = packet::versions::translate_internal_packet_id_for_version(version, state, dir, id, true);
                                    match internal_id {
                                    $(
                                        self::$state::$dir::internal_ids::$name => {
                                            use self::$state::$dir::$name;
                                            let mut packet : $name = $name::default();
                                            $(
                                                if true $(&& ($cond(&packet)))* {
                                                    packet.$field = Serializable::read_from(&mut buf)?;
                                                }
                                            )+
                                            Result::Ok(Option::Some(Packet::$name(packet)))
                                        },
                                    )*
                                        _ => Result::Ok(Option::None)
                                    }
                                }
                            )+
                        }
                    }
                )+
            }
        }
    }
}

#[macro_export]
macro_rules! protocol_packet_ids {
    ($($state:ident $stateName:ident {
       $($dir:ident $dirName:ident {
           $(
               $(#[$attr:meta])*
               $id:expr => $name:ident
           )*
       })+
    })+) => {
        use crate::protocol::*;

        pub fn translate_internal_packet_id(state: State, dir: Direction, id: i32, to_internal: bool) -> i32 {
            match state {
                $(
                    State::$stateName => {
                        match dir {
                            $(
                                Direction::$dirName => {
                                    if to_internal {
                                        match id {
                                        $(
                                            $id => crate::protocol::packet::$state::$dir::internal_ids::$name,
                                        )*
                                            _ => panic!("bad packet id 0x{:x} in {:?} {:?}", id, dir, state),
                                        }
                                    } else {
                                        match id {
                                        $(
                                            crate::protocol::packet::$state::$dir::internal_ids::$name => $id,
                                        )*
                                            _ => panic!("bad packet internal id 0x{:x} in {:?} {:?}", id, dir, state),
                                        }
                                    }
                                }
                            )*
                        }
                    }
                )*
            }
        }
    }
}

pub mod packet;
pub mod versions;
pub trait Serializable: Sized {
    fn read_from<R: io::Read>(buf: &mut R) -> Result<Self, Error>;
    fn write_to<W: io::Write>(&self, buf: &mut W) -> Result<(), Error>;
}

impl Serializable for Vec<u8> {
    fn read_from<R: io::Read>(buf: &mut R) -> Result<Vec<u8>, Error> {
        let mut v = Vec::new();
        buf.read_to_end(&mut v)?;
        Ok(v)
    }

    fn write_to<W: io::Write>(&self, buf: &mut W) -> Result<(), Error> {
        buf.write_all(&self[..]).map_err(|v| v.into())
    }
}

impl Serializable for Option<nbt::NamedTag> {
    fn read_from<R: io::Read>(buf: &mut R) -> Result<Option<nbt::NamedTag>, Error> {
        let ty = buf.read_u8()?;
        if ty == 0 {
            Result::Ok(None)
        } else {
            let name = nbt::read_string(buf)?;
            let tag = nbt::Tag::read_from(buf)?;
            Result::Ok(Some(nbt::NamedTag(name, tag)))
        }
    }
    fn write_to<W: io::Write>(&self, buf: &mut W) -> Result<(), Error> {
        match *self {
            Some(ref val) => {
                buf.write_u8(10)?;
                nbt::write_string(buf, &val.0)?;
                val.1.write_to(buf)?;
            }
            None => buf.write_u8(0)?,
        }
        Result::Ok(())
    }
}

impl<T> Serializable for Option<T>
where
    T: Serializable,
{
    fn read_from<R: io::Read>(buf: &mut R) -> Result<Option<T>, Error> {
        Result::Ok(Some(T::read_from(buf)?))
    }
    fn write_to<W: io::Write>(&self, buf: &mut W) -> Result<(), Error> {
        if self.is_some() {
            self.as_ref().unwrap().write_to(buf)?;
        }
        Result::Ok(())
    }
}

impl Serializable for String {
    fn read_from<R: io::Read>(buf: &mut R) -> Result<String, Error> {
        let len = VarInt::read_from(buf)?.0;
        debug_assert!(len >= 0, "Negative string length: {}", len);
        debug_assert!(len <= 65536, "String length too big: {}", len);
        let mut bytes = Vec::<u8>::new();
        buf.take(len as u64).read_to_end(&mut bytes)?;
        let ret = String::from_utf8(bytes).unwrap();
        Result::Ok(ret)
    }
    fn write_to<W: io::Write>(&self, buf: &mut W) -> Result<(), Error> {
        let bytes = self.as_bytes();
        VarInt(bytes.len() as i32).write_to(buf)?;
        buf.write_all(bytes)?;
        Result::Ok(())
    }
}

impl Serializable for format::Component {
    fn read_from<R: io::Read>(buf: &mut R) -> Result<Self, Error> {
        let len = VarInt::read_from(buf)?.0;
        let mut bytes = Vec::<u8>::new();
        buf.take(len as u64).read_to_end(&mut bytes)?;
        let ret = String::from_utf8(bytes).unwrap();
        Result::Ok(Self::from_string(&ret[..]))
    }
    fn write_to<W: io::Write>(&self, buf: &mut W) -> Result<(), Error> {
        let val = serde_json::to_string(&self.to_value()).unwrap();
        let bytes = val.as_bytes();
        VarInt(bytes.len() as i32).write_to(buf)?;
        buf.write_all(bytes)?;
        Result::Ok(())
    }
}

impl Serializable for () {
    fn read_from<R: io::Read>(_: &mut R) -> Result<(), Error> {
        Result::Ok(())
    }
    fn write_to<W: io::Write>(&self, _: &mut W) -> Result<(), Error> {
        Result::Ok(())
    }
}

impl Serializable for bool {
    fn read_from<R: io::Read>(buf: &mut R) -> Result<bool, Error> {
        Result::Ok(buf.read_u8()? != 0)
    }
    fn write_to<W: io::Write>(&self, buf: &mut W) -> Result<(), Error> {
        buf.write_u8(if *self { 1 } else { 0 })?;
        Result::Ok(())
    }
}

impl Serializable for i8 {
    fn read_from<R: io::Read>(buf: &mut R) -> Result<i8, Error> {
        Result::Ok(buf.read_i8()?)
    }
    fn write_to<W: io::Write>(&self, buf: &mut W) -> Result<(), Error> {
        buf.write_i8(*self)?;
        Result::Ok(())
    }
}

impl Serializable for i16 {
    fn read_from<R: io::Read>(buf: &mut R) -> Result<i16, Error> {
        Result::Ok(buf.read_i16::<BigEndian>()?)
    }
    fn write_to<W: io::Write>(&self, buf: &mut W) -> Result<(), Error> {
        buf.write_i16::<BigEndian>(*self)?;
        Result::Ok(())
    }
}

impl Serializable for i32 {
    fn read_from<R: io::Read>(buf: &mut R) -> Result<i32, Error> {
        Result::Ok(buf.read_i32::<BigEndian>()?)
    }
    fn write_to<W: io::Write>(&self, buf: &mut W) -> Result<(), Error> {
        buf.write_i32::<BigEndian>(*self)?;
        Result::Ok(())
    }
}

impl Serializable for i64 {
    fn read_from<R: io::Read>(buf: &mut R) -> Result<i64, Error> {
        Result::Ok(buf.read_i64::<BigEndian>()?)
    }
    fn write_to<W: io::Write>(&self, buf: &mut W) -> Result<(), Error> {
        buf.write_i64::<BigEndian>(*self)?;
        Result::Ok(())
    }
}

impl Serializable for u8 {
    fn read_from<R: io::Read>(buf: &mut R) -> Result<u8, Error> {
        Result::Ok(buf.read_u8()?)
    }
    fn write_to<W: io::Write>(&self, buf: &mut W) -> Result<(), Error> {
        buf.write_u8(*self)?;
        Result::Ok(())
    }
}

impl Serializable for u16 {
    fn read_from<R: io::Read>(buf: &mut R) -> Result<u16, Error> {
        Result::Ok(buf.read_u16::<BigEndian>()?)
    }
    fn write_to<W: io::Write>(&self, buf: &mut W) -> Result<(), Error> {
        buf.write_u16::<BigEndian>(*self)?;
        Result::Ok(())
    }
}

impl Serializable for u64 {
    fn read_from<R: io::Read>(buf: &mut R) -> Result<u64, Error> {
        Result::Ok(buf.read_u64::<BigEndian>()?)
    }
    fn write_to<W: io::Write>(&self, buf: &mut W) -> Result<(), Error> {
        buf.write_u64::<BigEndian>(*self)?;
        Result::Ok(())
    }
}

impl Serializable for f32 {
    fn read_from<R: io::Read>(buf: &mut R) -> Result<f32, Error> {
        Result::Ok(buf.read_f32::<BigEndian>()?)
    }
    fn write_to<W: io::Write>(&self, buf: &mut W) -> Result<(), Error> {
        buf.write_f32::<BigEndian>(*self)?;
        Result::Ok(())
    }
}

impl Serializable for f64 {
    fn read_from<R: io::Read>(buf: &mut R) -> Result<f64, Error> {
        Result::Ok(buf.read_f64::<BigEndian>()?)
    }
    fn write_to<W: io::Write>(&self, buf: &mut W) -> Result<(), Error> {
        buf.write_f64::<BigEndian>(*self)?;
        Result::Ok(())
    }
}

#[derive(Debug, PartialEq, Eq, Hash, Clone)]
pub struct UUID(u64, u64);

#[derive(Debug)]
pub struct UUIDParseError;
impl std::error::Error for UUIDParseError {}

impl fmt::Display for UUIDParseError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "Invalid UUID format")
    }
}

impl std::str::FromStr for UUID {
    type Err = UUIDParseError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.len() != 36 {
            return Err(UUIDParseError {});
        }
        let mut parts = hex::decode(&s[..8]).unwrap();
        parts.extend_from_slice(&hex::decode(&s[9..13]).unwrap());
        parts.extend_from_slice(&hex::decode(&s[14..18]).unwrap());
        parts.extend_from_slice(&hex::decode(&s[19..23]).unwrap());
        parts.extend_from_slice(&hex::decode(&s[24..36]).unwrap());
        let mut high = 0u64;
        let mut low = 0u64;
        for i in 0..8 {
            high |= (parts[i] as u64) << (56 - i * 8);
            low |= (parts[i + 8] as u64) << (56 - i * 8);
        }
        Ok(UUID(high, low))
    }
}

impl Default for UUID {
    fn default() -> Self {
        UUID(0, 0)
    }
}

impl Serializable for UUID {
    fn read_from<R: io::Read>(buf: &mut R) -> Result<UUID, Error> {
        Result::Ok(UUID(
            buf.read_u64::<BigEndian>()?,
            buf.read_u64::<BigEndian>()?,
        ))
    }
    fn write_to<W: io::Write>(&self, buf: &mut W) -> Result<(), Error> {
        buf.write_u64::<BigEndian>(self.0)?;
        buf.write_u64::<BigEndian>(self.1)?;
        Result::Ok(())
    }
}

pub struct Biomes3D {
    pub data: [i32; 1024],
}

impl fmt::Debug for Biomes3D {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "Biomes3D(")?;
        for i in 0..1024 {
            write!(f, "{}, ", self.data[i])?;
        }
        write!(f, ")")
    }
}

impl Default for Biomes3D {
    fn default() -> Self {
        Biomes3D { data: [0; 1024] }
    }
}

impl Serializable for Biomes3D {
    fn read_from<R: io::Read>(buf: &mut R) -> Result<Biomes3D, Error> {
        let data: [i32; 1024] = [0; 1024];

        // Non-length-prefixed three-dimensional biome data
        for item in &mut data.to_vec() {
            let b: i32 = Serializable::read_from(buf)?;
            *item = b;
        }

        Result::Ok(Biomes3D { data })
    }
    fn write_to<W: io::Write>(&self, _buf: &mut W) -> Result<(), Error> {
        unimplemented!()
    }
}

pub trait Lengthable: Serializable + Copy + Default {
    fn into_len(self) -> usize;
    fn from_len(_: usize) -> Self;
}

pub struct LenPrefixed<L: Lengthable, V> {
    len: L,
    pub data: Vec<V>,
}

impl<L: Lengthable, V: Default> LenPrefixed<L, V> {
    pub fn new(data: Vec<V>) -> LenPrefixed<L, V> {
        LenPrefixed {
            len: Default::default(),
            data,
        }
    }
}

impl<L: Lengthable, V: Serializable> Serializable for LenPrefixed<L, V> {
    fn read_from<R: io::Read>(buf: &mut R) -> Result<LenPrefixed<L, V>, Error> {
        let len_data: L = Serializable::read_from(buf)?;
        let len: usize = len_data.into_len();
        let mut data: Vec<V> = Vec::with_capacity(len);
        for _ in 0..len {
            data.push(Serializable::read_from(buf)?);
        }
        Result::Ok(LenPrefixed {
            len: len_data,
            data,
        })
    }

    fn write_to<W: io::Write>(&self, buf: &mut W) -> Result<(), Error> {
        let len_data: L = L::from_len(self.data.len());
        len_data.write_to(buf)?;
        let data = &self.data;
        for val in data {
            val.write_to(buf)?;
        }
        Result::Ok(())
    }
}

impl<L: Lengthable, V: Default> Default for LenPrefixed<L, V> {
    fn default() -> Self {
        LenPrefixed {
            len: default::Default::default(),
            data: default::Default::default(),
        }
    }
}

impl<L: Lengthable, V: fmt::Debug> fmt::Debug for LenPrefixed<L, V> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        self.data.fmt(f)
    }
}

// Optimization
pub struct LenPrefixedBytes<L: Lengthable> {
    len: L,
    pub data: Vec<u8>,
}

impl<L: Lengthable> LenPrefixedBytes<L> {
    pub fn new(data: Vec<u8>) -> LenPrefixedBytes<L> {
        LenPrefixedBytes {
            len: Default::default(),
            data,
        }
    }
}

impl<L: Lengthable> Serializable for LenPrefixedBytes<L> {
    fn read_from<R: io::Read>(buf: &mut R) -> Result<LenPrefixedBytes<L>, Error> {
        let len_data: L = Serializable::read_from(buf)?;
        let len: usize = len_data.into_len();
        let mut data: Vec<u8> = Vec::with_capacity(len);
        buf.take(len as u64).read_to_end(&mut data)?;
        Result::Ok(LenPrefixedBytes {
            len: len_data,
            data,
        })
    }

    fn write_to<W: io::Write>(&self, buf: &mut W) -> Result<(), Error> {
        let len_data: L = L::from_len(self.data.len());
        len_data.write_to(buf)?;
        buf.write_all(&self.data[..])?;
        Result::Ok(())
    }
}

impl<L: Lengthable> Default for LenPrefixedBytes<L> {
    fn default() -> Self {
        LenPrefixedBytes {
            len: default::Default::default(),
            data: default::Default::default(),
        }
    }
}

impl<L: Lengthable> fmt::Debug for LenPrefixedBytes<L> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        self.data.fmt(f)
    }
}

impl Lengthable for bool {
    fn into_len(self) -> usize {
        if self {
            1
        } else {
            0
        }
    }

    fn from_len(u: usize) -> bool {
        u != 0
    }
}

impl Lengthable for u8 {
    fn into_len(self) -> usize {
        self as usize
    }

    fn from_len(u: usize) -> u8 {
        u as u8
    }
}

impl Lengthable for i16 {
    fn into_len(self) -> usize {
        self as usize
    }

    fn from_len(u: usize) -> i16 {
        u as i16
    }
}

impl Lengthable for i32 {
    fn into_len(self) -> usize {
        self as usize
    }

    fn from_len(u: usize) -> i32 {
        u as i32
    }
}

use num_traits::cast::{cast, NumCast};
use trust_dns_resolver::Resolver;
use std::sync::{RwLock, Arc};

/// `FixedPoint5` has the 5 least-significant bits for the fractional
/// part, upper for integer part: https://wiki.vg/Data_types#Fixed-point_numbers
#[derive(Clone, Copy)]
pub struct FixedPoint5<T>(T);

impl<T: Serializable> Serializable for FixedPoint5<T> {
    fn read_from<R: io::Read>(buf: &mut R) -> Result<Self, Error> {
        Ok(Self(Serializable::read_from(buf)?))
    }

    fn write_to<W: io::Write>(&self, buf: &mut W) -> Result<(), Error> {
        self.0.write_to(buf)
    }
}

impl<T: default::Default> default::Default for FixedPoint5<T> {
    fn default() -> Self {
        Self(T::default())
    }
}

impl<T: NumCast> convert::From<f64> for FixedPoint5<T> {
    fn from(x: f64) -> Self {
        let n: T = cast(x * 32.0).unwrap();
        FixedPoint5::<T>(n)
    }
}

impl<T: NumCast> convert::From<FixedPoint5<T>> for f64 {
    fn from(x: FixedPoint5<T>) -> Self {
        let f: f64 = cast(x.0).unwrap();
        f / 32.0
    }
}

impl<T> fmt::Debug for FixedPoint5<T>
where
    T: fmt::Display,
    f64: convert::From<T>,
    T: NumCast + Copy,
{
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let x: f64 = (*self).into();
        write!(f, "FixedPoint5(#{} = {}f)", self.0, x)
    }
}

/// `FixedPoint12` is like `FixedPoint5` but the fractional part is 12-bit
#[derive(Clone, Copy)]
pub struct FixedPoint12<T>(T);

impl<T: Serializable> Serializable for FixedPoint12<T> {
    fn read_from<R: io::Read>(buf: &mut R) -> Result<Self, Error> {
        Ok(Self(Serializable::read_from(buf)?))
    }

    fn write_to<W: io::Write>(&self, buf: &mut W) -> Result<(), Error> {
        self.0.write_to(buf)
    }
}

impl<T: default::Default> default::Default for FixedPoint12<T> {
    fn default() -> Self {
        Self(T::default())
    }
}

impl<T: NumCast> convert::From<f64> for FixedPoint12<T> {
    fn from(x: f64) -> Self {
        let n: T = cast(x * 32.0 * 128.0).unwrap();
        FixedPoint12::<T>(n)
    }
}

impl<T: NumCast> convert::From<FixedPoint12<T>> for f64 {
    fn from(x: FixedPoint12<T>) -> Self {
        let f: f64 = cast(x.0).unwrap();
        f / (32.0 * 128.0)
    }
}

impl<T> fmt::Debug for FixedPoint12<T>
where
    T: fmt::Display,
    f64: convert::From<T>,
    T: NumCast + Copy,
{
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let x: f64 = (*self).into();
        write!(f, "FixedPoint12(#{} = {}f)", self.0, x)
    }
}

/// `VarInt` have a variable size (between 1 and 5 bytes) when encoded based
/// on the size of the number
#[derive(Clone, Copy)]
pub struct VarInt(pub i32);

impl Lengthable for VarInt {
    fn into_len(self) -> usize {
        self.0 as usize
    }

    fn from_len(u: usize) -> VarInt {
        VarInt(u as i32)
    }
}

impl Serializable for VarInt {
    /// Decodes a `VarInt` from the Reader
    fn read_from<R: io::Read>(buf: &mut R) -> Result<VarInt, Error> {
        const PART: u32 = 0x7F;
        let mut size = 0;
        let mut val = 0u32;
        loop {
            let b = buf.read_u8()? as u32;
            val |= (b & PART) << (size * 7);
            size += 1;
            if size > 5 {
                return Result::Err(Error::Err("VarInt too big".to_owned()));
            }
            if (b & 0x80) == 0 {
                break;
            }
        }

        Result::Ok(VarInt(val as i32))
    }

    /// Encodes a `VarInt` into the Writer
    fn write_to<W: io::Write>(&self, buf: &mut W) -> Result<(), Error> {
        const PART: u32 = 0x7F;
        let mut val = self.0 as u32;
        loop {
            if (val & !PART) == 0 {
                buf.write_u8(val as u8)?;
                return Result::Ok(());
            }
            buf.write_u8(((val & PART) | 0x80) as u8)?;
            val >>= 7;
        }
    }
}

impl default::Default for VarInt {
    fn default() -> VarInt {
        VarInt(0)
    }
}

impl fmt::Debug for VarInt {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// `VarShort` have a variable size (2 or 3 bytes) and are backwards-compatible
/// with vanilla shorts, used for Forge custom payloads
#[derive(Clone, Copy)]
pub struct VarShort(pub i32);

impl Lengthable for VarShort {
    fn into_len(self) -> usize {
        self.0 as usize
    }

    fn from_len(u: usize) -> VarShort {
        VarShort(u as i32)
    }
}

impl Serializable for VarShort {
    fn read_from<R: io::Read>(buf: &mut R) -> Result<VarShort, Error> {
        let low = buf.read_u16::<BigEndian>()? as u32;
        let val = if (low & 0x8000) != 0 {
            let high = buf.read_u8()? as u32;

            (high << 15) | (low & 0x7fff)
        } else {
            low
        };

        Result::Ok(VarShort(val as i32))
    }

    fn write_to<W: io::Write>(&self, buf: &mut W) -> Result<(), Error> {
        assert!(
            self.0 >= 0 && self.0 <= 0x7fffff,
            "VarShort invalid value: {}",
            self.0
        );
        let mut low = self.0 & 0x7fff;
        let high = (self.0 & 0x7f8000) >> 15;
        if high != 0 {
            low |= 0x8000;
        }

        buf.write_u16::<BigEndian>(low as u16)?;

        if high != 0 {
            buf.write_u8(high as u8)?;
        }

        Ok(())
    }
}

impl default::Default for VarShort {
    fn default() -> VarShort {
        VarShort(0)
    }
}

impl fmt::Debug for VarShort {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// `VarLong` have a variable size (between 1 and 10 bytes) when encoded based
/// on the size of the number
#[derive(Clone, Copy)]
pub struct VarLong(pub i64);

impl Lengthable for VarLong {
    fn into_len(self) -> usize {
        self.0 as usize
    }

    fn from_len(u: usize) -> VarLong {
        VarLong(u as i64)
    }
}

impl Serializable for VarLong {
    /// Decodes a `VarLong` from the Reader
    fn read_from<R: io::Read>(buf: &mut R) -> Result<VarLong, Error> {
        const PART: u64 = 0x7F;
        let mut size = 0;
        let mut val = 0u64;
        loop {
            let b = buf.read_u8()? as u64;
            val |= (b & PART) << (size * 7);
            size += 1;
            if size > 10 {
                return Result::Err(Error::Err("VarLong too big".to_owned()));
            }
            if (b & 0x80) == 0 {
                break;
            }
        }

        Result::Ok(VarLong(val as i64))
    }

    /// Encodes a `VarLong` into the Writer
    fn write_to<W: io::Write>(&self, buf: &mut W) -> Result<(), Error> {
        const PART: u64 = 0x7F;
        let mut val = self.0 as u64;
        loop {
            if (val & !PART) == 0 {
                buf.write_u8(val as u8)?;
                return Result::Ok(());
            }
            buf.write_u8(((val & PART) | 0x80) as u8)?;
            val >>= 7;
        }
    }
}

impl default::Default for VarLong {
    fn default() -> VarLong {
        VarLong(0)
    }
}

impl fmt::Debug for VarLong {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl Serializable for Position {
    fn read_from<R: io::Read>(buf: &mut R) -> Result<Position, Error> {
        let pos = buf.read_u64::<BigEndian>()?;
        Ok(Position::new(
            ((pos as i64) >> 38) as i32,
            ((pos as i64) & 0xFFF) as i32,
            ((pos as i64) << 26 >> 38) as i32,
        ))
    }
    fn write_to<W: io::Write>(&self, buf: &mut W) -> Result<(), Error> {
        let pos = (((self.x as u64) & 0x3FFFFFF) << 38)
            | ((self.y as u64) & 0xFFF)
            | (((self.z as u64) & 0x3FFFFFF) << 12);

        buf.write_u64::<BigEndian>(pos)?;
        Result::Ok(())
    }
}

/// Direction is used to define whether packets are going to the
/// server or the client.
#[derive(Clone, Copy, Debug)]
pub enum Direction {
    Serverbound,
    Clientbound,
}

/// The protocol has multiple 'sub-protocols' or states which control which
/// packet an id points to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Handshaking,
    Play,
    Status,
    Login,
}

/// Return for any protocol related error.
#[derive(Debug)]
pub enum Error {
    Err(String),
    Disconnect(format::Component),
    IOError(io::Error),
    Json(serde_json::Error),
    Reqwest(reqwest::Error),
}

impl convert::From<io::Error> for Error {
    fn from(e: io::Error) -> Error {
        Error::IOError(e)
    }
}

impl convert::From<serde_json::Error> for Error {
    fn from(e: serde_json::Error) -> Error {
        Error::Json(e)
    }
}

impl convert::From<reqwest::Error> for Error {
    fn from(e: reqwest::Error) -> Error {
        Error::Reqwest(e)
    }
}

impl ::std::error::Error for Error {}

impl ::std::fmt::Display for Error {
    fn fmt(&self, f: &mut ::std::fmt::Formatter) -> ::std::fmt::Result {
        match *self {
            Error::Err(ref val) => write!(f, "protocol error: {}", val),
            Error::Disconnect(ref val) => write!(f, "{}", val),
            Error::IOError(ref e) => e.fmt(f),
            Error::Json(ref e) => e.fmt(f),
            Error::Reqwest(ref e) => e.fmt(f),
        }
    }
}

type Aes128Cfb = Cfb8<Aes128>;

pub struct Conn {
    stream: /*Arc<RwLock<*/TcpStream/*>>*/,
    pub host: String,
    pub port: u16,
    direction: Direction,
    pub protocol_version: i32,
    pub state: State,

    read_cipher: Option<Arc<RwLock<Aes128Cfb>>>,
    write_cipher: Option<Arc<RwLock<Aes128Cfb>>>,

    pub compression_threshold: i32,
    pub send: Option<Sender<(i32, bool, Vec<u8>)>>,
}

lazy_static! {
    static ref IPADDRESS_PATTERN: Regex =
        Regex::new(format!("{}{}{}{}", "^([01]?\\d\\d?|2[0-4]\\d|25[0-5])\\.",
        "([01]?\\d\\d?|2[0-4]\\d|25[0-5])\\.",
        "([01]?\\d\\d?|2[0-4]\\d|25[0-5])\\.",
        "([01]?\\d\\d?|2[0-4]\\d|25[0-5])$").as_str()).unwrap();
    static ref RESOLVER: Resolver =
    Resolver::new(ResolverConfig::default(), ResolverOpts::default()).unwrap();
}

impl Conn {

    fn get_server_addresses(mut hostname: &str) -> Vec<String> {
        let mut addresses = vec![];
        let parts = hostname.split(':').collect::<Vec<&str>>();
        if parts.len() > 1 {
            addresses.push(hostname.to_string());
            hostname = parts[0];
        }
        let records = RESOLVER.srv_lookup(format!("_minecraft._tcp.{}", hostname));
        if records.is_ok() {
            for record in records.unwrap() {
                println!("{}:{}", record.target(), record.port());
                addresses.push(format!("{}:{}", record.target(), record.port()));
            }
        }
        addresses.push(format!("{}:25565", hostname));
        addresses
    }

    pub fn new(target: &str, protocol_version: i32) -> Result<Conn, Error> {
        CURRENT_PROTOCOL_VERSION.store(protocol_version, Ordering::Relaxed);

        let mut address = target.to_string();
        if !IPADDRESS_PATTERN.is_match(target) {
            println!("{} has an no address! :(", address);
            let result = Conn::get_server_addresses(target);
            // TODO: Try all possible ips not just the first!
            let next = result.iter().next().unwrap();
            println!("{}'s ip may be {}.", address, next);
            address = next.to_string();
        }

        let stream = TcpStream::connect(&*address)?;
        let parts = address.split(':').collect::<Vec<&str>>();
        Result::Ok(Conn {
            stream: /*Arc::new(RwLock::new(*/stream/*))*/,
            host: parts[0].to_owned(),
            port: parts[1].parse().unwrap(),
            direction: Direction::Serverbound,
            state: State::Handshaking,
            protocol_version,
            read_cipher: None,
            write_cipher: None,
            compression_threshold: -1,
            send: None,
        })
    }

    pub fn write_packet<T: PacketType>(&mut self, packet: T) -> Result<(), Error> {
        let mut buf = Vec::new();
        VarInt(packet.packet_id(self.protocol_version)).write_to(&mut buf)?;
        packet.write(&mut buf)?;

        let mut extra = if self.compression_threshold >= 0 {
            1
        } else {
            0
        };
        if self.compression_threshold >= 0 && buf.len() as i32 > self.compression_threshold {
            extra = 0;
            let uncompressed_size = buf.len();
            let mut new = Vec::new();
            VarInt(uncompressed_size as i32).write_to(&mut new)?;
            let mut write = ZlibEncoder::new(io::Cursor::new(buf), Compression::default());
            write.read_to_end(&mut new)?;
            if is_network_debug() {
                debug!(
                    "Compressed for sending {} bytes to {} since > threshold {}, new={:?}",
                    uncompressed_size,
                    new.len(),
                    self.compression_threshold,
                    new
                );
            }
            buf = new;
        }
        // let mut send_buf = Vec::new();
        if self.send.is_some()/* && false*/ {
            // VarInt(buf.len() as i32 + extra).write_to(&mut send_buf/*self*/)?;
            /*if self.compression_threshold >= 0 && extra == 1 {
                VarInt(0).write_to(&mut send_buf/*self*/)?;
            }*/
            // send_buf.append(&mut buf);
            self.send.as_ref().unwrap()/*.clone()*/.send((extra, self.compression_threshold >= 0, buf)).unwrap();
        }else {
            VarInt(buf.len() as i32 + extra).write_to(/*&mut */self)?;
            if self.compression_threshold >= 0 && extra == 1 {
                VarInt(0).write_to(/*&mut */self)?;
            }
            self.write_all(&buf).unwrap();
        }
        // self.write_all(&send_buf/*&buf*/)?;
        // self.write_buffer(buffer/*buf*/);

        Ok(())
    }

    /*
    pub fn write_buffer(&mut self, buffer: Vec<u8>) {
        // self.queue.clone().write().unwrap().push_back(buffer);
        if self.send.is_some() && false {
            self.send.as_ref().unwrap().send(buffer).unwrap();
        }else {
            self.write_all(&buffer).unwrap();
        }
    }*/
    /*
    keep alive!
thread '<unnamed>' panicked at 'attempt to shift left with overflow', protocol/src/protocol/mod.rs:793:20
stack backtrace:
   0: rust_begin_unwind
             at /rustc/53cb7b09b00cbea8754ffb78e7e3cb521cb8af4b/library/std/src/panicking.rs:493:5
   1: core::panicking::panic_fmt
             at /rustc/53cb7b09b00cbea8754ffb78e7e3cb521cb8af4b/library/core/src/panicking.rs:92:14
   2: core::panicking::panic
             at /rustc/53cb7b09b00cbea8754ffb78e7e3cb521cb8af4b/library/core/src/panicking.rs:50:5
   3: <steven_protocol::protocol::VarInt as steven_protocol::protocol::Serializable>::read_from
             at /home/threadexception/IdeaProjects/stevenarella/protocol/src/protocol/mod.rs:793:20
   4: steven_protocol::protocol::Conn::read_raw_packet_from
             at /home/threadexception/IdeaProjects/stevenarella/protocol/src/protocol/mod.rs:1243:37
   5: steven_protocol::protocol::Conn::read_packet
             at /home/threadexception/IdeaProjects/stevenarella/protocol/src/protocol/mod.rs:1269:29
   6: stevenarella::server::Server::spawn_reader::{{closure}}
             at /home/threadexception/IdeaProjects/stevenarella/src/server/mod.rs:355:23
note: Some details are omitted, run with `RUST_BACKTRACE=full` for a verbose backtrace.
Diff5 took 9376
     */

    /*
    thread 'main' panicked at 'Err: IOError(Error { kind: UnexpectedEof, message: "failed to fill whole buffer" })', src/server/mod.rs:806:33
stack backtrace:
   0: rust_begin_unwind
             at /rustc/53cb7b09b00cbea8754ffb78e7e3cb521cb8af4b/library/std/src/panicking.rs:493:5
   1: std::panicking::begin_panic_fmt
             at /rustc/53cb7b09b00cbea8754ffb78e7e3cb521cb8af4b/library/std/src/panicking.rs:435:5
   2: stevenarella::server::Server::entity_tick
             at /home/threadexception/IdeaProjects/stevenarella/src/server/mod.rs:806:33
   3: stevenarella::server::Server::tick
             at /home/threadexception/IdeaProjects/stevenarella/src/server/mod.rs:670:9
   4: stevenarella::tick_all
             at /home/threadexception/IdeaProjects/stevenarella/src/main.rs:452:5
   5: stevenarella::main::{{closure}}
             at /home/threadexception/IdeaProjects/stevenarella/src/main.rs:384:9
   6: winit::platform_impl::platform::sticky_exit_callback
             at /home/threadexception/.cargo/registry/src/github.com-1ecc6299db9ec823/winit-0.25.0/src/platform_impl/linux/mod.rs:746:5
   7: winit::platform_impl::platform::wayland::event_loop::EventLoop<T>::run_return
             at /home/threadexception/.cargo/registry/src/github.com-1ecc6299db9ec823/winit-0.25.0/src/platform_impl/linux/wayland/event_loop/mod.rs:354:13
   8: winit::platform_impl::platform::wayland::event_loop::EventLoop<T>::run
             at /home/threadexception/.cargo/registry/src/github.com-1ecc6299db9ec823/winit-0.25.0/src/platform_impl/linux/wayland/event_loop/mod.rs:191:9
   9: winit::platform_impl::platform::EventLoop<T>::run
             at /home/threadexception/.cargo/registry/src/github.com-1ecc6299db9ec823/winit-0.25.0/src/platform_impl/linux/mod.rs:662:56
  10: winit::event_loop::EventLoop<T>::run
             at /home/threadexception/.cargo/registry/src/github.com-1ecc6299db9ec823/winit-0.25.0/src/event_loop.rs:154:9
  11: stevenarella::main
             at /home/threadexception/IdeaProjects/stevenarella/src/main.rs:364:5
  12: core::ops::function::FnOnce::call_once
             at /home/threadexception/.rustup/toolchains/stable-x86_64-unknown-linux-gnu/lib/rustlib/src/rust/library/core/src/ops/function.rs:227:5
note: Some details are omitted, run with `RUST_BACKTRACE=full` for a verbose backtrace.
thread '<unnamed>' panicked at 'called `Result::unwrap()` on an `Err` value: RecvError', src/server/mod.rs:468:43
stack backtrace:
   0: rust_begin_unwind
             at /rustc/53cb7b09b00cbea8754ffb78e7e3cb521cb8af4b/library/std/src/panicking.rs:493:5
   1: core::panicking::panic_fmt
             at /rustc/53cb7b09b00cbea8754ffb78e7e3cb521cb8af4b/library/core/src/panicking.rs:92:14
   2: core::result::unwrap_failed
             at /rustc/53cb7b09b00cbea8754ffb78e7e3cb521cb8af4b/library/core/src/result.rs:1355:5
   3: core::result::Result<T,E>::unwrap
             at /home/threadexception/.rustup/toolchains/stable-x86_64-unknown-linux-gnu/lib/rustlib/src/rust/library/core/src/result.rs:1037:23
   4: stevenarella::server::Server::spawn_writer::{{closure}}
             at /home/threadexception/IdeaProjects/stevenarella/src/server/mod.rs:468:33
note: Some details are omitted, run with `RUST_BACKTRACE=full` for a verbose backtrace.
     */

    /*
    entity_tick!
keep alive!
memory allocation of 18446744072381014920 bytes failed
     */

    /*
    ChunkDiff2 took 0
Diff6 took 1
Ticking took 1
thread 'main' panicked at 'Err: IOError(Error { kind: UnexpectedEof, message: "failed to fill whole buffer" })', src/server/mod.rs:814:33
stack backtrace:
   0: rust_begin_unwind
             at /rustc/53cb7b09b00cbea8754ffb78e7e3cb521cb8af4b/library/std/src/panicking.rs:493:5
   1: std::panicking::begin_panic_fmt
             at /rustc/53cb7b09b00cbea8754ffb78e7e3cb521cb8af4b/library/std/src/panicking.rs:435:5
   2: stevenarella::server::Server::entity_tick
             at /home/threadexception/IdeaProjects/stevenarella/src/server/mod.rs:814:33
   3: stevenarella::server::Server::tick
             at /home/threadexception/IdeaProjects/stevenarella/src/server/mod.rs:678:9
   4: stevenarella::tick_all
             at /home/threadexception/IdeaProjects/stevenarella/src/main.rs:494:5
   5: stevenarella::main::{{closure}}
             at /home/threadexception/IdeaProjects/stevenarella/src/main.rs:426:9
   6: winit::platform_impl::platform::sticky_exit_callback
             at /home/threadexception/.cargo/registry/src/github.com-1ecc6299db9ec823/winit-0.25.0/src/platform_impl/linux/mod.rs:746:5
   7: winit::platform_impl::platform::wayland::event_loop::EventLoop<T>::run_return
             at /home/threadexception/.cargo/registry/src/github.com-1ecc6299db9ec823/winit-0.25.0/src/platform_impl/linux/wayland/event_loop/mod.rs:354:13
   8: winit::platform_impl::platform::wayland::event_loop::EventLoop<T>::run
             at /home/threadexception/.cargo/registry/src/github.com-1ecc6299db9ec823/winit-0.25.0/src/platform_impl/linux/wayland/event_loop/mod.rs:191:9
   9: winit::platform_impl::platform::EventLoop<T>::run
             at /home/threadexception/.cargo/registry/src/github.com-1ecc6299db9ec823/winit-0.25.0/src/platform_impl/linux/mod.rs:662:56
  10: winit::event_loop::EventLoop<T>::run
             at /home/threadexception/.cargo/registry/src/github.com-1ecc6299db9ec823/winit-0.25.0/src/event_loop.rs:154:9
  11: stevenarella::main
             at /home/threadexception/IdeaProjects/stevenarella/src/main.rs:406:5
  12: core::ops::function::FnOnce::call_once
             at /home/threadexception/.rustup/toolchains/stable-x86_64-unknown-linux-gnu/lib/rustlib/src/rust/library/core/src/ops/function.rs:227:5
note: Some details are omitted, run with `RUST_BACKTRACE=full` for a verbose backtrace.
thread '<unnamed>' panicked at 'called `Result::unwrap()` on an `Err` value: RecvError', src/server/mod.rs:469:56
stack backtrace:
   0: rust_begin_unwind
             at /rustc/53cb7b09b00cbea8754ffb78e7e3cb521cb8af4b/library/std/src/panicking.rs:493:5
   1: core::panicking::panic_fmt
             at /rustc/53cb7b09b00cbea8754ffb78e7e3cb521cb8af4b/library/core/src/panicking.rs:92:14
   2: core::result::unwrap_failed
             at /rustc/53cb7b09b00cbea8754ffb78e7e3cb521cb8af4b/library/core/src/result.rs:1355:5
   3: core::result::Result<T,E>::unwrap
             at /home/threadexception/.rustup/toolchains/stable-x86_64-unknown-linux-gnu/lib/rustlib/src/rust/library/core/src/result.rs:1037:23
   4: stevenarella::server::Server::spawn_writer::{{closure}}
             at /home/threadexception/IdeaProjects/stevenarella/src/server/mod.rs:469:46
note: Some details are omitted, run with `RUST_BACKTRACE=full` for a verbose backtrace.

Process finished with exit code 101
     */
    /*
    Ticking took 1
keep alive!
thread '<unnamed>' panicked at 'Tried to read more than 1GB of data!', protocol/src/protocol/mod.rs:1371:13
stack backtrace:
   0: std::panicking::begin_panic
             at /home/threadexception/.rustup/toolchains/stable-x86_64-unknown-linux-gnu/lib/rustlib/src/rust/library/std/src/panicking.rs:519:12
   1: steven_protocol::protocol::Conn::read_raw_packet_from
             at /home/threadexception/IdeaProjects/stevenarella/protocol/src/protocol/mod.rs:1371:13
   2: steven_protocol::protocol::Conn::read_packet
             at /home/threadexception/IdeaProjects/stevenarella/protocol/src/protocol/mod.rs:1405:29
   3: stevenarella::server::Server::spawn_reader::{{closure}}
             at /home/threadexception/IdeaProjects/stevenarella/src/server/mod.rs:359:23
note: Some details are omitted, run with `RUST_BACKTRACE=full` for a verbose backtrace.
[server/mod.rs:2079][INFO] Received chat message: Don't forget to vote for us! You currently have 0 votes.

Process finished with exit code 137 (interrupted by signal 9: SIGKILL)
     */

    pub fn write_plugin_message(&mut self, channel: &str, data: &[u8]) -> Result<(), Error> {
        if is_network_debug() {
            debug!(
                "Sending plugin message: channel={}, data={:?}",
                channel, data
            );
        }
        debug_assert!(self.state == State::Play);
        if self.protocol_version >= 47 {
            self.write_packet(packet::play::serverbound::PluginMessageServerbound {
                channel: channel.to_string(),
                data: data.to_vec(),
            })?;
        } else {
            self.write_packet(packet::play::serverbound::PluginMessageServerbound_i16 {
                channel: channel.to_string(),
                data: LenPrefixedBytes::<VarShort>::new(data.to_vec()),
            })?;
        }

        Ok(())
    }

    pub fn write_fmlhs_plugin_message(&mut self, msg: &forge::FmlHs) -> Result<(), Error> {
        let mut buf: Vec<u8> = vec![];
        msg.write_to(&mut buf)?;

        self.write_plugin_message("FML|HS", &buf)
    }

    pub fn write_login_plugin_response(
        &mut self,
        message_id: VarInt,
        successful: bool,
        data: &[u8],
    ) -> Result<(), Error> {
        if is_network_debug() {
            debug!(
                "Sending login plugin message: message_id={:?}, successful={:?}, data={:?}",
                message_id, successful, data,
            );
        }
        debug_assert!(self.state == State::Login);
        self.write_packet(packet::login::serverbound::LoginPluginResponse {
            message_id,
            successful,
            data: data.to_vec(),
        })
    }

    pub fn write_fml2_handshake_plugin_message(
        &mut self,
        message_id: VarInt,
        msg: Option<&forge::fml2::FmlHandshake>,
    ) -> Result<(), Error> {
        if let Some(msg) = msg {
            let mut inner_buf: Vec<u8> = vec![];
            msg.write_to(&mut inner_buf)?;

            let mut outer_buf: Vec<u8> = vec![];
            "fml:handshake".to_string().write_to(&mut outer_buf)?;
            VarInt(inner_buf.len() as i32).write_to(&mut outer_buf)?;
            inner_buf.write_to(&mut outer_buf)?;

            self.write_login_plugin_response(message_id, true, &outer_buf)
        } else {
            unimplemented!() // successful: false, no payload
        }
    }

    #[allow(clippy::type_complexity)]
    pub fn read_raw_packet_from<R: io::Read>(
        buf: &mut R,
        compression_threshold: i32,
    ) -> Result<(i32, Box<io::Cursor<Vec<u8>>>), Error> {
        let len = VarInt::read_from(buf)?.0 as usize;
        if len > 1000000000 {
            panic!("Tried to read more than 1GB of data!");
        }
        let mut ibuf = vec![0; len];
        buf.read_exact(&mut ibuf)?;

        let mut buf = io::Cursor::new(ibuf);

        if compression_threshold >= 0 {
            let uncompressed_size = VarInt::read_from(&mut buf)?.0;
            if uncompressed_size != 0 {
                let mut new = Vec::with_capacity(uncompressed_size as usize);
                {
                    let mut reader = ZlibDecoder::new(buf);
                    reader.read_to_end(&mut new)?;
                }
                if is_network_debug() {
                    debug!(
                        "Decompressed threshold={} len={} uncompressed_size={} to {} bytes",
                        compression_threshold,
                        len,
                        uncompressed_size,
                        new.len()
                    );
                }
                buf = io::Cursor::new(new);
            }
        }
        let id = VarInt::read_from(&mut buf)?.0;

        Ok((id, Box::new(buf)))
    }

    pub fn read_packet(&mut self) -> Result<packet::Packet, Error> {
        let compression_threshold = self.compression_threshold;
        let (id, mut buf) = Conn::read_raw_packet_from(self, compression_threshold)?;

        let dir = match self.direction {
            Direction::Clientbound => Direction::Serverbound,
            Direction::Serverbound => Direction::Clientbound,
        };

        if is_network_debug() {
            debug!(
                "about to parse id={:x}, dir={:?} state={:?}",
                id, dir, self.state
            );
            fs::File::create("last-packet")?.write_all(buf.get_ref())?;
        }

        let packet = packet::packet_by_id(self.protocol_version, self.state, dir, id, &mut buf)?;

        if is_network_debug() {
            debug!("packet = {:?}", packet);
        }

        match packet {
            Some(val) => {
                let pos = buf.position() as usize;
                let ibuf = buf.into_inner();
                if ibuf.len() != pos {
                    debug!("pos = {:?}", pos);
                    debug!("ibuf = {:?}", ibuf);
                    return Result::Err(Error::Err(format!(
                        "Failed to read all of packet 0x{:X}, \
                                                           had {} bytes left",
                        id,
                        ibuf.len() - pos
                    )));
                }
                Result::Ok(val)
            }
            None => Result::Err(Error::Err("missing packet".to_owned())),
        }
    }

    pub fn enable_encyption(&mut self, key: &[u8]/*, _decrypt: bool*/) {
        let read_cipher = Arc::new(RwLock::new(Aes128Cfb::new_var(key, key).unwrap()));
        let write_cipher = Arc::new(RwLock::new(Aes128Cfb::new_var(key, key).unwrap()));
        self.read_cipher = Some(read_cipher);
        self.write_cipher = Some(write_cipher);
    }

    pub fn set_compresssion(&mut self, threshold: i32) {
        self.compression_threshold = threshold;
    }

    pub fn do_status(mut self) -> Result<(Status, Duration), Error> {
        use self::packet::handshake::serverbound::Handshake;
        use self::packet::status::serverbound::*;
        use self::packet::Packet;
        use serde_json::Value;
        let host = self.host.clone();
        let port = self.port;
        self.write_packet(Handshake {
            protocol_version: VarInt(self.protocol_version),
            host,
            port,
            next: VarInt(1),
        })?;
        self.state = State::Status;

        self.write_packet(StatusRequest { empty: () })?;

        let status = if let Packet::StatusResponse(res) = self.read_packet()? {
            res.status
        } else {
            return Err(Error::Err("Wrong packet".to_owned()));
        };

        let start = Instant::now();
        self.write_packet(StatusPing { ping: 42 })?;

        if let Packet::StatusPong(_) = self.read_packet()? {
        } else {
            return Err(Error::Err("Wrong packet".to_owned()));
        };

        let ping = start.elapsed();

        let val: Value = match serde_json::from_str(&status) {
            Ok(val) => val,
            Err(_) => return Err(Error::Err("Json parse error".to_owned())),
        };

        let invalid_status = || Error::Err("Invalid status".to_owned());

        let version = val.get("version").ok_or_else(invalid_status)?;
        let players = val.get("players").ok_or_else(invalid_status)?;

        // For modded servers, get the list of Forge mods installed
        let mut forge_mods: std::vec::Vec<crate::protocol::forge::ForgeMod> = vec![];
        let mut fml_network_version: Option<i64> = None;
        if let Some(modinfo) = val.get("modinfo") {
            if let Some(modinfo_type) = modinfo.get("type") {
                if modinfo_type == "FML" {
                    if let Some(modlist) = modinfo.get("modList") {
                        if let Value::Array(items) = modlist {
                            for item in items {
                                if let Value::Object(obj) = item {
                                    let modid =
                                        obj.get("modid").unwrap().as_str().unwrap().to_string();
                                    let version =
                                        obj.get("version").unwrap().as_str().unwrap().to_string();

                                    forge_mods
                                        .push(crate::protocol::forge::ForgeMod { modid, version });
                                }
                            }
                            fml_network_version = Some(1);
                        }
                    }
                } else {
                    warn!(
                        "Unrecognized modinfo type in server ping response: {} in {}",
                        modinfo_type, modinfo
                    );
                }
            }
        }
        // Forge 1.13+ TODO: update for 1.14+ and test
        if let Some(forge_data) = val.get("forgeData") {
            if let Some(mods) = forge_data.get("mods") {
                if let Value::Array(items) = mods {
                    for item in items {
                        if let Value::Object(obj) = item {
                            let modid = obj.get("modId").unwrap().as_str().unwrap().to_string();
                            let modmarker =
                                obj.get("modmarker").unwrap().as_str().unwrap().to_string();

                            let version = modmarker;

                            forge_mods.push(crate::protocol::forge::ForgeMod { modid, version });
                        }
                    }
                }
            }
            fml_network_version = Some(
                forge_data
                    .get("fmlNetworkVersion")
                    .unwrap()
                    .as_i64()
                    .unwrap(),
            );
        }

        Ok((
            Status {
                version: StatusVersion {
                    name: version
                        .get("name")
                        .and_then(Value::as_str)
                        .ok_or_else(invalid_status)?
                        .to_owned(),
                    protocol: version
                        .get("protocol")
                        .and_then(Value::as_i64)
                        .ok_or_else(invalid_status)? as i32,
                },
                players: StatusPlayers {
                    max: players
                        .get("max")
                        .and_then(Value::as_i64)
                        .ok_or_else(invalid_status)? as i32,
                    online: players
                        .get("online")
                        .and_then(Value::as_i64)
                        .ok_or_else(invalid_status)? as i32,
                    sample: Vec::new(), /* TODO */
                },
                description: format::Component::from_value(
                    val.get("description").ok_or_else(invalid_status)?,
                ),
                favicon: val
                    .get("favicon")
                    .and_then(Value::as_str)
                    .map(|v| v.to_owned()),
                forge_mods,
                fml_network_version,
            },
            ping,
        ))
    }
}

/// Parse a clientbound packet, for debugging packet parsing issues (Conn::read_packet)
pub fn try_parse_packet(ibuf: Vec<u8>, protocol_version: i32) {
    println!("trying to parse packet data {:?}", ibuf);

    let mut buf = io::Cursor::new(ibuf);

    let id = VarInt::read_from(&mut buf).unwrap().0;
    let dir = Direction::Clientbound;
    let state = State::Play; // TODO: allow parsing other states

    println!(
        "about to parse id={:x}, dir={:?} state={:?}",
        id, dir, state
    );

    let packet = packet::packet_by_id(protocol_version, state, dir, id, &mut buf).unwrap();

    println!("packet = {:?}", packet);

    match packet {
        Some(_val) => {
            let pos = buf.position() as usize;
            let ibuf = buf.into_inner();
            if ibuf.len() != pos {
                println!("pos = {:?}", pos);
                println!("ibuf = {:?}", ibuf);
                println!(
                    "Failed to read all of packet 0x{:X}, \
                                                       had {} bytes left",
                    id,
                    ibuf.len() - pos
                )
            }
        }
        None => println!("missing packet"),
    }
}

#[derive(Debug)]
pub struct Status {
    pub version: StatusVersion,
    pub players: StatusPlayers,
    pub description: format::Component,
    pub favicon: Option<String>,
    pub forge_mods: Vec<crate::protocol::forge::ForgeMod>,
    pub fml_network_version: Option<i64>,
}

#[derive(Debug)]
pub struct StatusVersion {
    pub name: String,
    pub protocol: i32,
}

#[derive(Debug)]
pub struct StatusPlayers {
    pub max: i32,
    pub online: i32,
    pub sample: Vec<StatusPlayer>,
}

#[derive(Debug)]
pub struct StatusPlayer {
    name: String,
    id: String,
}

impl Read for Conn {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self.read_cipher.as_ref() {
            Option::None => self.stream.read(buf),
            Option::Some(cipher) => {
                let ret = self.stream.read(buf)?;
                cipher.clone().write().unwrap().decrypt(&mut buf[..ret]);

                Ok(ret)
            }
        }
    }
}

impl Write for Conn {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self.write_cipher.as_ref() {
            Option::None => self.stream.write(buf),
            Option::Some(cipher) => {
                let mut data = vec![0; buf.len()];
                data[..buf.len()].clone_from_slice(&buf[..]);

                cipher.clone().write().unwrap().encrypt(&mut data);

                self.stream.write_all(&data)?;
                Ok(buf.len())
            }
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        self.stream.flush()
    }
}

impl Clone for Conn {
    fn clone(&self) -> Self {
        let send;
        if self.send.is_none() {
            send = None;
        }else {
            send = Some(self.send.as_ref().unwrap().clone());
        }
        let read_cipher;
        if self.read_cipher.is_none() {
            read_cipher = None;
        }else {
            read_cipher = Some(self.read_cipher.as_ref().unwrap().clone());
        }
        let write_cipher;
        if self.write_cipher.is_none() {
            write_cipher = None;
        }else {
            write_cipher = Some(self.write_cipher.as_ref().unwrap().clone());
        }
        Conn {
            stream: self.stream.try_clone().unwrap(),
            host: self.host.clone(),
            port: self.port,
            direction: self.direction,
            state: self.state,
            protocol_version: self.protocol_version,
            read_cipher,
            write_cipher,
            compression_threshold: self.compression_threshold,
            send,
        }
    }
}

pub trait PacketType {
    fn packet_id(&self, protocol_version: i32) -> i32;

    fn write<W: io::Write>(&self, buf: &mut W) -> Result<(), Error>;
}
