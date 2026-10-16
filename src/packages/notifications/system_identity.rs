// Prefix arbitrary application IDs so reserved Apple activation identifiers cannot collide.
pub fn native_action(id: &str) -> String {
    format!("gpui-action:{id}")
}
pub fn response_action(native: &str) -> Option<Option<String>> {
    match native {
        "com.apple.UNNotificationDefaultActionIdentifier" => Some(None),
        "com.apple.UNNotificationDismissActionIdentifier" => None,
        id => id
            .strip_prefix("gpui-action:")
            .map(|id| Some(id.to_owned())),
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn preserves_empty_unicode_and_reserved_action_ids() {
        for id in [
            "",
            "한글/a?%",
            "com.apple.UNNotificationDefaultActionIdentifier",
            "gpui-action:x",
        ] {
            assert_eq!(response_action(&native_action(id)), Some(Some(id.into())));
        }
        assert_eq!(
            response_action("com.apple.UNNotificationDefaultActionIdentifier"),
            Some(None)
        );
        assert_eq!(
            response_action("com.apple.UNNotificationDismissActionIdentifier"),
            None
        );
        assert_eq!(response_action("unrelated-app-action"), None);
    }
}
