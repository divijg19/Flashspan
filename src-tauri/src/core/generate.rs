use rand::{Rng, RngExt};

/// Inclusive magnitude domain for a digit width: the smallest and largest
/// value with exactly `digits` digits and no leading zero. A single digit
/// allows `1..=9` (never `0`, which would be a leading zero).
///
/// # Panics
///
/// Panics if `digits > 19`, where `10^digits` leaves `u64`. Normalized
/// sessions cap digits at 15 (see `crate::core::validate`).
pub(crate) fn magnitude_bounds(digits: u32) -> (u64, u64) {
    if digits <= 1 {
        return (1, 9);
    }

    let max_exclusive = 10u64
        .checked_pow(digits)
        .expect("digits <= 19 keeps 10^digits inside u64");
    let min = 10u64
        .checked_pow(digits - 1)
        .expect("digits <= 19 keeps 10^(digits-1) inside u64");
    (min, max_exclusive - 1)
}

/// One magnitude inside the digit width's own domain.
pub(crate) fn random_magnitude(rng: &mut impl Rng, digits: u32) -> u64 {
    let (min, max) = magnitude_bounds(digits);
    rng.random_range(min..=max)
}

/// One magnitude inside the domain, additionally capped by `max_inclusive`.
/// `None` when the cap admits no value of this width (the caller's fallback
/// is to draw from the uncapped domain).
pub(crate) fn random_magnitude_capped(
    rng: &mut impl Rng,
    digits: u32,
    max_inclusive: u64,
) -> Option<u64> {
    let (min, max) = magnitude_bounds(digits);
    if max_inclusive < min {
        return None;
    }

    Some(rng.random_range(min..=max_inclusive.min(max)))
}

/// Generate one constrained flash number.
///
/// # Panics
///
/// Panics if `digits > 19`, where the width's magnitude domain leaves `u64`
/// (see [`magnitude_bounds`]). Normalized sessions cap digits at 15.
///
/// Returns the signed value only: the magnitude is recoverable with
/// `unsigned_abs`, and every consumer works numerically, so nothing is ever
/// formatted into a string and parsed back.
///
/// The result always lies inside the digit width's magnitude domain, and the
/// running sum that consumes it can never go negative.
#[must_use]
pub fn random_number_with_constraints(
    rng: &mut impl Rng,
    digits: u32,
    allow_negative_numbers: bool,
    index: u32,
    running_sum: i128,
) -> i128 {
    // Requirement: first number is never negative.
    let allow_negative_here = allow_negative_numbers && index > 0;

    // Cap for negative magnitudes: cannot exceed the current running sum, and
    // cannot exceed the maximum representable magnitude for the digit width.
    let (_, max_for_digits) = magnitude_bounds(digits);
    let sum_cap = if running_sum <= 0 {
        0
    } else {
        // Non-negative by the branch guard and capped at max_for_digits,
        // so the conversion is exact.
        u64::try_from(running_sum.min(i128::from(max_for_digits)))
            .expect("capped non-negative running sum fits u64")
    };

    if allow_negative_here
        && sum_cap > 0
        && rng.random_bool(0.5)
        && let Some(magnitude) = random_magnitude_capped(rng, digits, sum_cap)
    {
        // The draw is capped at the running sum, so applying it keeps the sum
        // non-negative by construction: asserted rather than re-tested at
        // runtime, and covered by the plan-level property tests.
        debug_assert!(running_sum >= i128::from(magnitude));
        return -i128::from(magnitude);
    }

    i128::from(random_magnitude(rng, digits))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::rng;

    /// The domain must be exactly "numbers with `digits` digits, no leading
    /// zero": every generated value is checked against it.
    fn assert_in_domain(value: i128, digits: u32) {
        let (min, max) = magnitude_bounds(digits);
        let magnitude = value.unsigned_abs();
        assert!(
            magnitude >= u128::from(min) && magnitude <= u128::from(max),
            "{value} outside the {digits}-digit domain [{min}, {max}]"
        );
    }

    #[test]
    fn magnitude_bounds_match_digit_width() {
        assert_eq!(magnitude_bounds(0), (1, 9));
        assert_eq!(magnitude_bounds(1), (1, 9));
        for digits in 2..=19 {
            let (min, max) = magnitude_bounds(digits);
            assert_eq!(min, 10u64.pow(digits - 1));
            assert_eq!(max, 10u64.pow(digits) - 1);
        }
    }

    #[test]
    #[should_panic(expected = "digits <= 19 keeps 10^digits inside u64")]
    fn magnitude_bounds_rejects_widths_past_u64() {
        let _ = magnitude_bounds(20);
    }

    #[test]
    fn random_magnitude_stays_in_domain() {
        let mut rng = rng();
        for _ in 0..200 {
            assert_in_domain(i128::from(random_magnitude(&mut rng, 1)), 1);
            assert_in_domain(i128::from(random_magnitude(&mut rng, 3)), 3);
            assert_in_domain(i128::from(random_magnitude(&mut rng, 18)), 18);
        }
    }

    #[test]
    fn random_magnitude_capped_respects_cap() {
        let mut rng = rng();

        // A cap below the width's minimum admits nothing.
        assert!(random_magnitude_capped(&mut rng, 3, 50).is_none());
        assert!(random_magnitude_capped(&mut rng, 2, 9).is_none());
        assert!(random_magnitude_capped(&mut rng, 1, 0).is_none());

        // A cap equal to the minimum is the only admissible value.
        assert_eq!(random_magnitude_capped(&mut rng, 1, 1), Some(1));
        assert_eq!(random_magnitude_capped(&mut rng, 2, 10), Some(10));

        // A cap inside the domain bounds the draw.
        for _ in 0..200 {
            let value = random_magnitude_capped(&mut rng, 1, 5).expect("1..=5 admits values");
            assert!((1..=5).contains(&value));
        }
        for _ in 0..200 {
            let value = random_magnitude_capped(&mut rng, 2, 99).expect("10..=99 admits values");
            assert!((10..=99).contains(&value), "{value} outside [10, 99]");
        }
        // A cap past the width cannot widen it.
        for _ in 0..200 {
            let value = random_magnitude_capped(&mut rng, 3, u64::MAX).expect("always admits");
            assert!((100..=999).contains(&value), "{value} outside [100, 999]");
        }
    }

    #[test]
    fn random_number_never_negative_when_negatives_disabled() {
        let mut rng = rng();
        let mut running_sum: i128 = 0;
        for index in 0..500u32 {
            let value = random_number_with_constraints(&mut rng, 2, false, index, running_sum);
            assert!(value >= 0, "negative {value} at index {index}");
            assert_in_domain(value, 2);
            running_sum += value;
        }
    }

    #[test]
    fn random_number_first_is_never_negative() {
        let mut rng = rng();
        for digits in [1u32, 2, 5, 18] {
            let value = random_number_with_constraints(&mut rng, digits, true, 0, 0);
            assert!(value >= 0, "first number {value} was negative");
            assert_in_domain(value, digits);
        }
    }

    #[test]
    fn random_number_never_drives_the_running_sum_negative() {
        let mut rng = rng();
        let mut running_sum: i128 = 0;
        for index in 0..10_000u32 {
            let value = random_number_with_constraints(&mut rng, 3, true, index, running_sum);
            assert_in_domain(value, 3);
            running_sum += value;
            assert!(running_sum >= 0, "running sum went negative at {index}");
        }
    }

    #[test]
    fn random_number_with_a_tiny_running_sum_cannot_go_negative() {
        let mut rng = rng();
        // A running sum below the width's minimum magnitude cannot fund a
        // negative draw at all, so only the positive branch is reachable.
        for _ in 0..200 {
            let value = random_number_with_constraints(&mut rng, 5, true, 1, 1);
            assert!(value >= 0, "unfundable negative {value}");
            assert_in_domain(value, 5);
        }
    }
}
