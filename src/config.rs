use std::fs;
use std::path::{Path, PathBuf};
use toml_edit::{DocumentMut, Item, Table};

// ---------------------------------------------------------------------------
// Error

#[derive(Debug)]
pub enum Error {
    Io(std::io::Error),
    Parse(toml_edit::TomlError),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Io(e) => write!(f, "I/O error: {e}"),
            Error::Parse(e) => write!(f, "TOML parse error: {e}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Io(e)
    }
}

impl From<toml_edit::TomlError> for Error {
    fn from(e: toml_edit::TomlError) -> Self {
        Error::Parse(e)
    }
}

// ---------------------------------------------------------------------------
// NumberRepr

/// Controls whether whole-number values are written as TOML integers or floats.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NumberRepr {
    WholeAsInteger,
    AlwaysFloat,
}

impl NumberRepr {
    /// Integer `step` → integer literals; fractional `step` → float literals.
    pub fn from_step(step: f64) -> Self {
        if step.fract() == 0.0 {
            NumberRepr::WholeAsInteger
        } else {
            NumberRepr::AlwaysFloat
        }
    }

    /// No `.` in any option string → integer literals; otherwise floats.
    pub fn from_options(options: &[String]) -> Self {
        if options.iter().all(|o| !o.contains('.')) {
            NumberRepr::WholeAsInteger
        } else {
            NumberRepr::AlwaysFloat
        }
    }
}

/// Split `array[index].key` into its three parts. Plain dotted paths return `None`.
fn split_index(path: &str) -> Option<(&str, usize, &str)> {
    let open = path.find('[')?;
    let close = path.find(']')?;
    if close < open {
        return None;
    }
    let index: usize = path[open + 1..close].parse().ok()?;
    let key = path[close + 1..].strip_prefix('.')?;
    if key.is_empty() || key.contains('[') {
        return None;
    }
    Some((&path[..open], index, key))
}

fn number_item(value: f64, repr: NumberRepr) -> Item {
    match repr {
        NumberRepr::WholeAsInteger if value.fract() == 0.0 && value.is_finite() => {
            toml_edit::value(value as i64)
        }
        _ => toml_edit::value(value),
    }
}

// ---------------------------------------------------------------------------
// ConfigStore

pub struct ConfigStore {
    doc: DocumentMut,
    path: PathBuf,
    /// Set whenever `set_*()` or `remove()` mutates the in-memory state.
    /// Cleared by `save()` and `take_dirty()`.
    dirty: bool,
}

impl ConfigStore {
    /// Load a TOML config file from `path`, preserving all formatting and comments.
    pub fn load<P: AsRef<Path>>(path: P) -> Result<Self, Error> {
        let src = fs::read_to_string(path.as_ref())?;
        let doc: DocumentMut = src.parse()?;
        Ok(Self {
            doc,
            path: path.as_ref().to_path_buf(),
            dirty: false,
        })
    }

    /// Write the current state back to disk using an atomic rename.
    /// Original formatting and comments are preserved.
    /// Clears the dirty flag on success.
    pub fn save(&mut self) -> Result<(), Error> {
        let s = self.doc.to_string();
        let tmp = self.path.with_extension("toml.tmp");
        fs::write(&tmp, s.as_bytes())?;
        fs::rename(&tmp, &self.path)?;
        self.dirty = false;
        Ok(())
    }

    /// Returns `true` if unsaved changes exist, then clears the flag.
    #[allow(dead_code)] // retained for callers that peek-and-clear; auto-save uses `is_dirty`.
    pub fn take_dirty(&mut self) -> bool {
        let d = self.dirty;
        self.dirty = false;
        d
    }

    /// Returns `true` if unsaved changes exist, without clearing the flag.
    /// Used to enable/disable the "Apply" button.
    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    // -----------------------------------------------------------------------
    // Internal navigation

    fn get_item(&self, path: &str) -> Option<&Item> {
        let mut parts = path.split('.');
        let first = parts.next()?;
        let first_item = self.doc.get(first)?;
        parts.try_fold(first_item, |item, key| item.as_table_like()?.get(key))
    }

    /// Recursively descend through `table` along `keys`, creating any missing
    /// intermediate tables. Returns `None` if a non-table node is encountered.
    fn descend_or_create<'a>(table: &'a mut Table, keys: &[&str]) -> Option<&'a mut Table> {
        match keys {
            [] => Some(table),
            [key, rest @ ..] => {
                if !table.contains_key(*key) {
                    table.insert(*key, Item::Table(Table::new()));
                }
                let child = table.get_mut(*key)?.as_table_mut()?;
                Self::descend_or_create(child, rest)
            }
        }
    }

    /// Recursively descend through `table` along `keys` without creating anything.
    fn descend<'a>(table: &'a mut Table, keys: &[&str]) -> Option<&'a mut Table> {
        match keys {
            [] => Some(table),
            [key, rest @ ..] => {
                let child = table.get_mut(*key)?.as_table_mut()?;
                Self::descend(child, rest)
            }
        }
    }

    fn set_item(&mut self, path: &str, new_val: Item) -> bool {
        let parts: Vec<&str> = path.split('.').collect();
        let Some((last, parents)) = parts.split_last() else {
            return false;
        };
        let root = self.doc.as_table_mut();
        let Some(parent) = Self::descend_or_create(root, parents) else {
            return false;
        };
        parent.insert(last, new_val);
        self.dirty = true;
        true
    }

    // -----------------------------------------------------------------------
    // Public read API

    /// Get a string value by dot-separated path.
    pub fn get_str<'a>(&'a self, path: &str) -> Option<&'a str> {
        if let Some((array, index, key)) = split_index(path) {
            return self.record_value(array, index, key).and_then(|v| v.as_str());
        }
        self.get_item(path)?.as_str()
    }

    /// Get a boolean value by dot-separated path.
    pub fn get_bool(&self, path: &str) -> Option<bool> {
        self.get_item(path)?.as_bool()
    }

    /// Get a numeric (float or integer) value by dot-separated path.
    pub fn get_number(&self, path: &str) -> Option<f64> {
        if let Some((array, index, key)) = split_index(path) {
            let value = self.record_value(array, index, key)?;
            return value
                .as_float()
                .or_else(|| value.as_integer().map(|i| i as f64));
        }
        let item = self.get_item(path)?;
        item.as_float().or_else(|| item.as_integer().map(|i| i as f64))
    }

    /// Number of records in the array at `path`. Missing arrays have length 0.
    pub fn record_len(&self, path: &str) -> usize {
        self.get_item(path)
            .and_then(|item| item.as_array())
            .map(|array| array.len())
            .unwrap_or(0)
    }

    /// Append an empty record. Creates the array when it is absent.
    pub fn record_push(&mut self, path: &str) -> Option<usize> {
        self.ensure_array(path)?;
        let len = {
            let array = self.get_item_mut(path)?.as_array_mut()?;
            array.push(toml_edit::InlineTable::new());
            array.len()
        };
        self.dirty = true;
        Some(len - 1)
    }

    /// Remove the record at `index`.
    pub fn record_remove(&mut self, path: &str, index: usize) -> bool {
        let removed = {
            let Some(array) = self.get_item_mut(path).and_then(|item| item.as_array_mut()) else {
                return false;
            };
            if index >= array.len() {
                return false;
            }
            array.remove(index);
            true
        };
        if removed {
            self.dirty = true;
        }
        removed
    }

    /// Move the record at `from` so that it occupies `to`.
    pub fn record_move(&mut self, path: &str, from: usize, to: usize) -> bool {
        if from == to {
            return false;
        }
        let moved = {
            let Some(array) = self.get_item_mut(path).and_then(|item| item.as_array_mut()) else {
                return false;
            };
            if from >= array.len() || to >= array.len() {
                return false;
            }
            let value = array.remove(from);
            array.insert(to, value);
            true
        };
        if moved {
            self.dirty = true;
        }
        moved
    }

    fn record_value(&self, array: &str, index: usize, key: &str) -> Option<&toml_edit::Value> {
        self.get_item(array)?
            .as_array()?
            .get(index)?
            .as_inline_table()?
            .get(key)
    }

    fn ensure_array(&mut self, path: &str) -> Option<()> {
        if self.get_item(path).and_then(|item| item.as_array()).is_some() {
            return Some(());
        }
        self.set_item(path, toml_edit::Item::Value(toml_edit::Value::Array(toml_edit::Array::new())))
            .then_some(())
    }

    fn set_record_value(&mut self, array: &str, index: usize, key: &str, value: toml_edit::Value) -> bool {
        if self.ensure_array(array).is_none() {
            return false;
        }
        let wrote = {
            let Some(list) = self.get_item_mut(array).and_then(|item| item.as_array_mut()) else {
                return false;
            };
            while list.len() <= index {
                list.push(toml_edit::InlineTable::new());
            }
            let Some(toml_edit::Value::InlineTable(table)) = list.get_mut(index) else {
                return false;
            };
            table.insert(key, value);
            true
        };
        if wrote {
            self.dirty = true;
        }
        wrote
    }

    fn get_item_mut(&mut self, path: &str) -> Option<&mut Item> {
        let mut parts = path.split('.');
        let first = parts.next()?;
        let mut item = self.doc.get_mut(first)?;
        for key in parts {
            item = item.as_table_like_mut()?.get_mut(key)?;
        }
        Some(item)
    }

    // -----------------------------------------------------------------------
    // Public write API

    /// Set a string value by dot-separated path, creating intermediate tables as needed.
    pub fn set_str(&mut self, path: &str, value: &str) -> bool {
        if let Some((array, index, key)) = split_index(path) {
            return self.set_record_value(array, index, key, toml_edit::Value::from(value));
        }
        self.set_item(path, toml_edit::value(value))
    }

    /// Set a boolean value by dot-separated path, creating intermediate tables as needed.
    pub fn set_bool(&mut self, path: &str, value: bool) -> bool {
        self.set_item(path, toml_edit::value(value))
    }

    /// Set a numeric value by dot-separated path, creating intermediate tables as needed.
    /// Writes a float literal unless `repr` requests integers for whole values.
    pub fn set_number(&mut self, path: &str, value: f64, repr: NumberRepr) -> bool {
        if let Some((array, index, key)) = split_index(path) {
            let Some(toml_edit::Item::Value(stored)) = Some(number_item(value, repr)) else {
                return false;
            };
            return self.set_record_value(array, index, key, stored);
        }
        self.set_item(path, number_item(value, repr))
    }

    /// Insert an empty table at `path`, creating intermediate tables as needed.
    /// Used when adding a new section.
    pub fn set_table(&mut self, path: &str) -> bool {
        self.set_item(path, Item::Table(Table::new()))
    }

    // -----------------------------------------------------------------------
    // Other public API

    /// Return the filesystem path of the config file.
    pub fn path(&self) -> &std::path::Path {
        &self.path
    }

    /// Discard in-memory state and reload from disk.
    /// Clears the dirty flag on success.
    pub fn reload(&mut self) -> Result<(), Error> {
        let src = std::fs::read_to_string(&self.path)?;
        self.doc = src.parse()?;
        self.dirty = false;
        Ok(())
    }

    /// Return the immediate child keys of the table at `path`.
    /// Used to enumerate sub-sections for `section_map` tabs.
    pub fn section_keys(&self, path: &str) -> Vec<String> {
        self.child_keys(path)
    }

    /// Immediate child keys of the table at `path` (`""` = document root).
    pub fn child_keys(&self, path: &str) -> Vec<String> {
        if path.is_empty() {
            return self
                .doc
                .as_table()
                .iter()
                .map(|(k, _)| k.to_string())
                .collect();
        }
        self.get_item(path)
            .and_then(|item| item.as_table_like())
            .map(|t| t.iter().map(|(k, _)| k.to_string()).collect())
            .unwrap_or_default()
    }

    /// Remove a key by dot-separated path.
    /// Returns `true` if the key existed and was removed.
    pub fn remove(&mut self, path: &str) -> bool {
        let parts: Vec<&str> = path.split('.').collect();
        let Some((last, parents)) = parts.split_last() else {
            return false;
        };
        let root = self.doc.as_table_mut();
        let Some(parent) = Self::descend(root, parents) else {
            return false;
        };
        let removed = parent.remove(last).is_some();
        if removed {
            self.dirty = true;
        }
        removed
    }

    // -----------------------------------------------------------------------
    // Migration support (low-level moves + version tracking)

    /// Returns `true` if a value or table exists at `path`.
    pub fn contains(&self, path: &str) -> bool {
        self.get_item(path).is_some()
    }

    /// Read the migration version recorded at `key`. Returns `None` when the key
    /// is absent or is not a non-negative integer.
    pub fn get_version(&self, key: &str) -> Option<u32> {
        self.get_item(key)?
            .as_integer()
            .and_then(|i| u32::try_from(i).ok())
    }

    /// Record the migration version at `key`, written as a TOML integer.
    pub fn set_version(&mut self, key: &str, version: u32) -> bool {
        self.set_item(key, toml_edit::value(i64::from(version)))
    }

    /// Remove the item at `path` and return it, if present. Does not create
    /// intermediate tables.
    fn take_item(&mut self, path: &str) -> Option<Item> {
        let parts: Vec<&str> = path.split('.').collect();
        let (last, parents) = parts.split_last()?;
        let root = self.doc.as_table_mut();
        let parent = Self::descend(root, parents)?;
        let removed = parent.remove(last);
        if removed.is_some() {
            self.dirty = true;
        }
        removed
    }

    /// Remove now-empty ancestor tables of `path`, bottom-up, stopping at the
    /// first non-empty ancestor and never touching the document root. Explicit
    /// tables (e.g. `[display]`) otherwise linger after their keys are migrated
    /// away, so this keeps the file tidy.
    fn prune_empty_parents(&mut self, path: &str) {
        let parts: Vec<&str> = path.split('.').collect();
        for depth in (1..parts.len()).rev() {
            let parent_path = &parts[..depth];
            let root = self.doc.as_table_mut();
            let Some(table) = Self::descend(root, parent_path) else {
                break;
            };
            if !table.is_empty() {
                break;
            }
            let (last, grand) = parent_path.split_last().unwrap();
            let root = self.doc.as_table_mut();
            if let Some(gp) = Self::descend(root, grand)
                && gp.remove(last).is_some()
            {
                self.dirty = true;
            }
        }
    }

    /// Move the value at `from` to `to`, preserving the value. No-op (returns
    /// `false`) when `from` is absent, `to` already exists, or the destination
    /// path passes through a non-table. Ancestor tables emptied by the move are
    /// pruned.
    pub fn rename_key(&mut self, from: &str, to: &str) -> bool {
        if from == to || self.contains(to) {
            return false;
        }
        let Some(item) = self.take_item(from) else {
            return false;
        };
        if !self.set_item(to, item.clone()) {
            // Destination unusable — reinsert so the value is never lost.
            self.set_item(from, item);
            return false;
        }
        self.prune_empty_parents(from);
        true
    }

    /// Remove the key at `path` and prune any ancestor tables it emptied.
    pub fn delete_key(&mut self, path: &str) -> bool {
        let removed = self.remove(path);
        if removed {
            self.prune_empty_parents(path);
        }
        removed
    }

    /// Convert an enum-like string to a bool: writes `true` at `to` when the
    /// value at `from` equals `match_value`, otherwise `false`, then removes
    /// `from`. No-op (returns `false`) when `from` is absent or `to` exists.
    pub fn transform_enum_to_bool(
        &mut self,
        from: &str,
        to: &str,
        match_value: Option<&str>,
    ) -> bool {
        if self.contains(to) {
            return false;
        }
        let Some(current) = self.get_str(from).map(str::to_owned) else {
            return false;
        };
        let value = match_value == Some(current.as_str());
        self.set_bool(to, value);
        self.remove(from);
        self.prune_empty_parents(from);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn number_repr_from_step() {
        assert_eq!(NumberRepr::from_step(1.0), NumberRepr::WholeAsInteger);
        assert_eq!(NumberRepr::from_step(0.5), NumberRepr::AlwaysFloat);
    }

    #[test]
    fn number_repr_from_options() {
        assert_eq!(
            NumberRepr::from_options(&["0".into(), "1".into(), "5".into()]),
            NumberRepr::WholeAsInteger
        );
        assert_eq!(
            NumberRepr::from_options(&["0.5".into(), "1.0".into()]),
            NumberRepr::AlwaysFloat
        );
    }

    fn store_from(name: &str, src: &str) -> (ConfigStore, std::path::PathBuf) {
        let path = std::env::temp_dir().join(format!(
            "settings_migrate_{}_{}.toml",
            name,
            std::process::id()
        ));
        std::fs::write(&path, src).unwrap();
        (ConfigStore::load(&path).unwrap(), path)
    }

    #[test]
    fn rename_key_moves_value_and_prunes_empty_parent() {
        let (mut store, path) = store_from(
            "rename",
            "[display]\nfont_size = 14\n",
        );
        assert!(store.rename_key("display.font_size", "general.font_size"));
        assert_eq!(store.get_number("general.font_size"), Some(14.0));
        assert!(!store.contains("display.font_size"));
        // The now-empty [display] table is pruned.
        assert!(!store.contains("display"));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn rename_key_skips_when_destination_exists() {
        let (mut store, path) = store_from(
            "rename_skip",
            "[display]\nfont_size = 14\n\n[general]\nfont_size = 20\n",
        );
        assert!(!store.rename_key("display.font_size", "general.font_size"));
        // Existing destination is preserved; source is left intact.
        assert_eq!(store.get_number("general.font_size"), Some(20.0));
        assert_eq!(store.get_number("display.font_size"), Some(14.0));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn rename_key_noop_when_source_absent() {
        let (mut store, path) = store_from("rename_absent", "[general]\nx = 1\n");
        assert!(!store.rename_key("display.font_size", "general.font_size"));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn delete_key_removes_and_prunes() {
        let (mut store, path) = store_from("delete", "[display]\ntick_rate = 60\n");
        assert!(store.delete_key("display.tick_rate"));
        assert!(!store.contains("display.tick_rate"));
        assert!(!store.contains("display"));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn transform_enum_to_bool_matches_true() {
        let (mut store, path) = store_from("enum_true", "mode = \"vivarium\"\n");
        assert!(store.transform_enum_to_bool("mode", "vivarium.enabled", Some("vivarium")));
        assert_eq!(store.get_bool("vivarium.enabled"), Some(true));
        assert!(!store.contains("mode"));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn transform_enum_to_bool_non_match_false() {
        let (mut store, path) = store_from("enum_false", "mode = \"free\"\n");
        assert!(store.transform_enum_to_bool("mode", "vivarium.enabled", Some("vivarium")));
        assert_eq!(store.get_bool("vivarium.enabled"), Some(false));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn version_get_set_roundtrip() {
        let (mut store, path) = store_from("version", "[general]\nfont_size = 14\n");
        assert_eq!(store.get_version("schema_version"), None);
        assert!(store.set_version("schema_version", 3));
        assert_eq!(store.get_version("schema_version"), Some(3));
        store.save().unwrap();

        // Reload: the top-level key must round-trip at the document root, not
        // inside the [general] table.
        let reloaded = ConfigStore::load(&path).unwrap();
        assert_eq!(reloaded.get_version("schema_version"), Some(3));
        assert_eq!(reloaded.get_number("general.font_size"), Some(14.0));
        assert_eq!(reloaded.get_version("general.schema_version"), None);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn record_roundtrip_and_reorder() {
        let path = std::env::temp_dir().join(format!(
            "settings_record_test_{}.toml",
            std::process::id()
        ));
        std::fs::write(&path, "").unwrap();
        let mut store = ConfigStore::load(&path).unwrap();
        assert_eq!(store.record_push("pets"), Some(0));
        assert!(store.set_str("pets[0].name", "Mikan"));
        assert!(store.set_number("pets[0].hue", 38.0, NumberRepr::WholeAsInteger));
        assert_eq!(store.record_push("pets"), Some(1));
        assert!(store.set_str("pets[1].name", "Koke"));
        assert!(store.record_move("pets", 1, 0));
        assert_eq!(store.get_str("pets[0].name"), Some("Koke"));
        assert_eq!(store.get_number("pets[1].hue"), Some(38.0));
        assert!(store.record_remove("pets", 1));
        assert_eq!(store.record_len("pets"), 1);
        store.save().unwrap();
        let saved = std::fs::read_to_string(&path).unwrap();
        assert!(saved.contains("Koke"));
        let _ = std::fs::remove_file(path);
        let _ = store;
    }

    fn set_number_writes_integer_literal() {
        let path = std::env::temp_dir().join(format!(
            "settings_number_repr_test_{}.toml",
            std::process::id()
        ));
        std::fs::write(&path, "[section]\n").unwrap();

        let mut store = ConfigStore::load(&path).unwrap();
        store.set_number("section.count", 3.0, NumberRepr::WholeAsInteger);
        store.save().unwrap();

        let saved = std::fs::read_to_string(&path).unwrap();
        assert!(saved.contains("count = 3"));
        assert!(!saved.contains("count = 3.0"));

        let _ = std::fs::remove_file(path);
    }
}
