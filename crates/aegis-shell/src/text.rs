/// Truncate user-facing copy at a Unicode scalar boundary and reserve the
/// final slot for an ellipsis.
pub fn truncate(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let mut value: String = text.chars().take(max_chars.saturating_sub(1)).collect();
    value.push('…');
    value
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncation_is_unicode_safe_and_bounded() {
        assert_eq!(truncate("Fuji connected", 20), "Fuji connected");
        assert_eq!(truncate("真实通知已经联通", 6), "真实通知已…");
        assert_eq!(truncate("窗口操作", 3), "窗口…");
    }
}
