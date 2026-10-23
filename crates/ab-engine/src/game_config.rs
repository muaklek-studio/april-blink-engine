//! The game's configuration: named values shared by every system and script
//! (`Player.MaxHealth`, `Player.MoveSpeed`, `Game.Gravity`, `Game.DefaultLanguage`), in one
//! project file, `config.toml`, named by `game.toml` (`config = "config.toml"`).
//!
//! Each table is a group; each key a value, written plainly or with its rules:
//!
//! ```toml
//! [Player]
//! MoveSpeed = 5.5                                   # a setting: scripts may change it
//! MaxHealth = { value = 100, min = 1, max = 1000, constant = true, description = "Hit points" }
//!
//! [Game]
//! Gravity = { value = -9.81, min = -100.0, max = 0.0 }
//! Difficulty = { value = "normal", options = ["easy", "normal", "hard"] }
//! DefaultLanguage = { value = "en", constant = true }
//! ```
//!
//! A value's type (bool, integer, number, text) comes from what is written and never
//! changes: setting `MoveSpeed` to text is refused, and so is a value outside `min..=max`,
//! one not in `options`, or any change to a `constant`. Every refusal says which key and
//! why. The written value is the default [`GameConfig::reset`] goes back to.

use std::collections::BTreeMap;
use std::fmt;

/// A configuration value.
#[derive(Debug, Clone, PartialEq)]
pub enum ConfigValue {
    Bool(bool),
    Int(i64),
    Float(f64),
    Text(String),
}

impl ConfigValue {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Bool(_) => "a bool",
            Self::Int(_) => "an integer",
            Self::Float(_) => "a number",
            Self::Text(_) => "text",
        }
    }

    /// The value as a number (integers too).
    pub fn as_f64(&self) -> Option<f64> {
        match *self {
            Self::Int(v) => Some(v as f64),
            Self::Float(v) => Some(v),
            _ => None,
        }
    }

    fn to_toml(&self) -> toml::Value {
        match self {
            Self::Bool(v) => toml::Value::Boolean(*v),
            Self::Int(v) => toml::Value::Integer(*v),
            Self::Float(v) => toml::Value::Float(*v),
            Self::Text(v) => toml::Value::String(v.clone()),
        }
    }

    fn from_toml(value: &toml::Value) -> Option<Self> {
        Some(match value {
            toml::Value::Boolean(v) => Self::Bool(*v),
            toml::Value::Integer(v) => Self::Int(*v),
            toml::Value::Float(v) => Self::Float(*v),
            toml::Value::String(v) => Self::Text(v.clone()),
            _ => return None,
        })
    }
}

impl fmt::Display for ConfigValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Bool(v) => write!(f, "{v}"),
            Self::Int(v) => write!(f, "{v}"),
            Self::Float(v) => write!(f, "{v}"),
            Self::Text(v) => write!(f, "{v:?}"),
        }
    }
}

/// One named value and its rules.
#[derive(Debug, Clone, PartialEq)]
pub struct ConfigEntry {
    pub value: ConfigValue,
    /// The value written in the file.
    pub default: ConfigValue,
    /// Fixed for the whole game: scripts cannot change it.
    pub constant: bool,
    pub min: Option<f64>,
    pub max: Option<f64>,
    /// The texts a text value may be.
    pub options: Vec<String>,
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum ConfigError {
    #[error("config.toml: {0}")]
    Parse(String),
    #[error("`{key}`: {reason}")]
    Invalid { key: String, reason: String },
    #[error("no config value `{key}`{hint}")]
    Unknown { key: String, hint: String },
    #[error("`{key}` is a constant and cannot change")]
    Constant { key: String },
    #[error("`{key}` is {expected}, not {found}")]
    WrongType {
        key: String,
        expected: &'static str,
        found: &'static str,
    },
    #[error("`{key}` must be within {min}..={max}, not {value}")]
    OutOfRange {
        key: String,
        value: f64,
        min: String,
        max: String,
    },
    #[error("`{key}` must be one of {options:?}, not {value:?}")]
    NotAnOption {
        key: String,
        value: String,
        options: Vec<String>,
    },
}

/// Every configuration value (a resource; empty when the game has no config file).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct GameConfig {
    entries: BTreeMap<String, ConfigEntry>,
    revision: u64,
}

impl GameConfig {
    pub fn new() -> Self {
        Self::default()
    }

    /// Parses `config.toml` (see the module docs). Every problem of the file is reported,
    /// one per line, so they can all be fixed at once.
    pub fn from_toml(text: &str) -> Result<Self, ConfigError> {
        let table: toml::Table = text
            .parse()
            .map_err(|e: toml::de::Error| ConfigError::Parse(e.to_string()))?;
        let mut config = Self::default();
        let mut problems = Vec::new();
        for (group, values) in &table {
            let toml::Value::Table(values) = values else {
                problems.push(format!(
                    "`{group}` must be a [table] of values (like [Player])"
                ));
                continue;
            };
            for (name, raw) in values {
                let key = format!("{group}.{name}");
                match entry(raw) {
                    Ok(entry) => match check(&key, &entry, &entry.value) {
                        Ok(()) => {
                            config.entries.insert(key, entry);
                        }
                        Err(err) => problems.push(err.to_string()),
                    },
                    Err(reason) => problems.push(format!("`{key}`: {reason}")),
                }
            }
        }
        if problems.is_empty() {
            Ok(config)
        } else {
            Err(ConfigError::Parse(problems.join("\n")))
        }
    }

    /// The file again, with the current values as the written ones.
    pub fn to_toml(&self) -> String {
        let mut groups: BTreeMap<&str, toml::Table> = BTreeMap::new();
        for (key, entry) in &self.entries {
            let (group, name) = key.split_once('.').unwrap_or(("Game", key));
            let plain = !entry.constant
                && entry.min.is_none()
                && entry.max.is_none()
                && entry.options.is_empty()
                && entry.description.is_empty();
            let value = if plain {
                entry.value.to_toml()
            } else {
                let mut table = toml::Table::new();
                table.insert("value".into(), entry.value.to_toml());
                if entry.constant {
                    table.insert("constant".into(), true.into());
                }
                if let Some(min) = entry.min {
                    table.insert("min".into(), number(min, &entry.value));
                }
                if let Some(max) = entry.max {
                    table.insert("max".into(), number(max, &entry.value));
                }
                if !entry.options.is_empty() {
                    table.insert("options".into(), entry.options.clone().into());
                }
                if !entry.description.is_empty() {
                    table.insert("description".into(), entry.description.clone().into());
                }
                toml::Value::Table(table)
            };
            groups
                .entry(group)
                .or_default()
                .insert(name.to_owned(), value);
        }
        let mut out = String::new();
        for (group, values) in groups {
            if !out.is_empty() {
                out.push('\n');
            }
            out.push_str(&format!("[{group}]\n"));
            for (name, value) in values {
                let value = match value {
                    toml::Value::Table(table) => {
                        let fields: Vec<String> =
                            table.iter().map(|(k, v)| format!("{k} = {v}")).collect();
                        format!("{{ {} }}", fields.join(", "))
                    }
                    other => other.to_string(),
                };
                out.push_str(&format!("{name} = {value}\n"));
            }
        }
        out
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Every key and entry, sorted by key.
    pub fn entries(&self) -> impl Iterator<Item = (&str, &ConfigEntry)> {
        self.entries.iter().map(|(k, e)| (k.as_str(), e))
    }

    pub fn entry(&self, key: &str) -> Result<&ConfigEntry, ConfigError> {
        self.entries.get(key).ok_or_else(|| self.unknown(key))
    }

    pub fn get(&self, key: &str) -> Result<&ConfigValue, ConfigError> {
        self.entry(key).map(|e| &e.value)
    }

    /// A number (an integer value counts).
    pub fn number(&self, key: &str) -> Result<f64, ConfigError> {
        let value = self.get(key)?;
        value.as_f64().ok_or_else(|| ConfigError::WrongType {
            key: key.to_owned(),
            expected: "a number",
            found: value.kind(),
        })
    }

    pub fn f32(&self, key: &str) -> Result<f32, ConfigError> {
        self.number(key).map(|v| v as f32)
    }

    pub fn int(&self, key: &str) -> Result<i64, ConfigError> {
        match self.get(key)? {
            ConfigValue::Int(v) => Ok(*v),
            other => Err(self.wrong(key, "an integer", other)),
        }
    }

    pub fn bool(&self, key: &str) -> Result<bool, ConfigError> {
        match self.get(key)? {
            ConfigValue::Bool(v) => Ok(*v),
            other => Err(self.wrong(key, "a bool", other)),
        }
    }

    pub fn text(&self, key: &str) -> Result<&str, ConfigError> {
        match self.get(key)? {
            ConfigValue::Text(v) => Ok(v),
            other => Err(self.wrong(key, "text", other)),
        }
    }

    /// Changes a setting, if the value fits its rules. An integer given for a number is
    /// taken as that number.
    pub fn set(&mut self, key: &str, value: ConfigValue) -> Result<(), ConfigError> {
        let entry = self.entries.get(key).ok_or_else(|| self.unknown(key))?;
        if entry.constant {
            return Err(ConfigError::Constant {
                key: key.to_owned(),
            });
        }
        let value = match (&entry.default, value) {
            (ConfigValue::Float(_), ConfigValue::Int(v)) => ConfigValue::Float(v as f64),
            (_, value) => value,
        };
        check(key, entry, &value)?;
        let entry = self.entries.get_mut(key).expect("found above");
        if entry.value != value {
            entry.value = value;
            self.revision += 1;
        }
        Ok(())
    }

    /// Puts a setting back to its written value.
    pub fn reset(&mut self, key: &str) -> Result<(), ConfigError> {
        let default = self.entry(key)?.default.clone();
        let entry = self.entries.get_mut(key).expect("found above");
        if entry.value != default {
            entry.value = default;
            self.revision += 1;
        }
        Ok(())
    }

    /// Makes the current values the written ones (the editor saving its edits).
    pub fn commit(&mut self) {
        for entry in self.entries.values_mut() {
            entry.default = entry.value.clone();
        }
    }

    /// Bumped on every change, for systems that react to settings.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    fn wrong(&self, key: &str, expected: &'static str, found: &ConfigValue) -> ConfigError {
        ConfigError::WrongType {
            key: key.to_owned(),
            expected,
            found: found.kind(),
        }
    }

    /// An unknown key, with the closest known one when it looks like a typo.
    fn unknown(&self, key: &str) -> ConfigError {
        let lower = key.to_ascii_lowercase();
        let close = self
            .entries
            .keys()
            .map(|k| (distance(&k.to_ascii_lowercase(), &lower), k))
            .filter(|(d, _)| *d <= 2)
            .min();
        ConfigError::Unknown {
            key: key.to_owned(),
            hint: close
                .map(|(_, k)| format!(" (did you mean `{k}`?)"))
                .unwrap_or_default(),
        }
    }
}

fn number(v: f64, like: &ConfigValue) -> toml::Value {
    match like {
        ConfigValue::Int(_) if v.fract() == 0.0 => toml::Value::Integer(v as i64),
        _ => toml::Value::Float(v),
    }
}

/// An entry from a plain value or a `{ value = …, … }` table.
fn entry(raw: &toml::Value) -> Result<ConfigEntry, String> {
    let plain = |value: ConfigValue| ConfigEntry {
        default: value.clone(),
        value,
        constant: false,
        min: None,
        max: None,
        options: Vec::new(),
        description: String::new(),
    };
    let toml::Value::Table(table) = raw else {
        return ConfigValue::from_toml(raw)
            .map(plain)
            .ok_or_else(|| "must be a bool, a number, text or { value = … }".to_owned());
    };
    for key in table.keys() {
        if !["value", "constant", "min", "max", "options", "description"].contains(&key.as_str()) {
            return Err(format!(
                "unknown field `{key}` (use value, constant, min, max, options, description)"
            ));
        }
    }
    let value = table.get("value").ok_or("needs `value`")?;
    let value = ConfigValue::from_toml(value).ok_or("`value` must be a bool, a number or text")?;
    let bound = |name: &str| -> Result<Option<f64>, String> {
        match table.get(name) {
            None => Ok(None),
            Some(toml::Value::Integer(v)) => Ok(Some(*v as f64)),
            Some(toml::Value::Float(v)) => Ok(Some(*v)),
            Some(_) => Err(format!("`{name}` must be a number")),
        }
    };
    let (min, max) = (bound("min")?, bound("max")?);
    if (min.is_some() || max.is_some()) && value.as_f64().is_none() {
        return Err("`min` and `max` are for numbers".into());
    }
    if let (Some(min), Some(max)) = (min, max) {
        if min > max {
            return Err(format!("`min` ({min}) is above `max` ({max})"));
        }
    }
    let options = match table.get("options") {
        None => Vec::new(),
        Some(toml::Value::Array(items)) => items
            .iter()
            .map(|v| v.as_str().map(str::to_owned))
            .collect::<Option<Vec<_>>>()
            .ok_or("`options` must be a list of texts")?,
        Some(_) => return Err("`options` must be a list of texts".into()),
    };
    if !options.is_empty() && !matches!(value, ConfigValue::Text(_)) {
        return Err("`options` are for text values".into());
    }
    let constant = match table.get("constant") {
        None => false,
        Some(toml::Value::Boolean(v)) => *v,
        Some(_) => return Err("`constant` must be true or false".into()),
    };
    let description = match table.get("description") {
        None => String::new(),
        Some(toml::Value::String(v)) => v.clone(),
        Some(_) => return Err("`description` must be text".into()),
    };
    Ok(ConfigEntry {
        default: value.clone(),
        value,
        constant,
        min,
        max,
        options,
        description,
    })
}

/// Whether `value` fits `entry`'s type and rules.
fn check(key: &str, entry: &ConfigEntry, value: &ConfigValue) -> Result<(), ConfigError> {
    if std::mem::discriminant(value) != std::mem::discriminant(&entry.default) {
        return Err(ConfigError::WrongType {
            key: key.to_owned(),
            expected: entry.default.kind(),
            found: value.kind(),
        });
    }
    if let Some(v) = value.as_f64() {
        let low = entry.min.is_some_and(|min| v < min);
        let high = entry.max.is_some_and(|max| v > max);
        if low || high || !v.is_finite() {
            let bound = |b: Option<f64>| b.map_or_else(|| "…".to_owned(), |b| b.to_string());
            return Err(ConfigError::OutOfRange {
                key: key.to_owned(),
                value: v,
                min: bound(entry.min),
                max: bound(entry.max),
            });
        }
    }
    if let ConfigValue::Text(text) = value {
        if !entry.options.is_empty() && !entry.options.contains(text) {
            return Err(ConfigError::NotAnOption {
                key: key.to_owned(),
                value: text.clone(),
                options: entry.options.clone(),
            });
        }
    }
    Ok(())
}

/// Edit distance, for typo hints.
fn distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut previous = row[0];
        row[0] = i + 1;
        for (j, &cb) in b.iter().enumerate() {
            let here = row[j + 1];
            row[j + 1] = (row[j + 1] + 1)
                .min(row[j] + 1)
                .min(previous + usize::from(ca != cb));
            previous = here;
        }
    }
    row[b.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    const CONFIG: &str = r#"
[Player]
MoveSpeed = 5.5
MaxHealth = { value = 100, min = 1, max = 1000, constant = true, description = "Hit points" }
GodMode = false

[Game]
Gravity = { value = -9.81, min = -100.0, max = 0.0 }
Difficulty = { value = "normal", options = ["easy", "normal", "hard"] }
DefaultLanguage = { value = "en", constant = true }
"#;

    #[test]
    fn values_read_by_type() {
        let config = GameConfig::from_toml(CONFIG).unwrap();
        assert_eq!(config.len(), 6);
        assert_eq!(config.f32("Player.MoveSpeed").unwrap(), 5.5);
        assert_eq!(config.int("Player.MaxHealth").unwrap(), 100);
        assert_eq!(config.number("Player.MaxHealth").unwrap(), 100.0);
        assert!(!config.bool("Player.GodMode").unwrap());
        assert_eq!(config.text("Game.DefaultLanguage").unwrap(), "en");
        assert_eq!(
            config.entry("Player.MaxHealth").unwrap().description,
            "Hit points"
        );
        let err = config.int("Player.MoveSpeed").unwrap_err();
        assert_eq!(
            err.to_string(),
            "`Player.MoveSpeed` is an integer, not a number"
        );
        let err = config.get("Player.MaxHeath").unwrap_err();
        assert_eq!(
            err.to_string(),
            "no config value `Player.MaxHeath` (did you mean `Player.MaxHealth`?)"
        );
        assert!(!config
            .get("Nope.Nothing")
            .unwrap_err()
            .to_string()
            .contains("mean"));
    }

    #[test]
    fn settings_change_within_their_rules_and_constants_never() {
        let mut config = GameConfig::from_toml(CONFIG).unwrap();
        let revision = config.revision();
        config.set("Player.MoveSpeed", ConfigValue::Int(7)).unwrap();
        assert_eq!(
            config.get("Player.MoveSpeed").unwrap(),
            &ConfigValue::Float(7.0)
        );
        assert!(config.revision() > revision);
        assert!(matches!(
            config.set("Player.MaxHealth", ConfigValue::Int(5)),
            Err(ConfigError::Constant { .. })
        ));
        assert!(matches!(
            config.set("Player.MoveSpeed", ConfigValue::Text("fast".into())),
            Err(ConfigError::WrongType { .. })
        ));
        let err = config
            .set("Game.Gravity", ConfigValue::Float(3.0))
            .unwrap_err();
        assert_eq!(
            err.to_string(),
            "`Game.Gravity` must be within -100..=0, not 3"
        );
        assert!(matches!(
            config.set("Game.Difficulty", ConfigValue::Text("insane".into())),
            Err(ConfigError::NotAnOption { .. })
        ));
        config
            .set("Game.Difficulty", ConfigValue::Text("hard".into()))
            .unwrap();
        config.reset("Game.Difficulty").unwrap();
        assert_eq!(config.text("Game.Difficulty").unwrap(), "normal");
        assert!(matches!(
            config.set("Game.Gravity", ConfigValue::Float(f64::NAN)),
            Err(ConfigError::OutOfRange { .. })
        ));
    }

    #[test]
    fn every_problem_of_a_file_is_reported() {
        let bad = r#"
loose = 3
[Player]
Speed = [1, 2]
Health = { value = 5, min = 10 }
Mode = { value = 1, options = ["a"] }
Typo = { value = 1, maximum = 3 }
Range = { value = 1, min = 5, max = 2 }
"#;
        let err = GameConfig::from_toml(bad).unwrap_err().to_string();
        for needle in [
            "`loose`",
            "`Player.Speed`",
            "`Player.Health` must be within 10..=…",
            "`Player.Mode`: `options` are for text",
            "unknown field `maximum`",
            "`min` (5) is above `max` (2)",
        ] {
            assert!(err.contains(needle), "{needle} in:\n{err}");
        }
        assert!(GameConfig::from_toml("not = [toml").is_err());
    }

    #[test]
    fn saving_keeps_the_rules() {
        let mut config = GameConfig::from_toml(CONFIG).unwrap();
        config
            .set("Player.MoveSpeed", ConfigValue::Float(6.25))
            .unwrap();
        config.commit();
        let text = config.to_toml();
        let again = GameConfig::from_toml(&text).unwrap();
        assert_eq!(again.f32("Player.MoveSpeed").unwrap(), 6.25);
        let health = again.entry("Player.MaxHealth").unwrap();
        assert!(health.constant && health.min == Some(1.0) && health.max == Some(1000.0));
        assert_eq!(again.entry("Game.Difficulty").unwrap().options.len(), 3);
        assert_eq!(again, {
            let mut c = config.clone();
            c.revision = 0;
            c
        });
    }
}
