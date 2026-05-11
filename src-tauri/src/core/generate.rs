use rand::{Rng, RngExt};

pub(crate) fn random_fixed_digits_no_leading_zero(rng: &mut impl Rng, digits: u32) -> String {
    if digits <= 1 {
        // No leading zero => 0 is excluded.
        return rng.random_range(1u32..=9u32).to_string();
    }

    let min = 10u64.pow(digits - 1);
    let max_exclusive = 10u64.pow(digits);
    rng.random_range(min..max_exclusive).to_string()
}

pub(crate) fn random_fixed_digits_no_leading_zero_capped(
    rng: &mut impl Rng,
    digits: u32,
    max_inclusive: u64,
) -> Option<String> {
    if digits <= 1 {
        if max_inclusive < 1 {
            return None;
        }
        let max = max_inclusive.min(9);
        return Some(rng.random_range(1u64..=max).to_string());
    }

    let min = 10u64.pow(digits - 1);
    if max_inclusive < min {
        return None;
    }

    let max_exclusive = 10u64.pow(digits);
    let cap_exclusive = (max_inclusive.saturating_add(1)).min(max_exclusive);
    Some(rng.random_range(min..cap_exclusive).to_string())
}

pub fn random_number_with_constraints(
    rng: &mut impl Rng,
    digits: u32,
    allow_negative_numbers: bool,
    index: u32,
    running_sum: i128,
) -> (String, i128) {
    // Requirement: first number is never negative.
    let allow_negative_here = allow_negative_numbers && index > 0;

    // Cap for negative magnitudes: cannot exceed current running sum, and cannot exceed
    // the maximum representable magnitude for the requested digit count.
    let max_for_digits = if digits <= 1 {
        9u64
    } else {
        10u64.pow(digits).saturating_sub(1)
    };

    let sum_cap_u64 = if running_sum <= 0 {
        0u64
    } else {
        (running_sum.min(max_for_digits as i128)) as u64
    };

    let can_choose_negative = allow_negative_here && sum_cap_u64 > 0;
    let try_negative = can_choose_negative && rng.random_bool(0.5);

    if try_negative
        && let Some(magnitude) =
            random_fixed_digits_no_leading_zero_capped(rng, digits, sum_cap_u64)
    {
        let magnitude_value: i128 = magnitude
            .parse::<i128>()
            .expect("generated magnitude should parse as integer");
        // Enforce non-negative running sum after applying this value.
        if running_sum - magnitude_value >= 0 {
            return (format!("-{magnitude}"), -magnitude_value);
        }
    }

    let magnitude = random_fixed_digits_no_leading_zero(rng, digits);
    let magnitude_value: i128 = magnitude
        .parse::<i128>()
        .expect("generated magnitude should parse as integer");
    (magnitude, magnitude_value)
}
