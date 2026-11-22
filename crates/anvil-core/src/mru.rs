//! Most-recently-used order, as VS' Ctrl+Tab window switcher shows files.

/// Moves `item` to the front (adding it when new).
pub fn touch<T: PartialEq>(list: &mut Vec<T>, item: T) {
    list.retain(|x| *x != item);
    list.insert(0, item);
}

/// The index Ctrl+Tab moves to from `from`, wrapping in both directions.
pub fn step(len: usize, from: usize, forward: bool) -> usize {
    if len == 0 {
        return 0;
    }
    if forward { (from + 1) % len } else { (from + len - 1) % len }
}

/// Where the switcher starts: on the file before the current one, so a
/// quick Ctrl+Tab flips between the last two files.
pub fn first_choice(len: usize, forward: bool) -> usize {
    match len {
        0 | 1 => 0,
        _ if forward => 1,
        _ => len - 1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn touch_moves_to_front() {
        let mut l = vec!["a", "b", "c"];
        touch(&mut l, "c");
        assert_eq!(l, ["c", "a", "b"]);
        touch(&mut l, "d");
        assert_eq!(l, ["d", "c", "a", "b"]);
    }

    #[test]
    fn steps_wrap() {
        assert_eq!(step(3, 2, true), 0);
        assert_eq!(step(3, 0, false), 2);
        assert_eq!(step(0, 0, true), 0);
        assert_eq!(first_choice(3, true), 1);
        assert_eq!(first_choice(3, false), 2);
        assert_eq!(first_choice(1, true), 0);
    }
}
