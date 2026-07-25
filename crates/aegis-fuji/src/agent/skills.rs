//! Skill discovery: scan configured roots for `*/SKILL.md` files and parse
//! their YAML frontmatter. The parser intentionally understands only flat
//! `key: value` scalars — enough for `name` and `description`.

use std::path::{Path, PathBuf};

/// One discovered skill with its full instruction body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skill {
    pub name: String,
    pub description: String,
    pub body: String,
    pub path: PathBuf,
}

/// Scan each root's immediate children for `SKILL.md` files. Unreadable or
/// malformed entries are skipped, never fatal.
pub fn discover(paths: &[PathBuf]) -> Vec<Skill> {
    let mut skills = Vec::new();
    for root in paths {
        let Ok(entries) = std::fs::read_dir(root) else {
            continue;
        };
        let mut entries: Vec<_> = entries.filter_map(|entry| entry.ok()).collect();
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let file = entry.path().join("SKILL.md");
            let Ok(text) = std::fs::read_to_string(&file) else {
                continue;
            };
            if let Some(skill) = parse(&text, &entry.file_name().to_string_lossy(), &file) {
                skills.push(skill);
            }
        }
    }
    skills.sort_by(|a, b| a.name.cmp(&b.name));
    skills
}

/// One `- name: description` line per skill for the system prompt.
pub fn summary_lines(skills: &[Skill]) -> Vec<String> {
    skills
        .iter()
        .map(|skill| format!("- {}: {}", skill.name, skill.description))
        .collect()
}

fn parse(text: &str, dir_name: &str, path: &Path) -> Option<Skill> {
    let (frontmatter, body) = split_frontmatter(text)?;
    let mut name = None;
    let mut description = String::new();
    for line in frontmatter.lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let value = unquote(value.trim());
        match key.trim() {
            "name" => name = Some(value),
            "description" => description = value,
            _ => {}
        }
    }
    Some(Skill {
        name: name
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| dir_name.to_string()),
        description,
        body: body.trim().to_string(),
        path: path.to_path_buf(),
    })
}

/// Split `---\nfrontmatter\n---\nbody`; `None` when the fences are absent.
fn split_frontmatter(text: &str) -> Option<(&str, &str)> {
    let rest = text.strip_prefix("---")?;
    let rest = rest
        .strip_prefix("\r\n")
        .or_else(|| rest.strip_prefix('\n'))?;
    let end = rest.find("\n---")?;
    let frontmatter = &rest[..end];
    let after = &rest[end + 4..];
    let body = after
        .strip_prefix("\r\n")
        .or_else(|| after.strip_prefix('\n'))
        .unwrap_or(after);
    Some((frontmatter, body))
}

fn unquote(value: &str) -> String {
    let bytes = value.as_bytes();
    if bytes.len() >= 2
        && ((bytes[0] == b'"' && bytes[bytes.len() - 1] == b'"')
            || (bytes[0] == b'\'' && bytes[bytes.len() - 1] == b'\''))
    {
        return value[1..value.len() - 1].to_string();
    }
    value.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discovers_skills_and_falls_back_to_dir_names() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        std::fs::create_dir(root.join("alpha")).expect("alpha");
        std::fs::write(
            root.join("alpha").join("SKILL.md"),
            "---\nname: alpha\ndescription: \"first skill\"\ntags: [a, b]\n---\n# Alpha\nDo the thing.\n",
        )
        .expect("alpha skill");
        std::fs::create_dir(root.join("beta")).expect("beta");
        std::fs::write(
            root.join("beta").join("SKILL.md"),
            "---\ndescription: second\n---\nbody\n",
        )
        .expect("beta skill");
        std::fs::create_dir(root.join("broken")).expect("broken");
        std::fs::write(root.join("broken").join("SKILL.md"), "no fences\n").expect("broken");
        std::fs::write(root.join("stray.md"), "not a skill\n").expect("stray");

        let skills = discover(&[root.to_path_buf()]);
        assert_eq!(skills.len(), 2);
        assert_eq!(skills[0].name, "alpha");
        assert_eq!(skills[0].description, "first skill");
        assert_eq!(skills[0].body, "# Alpha\nDo the thing.");
        assert_eq!(skills[1].name, "beta");
        assert_eq!(skills[1].description, "second");
    }

    #[test]
    fn parses_the_shipped_ass_desktop_realm_skill() {
        let text = "---\nname: ass-desktop-realm\ndescription: Use when operating windows\nshort-description: short\npolicy:\n  allow_implicit_invocation: true\ndependencies:\n  - type: mcp\n    value: ass\n---\n# Title\n";
        let (frontmatter, _) = split_frontmatter(text).expect("frontmatter");
        let skill = parse(text, "fallback", Path::new("SKILL.md")).expect("skill");
        assert!(frontmatter.contains("policy:"));
        assert_eq!(skill.name, "ass-desktop-realm");
        assert_eq!(skill.description, "Use when operating windows");
        assert_eq!(skill.body, "# Title");
    }
}
