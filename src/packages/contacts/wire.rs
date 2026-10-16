// Decode only the delimiters escaped by the matching Java bridge. Decode '%' last
// so a literal "%7C" survives a single round trip rather than becoming '|'.
pub(super) fn decode(value: &str) -> String {
    value
        .replace("%7C", "|")
        .replace("%0A", "\n")
        .replace("%0D", "\r")
        .replace("%2C", ",")
        .replace("%3A", ":")
        .replace("%25", "%")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn names_labels_and_literal_escapes_round_trip() {
        assert_eq!(decode("김%7C이%0Ahome%0D%2C%3A%257C"), "김|이\nhome\r,:%7C");
        assert_eq!(decode("a+b@example.com"), "a+b@example.com");
    }
}
