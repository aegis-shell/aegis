use std::io::{Read as _, Write as _};
use std::os::unix::fs::{MetadataExt as _, OpenOptionsExt as _};
use std::path::{Path, PathBuf};

use toml_edit::{DocumentMut, Item, Table, value};

use super::{
    Config, Diagnostic, LoadError, SUPPORTED_SCHEMA_VERSION, byte_to_line, write_document_atomic,
};

const LEGACY_SCHEMA_VERSION: u32 = 1;
const MAX_CONFIG_BYTES: u64 = 4 * 1024 * 1024;

/// Comment-preserving result of an explicit configuration migration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigMigration {
    pub from_version: u32,
    pub to_version: u32,
    pub changed: bool,
    pub contents: String,
}

/// Filesystem result of [`migrate_file`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MigrationOutcome {
    AlreadyCurrent {
        path: PathBuf,
        version: u32,
    },
    Migrated {
        path: PathBuf,
        backup: PathBuf,
        from_version: u32,
        to_version: u32,
    },
}

/// Explicitly migrate one configuration document to the current schema.
///
/// Loading never invokes this function implicitly. Version 1 resource-budget
/// tables are renamed to `interaction_domain_sandbox`, but ambient network or
/// host-path access cannot be represented safely by version 2 and therefore
/// blocks migration instead of being silently discarded.
pub fn migrate_text(text: &str) -> Result<ConfigMigration, Vec<Diagnostic>> {
    let mut document = text.parse::<DocumentMut>().map_err(|error| {
        vec![Diagnostic {
            line: error.span().map(|span| byte_to_line(text, span.start)),
            field: None,
            message: format!("parse error: {error}"),
        }]
    })?;
    let from_version = document
        .get("schema_version")
        .and_then(Item::as_integer)
        .and_then(|version| u32::try_from(version).ok())
        .ok_or_else(|| {
            vec![Diagnostic::new(
                Some("schema_version".into()),
                "must be an unsigned integer",
            )]
        })?;

    if from_version == SUPPORTED_SCHEMA_VERSION {
        Config::parse(text)?;
        return Ok(ConfigMigration {
            from_version,
            to_version: SUPPORTED_SCHEMA_VERSION,
            changed: false,
            contents: text.to_owned(),
        });
    }
    if from_version != LEGACY_SCHEMA_VERSION {
        return Err(vec![Diagnostic::new(
            Some("schema_version".into()),
            format!(
                "no migration path from schema version {from_version} to {SUPPORTED_SCHEMA_VERSION}"
            ),
        )]);
    }
    if document.contains_key("interaction_domain_sandbox") && document.contains_key("realm_sandbox")
    {
        return Err(vec![Diagnostic::new(
            Some("interaction_domain_sandbox".into()),
            "cannot migrate when both realm_sandbox and interaction_domain_sandbox exist",
        )]);
    }

    if let Some(mut sandbox) = document.remove("realm_sandbox") {
        let mut diagnostics = Vec::new();
        validate_legacy_sandbox(&sandbox, "realm_sandbox", &mut diagnostics);
        if !diagnostics.is_empty() {
            return Err(diagnostics);
        }
        strip_removed_authority(&mut sandbox);
        document.insert("interaction_domain_sandbox", sandbox);
    }
    let version_decor = document["schema_version"]
        .as_value()
        .map(|version| version.decor().clone());
    document["schema_version"] = value(i64::from(SUPPORTED_SCHEMA_VERSION));
    if let (Some(decor), Some(version)) = (version_decor, document["schema_version"].as_value_mut())
    {
        *version.decor_mut() = decor;
    }
    let contents = document.to_string();
    Config::parse(&contents)?;
    Ok(ConfigMigration {
        from_version,
        to_version: SUPPORTED_SCHEMA_VERSION,
        changed: true,
        contents,
    })
}

/// Migrate one on-disk file with a durable, non-overwriting versioned backup.
///
/// The source must be a regular file owned by the current Unix user, must not
/// be a symlink or hard-linked alias, and is bounded before it is read. The
/// backup is synchronized before the normal atomic configuration writer
/// replaces the active file.
pub fn migrate_file(path: &Path) -> Result<MigrationOutcome, LoadError> {
    let mut source_file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
        .open(path)
        .map_err(|source| LoadError::Read {
            path: path.into(),
            source,
        })?;
    let metadata = source_file.metadata().map_err(|source| LoadError::Read {
        path: path.into(),
        source,
    })?;
    if !metadata.file_type().is_file()
        || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.nlink() != 1
        || metadata.len() > MAX_CONFIG_BYTES
    {
        return Err(LoadError::Read {
            path: path.into(),
            source: std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "configuration must be a bounded, owner-controlled regular file with one link",
            ),
        });
    }
    let mut text = String::with_capacity(metadata.len() as usize);
    std::io::Read::take(&mut source_file, MAX_CONFIG_BYTES + 1)
        .read_to_string(&mut text)
        .map_err(|source| LoadError::Read {
            path: path.into(),
            source,
        })?;
    if text.len() as u64 > MAX_CONFIG_BYTES {
        return Err(LoadError::Read {
            path: path.into(),
            source: std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "configuration exceeds the migration size limit",
            ),
        });
    }
    let migration = migrate_text(&text).map_err(|diagnostics| LoadError::Invalid {
        path: path.into(),
        diagnostics,
    })?;
    if !migration.changed {
        return Ok(MigrationOutcome::AlreadyCurrent {
            path: path.into(),
            version: migration.to_version,
        });
    }

    let backup = write_backup(path, migration.from_version, text.as_bytes()).map_err(|source| {
        LoadError::Write {
            path: path.into(),
            source,
        }
    })?;
    write_document_atomic(path, &migration.contents)?;
    Ok(MigrationOutcome::Migrated {
        path: path.into(),
        backup,
        from_version: migration.from_version,
        to_version: migration.to_version,
    })
}

fn validate_legacy_sandbox(item: &Item, prefix: &str, diagnostics: &mut Vec<Diagnostic>) {
    let Some(table) = item.as_table() else {
        diagnostics.push(Diagnostic::new(Some(prefix.into()), "must be a table"));
        return;
    };
    validate_removed_authority(table, prefix, diagnostics);
    if let Some(apps) = table.get("app") {
        let Some(apps) = apps.as_array_of_tables() else {
            diagnostics.push(Diagnostic::new(
                Some(format!("{prefix}.app")),
                "must be an array of tables",
            ));
            return;
        };
        for (index, app) in apps.iter().enumerate() {
            validate_removed_authority(app, &format!("{prefix}.app.{index}"), diagnostics);
        }
    }
}

fn validate_removed_authority(table: &Table, prefix: &str, diagnostics: &mut Vec<Diagnostic>) {
    if table
        .get("network")
        .is_some_and(|item| item.as_bool() != Some(false))
    {
        diagnostics.push(Diagnostic::new(
            Some(format!("{prefix}.network")),
            "ambient network access cannot be migrated; use an exact runtime NetworkOrigin grant",
        ));
    }
    for key in ["readable_paths", "writable_paths"] {
        if table
            .get(key)
            .is_some_and(|item| item.as_array().is_none_or(|paths| !paths.is_empty()))
        {
            diagnostics.push(Diagnostic::new(
                Some(format!("{prefix}.{key}")),
                "ambient host paths cannot be migrated; use exact runtime filesystem grants",
            ));
        }
    }
}

fn strip_removed_authority(item: &mut Item) {
    let Some(table) = item.as_table_mut() else {
        return;
    };
    for key in ["network", "readable_paths", "writable_paths"] {
        table.remove(key);
    }
    if let Some(apps) = table.get_mut("app").and_then(Item::as_array_of_tables_mut) {
        for app in apps.iter_mut() {
            for key in ["network", "readable_paths", "writable_paths"] {
                app.remove(key);
            }
        }
    }
}

fn write_backup(path: &Path, version: u32, contents: &[u8]) -> std::io::Result<PathBuf> {
    let parent = path.parent().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "configuration path has no parent directory",
        )
    })?;
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("config.toml");
    for attempt in 0..32_u32 {
        let suffix = if attempt == 0 {
            format!("schema-v{version}.bak")
        } else {
            format!("schema-v{version}.{attempt}.bak")
        };
        let candidate = parent.join(format!("{name}.{suffix}"));
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
            .open(&candidate)
        {
            Ok(mut file) => {
                file.write_all(contents)?;
                file.sync_all()?;
                if let Ok(directory) = std::fs::File::open(parent) {
                    directory.sync_all()?;
                }
                return Ok(candidate);
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::AlreadyExists,
        "could not allocate a migration backup",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn v1_migration_preserves_comments_and_renames_safe_resource_limits() {
        let migrated = migrate_text(
            "schema_version = 1 # keep\n\n[realm_sandbox]\nnetwork = false\nreadable_paths = []\nmemory_max_mib = 512\n",
        )
        .unwrap();
        assert!(migrated.changed);
        assert!(migrated.contents.contains("schema_version = 2 # keep"));
        assert!(migrated.contents.contains("[interaction_domain_sandbox]"));
        assert!(migrated.contents.contains("memory_max_mib = 512"));
        assert!(!migrated.contents.contains("realm_sandbox"));
        assert!(!migrated.contents.contains("network ="));
        Config::parse(&migrated.contents).unwrap();
    }

    #[test]
    fn migration_refuses_to_discard_ambient_authority() {
        for text in [
            "schema_version = 1\n[realm_sandbox]\nnetwork = true\n",
            "schema_version = 1\n[realm_sandbox]\nreadable_paths = [\"/srv\"]\n",
            "schema_version = 1\n[[realm_sandbox.app]]\ndesktop_id = \"browser.desktop\"\nwritable_paths = [\"/tmp\"]\n",
        ] {
            let diagnostics = migrate_text(text).unwrap_err();
            assert!(
                diagnostics
                    .iter()
                    .any(|diagnostic| diagnostic.message.contains("cannot be migrated"))
            );
        }
    }

    #[test]
    fn file_migration_keeps_an_owner_only_backup() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.toml");
        std::fs::write(&path, "schema_version = 1\n[ui]\nreduced_motion = true\n").unwrap();
        let outcome = migrate_file(&path).unwrap();
        let MigrationOutcome::Migrated { backup, .. } = outcome else {
            panic!("expected migration");
        };
        assert_eq!(
            std::fs::read_to_string(&backup).unwrap(),
            "schema_version = 1\n[ui]\nreduced_motion = true\n"
        );
        assert_eq!(std::fs::metadata(&backup).unwrap().mode() & 0o777, 0o600);
        Config::parse(&std::fs::read_to_string(path).unwrap()).unwrap();
    }
}
