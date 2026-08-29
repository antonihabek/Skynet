//! Fixed-capacity containers.
//!
//! The kernel is `#![no_std]` and does not link `alloc`, so "this never allocates" is a property
//! the compiler enforces rather than a promise a reviewer makes (§23.1). That leaves it needing
//! somewhere to put a list whose length is not known until the cycle runs, and this is that place:
//! an array with a length, a bound checked at push time, and an overflow flag instead of a panic.
//!
//! **Overflow does not panic, and it is not silently ignored either.** A kernel that panicked
//! because a body had one more joint than the build assumed would be a kernel that stops a robot
//! over a build-time constant; a kernel that dropped the extra channel would be one that forwards
//! an unchecked command. So the flag is set, the caller sees it, and [`crate::evaluate_cycle`]
//! turns it into a `stop_cat2` — the same response a stalled producer gets, because "there is a
//! command I could not evaluate" and "there is a command I did not receive" are the same fact.

/// A vector with a compile-time capacity and no heap behind it.
#[derive(Debug, Clone)]
pub struct FixedVec<T, const N: usize> {
    items: [Option<T>; N],
    len: usize,
    overflowed: bool,
}

impl<T, const N: usize> Default for FixedVec<T, N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T, const N: usize> FixedVec<T, N> {
    /// An empty vector.
    pub fn new() -> Self {
        FixedVec {
            items: [const { None }; N],
            len: 0,
            overflowed: false,
        }
    }

    /// Append, or record an overflow. Returns whether the value was stored.
    pub fn push(&mut self, value: T) -> bool {
        if self.len >= N {
            self.overflowed = true;
            return false;
        }
        self.items[self.len] = Some(value);
        self.len += 1;
        true
    }

    /// Number of stored items.
    pub fn len(&self) -> usize {
        self.len
    }

    /// Whether nothing has been stored.
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Whether a push was refused for want of capacity.
    ///
    /// The caller is expected to treat this as a fault rather than as a detail: see the module
    /// note above.
    pub fn overflowed(&self) -> bool {
        self.overflowed
    }

    /// Capacity, as a constant.
    pub const fn capacity() -> usize {
        N
    }

    /// Element access.
    pub fn get(&self, index: usize) -> Option<&T> {
        if index >= self.len {
            return None;
        }
        self.items[index].as_ref()
    }

    /// Mutable element access.
    pub fn get_mut(&mut self, index: usize) -> Option<&mut T> {
        if index >= self.len {
            return None;
        }
        self.items[index].as_mut()
    }

    /// Iterate the stored items in insertion order.
    pub fn iter(&self) -> impl Iterator<Item = &T> {
        self.items[..self.len].iter().filter_map(Option::as_ref)
    }
}

impl<T: PartialEq, const N: usize> FixedVec<T, N> {
    /// Whether the value is already stored.
    pub fn contains(&self, value: &T) -> bool {
        self.iter().any(|item| item == value)
    }

    /// Append unless already present. Returns whether anything changed.
    ///
    /// The guard list uses this: a guard trips at most once per cycle however many channels
    /// provoked it, which is what makes `tripped` a set of *guards* rather than a tally of
    /// violations.
    pub fn push_unique(&mut self, value: T) -> bool {
        if self.contains(&value) {
            return false;
        }
        self.push(value)
    }
}

impl<'a, T, const N: usize> IntoIterator for &'a FixedVec<T, N> {
    type Item = &'a T;
    type IntoIter =
        core::iter::FilterMap<core::slice::Iter<'a, Option<T>>, fn(&'a Option<T>) -> Option<&'a T>>;

    fn into_iter(self) -> Self::IntoIter {
        self.items[..self.len].iter().filter_map(Option::as_ref)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stores_up_to_capacity() {
        let mut v: FixedVec<u8, 3> = FixedVec::new();
        assert!(v.push(1));
        assert!(v.push(2));
        assert!(v.push(3));
        assert_eq!(v.len(), 3);
        assert!(!v.overflowed());
    }

    #[test]
    fn overflow_is_recorded_not_panicked_and_not_ignored() {
        let mut v: FixedVec<u8, 2> = FixedVec::new();
        v.push(1);
        v.push(2);
        assert!(!v.push(3), "the push is refused");
        assert!(v.overflowed(), "and the refusal is visible");
        assert_eq!(v.len(), 2, "nothing already stored was disturbed");
    }

    #[test]
    fn push_unique_makes_a_set() {
        let mut v: FixedVec<u8, 4> = FixedVec::new();
        assert!(v.push_unique(7));
        assert!(!v.push_unique(7));
        assert_eq!(v.len(), 1);
    }

    #[test]
    fn iterates_in_insertion_order() {
        let mut v: FixedVec<u8, 4> = FixedVec::new();
        v.push(3);
        v.push(1);
        v.push(2);
        let seen: [u8; 3] = [*v.get(0).unwrap(), *v.get(1).unwrap(), *v.get(2).unwrap()];
        assert_eq!(seen, [3, 1, 2]);
        assert_eq!(v.iter().count(), 3);
    }
}
