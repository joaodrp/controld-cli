//! The persisted configuration: `$XDG_CONFIG_HOME/cdctl/config.toml` (D6).
//!
//! Context-keyed from the start so org contexts (D15) slot in without a
//! migration. The token is stored in plaintext — the `0600` file mode is the
//! guard, and `secrecy` protects logs and debug output, not the disk. Writes
//! go through a same-directory tempfile and an atomic rename; a symlinked
//! config file is refused; a group/world-readable one draws a warning.

use std::collections::BTreeMap;
use std::fmt;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use etcetera::BaseStrategy;
use secrecy::{ExposeSecret, SecretString};
use serde::Deserialize;

use crate::error::Error;

pub const DEFAULT_CONTEXT: &str = "personal";
pub const TOKEN_ENV_VAR: &str = "CONTROLD_API_TOKEN";

/// Deliberately not `Serialize`: a config holds the token, and the only
/// writer allowed to see it is [`merge_into_document`] inside
/// [`Store::save`] — no output path can print a `Config` by accident.
#[derive(Debug, Default, Deserialize)]
pub struct Config {
    pub current_context: Option<String>,
    #[serde(default)]
    pub contexts: BTreeMap<String, Context>,
}

#[derive(Debug, Default, Deserialize)]
pub struct Context {
    pub token: Option<SecretString>,
    pub default_profile: Option<String>,
    /// Reserved for org contexts (D15); carried verbatim, unused in v1.
    pub org: Option<String>,
}

/// The known fields merged into the on-disk TOML document — the single place
/// the token deliberately leaves [`secrecy`]'s guard; that is what the 0600
/// file is for. Merging (rather than serializing a struct from scratch)
/// keeps hand-written comments and keys a newer cdctl wrote: the file is an
/// interface, and this version's writes must not destroy what its reads
/// already tolerate ([`tests::unknown_keys_are_tolerated`]).
fn merge_into_document(config: &Config, doc: &mut toml_edit::DocumentMut) {
    use toml_edit::{Item, Table, TableLike};

    fn set_or_remove(table: &mut dyn TableLike, key: &str, value: Option<&str>) {
        match value {
            Some(value) => {
                table.insert(key, toml_edit::value(value));
            }
            None => {
                table.remove(key);
            }
        }
    }

    /// The item as an editable table: standard and inline tables (both legal
    /// on disk) are edited in place. Anything else — this document is an
    /// independent re-read, so `load`'s shape checks say nothing about it —
    /// gets the same posture as an unparseable file: rebuilt from scratch.
    fn ensure_table(item: &mut Item, implicit: bool) -> &mut dyn TableLike {
        if !item.is_table_like() {
            let mut table = Table::new();
            // A bare `[contexts]` header would be noise — only the
            // per-context `[contexts.<name>]` sections should appear.
            table.set_implicit(implicit);
            *item = Item::Table(table);
        }
        item.as_table_like_mut().expect("just made table-like")
    }

    set_or_remove(
        doc.as_table_mut(),
        "current_context",
        config.current_context.as_deref(),
    );

    if config.contexts.is_empty() {
        return;
    }
    if !doc.contains_key("contexts") {
        let mut implicit = Table::new();
        implicit.set_implicit(true);
        doc.insert("contexts", Item::Table(implicit));
    }
    let contexts = ensure_table(doc.get_mut("contexts").expect("just inserted"), true);
    for (name, context) in &config.contexts {
        if contexts.get(name).is_none() {
            contexts.insert(name, Item::Table(Table::new()));
        }
        let table = ensure_table(contexts.get_mut(name).expect("just inserted"), false);
        set_or_remove(
            table,
            "token",
            context.token.as_ref().map(ExposeSecret::expose_secret),
        );
        set_or_remove(table, "default_profile", context.default_profile.as_deref());
        set_or_remove(table, "org", context.org.as_deref());
    }
}

impl Config {
    pub fn current_context_name(&self) -> &str {
        self.current_context.as_deref().unwrap_or(DEFAULT_CONTEXT)
    }

    pub fn active_context(&self) -> Option<&Context> {
        self.contexts.get(self.current_context_name())
    }

    pub fn active_context_mut(&mut self) -> &mut Context {
        let name = self.current_context_name().to_owned();
        self.contexts.entry(name).or_default()
    }

    /// The active context's `default_profile`, or `None` if unset (D8).
    pub fn default_profile(&self) -> Option<&str> {
        self.active_context()
            .and_then(|c| c.default_profile.as_deref())
    }
}

/// A loaded config plus non-fatal findings (e.g. lax file permissions) the
/// caller surfaces on stderr.
#[derive(Debug)]
pub struct Loaded {
    pub config: Config,
    pub warnings: Vec<String>,
}

/// The config file's location and I/O. Discovery reads the environment once;
/// tests construct stores at explicit paths.
#[derive(Debug, Clone)]
pub struct Store {
    path: PathBuf,
}

impl Store {
    pub fn discover() -> Result<Self, Error> {
        let strategy = etcetera::choose_base_strategy().map_err(|e| {
            Error::config_invalid(format!("could not locate a config directory: {e}"))
        })?;
        Ok(Self {
            path: strategy.config_dir().join("cdctl").join("config.toml"),
        })
    }

    #[cfg(test)]
    pub fn at(path: PathBuf) -> Self {
        Self { path }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn load(&self) -> Result<Loaded, Error> {
        let metadata = match std::fs::symlink_metadata(&self.path) {
            Ok(metadata) => metadata,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                return Ok(Loaded {
                    config: Config::default(),
                    warnings: Vec::new(),
                });
            }
            Err(e) => {
                return Err(Error::config_invalid(format!(
                    "could not read {}: {e}",
                    self.path.display()
                )));
            }
        };

        // A symlink invites token exfiltration by link swap; the real file
        // belongs where the docs say it lives.
        if metadata.file_type().is_symlink() {
            return Err(Error::config_invalid(format!(
                "{} is a symlink; refusing to read a config file that points elsewhere",
                self.path.display()
            )));
        }

        let mut warnings = Vec::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let mode = metadata.mode() & 0o777;
            if mode & 0o077 != 0 {
                warnings.push(format!(
                    "{} is group/world-accessible (mode {mode:03o}); it holds the API token — run: chmod 600 {}",
                    self.path.display(),
                    self.path.display()
                ));
            }
        }

        let raw = std::fs::read_to_string(&self.path).map_err(|e| {
            Error::config_invalid(format!("could not read {}: {e}", self.path.display()))
        })?;
        let config = toml::from_str(&raw).map_err(|e| {
            Error::config_invalid(format!("{} is not valid: {e}", self.path.display()))
        })?;
        Ok(Loaded { config, warnings })
    }

    pub fn save(&self, config: &Config) -> Result<(), Error> {
        let dir = self.path.parent().unwrap_or_else(|| Path::new("."));
        create_private_dir(dir)?;

        // Merge into the existing document so comments and unknown keys
        // survive. A missing or unparseable file starts fresh — every save
        // in a command follows a successful `load`, so unparseable here
        // means the file changed underneath us and a full rewrite matches
        // the old behavior.
        let mut doc = std::fs::read_to_string(&self.path)
            .ok()
            .and_then(|raw| raw.parse::<toml_edit::DocumentMut>().ok())
            .unwrap_or_default();
        merge_into_document(config, &mut doc);
        let serialized = doc.to_string();

        // Same-directory tempfile (0600 on Unix by construction) + atomic
        // rename: readers see the old file or the new one, never a torn write.
        let mut tempfile = tempfile::Builder::new()
            .prefix(".config-")
            .suffix(".tmp")
            .tempfile_in(dir)
            .map_err(|e| {
                Error::config_invalid(format!("could not stage {}: {e}", self.path.display()))
            })?;
        tempfile
            .write_all(serialized.as_bytes())
            .and_then(|()| tempfile.as_file().sync_all())
            .map_err(|e| {
                Error::config_invalid(format!("could not write {}: {e}", self.path.display()))
            })?;
        tempfile.persist(&self.path).map_err(|e| {
            Error::config_invalid(format!("could not replace {}: {e}", self.path.display()))
        })?;
        Ok(())
    }
}

#[cfg(unix)]
fn create_private_dir(dir: &Path) -> Result<(), Error> {
    use std::os::unix::fs::DirBuilderExt;
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)
        .map_err(|e| Error::config_invalid(format!("could not create {}: {e}", dir.display())))
}

#[cfg(not(unix))]
fn create_private_dir(dir: &Path) -> Result<(), Error> {
    // Windows relies on %APPDATA%'s user-scoped ACLs; no mode bits exist (D6).
    std::fs::create_dir_all(dir)
        .map_err(|e| Error::config_invalid(format!("could not create {}: {e}", dir.display())))
}

/// Where the effective token came from; reported by `auth status`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenSource {
    Env,
    Config,
}

impl fmt::Display for TokenSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Env => "env",
            Self::Config => "config",
        })
    }
}

#[derive(Debug)]
pub struct ResolvedToken {
    pub token: SecretString,
    pub source: TokenSource,
}

/// One policy for every `CONTROLD_*` read: present -> the value, absent or
/// empty -> `None`, set-but-not-UTF-8 -> usage error. A mis-set variable must
/// never silently mean "unset" — a garbled token would fall through to a
/// *different* stored token and operate on the wrong account.
pub fn env_var(name: &str) -> Result<Option<String>, Error> {
    match std::env::var(name) {
        Ok(value) if value.is_empty() => Ok(None),
        Ok(value) => Ok(Some(value)),
        Err(std::env::VarError::NotPresent) => Ok(None),
        Err(std::env::VarError::NotUnicode(_)) => Err(Error::usage(format!(
            "{name} is set but is not valid UTF-8"
        ))),
    }
}

/// D6 precedence: `CONTROLD_API_TOKEN` beats the active context's stored
/// token. `None` is not an error here — auth is resolved lazily (D7), so only
/// handlers that make requests turn it into `auth.missing_token`.
///
/// `env_token` must be an [`env_var`]-policy value: both production callers
/// pass its output straight through, so `Some("")` never reaches here —
/// empty-means-unset is already enforced there, once, rather than re-checked
/// on every read.
pub fn resolve_token(env_token: Option<String>, config: &Config) -> Option<ResolvedToken> {
    if let Some(token) = env_token {
        return Some(ResolvedToken {
            token: SecretString::from(token),
            source: TokenSource::Env,
        });
    }
    config
        .active_context()
        .and_then(|context| context.token.as_ref())
        .map(|token| ResolvedToken {
            token: token.clone(),
            source: TokenSource::Config,
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store_in(dir: &Path) -> Store {
        Store::at(dir.join("cdctl").join("config.toml"))
    }

    fn config_with_token(token: &str) -> Config {
        let mut config = Config::default();
        config.active_context_mut().token = Some(SecretString::from(token));
        config
    }

    #[test]
    fn roundtrips_through_disk() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = store_in(dir.path());

        let mut config = config_with_token("api.secret123");
        config.active_context_mut().default_profile = Some("Home".into());
        store.save(&config).expect("save");

        let loaded = store.load().expect("load");
        assert!(loaded.warnings.is_empty());
        let context = loaded.config.active_context().expect("active context");
        assert_eq!(
            context.token.as_ref().expect("token").expose_secret(),
            "api.secret123"
        );
        assert_eq!(context.default_profile.as_deref(), Some("Home"));
    }

    #[test]
    fn missing_file_loads_the_default() {
        let dir = tempfile::tempdir().expect("tempdir");
        let loaded = store_in(dir.path()).load().expect("load");
        assert!(loaded.config.contexts.is_empty());
        assert_eq!(loaded.config.current_context_name(), DEFAULT_CONTEXT);
    }

    #[cfg(unix)]
    #[test]
    fn file_is_0600_and_dir_is_0700() {
        use std::os::unix::fs::MetadataExt;
        let dir = tempfile::tempdir().expect("tempdir");
        let store = store_in(dir.path());
        store.save(&config_with_token("api.x")).expect("save");

        let file_mode = std::fs::metadata(store.path()).expect("file").mode() & 0o777;
        assert_eq!(file_mode, 0o600, "config file must be private");
        let dir_mode = std::fs::metadata(store.path().parent().expect("parent"))
            .expect("dir")
            .mode()
            & 0o777;
        assert_eq!(dir_mode, 0o700, "config dir must be private");
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_config_is_refused() {
        let dir = tempfile::tempdir().expect("tempdir");
        let real = dir.path().join("elsewhere.toml");
        std::fs::write(&real, "").expect("write target");
        let store = store_in(dir.path());
        std::fs::create_dir_all(store.path().parent().expect("parent")).expect("mkdir");
        std::os::unix::fs::symlink(&real, store.path()).expect("symlink");

        let error = store.load().expect_err("symlink refused");
        assert!(error.message.contains("symlink"), "got: {}", error.message);
    }

    #[cfg(unix)]
    #[test]
    fn lax_permissions_draw_a_warning() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().expect("tempdir");
        let store = store_in(dir.path());
        store.save(&Config::default()).expect("save");
        std::fs::set_permissions(store.path(), std::fs::Permissions::from_mode(0o644))
            .expect("chmod");

        let loaded = store.load().expect("still loads");
        assert_eq!(loaded.warnings.len(), 1);
        assert!(loaded.warnings[0].contains("chmod 600"));
    }

    #[test]
    fn save_replaces_atomically() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = store_in(dir.path());
        store
            .save(&config_with_token("api.first"))
            .expect("first save");
        store
            .save(&config_with_token("api.second"))
            .expect("second save");

        let raw = std::fs::read_to_string(store.path()).expect("readable");
        assert!(raw.contains("api.second"));
        assert!(!raw.contains("api.first"));
        let staging_leftovers = std::fs::read_dir(store.path().parent().expect("parent"))
            .expect("dir")
            .filter_map(Result::ok)
            .filter(|e| e.file_name().to_string_lossy().ends_with(".tmp"))
            .count();
        assert_eq!(staging_leftovers, 0, "no staging files survive a save");
    }

    #[test]
    fn unknown_keys_are_tolerated() {
        // Forward compatibility: a newer cdctl may have written fields this
        // version does not know (the org context, D15).
        let dir = tempfile::tempdir().expect("tempdir");
        let store = store_in(dir.path());
        std::fs::create_dir_all(store.path().parent().expect("parent")).expect("mkdir");
        std::fs::write(
            store.path(),
            "future_top_level = 1\n\n[contexts.acme]\ntoken = \"api.x\"\norg = \"o1\"\nfuture_key = true\n",
        )
        .expect("write");

        let loaded = store.load().expect("lenient load");
        assert_eq!(loaded.config.contexts["acme"].org.as_deref(), Some("o1"));
    }

    #[test]
    fn save_preserves_comments_and_unknown_keys() {
        // The config file is an interface: keys a newer cdctl wrote and
        // comments a person left must survive this version's writes, not
        // just its reads.
        let dir = tempfile::tempdir().expect("tempdir");
        let store = store_in(dir.path());
        std::fs::create_dir_all(store.path().parent().expect("parent")).expect("mkdir");
        std::fs::write(
            store.path(),
            "# hand-written note\nfuture_top_level = 1\n\n[contexts.personal]\ntoken = \"api.x\"\nfuture_key = true\n",
        )
        .expect("write");

        let mut config = store.load().expect("load").config;
        config.active_context_mut().default_profile = Some("Home".into());
        store.save(&config).expect("save");

        let raw = std::fs::read_to_string(store.path()).expect("readable");
        for kept in [
            "# hand-written note",
            "future_top_level = 1",
            "future_key = true",
            "token = \"api.x\"",
        ] {
            assert!(raw.contains(kept), "{kept:?} must survive a save:\n{raw}");
        }
        assert!(
            raw.contains("default_profile = \"Home\""),
            "the mutation lands:\n{raw}"
        );
    }

    #[test]
    fn save_accepts_inline_table_contexts() {
        // Inline tables are legal TOML and `load` accepts them, so a save
        // must edit them in place, not panic. Unknown keys inside survive.
        let dir = tempfile::tempdir().expect("tempdir");
        let store = store_in(dir.path());
        std::fs::create_dir_all(store.path().parent().expect("parent")).expect("mkdir");
        for raw in [
            "contexts = { personal = { token = \"api.x\", future_key = true } }\n",
            "[contexts]\npersonal = { token = \"api.x\", future_key = true }\n",
        ] {
            std::fs::write(store.path(), raw).expect("write");
            let mut config = store.load().expect("load accepts inline tables").config;
            config.active_context_mut().default_profile = Some("Home".into());
            store
                .save(&config)
                .expect("save edits inline tables in place");

            let raw_after = std::fs::read_to_string(store.path()).expect("readable");
            assert!(
                raw_after.contains("future_key = true"),
                "kept:\n{raw_after}"
            );
            assert!(
                raw_after.contains("default_profile = \"Home\""),
                "the mutation lands:\n{raw_after}"
            );
            let reloaded = store.load().expect("reload").config;
            assert_eq!(reloaded.default_profile(), Some("Home"));
        }
    }

    #[test]
    fn save_replaces_a_non_table_contexts_key() {
        // `contexts = 1` never survives a `load`, but `save` re-reads the
        // file independently, so a file changed underneath us must get the
        // same rebuild-it posture as an unparseable one - never a panic.
        let dir = tempfile::tempdir().expect("tempdir");
        let store = store_in(dir.path());
        std::fs::create_dir_all(store.path().parent().expect("parent")).expect("mkdir");
        for raw in [
            "contexts = 1\n",
            "[contexts]\npersonal = 1\n",
            "contexts = \"nope\"\n",
        ] {
            std::fs::write(store.path(), raw).expect("write");
            store
                .save(&config_with_token("api.rebuilt"))
                .expect("save rebuilds a wrong-shape key");
            let reloaded = store.load().expect("the result is well-formed").config;
            assert_eq!(
                reloaded
                    .active_context()
                    .and_then(|c| c.token.as_ref())
                    .map(|t| t.expose_secret().to_owned()),
                Some("api.rebuilt".to_owned())
            );
        }
    }

    #[test]
    fn save_removes_a_cleared_known_key() {
        // Logout sets the token to None: the merge must delete the key, not
        // leave the old secret on disk.
        let dir = tempfile::tempdir().expect("tempdir");
        let store = store_in(dir.path());
        store
            .save(&config_with_token("api.doomed"))
            .expect("first save");

        let mut config = store.load().expect("load").config;
        config.active_context_mut().token = None;
        store.save(&config).expect("save");

        let raw = std::fs::read_to_string(store.path()).expect("readable");
        assert!(!raw.contains("api.doomed"), "cleared token is gone:\n{raw}");
    }

    #[test]
    fn env_token_beats_config() {
        let config = config_with_token("api.from-config");
        let resolved = resolve_token(Some("api.from-env".into()), &config).expect("env wins");
        assert_eq!(resolved.source, TokenSource::Env);
        assert_eq!(resolved.token.expose_secret(), "api.from-env");
    }

    #[test]
    fn no_env_token_falls_back_to_config() {
        // `resolve_token`'s precondition: an unset env token arrives as
        // `None` (`env_var` already turned an empty `CONTROLD_API_TOKEN`
        // into `None` before this ever runs), not as `Some("")`.
        let config = config_with_token("api.from-config");
        let resolved = resolve_token(None, &config).expect("falls back");
        assert_eq!(resolved.source, TokenSource::Config);
        assert_eq!(resolved.token.expose_secret(), "api.from-config");
    }

    #[test]
    fn a_missing_token_resolves_to_none_not_an_error() {
        assert!(
            resolve_token(None, &Config::default()).is_none(),
            "lazy auth (D7): absence surfaces at first use, not at startup"
        );
    }

    #[test]
    fn contexts_are_keyed_and_switchable() {
        let mut config = Config::default();
        config.active_context_mut().token = Some(SecretString::from("api.personal"));
        config.current_context = Some("acme".into());
        config.active_context_mut().token = Some(SecretString::from("api.acme"));

        assert_eq!(config.contexts.len(), 2);
        let resolved = resolve_token(None, &config).expect("acme token");
        assert_eq!(resolved.token.expose_secret(), "api.acme");
    }
}
