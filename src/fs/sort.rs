use std::cmp::Ordering;

/// Compares already-normalized strings using natural ordering.
///
/// Text is compared lexically, but adjacent ASCII digit runs are compared numerically so
/// filenames like `2` sort before `10`.
pub(crate) fn natural_cmp(left: &str, right: &str) -> Ordering {
    let mut left_chars = left.chars().peekable();
    let mut right_chars = right.chars().peekable();

    loop {
        match (left_chars.peek(), right_chars.peek()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(left_ch), Some(right_ch))
                if left_ch.is_ascii_digit() && right_ch.is_ascii_digit() =>
            {
                let left_digits = take_digit_run(&mut left_chars);
                let right_digits = take_digit_run(&mut right_chars);
                match compare_numeric_runs(&left_digits, &right_digits) {
                    Ordering::Equal => {}
                    order => return order,
                }
            }
            (Some(_), Some(_)) => {
                let left_ch = left_chars.next().unwrap_or_default();
                let right_ch = right_chars.next().unwrap_or_default();
                match left_ch.cmp(&right_ch) {
                    Ordering::Equal => {}
                    order => return order,
                }
            }
        }
    }
}

fn take_digit_run(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) -> String {
    let mut digits = String::new();
    while chars.peek().is_some_and(|ch| ch.is_ascii_digit()) {
        digits.push(chars.next().unwrap_or_default());
    }
    digits
}

fn compare_numeric_runs(left: &str, right: &str) -> Ordering {
    let left_trimmed = left.trim_start_matches('0');
    let right_trimmed = right.trim_start_matches('0');
    let left_normalized = if left_trimmed.is_empty() {
        "0"
    } else {
        left_trimmed
    };
    let right_normalized = if right_trimmed.is_empty() {
        "0"
    } else {
        right_trimmed
    };

    match left_normalized.len().cmp(&right_normalized.len()) {
        Ordering::Equal => match left_normalized.cmp(right_normalized) {
            Ordering::Equal => left.len().cmp(&right.len()),
            order => order,
        },
        order => order,
    }
}

#[cfg(test)]
mod tests {
    use super::natural_cmp;
    use std::cmp::Ordering;

    #[test]
    fn natural_cmp_orders_numeric_suffixes() {
        assert_eq!(natural_cmp("chapter 2", "chapter 10"), Ordering::Less);
        assert_eq!(natural_cmp("chapter 10", "chapter 2"), Ordering::Greater);
    }

    #[test]
    fn natural_cmp_handles_non_latin_text_around_numbers() {
        assert_eq!(natural_cmp("北斗の拳 2巻", "北斗の拳 10巻"), Ordering::Less);
    }

    #[test]
    fn natural_cmp_keeps_zero_padded_numbers_stable() {
        assert_eq!(natural_cmp("page 1", "page 01"), Ordering::Less);
        assert_eq!(natural_cmp("page 01", "page 001"), Ordering::Less);
    }
}
