use std::fmt;

/// A set drawn from the six symbolic object versions x, y, z, p, q, and r.
#[derive(Clone, Copy, Default, Eq, Hash, PartialEq)]
pub struct Objects(u8);

const OBJECT_NAMES: [&str; 6] = ["x", "y", "z", "p", "q", "r"];

impl Objects {
    pub const EMPTY: Self = Self(0);
    pub const X: Self = Self(1);
    pub const Y: Self = Self(2);
    pub const Z: Self = Self(4);
    pub const P: Self = Self(8);
    pub const Q: Self = Self(16);
    pub const R: Self = Self(32);
    pub const INITIAL: Self = Self(7);

    pub fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    pub fn intersection(self, other: Self) -> Self {
        Self(self.0 & other.0)
    }

    pub fn without(self, other: Self) -> Self {
        Self(self.0 & !other.0)
    }

    pub fn contains(self, other: Self) -> bool {
        self.intersection(other) == other
    }

    pub fn overlaps(self, other: Self) -> bool {
        self.intersection(other).0 != 0
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

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct Effects {
    pub created: Objects,
    pub nullifiers: Objects,
}

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum Coverage {
    /// No expected effects are present; this alone does not establish applicability.
    #[default]
    None,
    /// Some expected effects already exist, blocking this attempt on the observed branch.
    /// This is overlap with other transactions, not partial execution of this one.
    Partial,
    /// All expected effects exist, possibly from other transactions. This says
    /// nothing about finality or whether the created objects are still unspent.
    Full,
}

impl Effects {
    pub fn coverage(self, observed: Self) -> Coverage {
        if observed.created.contains(self.created) && observed.nullifiers.contains(self.nullifiers)
        {
            Coverage::Full
        } else if self.created.overlaps(observed.created)
            || self.nullifiers.overlaps(observed.nullifiers)
        {
            Coverage::Partial
        } else {
            Coverage::None
        }
    }

    pub fn collides(self, observed: Self) -> bool {
        self.created.overlaps(observed.created) || self.nullifiers.overlaps(observed.nullifiers)
    }

    pub fn apply(self, observed: Self) -> Option<Self> {
        (!self.collides(observed)).then_some(Self {
            created: observed.created.union(self.created),
            nullifiers: observed.nullifiers.union(self.nullifiers),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An attempt intends three effects: consuming x, consuming y, and creating p.
    /// The observed view can contain any subset of those effects, and each subset
    /// has a different implication for reporting the result:
    ///
    /// - No overlap leaves the intent unsatisfied and introduces no effect collision.
    /// - A proper subset leaves it unsatisfied but prevents this exact payload applying.
    /// - All three effects satisfy it and also prevent the payload applying again.
    ///
    /// Additional effects do not undo satisfaction. The final case includes another
    /// creation and consumes p itself: the intent remains fully covered even though
    /// its expected output is no longer unspent. This comparison concerns effects;
    /// it does not establish grounding validity or the availability of local data.
    #[test]
    fn any_proper_subset_conflicts_and_supersets_satisfy() {
        let expected = Effects {
            created: Objects::P,
            nullifiers: Objects::X.union(Objects::Y),
        };
        let present = |included: bool, objects: Objects| {
            if included { objects } else { Objects::EMPTY }
        };
        for subset in 0u8..8 {
            let observed = Effects {
                created: present(subset & 1 != 0, Objects::P),
                nullifiers: present(subset & 2 != 0, Objects::X)
                    .union(present(subset & 4 != 0, Objects::Y)),
            };
            let coverage = match subset {
                0 => Coverage::None,
                7 => Coverage::Full,
                _ => Coverage::Partial,
            };
            assert_eq!(expected.coverage(observed), coverage);
            assert_eq!(expected.apply(observed).is_some(), subset == 0);
        }
        assert_eq!(
            expected.coverage(Effects {
                created: Objects::P.union(Objects::Q),
                nullifiers: Objects::INITIAL.union(Objects::P),
            }),
            Coverage::Full,
        );
    }

    /// The same symbolic object p identifies both a creation commitment and an
    /// associated nullifier. They belong to different sets and cannot substitute
    /// for each other when comparing an intent with observed effects.
    ///
    /// Here the intent contains only p's creation, while the comparison set contains
    /// only its nullifier. That nullifier neither satisfies the intended creation
    /// nor causes a creation collision. The test isolates this distinction; it does
    /// not claim that the comparison set is a reachable complete chain state.
    #[test]
    fn creation_and_nullification_are_different_effects() {
        let creation = Effects {
            created: Objects::P,
            ..Effects::default()
        };
        let consumption = Effects {
            nullifiers: Objects::P,
            ..Effects::default()
        };
        assert_eq!(creation.coverage(consumption), Coverage::None);
        assert!(creation.apply(consumption).is_some());
    }

    /// A transaction can create an object without consuming an external input,
    /// or consume an object without creating an output. Neither case should be
    /// mistaken for an already satisfied intent merely because one effect set is empty.
    ///
    /// For each case, an empty observation gives no coverage. Once its one expected
    /// effect is present, coverage is full and replay would collide with that effect.
    /// This checks that both sides of the effect comparison work independently.
    #[test]
    fn no_input_and_no_output_transactions_have_effects() {
        for expected in [
            Effects {
                created: Objects::P,
                ..Effects::default()
            },
            Effects {
                nullifiers: Objects::X,
                ..Effects::default()
            },
        ] {
            assert_eq!(expected.coverage(Effects::default()), Coverage::None);
            assert_eq!(expected.coverage(expected), Coverage::Full);
            assert!(expected.apply(expected).is_none());
        }
    }
}
