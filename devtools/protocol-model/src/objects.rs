use std::fmt;

/// A set drawn from the symbolic object versions x, y, z, p, and q.
#[derive(Clone, Copy, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Objects(u8);

const OBJECT_NAMES: [&str; 5] = ["x", "y", "z", "p", "q"];

impl Objects {
    pub const X: Self = Self(1);
    pub const Y: Self = Self(2);
    pub const Z: Self = Self(4);
    pub const P: Self = Self(8);
    pub const Q: Self = Self(16);

    pub fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    pub fn intersection(self, other: Self) -> Self {
        Self(self.0 & other.0)
    }

    pub fn overlaps(self, other: Self) -> bool {
        self.0 & other.0 != 0
    }

    pub fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// Every subset, the empty set and this set included.
    pub fn subsets(self) -> impl Iterator<Item = Self> {
        (0..=self.0)
            .filter(move |bits| bits & !self.0 == 0)
            .map(Self)
    }
}

impl fmt::Debug for Objects {
    fn fmt(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        let mut set = formatter.debug_set();
        for (bit, name) in OBJECT_NAMES.iter().enumerate() {
            if self.0 & (1 << bit) != 0 {
                set.entry(&format_args!("{name}"));
            }
        }
        set.finish()
    }
}
