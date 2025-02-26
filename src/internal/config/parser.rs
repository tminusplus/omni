use std::collections::HashMap;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Mutex;

use lazy_static::lazy_static;
use serde::Deserialize;
use serde::Serialize;

use crate::internal::config::config_loader;
use crate::internal::config::config_value::ConfigData;
use crate::internal::config::flush_config_loader;
use crate::internal::config::global_config_loader;
use crate::internal::config::up::UpConfig;
use crate::internal::config::ConfigSource;
use crate::internal::config::ConfigValue;
use crate::internal::env::cache_home;
use crate::internal::env::omni_git_env;
use crate::internal::env::shell_is_interactive;
use crate::internal::env::user_home;
use crate::internal::git::package_path_from_handle;
use crate::internal::git::package_root_path;
use crate::internal::git::update_git_repo;
use crate::internal::workdir;

lazy_static! {
    #[derive(Debug, Serialize, Deserialize, Clone)]
    pub static ref CONFIG_PER_PATH: Mutex<OmniConfigPerPath> = Mutex::new(OmniConfigPerPath::new());

    #[derive(Debug, Serialize, Deserialize, Clone)]
    pub static ref CONFIG_GLOBAL: OmniConfig = {
        let config_loader = global_config_loader();
        OmniConfig::from_config_value(&config_loader.raw_config)
    };

    #[derive(Debug, Serialize, Deserialize, Clone)]
    pub static ref DEFAULT_WORKTREE: String = {
        let home = user_home();
        let mut default_worktree_path = format!("{}/git", home);
        if !std::path::Path::new(&default_worktree_path).is_dir() {
            // Check if GOPATH is set and GOPATH/src exists and is a directory
            let gopath = std::env::var("GOPATH").unwrap_or_else(|_| "".to_string());
            if !gopath.is_empty() {
                let gopath_src = format!("{}/src", gopath);
                if std::path::Path::new(&gopath_src).is_dir() {
                    default_worktree_path = gopath_src;
                }
            }
        }
        default_worktree_path
    };
}

pub fn config(path: &str) -> OmniConfig {
    let path = std::fs::canonicalize(path)
        .unwrap_or(path.to_owned().into())
        .to_str()
        .unwrap()
        .to_owned();
    let mut config_per_path = CONFIG_PER_PATH.lock().unwrap();
    config_per_path.get(&path).clone()
}

pub fn flush_config(path: &str) {
    let path = std::fs::canonicalize(path)
        .unwrap_or(path.to_owned().into())
        .to_str()
        .unwrap()
        .to_owned();

    // Flush the config loader for the path
    flush_config_loader(&path);

    // Then flush the configuration
    let mut config_per_path = CONFIG_PER_PATH.lock().unwrap();
    config_per_path.config.remove(&path);
}

pub fn global_config() -> OmniConfig {
    (*CONFIG_GLOBAL).clone()
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct OmniConfigPerPath {
    config: HashMap<String, OmniConfig>,
}

impl OmniConfigPerPath {
    pub fn new() -> Self {
        Self {
            config: HashMap::new(),
        }
    }

    pub fn get(&mut self, path: &str) -> &OmniConfig {
        // Get the git root path, if any
        let wd = workdir(path);
        let key = if let Some(wd_root) = wd.root() {
            wd_root
        } else {
            path
        };

        // Get the config for the path
        if !self.config.contains_key(key) {
            let config_loader = config_loader(key);
            let new_config = OmniConfig::from_config_value(&config_loader.raw_config);
            self.config.insert(key.to_owned(), new_config);
        }

        self.config.get(key).unwrap()
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct OmniConfig {
    pub worktree: String,
    pub cache: CacheConfig,
    pub commands: HashMap<String, CommandDefinition>,
    pub command_match_min_score: f64,
    pub command_match_skip_prompt_if: MatchSkipPromptIfConfig,
    pub config_commands: ConfigCommandsConfig,
    pub makefile_commands: MakefileCommandsConfig,
    pub org: Vec<OrgConfig>,
    pub path: PathConfig,
    pub path_repo_updates: PathRepoUpdatesConfig,
    pub repo_path_format: String,
    pub env: HashMap<String, String>,
    pub cd: CdConfig,
    pub clone: CloneConfig,
    pub up: Option<UpConfig>,
    pub suggest_clone: SuggestCloneConfig,
    pub up_command: UpCommandConfig,
    pub shell_aliases: ShellAliasesConfig,
}

impl OmniConfig {
    pub fn from_config_value(config_value: &ConfigValue) -> Self {
        let mut commands_config = HashMap::new();
        if let Some(value) = config_value.get("commands") {
            for (key, value) in value.as_table().unwrap() {
                commands_config.insert(
                    key.to_string(),
                    CommandDefinition::from_config_value(&value),
                );
            }
        }

        let mut org_config = Vec::new();
        if let Some(value) = config_value.get("org") {
            for value in value.as_array().unwrap() {
                org_config.push(OrgConfig::from_config_value(&value));
            }
        }

        let mut env_config = HashMap::new();
        if let Some(value) = config_value.get("env") {
            for (key, value) in value.as_table().unwrap() {
                env_config.insert(key.to_string(), value.as_str().unwrap().to_string());
            }
        }

        Self {
            worktree: config_value
                .get_as_str("worktree")
                .unwrap_or_else(|| (*DEFAULT_WORKTREE).to_string()),
            cache: CacheConfig::from_config_value(&config_value.get("cache").unwrap()),
            commands: commands_config,
            command_match_min_score: config_value
                .get_as_float("command_match_min_score")
                .unwrap_or(0.12),
            command_match_skip_prompt_if: MatchSkipPromptIfConfig::from_config_value(
                config_value.get("command_match_skip_prompt_if"),
            ),
            config_commands: ConfigCommandsConfig::from_config_value(
                &config_value.get("config_commands").unwrap(),
            ),
            makefile_commands: MakefileCommandsConfig::from_config_value(
                &config_value.get("makefile_commands").unwrap(),
            ),
            org: org_config,
            path: PathConfig::from_config_value(&config_value.get("path").unwrap()),
            path_repo_updates: PathRepoUpdatesConfig::from_config_value(
                &config_value.get("path_repo_updates").unwrap(),
            ),
            repo_path_format: config_value
                .get_as_str("repo_path_format")
                .unwrap()
                .to_string(),
            env: env_config,
            cd: CdConfig::from_config_value(config_value.get("cd")),
            clone: CloneConfig::from_config_value(config_value.get("clone")),
            up: UpConfig::from_config_value(config_value.get("up")),
            suggest_clone: SuggestCloneConfig::from_config_value(config_value.get("suggest_clone")),
            up_command: UpCommandConfig::from_config_value(config_value.get("up_command")),
            shell_aliases: ShellAliasesConfig::from_config_value(config_value.get("shell_aliases")),
        }
    }

    pub fn worktree(&self) -> String {
        if let Some(omni_git) = omni_git_env() {
            return omni_git;
        }

        self.worktree.clone()
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct CacheConfig {
    pub path: String,
}

impl CacheConfig {
    fn from_config_value(config_value: &ConfigValue) -> Self {
        Self {
            path: match config_value.get("path") {
                Some(value) => value.as_str().unwrap().to_string(),
                None => cache_home(),
            },
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct CommandDefinition {
    pub desc: Option<String>,
    pub run: String,
    pub aliases: Vec<String>,
    pub syntax: Option<CommandSyntax>,
    pub category: Option<Vec<String>>,
    pub subcommands: Option<HashMap<String, CommandDefinition>>,
    pub source: ConfigSource,
}

impl CommandDefinition {
    fn from_config_value(config_value: &ConfigValue) -> Self {
        let syntax = match config_value.get("syntax") {
            Some(value) => CommandSyntax::from_config_value(&value),
            None => None,
        };

        let category = match config_value.get("category") {
            Some(value) => {
                let mut category = Vec::new();
                if value.is_array() {
                    for value in value.as_array().unwrap() {
                        category.push(value.as_str().unwrap().to_string());
                    }
                } else {
                    category.push(value.as_str().unwrap().to_string());
                }
                Some(category)
            }
            None => None,
        };

        let subcommands = match config_value.get("subcommands") {
            Some(value) => {
                let mut subcommands = HashMap::new();
                for (key, value) in value.as_table().unwrap() {
                    subcommands.insert(
                        key.to_string(),
                        CommandDefinition::from_config_value(&value),
                    );
                }
                Some(subcommands)
            }
            None => None,
        };

        let aliases = match config_value.get_as_array("aliases") {
            Some(value) => value
                .iter()
                .map(|value| value.as_str().unwrap().to_string())
                .collect(),
            None => vec![],
        };

        Self {
            desc: config_value
                .get("desc")
                .map(|value| value.as_str().unwrap().to_string()),
            run: config_value
                .get_as_str("run")
                .unwrap_or("true".to_string())
                .to_string(),
            aliases,
            syntax,
            category,
            subcommands,
            source: config_value.get_source().clone(),
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct CommandSyntax {
    pub usage: Option<String>,
    pub parameters: Vec<SyntaxOptArg>,
}

impl CommandSyntax {
    pub fn new() -> Self {
        CommandSyntax {
            usage: None,
            parameters: vec![],
        }
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = serde_yaml::Value::deserialize(deserializer)?;
        let config_value = ConfigValue::from_value(ConfigSource::Null, vec![], value);
        if let Some(command_syntax) = CommandSyntax::from_config_value(&config_value) {
            Ok(command_syntax)
        } else {
            Err(serde::de::Error::custom("invalid command syntax"))
        }
    }

    fn from_config_value(config_value: &ConfigValue) -> Option<Self> {
        let mut usage = None;
        let mut parameters = vec![];

        if let Some(array) = config_value.as_array() {
            parameters.extend(
                array
                    .iter()
                    .filter_map(|value| SyntaxOptArg::from_config_value(value, None)),
            );
        } else if let Some(table) = config_value.as_table() {
            let keys = [
                ("parameters", None),
                ("arguments", Some(true)),
                ("argument", Some(true)),
                ("options", Some(false)),
                ("option", Some(false)),
                ("optional", Some(false)),
            ];

            for (key, required) in keys {
                if let Some(value) = table.get(key) {
                    if let Some(value) = value.as_array() {
                        let arguments = value
                            .iter()
                            .filter_map(|value| SyntaxOptArg::from_config_value(value, required))
                            .collect::<Vec<SyntaxOptArg>>();
                        parameters.extend(arguments);
                    } else if let Some(arg) = SyntaxOptArg::from_config_value(value, required) {
                        parameters.push(arg);
                    }
                }
            }

            if let Some(value) = table.get("usage") {
                if let Some(value) = value.as_str() {
                    usage = Some(value.to_string());
                }
            }
        } else if let Some(value) = config_value.as_str() {
            usage = Some(value.to_string());
        }

        if parameters.is_empty() && usage.is_none() {
            return None;
        }

        Some(Self { usage, parameters })
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct SyntaxOptArg {
    pub name: String,
    pub desc: Option<String>,
    pub required: bool,
}

impl SyntaxOptArg {
    pub fn new(name: String, desc: Option<String>, required: bool) -> Self {
        Self {
            name,
            desc,
            required,
        }
    }

    fn from_config_value(config_value: &ConfigValue, required: Option<bool>) -> Option<Self> {
        let name;
        let mut desc = None;
        let mut required = required;

        if let Some(table) = config_value.as_table() {
            let value_for_details;

            if let Some(name_value) = table.get("name") {
                if let Some(name_value) = name_value.as_str() {
                    name = name_value.to_string();
                    value_for_details = Some(config_value.clone());
                } else {
                    return None;
                }
            } else if table.len() == 1 {
                if let Some((key, value)) = table.into_iter().next() {
                    name = key;
                    value_for_details = Some(value);
                } else {
                    return None;
                }
            } else {
                return None;
            }

            if let Some(value_for_details) = value_for_details {
                if let Some(value_str) = value_for_details.as_str() {
                    desc = Some(value_str.to_string());
                } else if let Some(value_table) = value_for_details.as_table() {
                    desc = value_table.get("desc")?.as_str();
                    if required.is_none() {
                        required = value_table.get("required")?.as_bool();
                    }
                }
            }
        } else {
            name = config_value.as_str().unwrap();
        }

        Some(Self {
            name,
            desc,
            required: required.unwrap_or(false),
        })
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct MatchSkipPromptIfConfig {
    pub enabled: bool,
    pub first_min: f64,
    pub second_max: f64,
}

impl MatchSkipPromptIfConfig {
    const DEFAULT_FIRST_MIN: f64 = 0.80;
    const DEFAULT_SECOND_MAX: f64 = 0.60;

    fn from_config_value(config_value: Option<ConfigValue>) -> Self {
        match config_value {
            Some(config_value) => Self {
                enabled: match config_value.get("enabled") {
                    Some(value) => value.as_bool().unwrap(),
                    None => {
                        config_value.get("first_min").is_some()
                            || config_value.get("second_max").is_some()
                    }
                },
                first_min: match config_value.get("first_min") {
                    Some(value) => value.as_float().unwrap(),
                    None => Self::DEFAULT_FIRST_MIN,
                },
                second_max: match config_value.get("second_max") {
                    Some(value) => value.as_float().unwrap(),
                    None => Self::DEFAULT_SECOND_MAX,
                },
            },
            None => Self {
                enabled: false,
                first_min: Self::DEFAULT_FIRST_MIN,
                second_max: Self::DEFAULT_SECOND_MAX,
            },
        }
    }

    fn default() -> Self {
        Self::from_config_value(None)
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ConfigCommandsConfig {
    pub split_on_dash: bool,
    pub split_on_slash: bool,
}

impl ConfigCommandsConfig {
    fn from_config_value(config_value: &ConfigValue) -> Self {
        Self {
            split_on_dash: match config_value.get("split_on_dash") {
                Some(value) => value.as_bool().unwrap(),
                None => true,
            },
            split_on_slash: match config_value.get("split_on_slash") {
                Some(value) => value.as_bool().unwrap(),
                None => true,
            },
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct MakefileCommandsConfig {
    pub enabled: bool,
    pub split_on_dash: bool,
    pub split_on_slash: bool,
}

impl MakefileCommandsConfig {
    fn from_config_value(config_value: &ConfigValue) -> Self {
        Self {
            enabled: match config_value.get("enabled") {
                Some(value) => value.as_bool().unwrap(),
                None => true,
            },
            split_on_dash: match config_value.get("split_on_dash") {
                Some(value) => value.as_bool().unwrap(),
                None => true,
            },
            split_on_slash: match config_value.get("split_on_slash") {
                Some(value) => value.as_bool().unwrap(),
                None => true,
            },
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct OrgConfig {
    pub handle: String,
    pub trusted: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub worktree: Option<String>,
}

impl OrgConfig {
    pub fn from_str(value_str: &str) -> Self {
        let mut split = value_str.split('=');
        let handle = split.next().unwrap().to_string();
        let worktree = split.next().map(|value| value.to_string());
        Self {
            handle,
            trusted: true,
            worktree,
        }
    }

    pub fn from_config_value(config_value: &ConfigValue) -> Self {
        // If the config_value contains a value directly, we want to consider
        // it as the "handle=worktree", and not as a table.
        if config_value.is_str() {
            let value_str = config_value.as_str().unwrap();
            return OrgConfig::from_str(&value_str);
        }

        Self {
            handle: config_value.get_as_str("handle").unwrap().to_string(),
            trusted: config_value.get_as_bool("trusted").unwrap_or(false),
            worktree: config_value.get_as_str("worktree"),
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct PathConfig {
    pub append: Vec<PathEntryConfig>,
    pub prepend: Vec<PathEntryConfig>,
}

impl PathConfig {
    fn from_config_value(config_value: &ConfigValue) -> Self {
        Self {
            append: config_value
                .get_as_array("append")
                .unwrap()
                .iter()
                .map(PathEntryConfig::from_config_value)
                .collect(),
            prepend: config_value
                .get_as_array("prepend")
                .unwrap()
                .iter()
                .map(PathEntryConfig::from_config_value)
                .collect(),
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct PathEntryConfig {
    pub path: String,
    pub package: Option<String>,
    pub full_path: String,
}

impl PathEntryConfig {
    pub fn from_path(path: &str) -> Self {
        Self {
            path: path.to_string(),
            package: None,
            full_path: if path.starts_with('/') {
                path.to_string()
            } else {
                "".to_string()
            },
        }
    }

    pub fn from_config_value(config_value: &ConfigValue) -> Self {
        if config_value.is_table() {
            let path = config_value
                .get_as_str("path")
                .unwrap_or("".to_string())
                .to_string();

            if !path.starts_with('/') {
                if let Some(package) = config_value.get("package") {
                    let package = package.as_str().unwrap();
                    if let Some(package_path) = package_path_from_handle(&package) {
                        let mut full_path = package_path;
                        if !path.is_empty() {
                            full_path = full_path.join(path.clone());
                        }

                        return Self {
                            path: path.clone(),
                            package: Some(package.to_string()),
                            full_path: full_path.to_str().unwrap().to_string(),
                        };
                    }
                }
            }

            Self {
                path: path.clone(),
                package: None,
                full_path: path,
            }
        } else {
            let path = config_value.as_str().unwrap_or("".to_string()).to_string();
            Self {
                path: path.clone(),
                package: None,
                full_path: path,
            }
        }
    }

    pub fn as_config_value(&self) -> ConfigValue {
        if let Some(package) = &self.package {
            let mut map = HashMap::new();
            map.insert("path".to_string(), ConfigValue::from_str(&self.path));
            map.insert("package".to_string(), ConfigValue::from_str(package));
            ConfigValue::new(
                ConfigSource::Null,
                vec![],
                Some(Box::new(ConfigData::Mapping(map))),
            )
        } else {
            ConfigValue::from_str(&self.full_path)
        }
    }

    pub fn is_package(&self) -> bool {
        self.package.is_some() || PathBuf::from(&self.full_path).starts_with(package_root_path())
    }

    pub fn package_path(&self) -> Option<PathBuf> {
        if let Some(package) = &self.package {
            return package_path_from_handle(package);
        }

        None
    }

    pub fn is_valid(&self) -> bool {
        !self.full_path.is_empty() && self.full_path.starts_with('/')
    }

    pub fn as_string(&self) -> String {
        self.full_path.clone()
    }

    pub fn starts_with(&self, path_entry: &PathEntryConfig) -> bool {
        if !self.is_valid() {
            return false;
        }

        PathBuf::from(&self.full_path).starts_with(&path_entry.full_path)
    }

    pub fn includes_path(&self, path: PathBuf) -> bool {
        if !self.is_valid() {
            return false;
        }

        PathBuf::from(&path).starts_with(&self.full_path)
    }

    pub fn replace(&mut self, path_from: &PathEntryConfig, path_to: &PathEntryConfig) -> bool {
        if self.starts_with(path_from) {
            let new_full_path = format!(
                "{}/{}",
                path_to.full_path,
                PathBuf::from(&self.full_path)
                    .strip_prefix(&path_from.full_path)
                    .unwrap()
                    .display(),
            );
            if let Some(package) = path_to.package.clone() {
                if let Some(package_path) = package_path_from_handle(&package) {
                    self.full_path = new_full_path;
                    self.package = Some(package);
                    self.path = PathBuf::from(&self.full_path)
                        .strip_prefix(&package_path)
                        .unwrap()
                        .display()
                        .to_string();

                    return true;
                }
            } else {
                self.full_path = new_full_path;
                self.package = None;
                self.path = self.full_path.clone();

                return true;
            }
        }
        false
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct PathRepoUpdatesConfig {
    pub enabled: bool,
    pub self_update: PathRepoUpdatesSelfUpdateEnum,
    pub pre_auth: bool,
    pub pre_auth_timeout: u64,
    pub background_updates: bool,
    pub background_updates_timeout: u64,
    pub interval: u64,
    pub ref_type: String,
    pub ref_match: Option<String>,
    pub per_repo_config: HashMap<String, PathRepoUpdatesPerRepoConfig>,
}

impl PathRepoUpdatesConfig {
    fn from_config_value(config_value: &ConfigValue) -> Self {
        let mut per_repo_config = HashMap::new();
        if let Some(value) = config_value.get("per_repo_config") {
            for (key, value) in value.as_table().unwrap() {
                per_repo_config.insert(
                    key.to_string(),
                    PathRepoUpdatesPerRepoConfig::from_config_value(&value),
                );
            }
        };

        Self {
            enabled: config_value.get_as_bool("enabled").unwrap_or(true),
            self_update: match (
                config_value.get_as_str("self_update"),
                config_value.get_as_bool("self_update"),
            ) {
                (_, Some(value)) => match value {
                    true => PathRepoUpdatesSelfUpdateEnum::True,
                    false => PathRepoUpdatesSelfUpdateEnum::False,
                },
                (Some(value), _) => match value.to_lowercase().as_str() {
                    "true" | "yes" | "y" => PathRepoUpdatesSelfUpdateEnum::True,
                    "false" | "no" | "n" => PathRepoUpdatesSelfUpdateEnum::False,
                    "nocheck" => PathRepoUpdatesSelfUpdateEnum::NoCheck,
                    "ask" => PathRepoUpdatesSelfUpdateEnum::Ask,
                    _ => PathRepoUpdatesSelfUpdateEnum::Ask,
                },
                (None, None) => PathRepoUpdatesSelfUpdateEnum::Ask,
            },
            pre_auth: config_value.get_as_bool("pre_auth").unwrap_or(true),
            pre_auth_timeout: config_value
                .get_as_unsigned_integer("pre_auth_timeout")
                .unwrap_or(120),
            background_updates: config_value
                .get_as_bool("background_updates")
                .unwrap_or(true),
            background_updates_timeout: config_value
                .get_as_unsigned_integer("background_updates_timeout")
                .unwrap_or(3600),
            interval: config_value
                .get_as_unsigned_integer("interval")
                .unwrap_or(12 * 60 * 60),
            ref_type: config_value
                .get_as_str("ref_type")
                .unwrap_or("branch".to_string()),
            ref_match: config_value.get_as_str("ref_match"),
            per_repo_config,
        }
    }

    pub fn update_config(&self, repo_id: &str) -> (bool, String, Option<String>) {
        match self.per_repo_config.get(repo_id) {
            Some(value) => (
                value.enabled,
                value.ref_type.clone(),
                value.ref_match.clone(),
            ),
            None => (self.enabled, self.ref_type.clone(), self.ref_match.clone()),
        }
    }

    pub fn update(&self, repo_id: &str) -> bool {
        let (enabled, ref_type, ref_match) = self.update_config(repo_id);

        if !enabled {
            return false;
        }

        update_git_repo(repo_id, ref_type, ref_match, None, None).unwrap_or(false)
    }
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub enum PathRepoUpdatesSelfUpdateEnum {
    True,
    False,
    NoCheck,
    #[serde(other)]
    Ask,
}

impl PathRepoUpdatesSelfUpdateEnum {
    // pub fn is_true(&self) -> bool {
    // match self {
    // PathRepoUpdatesSelfUpdateEnum::True => true,
    // _ => false,
    // }
    // }

    pub fn do_not_check(&self) -> bool {
        matches!(self, PathRepoUpdatesSelfUpdateEnum::NoCheck)
    }

    pub fn is_false(&self) -> bool {
        match self {
            PathRepoUpdatesSelfUpdateEnum::False => true,
            PathRepoUpdatesSelfUpdateEnum::Ask => !shell_is_interactive(),
            _ => false,
        }
    }

    pub fn is_ask(&self) -> bool {
        match self {
            PathRepoUpdatesSelfUpdateEnum::Ask => shell_is_interactive(),
            _ => false,
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct PathRepoUpdatesPerRepoConfig {
    pub enabled: bool,
    pub ref_type: String,
    pub ref_match: Option<String>,
}

impl PathRepoUpdatesPerRepoConfig {
    fn from_config_value(config_value: &ConfigValue) -> Self {
        Self {
            enabled: match config_value.get("enabled") {
                Some(value) => value.as_bool().unwrap(),
                None => true,
            },
            ref_type: match config_value.get("ref_type") {
                Some(value) => value.as_str().unwrap().to_string(),
                None => "branch".to_string(),
            },
            ref_match: config_value
                .get("ref_match")
                .map(|value| value.as_str().unwrap().to_string()),
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct CdConfig {
    pub path_match_min_score: f64,
    pub path_match_skip_prompt_if: MatchSkipPromptIfConfig,
}

impl CdConfig {
    const DEFAULT_PATH_MATCH_MIN_SCORE: f64 = 0.12;

    fn from_config_value(config_value: Option<ConfigValue>) -> Self {
        if config_value.is_none() {
            return Self {
                path_match_min_score: Self::DEFAULT_PATH_MATCH_MIN_SCORE,
                path_match_skip_prompt_if: MatchSkipPromptIfConfig::default(),
            };
        }
        let config_value = config_value.unwrap();

        Self {
            path_match_min_score: match config_value.get("path_match_min_score") {
                Some(value) => value.as_float().unwrap(),
                None => Self::DEFAULT_PATH_MATCH_MIN_SCORE,
            },
            path_match_skip_prompt_if: MatchSkipPromptIfConfig::from_config_value(
                config_value.get("path_match_skip_prompt_if"),
            ),
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct CloneConfig {
    pub ls_remote_timeout_seconds: u64,
}

impl CloneConfig {
    const DEFAULT_LS_REMOTE_TIMEOUT_SECONDS: u64 = 5;

    fn from_config_value(config_value: Option<ConfigValue>) -> Self {
        if config_value.is_none() {
            return Self {
                ls_remote_timeout_seconds: Self::DEFAULT_LS_REMOTE_TIMEOUT_SECONDS,
            };
        }
        let config_value = config_value.unwrap();

        Self {
            ls_remote_timeout_seconds: match config_value
                .get_as_unsigned_integer("ls_remote_timeout_seconds")
            {
                Some(value) => value,
                None => Self::DEFAULT_LS_REMOTE_TIMEOUT_SECONDS,
            },
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct SuggestCloneConfig {
    pub repositories: Vec<SuggestCloneRepositoryConfig>,
}

impl SuggestCloneConfig {
    fn from_config_value(config_value: Option<ConfigValue>) -> Self {
        let mut repositories = vec![];

        if let Some(config_value) = config_value {
            // We can filter by values provided by the repository, as this is only
            // a repository-scoped configuration
            if let Some(config_value) = config_value.select_label("git_repo") {
                if let Some(array) = config_value.as_array() {
                    for value in array {
                        if let Some(repository) =
                            SuggestCloneRepositoryConfig::from_config_value(&value)
                        {
                            repositories.push(repository);
                        }
                    }
                } else if let Some(table) = config_value.as_table() {
                    if let Some(array) = table.get("repositories") {
                        if let Some(array) = array.as_array() {
                            for value in array {
                                if let Some(repository) =
                                    SuggestCloneRepositoryConfig::from_config_value(&value)
                                {
                                    repositories.push(repository);
                                }
                            }
                        }
                    }
                }
            }
        }

        Self { repositories }
    }
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub enum SuggestCloneTypeEnum {
    Package,
    Worktree,
}

impl FromStr for SuggestCloneTypeEnum {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "package" => Ok(Self::Package),
            "worktree" => Ok(Self::Worktree),
            _ => Err(format!("Invalid: {}", s)),
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct SuggestCloneRepositoryConfig {
    pub handle: String,
    pub args: Vec<String>,
    pub clone_type: SuggestCloneTypeEnum,
}

impl SuggestCloneRepositoryConfig {
    fn from_config_value(config_value: &ConfigValue) -> Option<Self> {
        if let Some(value) = config_value.as_str() {
            return Some(Self {
                handle: value.to_string(),
                args: vec![],
                clone_type: SuggestCloneTypeEnum::Package,
            });
        } else if let Some(table) = config_value.as_table() {
            let mut handle = None;
            if let Some(value) = table.get("handle") {
                if let Some(value) = value.as_str() {
                    handle = Some(value.to_string());
                }
            }

            handle.as_ref()?;

            let mut args = Vec::new();
            if let Some(value) = table.get("args") {
                if let Some(value) = value.as_str() {
                    if let Ok(value) = shell_words::split(&value) {
                        args.extend(value);
                    }
                }
            }

            let mut clone_type = SuggestCloneTypeEnum::Package;
            if let Some(value) = table.get("clone_type") {
                if let Some(value) = value.as_str() {
                    if let Ok(value) = SuggestCloneTypeEnum::from_str(&value) {
                        clone_type = value;
                    }
                }
            }

            return Some(Self {
                handle: handle.unwrap(),
                args,
                clone_type,
            });
        }

        None
    }

    pub fn clone_as_package(&self) -> bool {
        self.clone_type == SuggestCloneTypeEnum::Package
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct UpCommandConfig {
    pub auto_bootstrap: bool,
}

impl UpCommandConfig {
    fn from_config_value(config_value: Option<ConfigValue>) -> Self {
        if let Some(config_value) = config_value {
            if let Some(config_value) = config_value.reject_label("git_repo") {
                return Self {
                    auto_bootstrap: match config_value.get("auto_bootstrap") {
                        Some(value) => value.as_bool().unwrap(),
                        None => true,
                    },
                };
            }
        }

        Self {
            auto_bootstrap: true,
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ShellAliasesConfig {
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub aliases: Vec<ShellAliasConfig>,
}

impl ShellAliasesConfig {
    fn from_config_value(config_value: Option<ConfigValue>) -> Self {
        let mut aliases = vec![];
        if let Some(config_value) = config_value {
            if let Some(array) = config_value.as_array() {
                for value in array {
                    if let Some(alias) = ShellAliasConfig::from_config_value(&value) {
                        aliases.push(alias);
                    }
                }
            }
        }
        Self { aliases }
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ShellAliasConfig {
    #[serde(skip_serializing_if = "String::is_empty")]
    pub alias: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
}

impl ShellAliasConfig {
    fn from_config_value(config_value: &ConfigValue) -> Option<Self> {
        if let Some(value) = config_value.as_str() {
            return Some(Self {
                alias: value.to_string(),
                target: None,
            });
        } else if let Some(table) = config_value.as_table() {
            let mut alias = None;
            if let Some(value) = table.get("alias") {
                if let Some(value) = value.as_str() {
                    alias = Some(value.to_string());
                }
            }

            alias.as_ref()?;

            let mut target = None;
            if let Some(value) = table.get("target") {
                if let Some(value) = value.as_str() {
                    target = Some(value.to_string());
                }
            }

            return Some(Self {
                alias: alias.unwrap(),
                target,
            });
        }

        None
    }
}
