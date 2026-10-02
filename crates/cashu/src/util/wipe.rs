//! Overwriting secret bytes when they go out of scope.

use std::fmt;
use std::hint::black_box;
use std::ops::Deref;
use std::sync::atomic::{compiler_fence, Ordering};

/// Bytes that can be overwritten in place.
pub trait Wipe {
    /// Overwrite the bytes with zeroes.
    fn wipe(&mut self);
}

impl Wipe for [u8] {
    /// The fence and the `black_box` stop the compiler from dropping the stores
    /// as dead. Without `unsafe` there are no volatile writes here, so treat the
    /// wipe as best effort rather than a guarantee.
    fn wipe(&mut self) {
        self.fill(0);
        compiler_fence(Ordering::SeqCst);
        black_box(self);
    }
}

impl<const N: usize> Wipe for [u8; N] {
    fn wipe(&mut self) {
        self.as_mut_slice().wipe();
    }
}

impl Wipe for Vec<u8> {
    /// Leaves the vector empty. Safe code cannot reach past the length, so a
    /// buffer that grew by reallocating may have left copies behind.
    fn wipe(&mut self) {
        self.as_mut_slice().wipe();
        self.clear();
    }
}

/// Owns a secret and overwrites it when it goes out of scope.
///
/// Read-only by design: handing out `&mut T` would let a caller replace or move
/// the secret out, leaving the original copy for the allocator.
pub struct ZeroOnDrop<T: Wipe>(T);

impl<T: Wipe> ZeroOnDrop<T> {
    /// Take ownership of a secret.
    pub fn new(value: T) -> Self {
        Self(value)
    }
}

impl<T: Wipe> From<T> for ZeroOnDrop<T> {
    fn from(value: T) -> Self {
        Self(value)
    }
}

impl<T: Wipe> Deref for ZeroOnDrop<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<T: Wipe> Drop for ZeroOnDrop<T> {
    fn drop(&mut self) {
        self.0.wipe();
    }
}

/// Redacts the secret so it cannot reach a log through a derived formatter.
impl<T: Wipe> fmt::Debug for ZeroOnDrop<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ZeroOnDrop(<redacted>)")
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::rc::Rc;

    use super::*;

    struct Tracked(Rc<Cell<bool>>);

    impl Wipe for Tracked {
        fn wipe(&mut self) {
            self.0.set(true);
        }
    }

    #[test]
    fn wipes_a_slice() {
        let mut bytes = [7u8; 8];
        bytes[..4].wipe();
        assert_eq!(bytes, [0, 0, 0, 0, 7, 7, 7, 7]);
    }

    #[test]
    fn wipes_an_array() {
        let mut seed = [9u8; 64];
        seed.wipe();
        assert_eq!(seed, [0u8; 64]);
    }

    #[test]
    fn wipes_and_empties_a_vec() {
        let mut seed = vec![9u8; 64];
        seed.wipe();
        assert!(seed.is_empty());
    }

    #[test]
    fn drop_wipes_the_value() {
        let wiped = Rc::new(Cell::new(false));
        drop(ZeroOnDrop::new(Tracked(Rc::clone(&wiped))));
        assert!(wiped.get());
    }

    #[test]
    fn derefs_to_the_wrapped_value() {
        let seed = ZeroOnDrop::new([3u8; 64]);
        assert_eq!(seed[0], 3);
        assert_eq!(seed.len(), 64);
    }
}
