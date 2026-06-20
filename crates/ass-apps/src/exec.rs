//! `Exec` field-code handling for desktop entries.
//!
//! Implements the
//! [Desktop Entry Spec §"Exec string"](https://specifications.freedesktop.org/desktop-entry-spec/latest/exec-variables.html):
//! strip / expand the `%`-prefixed field codes a `.desktop` file may embed in
//! its `Exec` line, leaving a string the launcher can hand to `sh -c` or an
//! argv it can `execv` directly.
//!
//! Codes handled:
//!
//! | Code | Meaning | Expansion |
//! |------|---------|-----------|
//! | `%f` `%F` | files (paths) | the provided file args, joined |
//! | `%u` `%U` | URIs | the provided uri args, joined |
//! | `%d` `%D` `%n` `%N` | deprecated (dir / name) | dropped |
//! | `%i` | icon | `--icon <icon>` |
//! | `%c` | translated name | the provided name |
//! | `%k` | desktop file path | the provided path |
//! | `%%` | literal `%` | `%` |
//!
//! Per the spec, at most one of `%f %F %u %U` should appear; if several do,
//! only the first one expands and the rest are dropped (a single code absorbs
//! all provided file arguments). Unknown `%x` codes are dropped.

/// Tokenize an `Exec` value into argv, honoring the desktop-entry quoting
/// rules (reserved chars quoted with `"` or `\`), *before* field-code
/// expansion. Exposed so callers can pass it to `execv` directly.
fn tokenize_quoted(s: &str) -> Vec<String> {
    let chars: Vec<char> = s.chars().collect();
    let mut out: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut have_token = false;
    let mut in_dq = false;
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if in_dq {
            match c {
                '"' => in_dq = false,
                '\\' => {
                    // Inside double quotes only `"`, `\`, `` ` ``, `$` keep
                    // their special meaning when escaped; otherwise the
                    // backslash is preserved literally.
                    if i + 1 < chars.len()
                        && matches!(chars[i + 1], '"' | '\\' | '`' | '$')
                    {
                        cur.push(chars[i + 1]);
                        i += 2;
                        continue;
                    } else {
                        cur.push('\\');
                    }
                }
                _ => cur.push(c),
            }
        } else {
            match c {
                '"' => {
                    in_dq = true;
                    have_token = true;
                }
                '\\' if i + 1 < chars.len() => {
                    cur.push(chars[i + 1]);
                    have_token = true;
                    i += 2;
                    continue;
                }
                ' ' | '\t' | '\n' => {
                    if have_token {
                        out.push(std::mem::take(&mut cur));
                        have_token = false;
                    }
                }
                _ => {
                    cur.push(c);
                    have_token = true;
                }
            }
        }
        i += 1;
    }
    if have_token {
        out.push(cur);
    }
    out
}

/// Split an `Exec` value into argv with field codes expanded.
///
/// `files` is substituted into the first `%f`/`%F`/`%u`/`%U` encountered;
/// `icon`, `name`, and `desktop_path` feed `%i`, `%c`, `%k` respectively.
/// Each may be `None`/empty, in which case the corresponding code is dropped.
pub fn expand_exec_tokens(
    exec: &str,
    files: &[String],
    icon: Option<&str>,
    name: Option<&str>,
    desktop_path: Option<&str>,
) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut files_consumed = false;

    for raw in tokenize_quoted(exec) {
        // Fast path: no field code in the token → emit verbatim.
        if !raw.contains('%') {
            out.push(raw);
            continue;
        }
        // Walk the token char-by-char, expanding codes. A token may glue a
        // code to surrounding text (e.g. `--file=%f`).
        let mut buf = String::new();
        let chars: Vec<char> = raw.chars().collect();
        let mut i = 0;
        let mut code_emitted_separate: Vec<String> = Vec::new();
        while i < chars.len() {
            let c = chars[i];
            if c != '%' {
                buf.push(c);
                i += 1;
                continue;
            }
            // Look at the code char.
            let next = chars.get(i + 1).copied();
            i += 2;
            match next {
                Some('%') => buf.push('%'),
                Some('f') | Some('F') | Some('u') | Some('U') => {
                    if !files_consumed {
                        files_consumed = true;
                        // If the code stands alone in the token, push each
                        // file as its own argv element; otherwise inline them
                        // into the current buffer.
                        let alone = buf.is_empty()
                            && (i >= chars.len()
                                || chars[i..].iter().all(|ch| matches!(ch, ' ' | '\t')));
                        if alone {
                            // Flush the (empty) buffer is a no-op; push files.
                            for f in files {
                                code_emitted_separate.push(f.clone());
                            }
                        } else {
                            for (k, f) in files.iter().enumerate() {
                                if k > 0 {
                                    buf.push(' ');
                                }
                                buf.push_str(f);
                            }
                        }
                    }
                }
                Some('i') => {
                    if let Some(ic) = icon.filter(|s| !s.is_empty()) {
                        if buf.is_empty() {
                            code_emitted_separate.push("--icon".to_string());
                            code_emitted_separate.push(ic.to_string());
                        } else {
                            buf.push_str("--icon ");
                            buf.push_str(ic);
                        }
                    }
                }
                Some('c') => {
                    if let Some(n) = name.filter(|s| !s.is_empty()) {
                        buf.push_str(n);
                    }
                }
                Some('k') => {
                    if let Some(p) = desktop_path.filter(|s| !s.is_empty()) {
                        buf.push_str(p);
                    }
                }
                // Deprecated %d %D %n %N and unknown codes: drop.
                _ => {}
            }
        }
        if !buf.is_empty() {
            out.push(buf);
        }
        for extra in code_emitted_separate {
            out.push(extra);
        }
    }
    out
}

/// Produce a shell-safe command string with field codes expanded, suitable
/// for `sh -c`. Each resulting argv element is single-quote-escaped.
pub fn expand_exec(
    exec: &str,
    files: &[String],
    icon: Option<&str>,
    name: Option<&str>,
    desktop_path: Option<&str>,
) -> String {
    let argv = expand_exec_tokens(exec, files, icon, name, desktop_path);
    argv.iter()
        .map(|a| shell_quote(a))
        .collect::<Vec<_>>()
        .join(" ")
}

/// POSIX single-quote shell escaping: wrap in `'…'`, replacing each `'` with
/// `'\''`. Safe for `sh -c` argv composition.
fn shell_quote(s: &str) -> String {
    if !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | '/' | ',' | ':' | '+'))
    {
        s.to_string()
    } else {
        let mut out = String::from("'");
        for c in s.chars() {
            if c == '\'' {
                out.push_str("'\\''");
            } else {
                out.push(c);
            }
        }
        out.push('\'');
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_passthrough() {
        assert_eq!(expand_exec_tokens("foot", &[], None, None, None), vec!["foot"]);
    }

    #[test]
    fn args_survive() {
        assert_eq!(
            expand_exec_tokens("alacritty --config-file /x/y.toml", &[], None, None, None),
            vec!["alacritty", "--config-file", "/x/y.toml"]
        );
    }

    #[test]
    fn file_code_absorbs_all_files() {
        assert_eq!(
            expand_exec_tokens("mpv %f", &["a.mkv".into(), "b.mkv".into()], None, None, None),
            vec!["mpv", "a.mkv", "b.mkv"]
        );
    }

    #[test]
    fn second_file_code_is_dropped() {
        assert_eq!(
            expand_exec_tokens("foo %f %u", &["a".into()], None, None, None),
            vec!["foo", "a"]
        );
    }

    #[test]
    fn uri_code() {
        assert_eq!(
            expand_exec_tokens("xdg-open %u", &["https://x".into()], None, None, None),
            vec!["xdg-open", "https://x"]
        );
    }

    #[test]
    fn icon_code_expands_to_two_args() {
        assert_eq!(
            expand_exec_tokens("foo %i", &[], Some("bar"), None, None),
            vec!["foo", "--icon", "bar"]
        );
    }

    #[test]
    fn name_and_path_codes() {
        assert_eq!(
            expand_exec_tokens("foo %c %k", &[], None, Some("Bar"), Some("/x.desktop")),
            vec!["foo", "Bar", "/x.desktop"]
        );
    }

    #[test]
    fn percent_percent_is_literal() {
        assert_eq!(expand_exec_tokens("echo 100%%", &[], None, None, None), vec!["echo", "100%"]);
    }

    #[test]
    fn quoted_token_with_spaces() {
        assert_eq!(
            expand_exec_tokens(r#"firefox "Profile Manager" %u"#, &[], None, None, None),
            vec!["firefox", "Profile Manager"]
        );
    }

    #[test]
    fn backslash_escape_outside_quotes() {
        assert_eq!(
            expand_exec_tokens(r"foo\ bar", &[], None, None, None),
            vec!["foo bar"]
        );
    }

    #[test]
    fn shell_quote_escapes_risky() {
        assert_eq!(shell_quote("plain-name_1"), "plain-name_1");
        assert_eq!(shell_quote("a b"), "'a b'");
        assert_eq!(shell_quote("it's"), "'it'\\''s'");
    }

    #[test]
    fn expand_exec_string_form() {
        let s = expand_exec("mpv %f", &["my video.mkv".into()], None, None, None);
        assert_eq!(s, "mpv 'my video.mkv'");
    }
}
