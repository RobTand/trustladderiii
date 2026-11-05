use crate::python_unicode::{CASED, DECIMAL, IGNORABLE, LOWER, SPACE, WORD};
fn in_ranges(c: char, ranges: &[(u32, u32)]) -> bool {
    let c = c as u32;
    let index = ranges.partition_point(|&(_, end)| end < c);
    ranges.get(index).is_some_and(|&(start, _)| start <= c)
}
pub(crate) fn is_word(c: char) -> bool {
    in_ranges(c, WORD)
}
pub(crate) fn is_space(c: char) -> bool {
    in_ranges(c, SPACE)
}
pub(crate) fn is_decimal(c: char) -> bool {
    in_ranges(c, DECIMAL)
}
pub(crate) fn lowercase(text: &str) -> String {
    let chars: Vec<_> = text.chars().collect();
    let mut output = String::new();
    for (index, &c) in chars.iter().enumerate() {
        if c == 'Σ' {
            let before = chars[..index]
                .iter()
                .rev()
                .find(|&&v| !in_ranges(v, IGNORABLE))
                .is_some_and(|&v| in_ranges(v, CASED));
            let after = chars[index + 1..]
                .iter()
                .find(|&&v| !in_ranges(v, IGNORABLE))
                .is_some_and(|&v| in_ranges(v, CASED));
            output.push(if before && !after { 'ς' } else { 'σ' });
        } else if let Ok(index) = LOWER.binary_search_by_key(&(c as u32), |&(point, _)| point) {
            output.push_str(LOWER[index].1);
        } else {
            output.push(c);
        }
    }
    output
}
