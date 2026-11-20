//! Statement parameters.

use std::fmt;

use crate::error::Result;
use crate::statement::Statement;
use crate::types::ToSql;

mod sealed {
    pub trait Sealed {}
}

/// Values that can be bound to a statement's parameters.
///
/// Implemented for `()`, arrays and slices of [`ToSql`] values, tuples, named
/// pairs (see [`named_params!`](crate::named_params)) and
/// [`params_from_iter`]. Use `[]` for a statement without parameters.
pub trait Params: sealed::Sealed {
    #[doc(hidden)]
    fn bind_in(self, stmt: &mut Statement<'_>) -> Result<()>;
}

impl sealed::Sealed for () {}
impl Params for () {
    fn bind_in(self, stmt: &mut Statement<'_>) -> Result<()> {
        stmt.ensure_parameter_count(0)
    }
}

// Lets `[]` infer an element type. Arrays of length zero are otherwise left
// out of the generic impls below.
impl sealed::Sealed for [&(dyn ToSql + Send + Sync); 0] {}
impl Params for [&(dyn ToSql + Send + Sync); 0] {
    fn bind_in(self, stmt: &mut Statement<'_>) -> Result<()> {
        stmt.ensure_parameter_count(0)
    }
}

impl sealed::Sealed for &[&dyn ToSql] {}
impl Params for &[&dyn ToSql] {
    fn bind_in(self, stmt: &mut Statement<'_>) -> Result<()> {
        stmt.bind_positional(self)
    }
}

impl sealed::Sealed for &[(&str, &dyn ToSql)] {}
impl Params for &[(&str, &dyn ToSql)] {
    fn bind_in(self, stmt: &mut Statement<'_>) -> Result<()> {
        stmt.bind_named(self)
    }
}

macro_rules! array_params {
    ($($n:literal)+) => {$(
        impl<T: ToSql> sealed::Sealed for [T; $n] {}
        impl<T: ToSql> Params for [T; $n] {
            fn bind_in(self, stmt: &mut Statement<'_>) -> Result<()> {
                stmt.bind_positional(self)
            }
        }
        impl<T: ToSql + ?Sized> sealed::Sealed for &[&T; $n] {}
        impl<T: ToSql + ?Sized> Params for &[&T; $n] {
            fn bind_in(self, stmt: &mut Statement<'_>) -> Result<()> {
                stmt.bind_positional(self)
            }
        }
        impl<T: ToSql + ?Sized> sealed::Sealed for &[(&str, &T); $n] {}
        impl<T: ToSql + ?Sized> Params for &[(&str, &T); $n] {
            fn bind_in(self, stmt: &mut Statement<'_>) -> Result<()> {
                stmt.bind_named(self)
            }
        }
    )+};
}

array_params!(1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16);

macro_rules! tuple_params {
    ($count:literal: $(($field:tt $t:ident)),+) => {
        impl<$($t: ToSql,)+> sealed::Sealed for ($($t,)+) {}
        impl<$($t: ToSql,)+> Params for ($($t,)+) {
            fn bind_in(self, stmt: &mut Statement<'_>) -> Result<()> {
                stmt.ensure_parameter_count($count)?;
                $(stmt.raw_bind_parameter($field + 1, &self.$field)?;)+
                Ok(())
            }
        }
    };
}

tuple_params!(1: (0 A));
tuple_params!(2: (0 A), (1 B));
tuple_params!(3: (0 A), (1 B), (2 C));
tuple_params!(4: (0 A), (1 B), (2 C), (3 D));
tuple_params!(5: (0 A), (1 B), (2 C), (3 D), (4 E));
tuple_params!(6: (0 A), (1 B), (2 C), (3 D), (4 E), (5 F));
tuple_params!(7: (0 A), (1 B), (2 C), (3 D), (4 E), (5 F), (6 G));
tuple_params!(8: (0 A), (1 B), (2 C), (3 D), (4 E), (5 F), (6 G), (7 H));

/// Positional parameters taken from an iterator.
pub struct ParamsFromIter<I>(I);

impl<I> fmt::Debug for ParamsFromIter<I> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ParamsFromIter").finish_non_exhaustive()
    }
}

/// Binds every item of `iter` as a positional parameter.
pub fn params_from_iter<I>(iter: I) -> ParamsFromIter<I>
where
    I: IntoIterator,
    I::Item: ToSql,
{
    ParamsFromIter(iter)
}

impl<I> sealed::Sealed for ParamsFromIter<I>
where
    I: IntoIterator,
    I::Item: ToSql,
{
}

impl<I> Params for ParamsFromIter<I>
where
    I: IntoIterator,
    I::Item: ToSql,
{
    fn bind_in(self, stmt: &mut Statement<'_>) -> Result<()> {
        stmt.bind_positional(self.0)
    }
}
