//
// vector.rs
//
// Copyright (C) 2022 Posit Software, PBC. All rights reserved.
//
//

use std::ffi::CStr;
use std::fmt::Display;
use std::marker::PhantomData;
use std::ops::Deref;
use std::ops::DerefMut;
use std::slice::Iter;

use libR_sys::*;

use crate::error::Result;
use crate::object::RObject;
use crate::traits::AsSlice;
use crate::traits::Number;
use crate::utils::r_assert_capacity;
use crate::utils::r_assert_type;

#[derive(Debug)]
pub struct Vector<const SEXPTYPE: u32, NativeType> {
    object: RObject,
    phantom: PhantomData<NativeType>,
}

// Useful type aliases for clients.
pub type RawVector = Vector<RAWSXP, u8>;
pub type LogicalVector = Vector<LGLSXP, i32>;
pub type IntegerVector = Vector<INTSXP, i32>;
pub type NumericVector = Vector<REALSXP, f64>;
pub type CharacterVector = Vector<STRSXP, &'static str>;

// Methods common to all R vectors.
impl<const SEXPTYPE: u32, NativeType> Vector<{ SEXPTYPE }, NativeType> {
    pub unsafe fn new(object: impl Into<SEXP>) -> Result<Self> {
        let object = object.into();
        r_assert_type(object, &[SEXPTYPE])?;
        Ok(Self::new_unchecked(object))
    }

    unsafe fn new_unchecked(object: impl Into<SEXP>) -> Self {
        let object = RObject::new(object.into());
        Self { object, phantom: PhantomData }
    }

    pub unsafe fn with_length(size: usize) -> Self {
        let data = Rf_allocVector(SEXPTYPE, size as isize);
        Self::new_unchecked(data)
    }

    // SAFETY: Rf_length() might allocate for ALTREP objects,
    // so users should be holding the R runtime lock.
    pub unsafe fn len(&self) -> usize {
        Rf_length(*self.object) as usize
    }

    pub fn cast(self) -> RObject {
        self.object
    }

    pub fn data(&self) -> SEXP {
        self.object.sexp
    }
}

pub trait IsNa {
    fn is_na(self) -> bool;
}

impl IsNa for u8 {
    fn is_na(self) -> bool {
        false
    }
}

impl IsNa for i32 {
    fn is_na(self) -> bool {
        self == unsafe { R_NaInt }
    }
}

impl IsNa for f64 {
    fn is_na(self) -> bool {
        unsafe { R_IsNA(self) == 1 }
    }
}

impl IsNa for SEXP {
    fn is_na(self) -> bool {
        self == unsafe { R_NaString }
    }
}

pub struct VectorIterator<'a, const SEXPTYPE: u32, NativeType> {
    iter: Iter<'a, NativeType>
}

impl<'a, const SEXPTYPE: u32, NativeType> VectorIterator<'a, {SEXPTYPE}, NativeType> {
    pub fn new(data: &'a Vector<{SEXPTYPE}, NativeType>) -> Self {
        unsafe {
            let len = data.len();
            let data = DATAPTR(*data.object) as *mut NativeType;
            let slice = std::slice::from_raw_parts(data, len);
            let iter = slice.iter();

            Self {
                iter
            }
        }
    }
}

impl<'a, const SEXPTYPE: u32, NativeType> Iterator for VectorIterator<'a, {SEXPTYPE}, NativeType>
where
    NativeType: Number + Copy + IsNa
{
    type Item = Option<NativeType>;

    fn next(&mut self) -> Option<Self::Item> {
        match self.iter.next() {
            None => None,
            Some(value) => {
                if value.is_na() {
                    Some(None)
                } else {
                    Some(Some(*value))
                }
            }
        }
    }
}

// Methods for vectors with primitive native types.
impl<const SEXPTYPE: u32, NativeType> Vector<{ SEXPTYPE }, NativeType>
where
    NativeType: Number + Copy + IsNa
{
    pub unsafe fn create<T: AsSlice<NativeType>>(data: T) -> Self {
        let data = data.as_slice();
        let vector = Vector::with_length(data.len());
        let pointer = DATAPTR(*vector) as *mut NativeType;
        pointer.copy_from(data.as_ptr(), data.len());
        vector
    }

    pub fn get(&self, index: isize) -> Result<Option<NativeType>> {
        unsafe {
            r_assert_capacity(self.data(), index as u32)?;
            Ok(self.get_unchecked(index))
        }
    }

    pub fn get_unchecked(&self, index: isize) -> Option<NativeType> {
        unsafe {
            let dataptr = DATAPTR(*self.object);
            let pointer = dataptr as *mut NativeType;
            let offset = pointer.offset(index);
            if (*offset).is_na() {
                None
            } else {
                Some(*offset)
            }
        }
    }

    pub fn iter<'a>(&'a self) -> VectorIterator<'a, {SEXPTYPE}, NativeType>{
        VectorIterator::<'a, {SEXPTYPE}, NativeType>::new(self)
    }

    pub fn into_vec(self) -> Vec<NativeType> {
        self.into_iter().map(|value| *value).collect::<Vec<_>>()
    }

}

// Character vectors.
pub struct CharacterVectorIterator<'a> {
    data: &'a CharacterVector,
    index: usize,
    size: usize,
}

impl<'a> CharacterVectorIterator<'a> {

    pub fn new(data: &'a CharacterVector) -> Self {
        unsafe {
            Self { data, index: 0, size: data.len() }
        }
    }
}

impl<'a> Iterator for CharacterVectorIterator<'a> {
    type Item = Option<&'static str>;

    fn next(&mut self) -> Option<Self::Item> {
        unsafe {
            if self.index == self.size {
                None
            } else {
                let value = self.data.get_unchecked(self.index);
                self.index = self.index + 1;
                Some(value)
            }
        }
    }
}

impl CharacterVector {

    pub unsafe fn create<T>(data: T) -> Self
    where
        T: IntoIterator,
        <T as IntoIterator>::IntoIter: ExactSizeIterator,
        <T as IntoIterator>::Item: AsRef<str>,
    {
        // convert into iterator
        let mut data = data.into_iter();

        // build our character vector
        let n = data.len();
        let vector = CharacterVector::with_length(n);
        for i in 0..data.len() {
            let value = data.next().unwrap_unchecked();
            let value = value.as_ref();
            let charsexp = Rf_mkCharLenCE(
                value.as_ptr() as *const i8,
                value.len() as i32,
                cetype_t_CE_UTF8,
            );
            SET_STRING_ELT(*vector, i as R_xlen_t, charsexp);
        }
        vector
    }

    pub unsafe fn get(&self, index: usize) -> Result<Option<&'static str>> {
        r_assert_capacity(self.data(), index as u32)?;
        Ok(self.get_unchecked(index))
    }

    pub unsafe fn get_unchecked(&self, index: usize) -> Option<&'static str> {
        let data = *self.object;
        let charsxp = STRING_ELT(data, index as R_xlen_t);
        if charsxp == R_NaString {
            None
        } else {
            let cstr = Rf_translateCharUTF8(charsxp);
            let bytes = CStr::from_ptr(cstr).to_bytes();
            Some(std::str::from_utf8_unchecked(bytes))
        }
    }

    pub fn iter(&self) -> CharacterVectorIterator {
        CharacterVectorIterator::new(self)
    }

}

// Traits.
impl<const SEXPTYPE: u32, NativeType> Deref
    for Vector<{ SEXPTYPE }, NativeType>
{
    type Target = SEXP;

    fn deref(&self) -> &Self::Target {
        &*self.object
    }
}

impl<const SEXPTYPE: u32, NativeType> DerefMut
    for Vector<{ SEXPTYPE }, NativeType>
{
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut *self.object
    }
}

impl<'a, T, const SEXPTYPE: u32, NativeType> PartialEq<T>
    for Vector<{ SEXPTYPE }, NativeType>
    where
        T: AsSlice<NativeType>,
        NativeType: Number + PartialEq,
{
    fn eq(&self, other: &T) -> bool {
        unsafe {
            let other = other.as_slice();
            if self.len() != other.len() {
                return false;
            }
            let pointer = DATAPTR(self.data()) as *mut NativeType;
            for i in 0..self.len() {
                let value = pointer.offset(i as isize);
                if (*value) != (*other.get_unchecked(i)) {
                    return false;
                }
            }
            true
        }
    }
}

impl<'a, T> PartialEq<T> for CharacterVector
    where T: AsSlice<&'a str>
{
    fn eq(&self, other: &T) -> bool {
        unsafe {
            let other = other.as_slice();
            if self.len() != other.len() {
                return false;
            }

            for i in 0..self.len() {
                let value = self.get_unchecked(i);

                // TODO: refine re NA aka Some(None)
                if value != Some(other[i]) {
                    return false;
                }
            }

            true

        }
    }
}

impl<'a, const SEXPTYPE: u32, NativeType> IntoIterator
    for &'a Vector<{ SEXPTYPE }, NativeType>
    where NativeType: Number
{
    type Item = &'a NativeType;
    type IntoIter = std::slice::Iter<'a, NativeType>;

    fn into_iter(self) -> Self::IntoIter {
        unsafe {
            let data = DATAPTR(self.data()) as *mut NativeType;
            let slice = std::slice::from_raw_parts(data, self.len());
            slice.iter()
        }
    }
}

impl<const SEXPTYPE: u32, NativeType> Vector<{ SEXPTYPE }, NativeType>
where
    NativeType: Number + Copy + IsNa + Display
{
    pub fn glimpse(&self, limit: usize) -> (bool, String) {
        let mut iter = self.iter();

        let mut out = String::new();
        let mut truncated = false;
        loop {
            match iter.next() {
                None => break,
                Some(None) => {
                    out.push_str(" _");
                },

                Some(Some(x)) => {
                    if out.len() > limit {
                        truncated = true;
                        break;
                    }
                    out.push_str(" ");
                    out.push_str(x.to_string().as_str());
                }
            }

        }

        (truncated, out)

    }
}

// TODO: this is mostly the same as the other glimpse
impl CharacterVector
{
    pub fn glimpse(&self, limit: usize) -> (bool, String) {
        let mut iter = self.iter();

        let mut out = String::new();
        let mut truncated = false;
        loop {
            match iter.next() {
                None => break,
                Some(None) => {
                    out.push_str(" _");
                },

                Some(Some(x)) => {
                    if out.len() > limit {
                        truncated = true;
                        break;
                    }
                    out.push_str(" ");
                    out.push_str(x);
                }
            }

        }

        (truncated, out)

    }
}


// NOTE (Kevin): I previously tried providing 'From' implementations here,
// but had too much trouble bumping into the From and TryFrom blanket
// implementations.
//
// https://github.com/rust-lang/rust/issues/50133
//
// For that reason, I avoid using 'from()' and instead have methods like 'create()'.
// Leaving this code around for now, in case we decide to re-visit.
//
// impl<const SEXPTYPE: u32, NativeType, T> From<T>
//     for Vector<{ SEXPTYPE }, NativeType>
//     where
//         T: AsSlice<NativeType> + Copy,
//         NativeType: Number,
// {
//     fn from(array: T) -> Self {
//         unsafe {
//
//             let array = array.as_slice();
//             let object = Rf_allocVector(SEXPTYPE, array.len() as isize);
//             let pointer = DATAPTR(object) as *mut NativeType;
//             pointer.copy_from(array.as_ptr(), array.len());
//
//             let object = RObject::new(object);
//             Vector::new_unchecked(object)
//         }
//     }
// }
//
// impl<const SEXPTYPE: u32, NativeType> TryFrom<RObject>
//     for Vector<{ SEXPTYPE }, NativeType>
// {
//     type Error = crate::error::Error;
//
//     fn try_from(value: RObject) -> std::result::Result<Self, Self::Error> {
//         Vector::new(value)
//     }
//
// }
//
// impl<const SEXPTYPE: u32, NativeType> Into<RObject>
//     for Vector<{ SEXPTYPE }, NativeType>
// {
//     fn into(self) -> RObject {
//         self.object
//     }
// }
//
// impl<'a, T: AsSlice<&'a str>> From<T> for CharacterVector {
//     fn from(value: T) -> Self {
//         unsafe {
//             CharacterVector::create(value)
//         }
//     }
// }

#[cfg(test)]
mod tests {
    use crate::r_test;
    use crate::vector::CharacterVector;
    use crate::vector::IntegerVector;
    use crate::vector::NumericVector;

    #[test]
    fn test_numeric_vector() {
        r_test! {

            let vector = NumericVector::create([1.0, 2.0, 3.0]);
            assert!(vector.len() == 3);
            assert!(vector.get_unchecked(0) == Some(1.0));
            assert!(vector.get_unchecked(1) == Some(2.0));
            assert!(vector.get_unchecked(2) == Some(3.0));

            let data = [1.0, 2.0, 3.0];
            assert!(vector == data);

            let data = &[1.0, 2.0, 3.0];
            assert!(vector == data);

            let slice = &data[..];
            assert!(vector == slice);

            let mut it = vector.iter();
            let value = it.next();
            assert!(value.is_some());
            assert!(value.unwrap() == Some(1.0));

            let value = it.next();
            assert!(value.is_some());
            assert!(value.unwrap() == Some(2.0));

            let value = it.next();
            assert!(value.is_some());
            assert!(value.unwrap() == Some(3.0));

            let value = it.next();
            assert!(value.is_none());

        }
    }

    #[test]
    fn test_character_vector() {
        r_test! {

            let vector = CharacterVector::create(&["hello", "world"]);
            assert!(vector == ["hello", "world"]);
            assert!(vector == &["hello", "world"]);

            let mut it = vector.iter();

            let value = it.next();
            assert!(value.is_some());
            assert!(value.unwrap() == Some("hello"));

            let value = it.next();
            assert!(value.is_some());
            assert!(value.unwrap() == Some("world"));

            let value = it.next();
            assert!(value.is_none());

            let vector = CharacterVector::create([
                "hello".to_string(),
                "world".to_string()
            ]);

            assert!(vector.get_unchecked(0) == Some("hello"));
            assert!(vector.get_unchecked(1) == Some("world"));

        }
    }

    #[test]
    fn test_integer_vector() {
        r_test! {
            let vector = IntegerVector::create(42);
            assert!(vector.len() == 1);
            assert!(vector.get_unchecked(0) == Some(42));
        }
    }
}
