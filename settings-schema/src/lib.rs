use std::collections::BTreeMap;
use std::path::Path;
use std::sync::OnceLock;

use serde::{Deserialize, Deserializer};

pub mod runtime;

/// Parse a schema from a TOML string.
pub fn parse(src: &str) -> Result<Schema, toml::de::Error> {
    toml::from_str(src)
}

static ACTIVE_LANG_CODE: OnceLock<String> = OnceLock::new();

/// Sets the language code used by [`LocalizedString::get`] at runtime.
/// Schema-check and build-time validation do not call this; they use `fallback` / `en`.
pub fn set_active_lang_code(code: impl Into<String>) {
    let _ = ACTIVE_LANG_CODE.set(code.into());
}

// ---------------------------------------------------------------------------
// LocalizedString

/// A user-facing string in the schema that may be authored either as a single
/// bare value (used regardless of language) or as a per-language table, e.g.
///
/// ```toml
/// label = "Name"                       # language-agnostic
/// label = { en = "Name", ja = "名前" } # localized
/// ```
///
/// The active UI language is set once at startup via [`set_active_lang_code`];
/// this type queries it at render time via [`LocalizedString::get`], so the same
/// schema serves every supported language without duplicating the structure.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LocalizedString {
    /// Value returned when the active language has no dedicated entry. Equals the
    /// bare string when the field was authored without a per-language table.
    fallback: String,
    /// Per-language variants keyed by language code (e.g. `"en"`, `"ja"`).
    variants: BTreeMap<String, String>,
}

impl LocalizedString {
    /// Returns the variant for the currently active UI language, falling back to
    /// the language-agnostic value (or the `en` entry) when no exact match exists.
    pub fn get(&self) -> &str {
        let code = ACTIVE_LANG_CODE.get().map(String::as_str).unwrap_or("en");
        self.variants
            .get(code)
            .map(String::as_str)
            .unwrap_or(&self.fallback)
    }
}

impl<'de> Deserialize<'de> for LocalizedString {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        // Accept either a bare string or a `{ <lang> = "..." }` table.
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw {
            Plain(String),
            Map(BTreeMap<String, String>),
        }

        Ok(match Raw::deserialize(deserializer)? {
            Raw::Plain(s) => LocalizedString {
                fallback: s,
                variants: BTreeMap::new(),
            },
            Raw::Map(map) => {
                // Prefer the English entry as the fallback, else the first entry.
                let fallback = map
                    .get("en")
                    .or_else(|| map.values().next())
                    .cloned()
                    .unwrap_or_default();
                LocalizedString {
                    fallback,
                    variants: map,
                }
            }
        })
    }
}

// ---------------------------------------------------------------------------
// SaveButtonMode

/// Controls whether a Save button is shown or changes are written automatically.
#[derive(Debug, Deserialize, Default, PartialEq, Clone, Copy)]
#[serde(rename_all = "snake_case")]
pub enum SaveButtonMode {
    /// Platform default: show a Save button on Windows/Linux, auto-save on macOS.
    #[default]
    Os,
    /// Always show a Save button; changes are written only when it is pressed.
    Show,
    /// Never show a Save button; changes are written immediately on every edit.
    Hide,
}

// ---------------------------------------------------------------------------
// Migration

/// A single config schema migration step (`[[migration]]` in TOML).
///
/// Applied when the config file's recorded version is below `version` and
/// `version` is at most [`Schema::schema_version`]. Every operation is
/// idempotent: a missing source key is a no-op, and destination collisions are
/// skipped, so re-running a migration never loses data.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct Migration {
    /// Version this step brings the config file up to. Must be unique across
    /// migrations and not exceed [`Schema::schema_version`].
    pub version: u32,
    /// Move a value from one dotted path to another (same or different table).
    #[serde(default)]
    pub rename: Vec<Rename>,
    /// Remove a key entirely.
    #[serde(default)]
    pub delete: Vec<Delete>,
    /// Move a value while converting it (see [`TransformKind`]).
    #[serde(default)]
    pub transform: Vec<Transform>,
}

/// Move a value between dotted paths without changing it.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct Rename {
    pub from: String,
    pub to: String,
}

/// Remove a key by dotted path.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct Delete {
    pub key: String,
}

/// Move a value from `from` to `to`, converting it per `kind`.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct Transform {
    pub from: String,
    pub to: String,
    #[serde(rename = "type")]
    pub kind: TransformKind,
    /// Source string that maps to `true` for [`TransformKind::EnumToBool`].
    /// Required for `enum_to_bool`; ignored otherwise.
    #[serde(default, rename = "match")]
    pub match_value: Option<String>,
}

/// Value conversion applied by a [`Transform`].
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TransformKind {
    /// Move the value unchanged (equivalent to [`Rename`]).
    Rename,
    /// Write `true` when the source string equals `match`, else `false`.
    EnumToBool,
}

// ---------------------------------------------------------------------------
// Validation (CEL)

/// A named CEL constraint (`[[constraints]]` in the schema).
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct Constraint {
    /// Stable identifier unique within the schema.
    pub id: String,
    /// CEL expression; must evaluate to `bool`.
    pub expr: String,
    /// Shown below the field when referenced from [`Field::validate`] and the
    /// expression is false. Not required for [`OptionState::when`] usage.
    #[serde(default)]
    pub message: Option<LocalizedString>,
}

/// One entry in a field's `validate` list — a [`Constraint`] id or an inline rule.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum ValidateEntry {
    /// Reference to a top-level [`Constraint`] by `id`.
    Ref(String),
    /// Inline CEL rule with a mandatory localized message.
    Inline(InlineValidate),
}

/// Inline field validation rule (CEL + message).
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct InlineValidate {
    pub expr: String,
    pub message: LocalizedString,
}

/// Per-option enablement for `segmented_control` and `select`.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct OptionState {
    /// Option value as written in `options` (always a string).
    pub value: String,
    /// Constraint id: enabled iff the constraint's `expr` is true when this
    /// field is hypothetically set to `value`.
    #[serde(default)]
    pub when: Option<String>,
    /// Explicit CEL expression: enabled when this evaluates to `true`.
    #[serde(default)]
    pub enabled: Option<String>,
}

/// Resolved validation rule with expression and message materialized.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolvedValidateRule {
    Named {
        id: String,
        expr: String,
        message: LocalizedString,
    },
    Inline(InlineValidate),
}

/// Semantic schema error (English messages for schema authors).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SchemaValidationError {
    DuplicateConstraintId { id: String },
    UnknownConstraintRef { id: String, location: String },
    MissingConstraintMessage { id: String, location: String },
    OptionStatesUnsupportedWidget { location: String, widget: String },
    OptionStateConflict { location: String, value: String },
    OptionStateMissingRule { location: String, value: String },
    DuplicateOptionStateValue { location: String, value: String },
    UnknownOptionValue { location: String, value: String },
    InvalidCelExpression {
        location: String,
        expr: String,
        detail: String,
    },
    DuplicateMigrationVersion { version: u32 },
    MigrationVersionExceedsTarget { version: u32, target: u32 },
    MigrationsWithoutSchemaVersion,
    TransformMissingMatch { location: String },
}

impl SchemaValidationError {
    /// Short error summary for the `error:` line (no location suffix).
    pub fn summary(&self) -> String {
        match self {
            Self::DuplicateConstraintId { id } => format!("duplicate constraint id {id:?}"),
            Self::UnknownConstraintRef { id, .. } => {
                format!("unknown constraint id {id:?}")
            }
            Self::MissingConstraintMessage { id, .. } => {
                format!("constraint {id:?} is referenced but has no `message`")
            }
            Self::OptionStatesUnsupportedWidget { widget, .. } => format!(
                "option_states is only allowed on segmented_control and select \
                 (found widget={widget:?})"
            ),
            Self::OptionStateConflict { value, .. } => format!(
                "option_states entry for value {value:?} must set either `when` or `enabled`, \
                 not both"
            ),
            Self::OptionStateMissingRule { value, .. } => format!(
                "option_states entry for value {value:?} must set `when` or `enabled`"
            ),
            Self::DuplicateOptionStateValue { value, .. } => {
                format!("duplicate option_states value {value:?}")
            }
            Self::UnknownOptionValue { value, .. } => {
                format!("option_states value {value:?} is not listed in `options`")
            }
            Self::InvalidCelExpression { detail, .. } => {
                format!("invalid CEL expression: {detail}")
            }
            Self::DuplicateMigrationVersion { version } => {
                format!("duplicate migration version {version}")
            }
            Self::MigrationVersionExceedsTarget { version, target } => format!(
                "migration version {version} exceeds schema_version {target}"
            ),
            Self::MigrationsWithoutSchemaVersion => {
                "[[migration]] entries require a top-level schema_version".to_owned()
            }
            Self::TransformMissingMatch { .. } => {
                "transform type \"enum_to_bool\" requires a `match` value".to_owned()
            }
        }
    }

    /// Field context for the `-->` line, when applicable.
    pub fn context(&self) -> Option<&str> {
        match self {
            Self::DuplicateConstraintId { .. } => Some("[[constraints]]"),
            Self::DuplicateMigrationVersion { .. }
            | Self::MigrationVersionExceedsTarget { .. }
            | Self::MigrationsWithoutSchemaVersion => Some("[[migration]]"),
            Self::UnknownConstraintRef { location, .. }
            | Self::MissingConstraintMessage { location, .. }
            | Self::OptionStatesUnsupportedWidget { location, .. }
            | Self::OptionStateConflict { location, .. }
            | Self::OptionStateMissingRule { location, .. }
            | Self::DuplicateOptionStateValue { location, .. }
            | Self::UnknownOptionValue { location, .. }
            | Self::InvalidCelExpression { location, .. }
            | Self::TransformMissingMatch { location } => Some(location),
        }
    }

    /// Actionable hint for the `help:` line.
    pub fn help(&self) -> &'static str {
        match self {
            Self::DuplicateConstraintId { .. } => {
                "use a unique `id` for each [[constraints]] entry"
            }
            Self::UnknownConstraintRef { .. } => {
                "add a [[constraints]] entry with this id or change the field's validate reference"
            }
            Self::MissingConstraintMessage { .. } => {
                "add a `message` to [[constraints]] or use an inline [[validate]] rule"
            }
            Self::OptionStatesUnsupportedWidget { .. } => {
                "remove [[tabs.fields.option_states]] or change widget to segmented_control / select"
            }
            Self::OptionStateConflict { .. } => {
                "set either `when` or `enabled` on the option_states entry, not both"
            }
            Self::OptionStateMissingRule { .. } => {
                "add `when` or `enabled` to the option_states entry"
            }
            Self::DuplicateOptionStateValue { .. } => {
                "remove the duplicate option_states entry for this value"
            }
            Self::UnknownOptionValue { .. } => {
                "use a value that appears in the field's `options` list"
            }
            Self::InvalidCelExpression { .. } => {
                "fix the CEL syntax or use a supported function"
            }
            Self::DuplicateMigrationVersion { .. } => {
                "use a unique `version` for each [[migration]] entry"
            }
            Self::MigrationVersionExceedsTarget { .. } => {
                "raise the top-level schema_version or lower the migration version"
            }
            Self::MigrationsWithoutSchemaVersion => {
                "add a top-level `schema_version` equal to the highest migration version"
            }
            Self::TransformMissingMatch { .. } => {
                "add `match = \"...\"` or change the transform type"
            }
        }
    }

    /// Extra detail lines shown after the context (e.g. CEL source).
    pub fn detail_lines(&self) -> Vec<String> {
        match self {
            Self::InvalidCelExpression { expr, .. } => vec![format!("expr = {expr:?}")],
            _ => Vec::new(),
        }
    }
}

impl std::fmt::Display for SchemaValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.summary())
    }
}

fn deserialize_validate_entries<'de, D>(d: D) -> Result<Vec<ValidateEntry>, D::Error>
where
    D: Deserializer<'de>,
{
    // `Many` must be listed before `One`: otherwise a heterogeneous TOML array
    // can be mis-read as a single inline table.
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Raw {
        Many(Vec<ValidateEntry>),
        One(ValidateEntry),
    }

    Ok(match Raw::deserialize(d)? {
        Raw::Many(entries) => entries,
        Raw::One(entry) => vec![entry],
    })
}

// ---------------------------------------------------------------------------
// Top-level

#[derive(Debug, Deserialize)]
pub struct Schema {
    /// Reusable named CEL constraints (`[[constraints]]` in TOML).
    #[serde(default)]
    pub constraints: Vec<Constraint>,
    pub tabs: Vec<Tab>,
    /// Icon variant to use for Material Symbols ("rounded" | "outlined" | "sharp").
    /// Only informational for the schema consumer; the actual font is selected at
    /// build time via `make icons [ICON_STYLE=…]`. Parsed only to validate and
    /// document the schema; never read at runtime.
    #[serde(default)]
    #[allow(dead_code)]
    pub icon_style: Option<String>,
    /// Which color variant to display.
    /// `"os"` (default, follow the OS) | `"light"` | `"dark"`
    #[serde(default)]
    pub theme: ThemeMode,
    /// UI chrome language.
    /// `"os"` (default, follow the OS locale) | BCP-47 code (e.g. `"en"`, `"ja"`, `"de"`)
    #[serde(default)]
    pub lang: LangMode,
    /// Dot-separated path to the config key that stores the parent application's
    /// language (e.g. `"display.language"`). When `lang = "os"`, Settings reads this
    /// value from the config file and matches it, so the settings window follows
    /// the same language as the parent app. Falls back to native OS detection when
    /// the key is absent or unset.
    #[serde(default)]
    pub lang_key: Option<String>,
    /// Light-variant color overrides. These flat top-level fields
    /// (`background_color`, `accent_color`, …) override the built-in light
    /// palette and are also used in Light Mode when `theme` follows the OS.
    #[serde(flatten)]
    pub colors_light: ColorOverrides,
    /// Dark-variant color overrides, supplied via a `[dark]` table. Override the
    /// built-in dark palette; ignored entirely in Light Mode.
    #[serde(default, rename = "dark")]
    pub colors_dark: ColorOverrides,
    /// Save button visibility / auto-save behavior.
    /// `"os"` (default) | `"show"` | `"hide"`
    #[serde(default)]
    pub save_button: SaveButtonMode,
    /// Optional overrides for built-in UI chrome strings (`[ui_strings]` table).
    #[serde(default)]
    pub ui_strings: UiStrings,
    /// Width of the content area in logical pixels (excludes title bar, tab bar,
    /// and bottom button bar). Defaults to 700 when omitted.
    #[serde(default)]
    pub content_width: Option<f32>,
    /// Height of the content area in logical pixels (excludes title bar, tab bar,
    /// and bottom button bar). Defaults to 370 when omitted.
    #[serde(default)]
    pub content_height: Option<f32>,
    /// Target config schema version. When set, Settings migrates the config file
    /// up to this version at startup (see [`Migration`]). Omit to disable migration.
    #[serde(default)]
    pub schema_version: Option<u32>,
    /// Config key that records the applied migration version. Defaults to
    /// `"schema_version"`. Override when the parent app already uses that key.
    #[serde(default)]
    pub migration_version_key: Option<String>,
    /// Ordered migration steps (`[[migration]]` in TOML). Applied in ascending
    /// `version` order for versions above the config file's recorded version.
    #[serde(default, rename = "migration")]
    pub migrations: Vec<Migration>,
}

impl Schema {
    /// Config key used to record the applied migration version.
    /// Falls back to `"schema_version"` when the schema does not override it.
    pub fn version_key(&self) -> &str {
        self.migration_version_key
            .as_deref()
            .unwrap_or("schema_version")
    }
}

impl Schema {
    /// Validate cross-references and field/rule consistency.
    ///
    /// Returns all errors at once so schema authors can fix them in one pass.
    pub fn validate(&self) -> Result<(), Vec<SchemaValidationError>> {
        let mut errors = Vec::new();
        let mut seen_ids = BTreeMap::<&str, usize>::new();

        for (index, constraint) in self.constraints.iter().enumerate() {
            if seen_ids.insert(constraint.id.as_str(), index).is_some() {
                errors.push(SchemaValidationError::DuplicateConstraintId {
                    id: constraint.id.clone(),
                });
            }
        }

        let constraint_ids: BTreeMap<&str, &Constraint> = self
            .constraints
            .iter()
            .map(|c| (c.id.as_str(), c))
            .collect();

        for (tab_index, tab) in self.tabs.iter().enumerate() {
            if let Some(fields) = &tab.fields {
                for (field_index, field) in fields.iter().enumerate() {
                    validate_field(
                        &mut errors,
                        &constraint_ids,
                        FieldContext {
                            tab_index,
                            tab_id: &tab.id,
                            field_index,
                            field,
                            section_prefix: None,
                        },
                    );
                }
            }
            if let Some(section_map) = &tab.section_map {
                for (field_index, field) in section_map.fields.iter().enumerate() {
                    validate_field(
                        &mut errors,
                        &constraint_ids,
                        FieldContext {
                            tab_index,
                            tab_id: &tab.id,
                            field_index,
                            field,
                            section_prefix: Some(section_map.key_prefix.as_str()),
                        },
                    );
                }
            }
        }

        validate_cel_expressions(self, &mut errors);
        validate_migrations(self, &mut errors);

        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }
}

fn validate_migrations(schema: &Schema, errors: &mut Vec<SchemaValidationError>) {
    if schema.migrations.is_empty() {
        return;
    }

    let Some(target) = schema.schema_version else {
        errors.push(SchemaValidationError::MigrationsWithoutSchemaVersion);
        // Without a target there is nothing to bound versions against; still
        // report per-entry issues below.
        return;
    };

    let mut seen = BTreeMap::<u32, ()>::new();
    for migration in &schema.migrations {
        if seen.insert(migration.version, ()).is_some() {
            errors.push(SchemaValidationError::DuplicateMigrationVersion {
                version: migration.version,
            });
        }
        if migration.version > target {
            errors.push(SchemaValidationError::MigrationVersionExceedsTarget {
                version: migration.version,
                target,
            });
        }
        for transform in &migration.transform {
            if transform.kind == TransformKind::EnumToBool && transform.match_value.is_none() {
                errors.push(SchemaValidationError::TransformMissingMatch {
                    location: format!(
                        "migration version={} transform from={:?} to={:?}",
                        migration.version, transform.from, transform.to
                    ),
                });
            }
        }
    }
}

fn validate_cel_expressions(schema: &Schema, errors: &mut Vec<SchemaValidationError>) {
    for constraint in &schema.constraints {
        compile_cel_expr(&constraint.expr, &format!("constraints id={:?}", constraint.id), errors);
    }

    for (tab_index, tab) in schema.tabs.iter().enumerate() {
        if let Some(fields) = &tab.fields {
            for (field_index, field) in fields.iter().enumerate() {
                collect_field_cel_exprs(
                    errors,
                    tab_index,
                    &tab.id,
                    field_index,
                    field,
                    None,
                );
            }
        }
        if let Some(section_map) = &tab.section_map {
            for (field_index, field) in section_map.fields.iter().enumerate() {
                collect_field_cel_exprs(
                    errors,
                    tab_index,
                    &tab.id,
                    field_index,
                    field,
                    Some(section_map.key_prefix.as_str()),
                );
            }
        }
    }
}

fn collect_field_cel_exprs(
    errors: &mut Vec<SchemaValidationError>,
    tab_index: usize,
    tab_id: &str,
    field_index: usize,
    field: &Field,
    section_prefix: Option<&str>,
) {
    let location = match section_prefix {
        Some(prefix) => format!(
            "tabs[{tab_index}].id={tab_id:?}.section_map.key_prefix={prefix:?}.fields[{field_index}] key={:?}",
            field.key
        ),
        None => format!(
            "tabs[{tab_index}].id={tab_id:?}.fields[{field_index}] key={:?}",
            field.key
        ),
    };

    for entry in &field.validate {
        if let ValidateEntry::Inline(inline) = entry {
            compile_cel_expr(&inline.expr, &location, errors);
        }
    }

    for state in &field.option_states {
        if let Some(expr) = &state.enabled {
            let loc = format!(
                "{location} option_states value={:?}",
                state.value
            );
            compile_cel_expr(expr, &loc, errors);
        }
    }
}

fn compile_cel_expr(expr: &str, location: &str, errors: &mut Vec<SchemaValidationError>) {
    if let Err(detail) = try_compile_cel(expr) {
        errors.push(SchemaValidationError::InvalidCelExpression {
            location: location.to_owned(),
            expr: expr.to_owned(),
            detail,
        });
    }
}

fn try_compile_cel(expr: &str) -> Result<(), String> {
    use cel::Program;
    Program::compile(expr)
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// Read `path`, parse, and run semantic + CEL validation.
pub fn check_schema_file(path: &Path) -> Result<(), String> {
    let src = std::fs::read_to_string(path).map_err(|e| {
        format!(
            "error: cannot read schema file {}: {e}",
            path.display()
        )
    })?;

    let schema = parse(&src).map_err(|e| {
        format!(
            "error: TOML parse failed in {}\n  {e}",
            path.display()
        )
    })?;

    match schema.validate() {
        Ok(()) => Ok(()),
        Err(errors) => Err(format_validation_errors(path, &errors)),
    }
}

/// Formats all validation errors for terminal output (English, rustc-style).
pub fn format_validation_errors(path: &Path, errors: &[SchemaValidationError]) -> String {
    let filename = path
        .file_name()
        .map(|f| f.to_string_lossy().into_owned())
        .unwrap_or_else(|| "schema.toml".to_owned());

    let mut out = String::new();
    for (index, err) in errors.iter().enumerate() {
        if index > 0 {
            out.push('\n');
        }
        out.push_str("error: ");
        out.push_str(&err.summary());
        out.push('\n');
        if let Some(context) = err.context() {
            out.push_str("  --> ");
            out.push_str(&filename);
            out.push_str(" (");
            out.push_str(context);
            out.push_str(")\n");
            for line in err.detail_lines() {
                out.push_str("      ");
                out.push_str(&line);
                out.push('\n');
            }
        }
        out.push_str("help: ");
        out.push_str(err.help());
        out.push('\n');
    }

    if errors.len() > 1 {
        out.push('\n');
        out.push_str(&format!(
            "error: {} validation errors in {}\n",
            errors.len(),
            path.display()
        ));
    }

    out.push_str("help: fix all reported issues, then run `settings-schema-checker ");
    out.push_str(&path.display().to_string());
    out.push_str("`\n");
    out
}

struct FieldContext<'a> {
    tab_index: usize,
    tab_id: &'a str,
    field_index: usize,
    field: &'a Field,
    section_prefix: Option<&'a str>,
}

impl FieldContext<'_> {
    fn location(&self) -> String {
        match self.section_prefix {
            Some(prefix) => format!(
                "tabs[{}].id={:?}.section_map.key_prefix={:?}.fields[{}] key={:?}",
                self.tab_index, self.tab_id, prefix, self.field_index, self.field.key
            ),
            None => format!(
                "tabs[{}].id={:?}.fields[{}] key={:?}",
                self.tab_index, self.tab_id, self.field_index, self.field.key
            ),
        }
    }
}

fn validate_field(
    errors: &mut Vec<SchemaValidationError>,
    constraints: &BTreeMap<&str, &Constraint>,
    ctx: FieldContext<'_>,
) {
    let location = ctx.location();
    let field = ctx.field;

    for entry in &field.validate {
        if let ValidateEntry::Ref(id) = entry {
            match constraints.get(id.as_str()) {
                None => errors.push(SchemaValidationError::UnknownConstraintRef {
                    id: id.clone(),
                    location: location.clone(),
                }),
                Some(constraint) if constraint.message.is_none() => {
                    errors.push(SchemaValidationError::MissingConstraintMessage {
                        id: id.clone(),
                        location: location.clone(),
                    });
                }
                Some(_) => {}
            }
        }
    }

    if field.option_states.is_empty() {
        return;
    }

    if !field.widget.supports_option_states() {
        errors.push(SchemaValidationError::OptionStatesUnsupportedWidget {
            location,
            widget: widget_kind_name(&field.widget).to_owned(),
        });
        return;
    }

    let mut seen_values = BTreeMap::<&str, ()>::new();
    for state in &field.option_states {
        let has_when = state.when.is_some();
        let has_enabled = state.enabled.is_some();
        if has_when && has_enabled {
            errors.push(SchemaValidationError::OptionStateConflict {
                location: location.clone(),
                value: state.value.clone(),
            });
        } else if !has_when && !has_enabled {
            errors.push(SchemaValidationError::OptionStateMissingRule {
                location: location.clone(),
                value: state.value.clone(),
            });
        }

        if state.when.as_ref().is_some_and(|id| !constraints.contains_key(id.as_str())) {
            errors.push(SchemaValidationError::UnknownConstraintRef {
                id: state.when.clone().unwrap(),
                location: location.clone(),
            });
        }

        if seen_values.insert(state.value.as_str(), ()).is_some() {
            errors.push(SchemaValidationError::DuplicateOptionStateValue {
                location: location.clone(),
                value: state.value.clone(),
            });
        }

        if let Some(options) = &field.options {
            if !options.iter().any(|o| o == &state.value) {
                errors.push(SchemaValidationError::UnknownOptionValue {
                    location: location.clone(),
                    value: state.value.clone(),
                });
            }
        }
    }
}

fn widget_kind_name(widget: &WidgetKind) -> &'static str {
    match widget {
        WidgetKind::TextInput => "text_input",
        WidgetKind::SecretInput => "secret_input",
        WidgetKind::Multiline => "multiline",
        WidgetKind::Checkbox => "checkbox",
        WidgetKind::Toggle => "toggle",
        WidgetKind::Select => "select",
        WidgetKind::SegmentedControl => "segmented_control",
        WidgetKind::ExclusiveRadio => "exclusive_radio",
        WidgetKind::Hotkey => "hotkey",
        WidgetKind::Slider => "slider",
        WidgetKind::DragValue => "drag_value",
        WidgetKind::Separator => "separator",
        WidgetKind::FilePath => "file_path",
        WidgetKind::ColorPicker => "color_picker",
        WidgetKind::KeyValueMap => "key_value_map",
    }
}

impl WidgetKind {
    fn supports_option_states(&self) -> bool {
        matches!(self, WidgetKind::SegmentedControl | WidgetKind::Select)
    }
}

impl Field {
    /// Expand `validate` entries against top-level [`Constraint`]s, preserving order.
    pub fn resolved_validate_rules(
        &self,
        schema: &Schema,
    ) -> Result<Vec<ResolvedValidateRule>, SchemaValidationError> {
        let constraints: BTreeMap<&str, &Constraint> = schema
            .constraints
            .iter()
            .map(|c| (c.id.as_str(), c))
            .collect();

        let mut rules = Vec::with_capacity(self.validate.len());
        for entry in &self.validate {
            match entry {
                ValidateEntry::Ref(id) => {
                    let constraint = constraints.get(id.as_str()).ok_or_else(|| {
                        SchemaValidationError::UnknownConstraintRef {
                            id: id.clone(),
                            location: format!("field key={:?}", self.key),
                        }
                    })?;
                    let message = constraint.message.clone().ok_or_else(|| {
                        SchemaValidationError::MissingConstraintMessage {
                            id: id.clone(),
                            location: format!("field key={:?}", self.key),
                        }
                    })?;
                    rules.push(ResolvedValidateRule::Named {
                        id: id.clone(),
                        expr: constraint.expr.clone(),
                        message,
                    });
                }
                ValidateEntry::Inline(inline) => {
                    rules.push(ResolvedValidateRule::Inline(inline.clone()));
                }
            }
        }
        Ok(rules)
    }
}

// ---------------------------------------------------------------------------
// Theme / language preferences

/// Which color variant the UI should display.
#[derive(Debug, Deserialize, Default, PartialEq, Eq, Clone, Copy)]
#[serde(rename_all = "snake_case")]
pub enum ThemeMode {
    /// Follow the operating system's light/dark setting (default).
    #[default]
    Os,
    Light,
    Dark,
}

/// Which language the built-in UI chrome should use.
///
/// `"os"` (default) follows the OS locale. Any BCP-47 base language code
/// (e.g. `"en"`, `"ja"`, `"de"`) selects that language explicitly; codes
/// without a built-in translation fall back to English unless `[ui_strings]`
/// supplies overrides.
#[derive(Debug, Default, PartialEq, Eq, Clone)]
pub enum LangMode {
    /// Follow the operating system locale (default).
    #[default]
    Os,
    /// Use the specified BCP-47 language code (e.g. `"en"`, `"ja"`, `"de"`).
    Fixed(String),
}

impl<'de> Deserialize<'de> for LangMode {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Ok(if s.eq_ignore_ascii_case("os") {
            LangMode::Os
        } else {
            LangMode::Fixed(s)
        })
    }
}

/// Optional overrides for built-in UI chrome strings, supplied via a
/// `[ui_strings]` table in the schema. Each field replaces the corresponding
/// built-in string for the active language. Omitted fields keep the built-in
/// translation (or English when no built-in translation exists for the language).
#[derive(Debug, Deserialize, Default)]
pub struct UiStrings {
    pub ok: Option<String>,
    pub apply: Option<String>,
    pub no_sections: Option<String>,
    pub add_section: Option<String>,
    pub section_name_label: Option<String>,
    pub add: Option<String>,
    pub cancel: Option<String>,
    pub enter_name: Option<String>,
    pub delete: Option<String>,
    /// Template for the delete confirmation dialog; use `{}` as a placeholder
    /// for the item name (e.g. `"Delete \"{}\"?"`).
    pub delete_confirm: Option<String>,
    pub browse: Option<String>,
    pub all_files: Option<String>,
    pub click_to_input: Option<String>,
    pub press_key: Option<String>,
    pub clear: Option<String>,
    pub show: Option<String>,
    pub hide: Option<String>,
    /// Body text of the external-change conflict dialog.
    pub file_changed: Option<String>,
    /// "Reload" button label in the conflict dialog.
    pub reload: Option<String>,
    /// "Keep Editing" button label in the conflict dialog.
    pub keep_editing: Option<String>,
    /// Window title bar text.
    pub window_title: Option<String>,
}

/// High-level color overrides shared by the light and dark variants. Each field
/// is an optional CSS hex string; `None` falls back to the built-in palette.
#[derive(Debug, Deserialize, Default)]
pub struct ColorOverrides {
    /// Panel and window background color (CSS hex, e.g. `"#ffffff"`).
    #[serde(default)]
    pub background_color: Option<String>,
    /// Accent color for the selected tab and interactive highlights (CSS hex).
    #[serde(default)]
    pub accent_color: Option<String>,
    /// Primary text color for labels and input text (CSS hex).
    #[serde(default)]
    pub text_color: Option<String>,
    /// Color of unselected tab icons and labels (CSS hex).
    #[serde(default)]
    pub tab_text_color: Option<String>,
    /// Background fill of the selected-tab highlight (CSS hex).
    /// Defaults to the accent color at reduced opacity when omitted.
    #[serde(default)]
    pub selection_bg_color: Option<String>,
}

// ---------------------------------------------------------------------------
// Tab

#[derive(Debug, Deserialize)]
pub struct Tab {
    /// Stable tab identifier. Parsed only to enforce its presence and document
    /// the schema; tabs are addressed by index at runtime, so it is never read.
    #[allow(dead_code)]
    pub id: String,
    pub label: LocalizedString,
    /// Material Symbol icon name (e.g. `"settings"`, `"translate"`).
    /// Shown in the tab bar when the icon font was embedded at build time.
    #[serde(default)]
    pub icon: Option<String>,
    /// Per-tab content width override in logical pixels.
    /// Falls back to `Schema::content_width` when omitted.
    #[serde(default)]
    pub content_width: Option<f32>,
    /// Per-tab content height override in logical pixels.
    /// Falls back to `Schema::content_height` when omitted.
    #[serde(default)]
    pub content_height: Option<f32>,
    /// Flat field list (mutually exclusive with `section_map`).
    pub fields: Option<Vec<Field>>,
    /// Sub-section map (mutually exclusive with `fields`).
    pub section_map: Option<SectionMap>,
}

// ---------------------------------------------------------------------------
// SectionMap

/// Visual style of the sub-section tab bar inside a `section_map`.
#[derive(Debug, Deserialize, PartialEq, Eq, Clone, Default)]
#[serde(rename_all = "snake_case")]
pub enum SectionTabStyle {
    /// Underline style: accent-colored thick underline on active tab,
    /// thin grey underline on inactive tabs (default).
    #[default]
    Underline,
    /// Segmented-control style: all tabs rendered as a single pill-based
    /// segmented control (best for a small, fixed number of sections).
    Segmented,
}

#[derive(Debug, Deserialize)]
pub struct SectionMap {
    pub key_prefix: String,
    pub allow_add_remove: bool,
    pub fields: Vec<Field>,
    /// Visual style of the sub-tab bar. Defaults to `"underline"`.
    #[serde(default)]
    pub tab_style: SectionTabStyle,
    /// Maximum width of the segmented control (px). Has no effect on underline style.
    pub max_width: Option<f32>,
}

// ---------------------------------------------------------------------------
// Field

#[derive(Debug, Deserialize)]
pub struct Field {
    #[serde(default)]
    pub key: String,
    #[serde(default)]
    pub label: LocalizedString,
    /// Declared TOML value type (`string` by default). Drives read/write for
    /// `segmented_control` (`string` vs `number`); otherwise used for schema validation.
    #[serde(rename = "type", default)]
    pub field_type: FieldType,
    pub widget: WidgetKind,
    /// Generic one-line hint shown below the widget.
    pub hint: Option<LocalizedString>,
    /// Hint shown for the "direct" variant of an exclusive_radio field.
    pub hint_direct: Option<LocalizedString>,
    /// Hint shown for the "env" variant of an exclusive_radio field.
    pub hint_env: Option<LocalizedString>,
    /// Static option list for `select` widgets.
    pub options: Option<Vec<String>>,
    /// Reference to a section_map whose keys become the option list at render time.
    pub options_from: Option<String>,
    /// Inline description placed to the right of the widget (non-bold, same font size).
    /// Intended for checkbox / toggle rows where the description belongs beside the control.
    /// Distinct from `hint`, which is rendered below the widget on a separate line.
    pub sublabel: Option<LocalizedString>,
    /// Minimum widget width (logical px). Applies to `text_input`, `multiline`,
    /// `segmented_control`, and similar widgets. No constraint when omitted.
    pub min_width: Option<f32>,
    /// Maximum widget width (logical px). Applies to `text_input`, `multiline`,
    /// `segmented_control`, and similar widgets. Full available width when omitted.
    pub max_width: Option<f32>,
    /// Number of visible rows for `multiline` widgets. Defaults to 4 when omitted.
    /// Overflow scrolls inside the widget; the form layout is not stretched.
    pub rows: Option<usize>,
    /// Minimum numeric value for `slider` and `drag_value` widgets.
    pub min: Option<f64>,
    /// Maximum numeric value for `slider` and `drag_value` widgets.
    pub max: Option<f64>,
    /// Step increment for `slider` and `drag_value` widgets. Defaults to 1 when omitted.
    pub step: Option<f64>,
    /// Suffix string appended to the displayed number (e.g. `" px"`, `" pt"`).
    pub suffix: Option<String>,
    /// Present only when `widget = "exclusive_radio"`.
    pub exclusive: Option<ExclusiveConfig>,

    /// Field validation rules (CEL). Evaluation order follows declaration order.
    #[serde(default, deserialize_with = "deserialize_validate_entries")]
    pub validate: Vec<ValidateEntry>,

    /// Per-option enablement for `segmented_control` and `select`.
    #[serde(default)]
    pub option_states: Vec<OptionState>,

    // --- file_path ---
    /// If `true`, the file dialog picks a directory instead of a file.
    #[serde(default)]
    pub is_directory: bool,
    /// Filter label shown in the file dialog (e.g. `"TOML files"`).
    /// Only consumed when building the dialog filter, which is compiled out on
    /// macOS (NSOpenPanel has no usable filter dropdown), so it is dead there.
    #[cfg_attr(target_os = "macos", allow(dead_code))]
    pub file_filter: Option<LocalizedString>,
    /// File extensions shown by the filter (e.g. `["toml", "txt"]`).
    /// Unused on macOS for the same reason as `file_filter`.
    #[cfg_attr(target_os = "macos", allow(dead_code))]
    pub file_extensions: Option<Vec<String>>,

    // --- key_value_map ---
    /// Header text for the key column. Defaults to `"Key"`.
    pub key_label: Option<LocalizedString>,
    /// Header text for the value column. Defaults to `"Value"`.
    pub value_label: Option<LocalizedString>,
}

// ---------------------------------------------------------------------------
// Enums

#[derive(Debug, Deserialize, PartialEq, Eq, Clone)]
#[serde(rename_all = "snake_case")]
pub enum WidgetKind {
    TextInput,
    SecretInput,
    Multiline,
    Checkbox,
    Toggle,
    Select,
    SegmentedControl,
    ExclusiveRadio,
    Hotkey,
    /// Horizontal slider with numeric value display.
    Slider,
    /// Drag-to-change numeric input (click to type directly).
    DragValue,
    /// A full-width horizontal rule. No key, label, or type needed in the schema.
    Separator,
    /// Text input + "Browse…" button that opens a native file/directory dialog.
    FilePath,
    /// Color swatch that opens an inline color picker popup. Stored as `"#rrggbb"` in TOML.
    ColorPicker,
    /// Editable table of string key → string value pairs.
    KeyValueMap,
}

#[derive(Debug, Deserialize, PartialEq, Eq, Clone, Default)]
#[serde(rename_all = "snake_case")]
pub enum FieldType {
    #[default]
    String,
    Bool,
    Number,
}

// ---------------------------------------------------------------------------
// ExclusiveConfig / ExclusiveVariant

#[derive(Debug, Deserialize, Clone)]
pub struct ExclusiveConfig {
    pub mode_key: String,
    pub mode_default: String,
    pub variants: Vec<ExclusiveVariant>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct ExclusiveVariant {
    pub value: String,
    pub label: LocalizedString,
    pub field_key: String,
    pub widget: WidgetKind,
}

// ---------------------------------------------------------------------------
// Tests

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_and_validate(src: &str) -> Result<Schema, Vec<SchemaValidationError>> {
        let schema = parse(src).expect("TOML parse");
        schema.validate().map(|()| schema).map_err(|e| e)
    }

    #[test]
    fn parse_validate_single_ref() {
        let src = r#"
[[constraints]]
id = "non_empty"
expr = "x.size() > 0"
message = "Required"

[[tabs]]
id = "main"
label = "Main"

[[tabs.fields]]
key = "x"
label = "X"
widget = "text_input"
validate = "non_empty"
"#;
        let schema = parse(src).unwrap();
        assert_eq!(schema.constraints.len(), 1);
        assert_eq!(schema.tabs[0].fields.as_ref().unwrap()[0].validate.len(), 1);
        assert!(matches!(
            schema.tabs[0].fields.as_ref().unwrap()[0].validate[0],
            ValidateEntry::Ref(ref id) if id == "non_empty"
        ));
        schema.validate().unwrap();
    }

    #[test]
    fn parse_validate_inline_array() {
        let src = r#"
[[tabs]]
id = "main"
label = "Main"

[[tabs.fields]]
key = "email"
label = "Email"
widget = "text_input"
validate = [
  "fmt",
  { expr = "email.size() > 0", message = "Required" },
]
[[constraints]]
id = "fmt"
expr = "true"
message = "bad format"
"#;
        let schema = parse(src).unwrap();
        let field = &schema.tabs[0].fields.as_ref().unwrap()[0];
        assert_eq!(
            field.validate.len(),
            2,
            "validate entries: {:?}",
            field.validate
        );
        schema.validate().unwrap();
        let rules = field.resolved_validate_rules(&schema).unwrap();
        assert_eq!(rules.len(), 2);
    }

    #[test]
    fn parse_validate_subtable_form() {
        let src = r#"
[[tabs]]
id = "main"
label = "Main"

[[tabs.fields]]
key = "name"
label = "Name"
widget = "text_input"

[[tabs.fields.validate]]
expr = "name.size() > 0"
message = "Required"
"#;
        let schema = parse(src).unwrap();
        assert!(matches!(
            schema.tabs[0].fields.as_ref().unwrap()[0].validate[0],
            ValidateEntry::Inline(_)
        ));
        schema.validate().unwrap();
    }

    #[test]
    fn parse_option_states_when() {
        let src = r#"
[[constraints]]
id = "min_one"
expr = "a + b >= 1"

[[tabs]]
id = "main"
label = "Main"

[[tabs.fields]]
key = "a"
label = "A"
type = "number"
widget = "segmented_control"
options = ["0", "1"]

[[tabs.fields.option_states]]
value = "0"
when = "min_one"

[[tabs.fields]]
key = "b"
label = "B"
type = "number"
widget = "segmented_control"
options = ["0", "1"]

[[tabs.fields.option_states]]
value = "0"
when = "min_one"
"#;
        parse_and_validate(src).unwrap();
    }

    #[test]
    fn error_duplicate_constraint_id() {
        let src = r#"
[[constraints]]
id = "dup"
expr = "true"
message = "a"

[[constraints]]
id = "dup"
expr = "false"
message = "b"

[[tabs]]
id = "main"
label = "Main"
"#;
        let errs = parse(src).unwrap().validate().unwrap_err();
        assert!(errs.iter().any(|e| matches!(e, SchemaValidationError::DuplicateConstraintId { id } if id == "dup")));
    }

    #[test]
    fn error_unknown_validate_ref() {
        let src = r#"
[[tabs]]
id = "main"
label = "Main"

[[tabs.fields]]
key = "x"
label = "X"
widget = "text_input"
validate = "missing"
"#;
        let errs = parse(src).unwrap().validate().unwrap_err();
        assert!(errs.iter().any(|e| matches!(
            e,
            SchemaValidationError::UnknownConstraintRef { id, .. } if id == "missing"
        )));
    }

    #[test]
    fn error_missing_constraint_message() {
        let src = r#"
[[constraints]]
id = "no_msg"
expr = "true"

[[tabs]]
id = "main"
label = "Main"

[[tabs.fields]]
key = "x"
label = "X"
widget = "text_input"
validate = "no_msg"
"#;
        let errs = parse(src).unwrap().validate().unwrap_err();
        assert!(errs.iter().any(|e| matches!(
            e,
            SchemaValidationError::MissingConstraintMessage { id, .. } if id == "no_msg"
        )));
    }

    #[test]
    fn error_option_states_on_text_input() {
        let src = r#"
[[tabs]]
id = "main"
label = "Main"

[[tabs.fields]]
key = "host"
label = "Host"
widget = "text_input"

[[tabs.fields.option_states]]
value = "0"
when = "x"
"#;
        let errs = parse(src).unwrap().validate().unwrap_err();
        assert!(errs.iter().any(|e| matches!(
            e,
            SchemaValidationError::OptionStatesUnsupportedWidget { widget, .. } if widget == "text_input"
        )));
    }

    #[test]
    fn error_option_state_conflict() {
        let src = r#"
[[constraints]]
id = "c"
expr = "true"

[[tabs]]
id = "main"
label = "Main"

[[tabs.fields]]
key = "mode"
label = "Mode"
widget = "select"
options = ["a", "b"]

[[tabs.fields.option_states]]
value = "a"
when = "c"
enabled = "true"
"#;
        let errs = parse(src).unwrap().validate().unwrap_err();
        assert!(errs.iter().any(|e| matches!(
            e,
            SchemaValidationError::OptionStateConflict { value, .. } if value == "a"
        )));
    }

    #[test]
    fn error_option_state_missing_rule() {
        let src = r#"
[[tabs]]
id = "main"
label = "Main"

[[tabs.fields]]
key = "mode"
label = "Mode"
widget = "select"
options = ["a"]

[[tabs.fields.option_states]]
value = "a"
"#;
        let errs = parse(src).unwrap().validate().unwrap_err();
        assert!(errs.iter().any(|e| matches!(
            e,
            SchemaValidationError::OptionStateMissingRule { value, .. } if value == "a"
        )));
    }

    #[test]
    fn error_duplicate_option_state_value() {
        let src = r#"
[[constraints]]
id = "c"
expr = "true"

[[tabs]]
id = "main"
label = "Main"

[[tabs.fields]]
key = "n"
label = "N"
widget = "segmented_control"
options = ["0", "1"]

[[tabs.fields.option_states]]
value = "0"
when = "c"

[[tabs.fields.option_states]]
value = "0"
enabled = "true"
"#;
        let errs = parse(src).unwrap().validate().unwrap_err();
        assert!(errs.iter().any(|e| matches!(
            e,
            SchemaValidationError::DuplicateOptionStateValue { value, .. } if value == "0"
        )));
    }

    #[test]
    fn error_unknown_option_value() {
        let src = r#"
[[constraints]]
id = "c"
expr = "true"

[[tabs]]
id = "main"
label = "Main"

[[tabs.fields]]
key = "n"
label = "N"
widget = "segmented_control"
options = ["1", "2"]

[[tabs.fields.option_states]]
value = "0"
when = "c"
"#;
        let errs = parse(src).unwrap().validate().unwrap_err();
        assert!(errs.iter().any(|e| matches!(
            e,
            SchemaValidationError::UnknownOptionValue { value, .. } if value == "0"
        )));
    }

    #[test]
    fn validate_section_map_field() {
        let src = r#"
[[constraints]]
id = "c"
expr = "true"
message = "nope"

[[tabs]]
id = "main"
label = "Main"

[tabs.section_map]
key_prefix = "profiles"
allow_add_remove = false

[[tabs.section_map.fields]]
key = "name"
label = "Name"
widget = "text_input"
validate = "c"
"#;
        parse_and_validate(src).unwrap();
    }

    #[test]
    fn demo_schema_file_validates() {
        let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../demo/schema.toml");
        check_schema_file(&path).expect("demo schema should be valid");
    }

    #[test]
    fn parse_migration_block() {
        let src = r#"
schema_version = 2

[[migration]]
version = 2

  [[migration.rename]]
  from = "display.font_size"
  to   = "general.font_size"

  [[migration.delete]]
  key = "display.tick_rate"

  [[migration.transform]]
  from  = "mode"
  to    = "vivarium.enabled"
  type  = "enum_to_bool"
  match = "vivarium"

[[tabs]]
id = "main"
label = "Main"
"#;
        let schema = parse_and_validate(src).unwrap();
        assert_eq!(schema.schema_version, Some(2));
        assert_eq!(schema.version_key(), "schema_version");
        assert_eq!(schema.migrations.len(), 1);
        let m = &schema.migrations[0];
        assert_eq!(m.rename.len(), 1);
        assert_eq!(m.delete.len(), 1);
        assert_eq!(m.transform.len(), 1);
        assert_eq!(m.transform[0].kind, TransformKind::EnumToBool);
        assert_eq!(m.transform[0].match_value.as_deref(), Some("vivarium"));
    }

    #[test]
    fn migration_version_key_override() {
        let src = r#"
schema_version = 1
migration_version_key = "config_schema_version"

[[migration]]
version = 1

[[tabs]]
id = "main"
label = "Main"
"#;
        let schema = parse_and_validate(src).unwrap();
        assert_eq!(schema.version_key(), "config_schema_version");
    }

    #[test]
    fn error_duplicate_migration_version() {
        let src = r#"
schema_version = 2

[[migration]]
version = 2

[[migration]]
version = 2

[[tabs]]
id = "main"
label = "Main"
"#;
        let errs = parse(src).unwrap().validate().unwrap_err();
        assert!(errs.iter().any(|e| matches!(
            e,
            SchemaValidationError::DuplicateMigrationVersion { version } if *version == 2
        )));
    }

    #[test]
    fn error_migration_version_exceeds_target() {
        let src = r#"
schema_version = 1

[[migration]]
version = 2

[[tabs]]
id = "main"
label = "Main"
"#;
        let errs = parse(src).unwrap().validate().unwrap_err();
        assert!(errs.iter().any(|e| matches!(
            e,
            SchemaValidationError::MigrationVersionExceedsTarget { version, target }
                if *version == 2 && *target == 1
        )));
    }

    #[test]
    fn error_migrations_without_schema_version() {
        let src = r#"
[[migration]]
version = 1

[[tabs]]
id = "main"
label = "Main"
"#;
        let errs = parse(src).unwrap().validate().unwrap_err();
        assert!(errs
            .iter()
            .any(|e| matches!(e, SchemaValidationError::MigrationsWithoutSchemaVersion)));
    }

    #[test]
    fn error_transform_missing_match() {
        let src = r#"
schema_version = 1

[[migration]]
version = 1

  [[migration.transform]]
  from = "mode"
  to   = "vivarium.enabled"
  type = "enum_to_bool"

[[tabs]]
id = "main"
label = "Main"
"#;
        let errs = parse(src).unwrap().validate().unwrap_err();
        assert!(errs
            .iter()
            .any(|e| matches!(e, SchemaValidationError::TransformMissingMatch { .. })));
    }

    #[test]
    fn transform_rename_kind_allows_missing_match() {
        let src = r#"
schema_version = 1

[[migration]]
version = 1

  [[migration.transform]]
  from = "old.key"
  to   = "new.key"
  type = "rename"

[[tabs]]
id = "main"
label = "Main"
"#;
        parse_and_validate(src).unwrap();
    }

    #[test]
    fn error_invalid_cel_expression() {
        let src = r#"
[[constraints]]
id = "bad"
expr = "1 + "
message = "nope"

[[tabs]]
id = "main"
label = "Main"
"#;
        let errs = parse(src).unwrap().validate().unwrap_err();
        assert!(errs.iter().any(|e| matches!(
            e,
            SchemaValidationError::InvalidCelExpression { .. }
        )));
    }
}
