//! Original ordinary786 arithmetic, including its behavior for raw invalid
//! dates. No host calendar, leap-day normalization or frame clock is used.

fn truncate_double(value: f64) -> i32 {
    if !value.is_finite() || value < -2_147_483_648.0 || value >= 2_147_483_648.0 {
        i32::MIN
    } else { value.trunc() as i32 }
}

fn signed_product_high(left: i32, right: i32) -> i32 {
    ((i64::from(left) * i64::from(right)) >> 32) as i32
}

fn divide_seven(value: i32) -> i32 {
    let shifted = signed_product_high(0x9249_2493u32 as i32, value)
        .wrapping_add(value) >> 2;
    shifted.wrapping_add(((shifted as u32) >> 31) as i32)
}

/// Return the raw DWORD from the recovered helper. All integer intermediates
/// wrap as x86 DWORDs; CVTTSD2SI's indefinite result is retained on overflow.
pub fn weekday(year: u32, month: u32, day: u32) -> u32 {
    let after_february = month as i32 > 2;
    let adjusted_year = if after_february { year } else { year.wrapping_sub(1) } as i32;
    let adjusted_month = month.wrapping_add(if after_february { 1 } else { 13 }) as i32;
    let month_term = truncate_double(f64::from(adjusted_month) * -30.6);
    let year_term = truncate_double(f64::from(adjusted_year) * 365.25);
    let product_high = signed_product_high(0x51eb_851f, adjusted_year);
    let quotient_400 = product_high >> 7;
    let quotient_400 = quotient_400.wrapping_add(((quotient_400 as u32) >> 31) as i32);
    let quotient_100 = product_high >> 5;
    let quotient_100 = quotient_100.wrapping_add(((quotient_100 as u32) >> 31) as i32);
    let value = year_term.wrapping_sub(month_term)
        .wrapping_add(quotient_400).wrapping_sub(quotient_100)
        .wrapping_add(day as i32).wrapping_sub(429);
    // Keep both signed remainders and the intervening +1. A normalized
    // Euclidean remainder would change the original's invalid-date results.
    let first = value.wrapping_sub(divide_seven(value).wrapping_mul(7)).wrapping_add(1);
    first.wrapping_sub(divide_seven(first).wrapping_mul(7)) as u32
}
