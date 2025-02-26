use std::collections::BTreeMap;
use std::collections::HashMap;
use std::collections::HashSet;
use std::sync::Mutex;

use lazy_static::lazy_static;

use strsim::normalized_damerau_levenshtein;

use crate::internal::commands::base::Command;
use crate::internal::commands::builtin::CdCommand;
use crate::internal::commands::builtin::CloneCommand;
use crate::internal::commands::builtin::ConfigBootstrapCommand;
use crate::internal::commands::builtin::ConfigPathSwitchCommand;
use crate::internal::commands::builtin::HelpCommand;
use crate::internal::commands::builtin::HookCommand;
use crate::internal::commands::builtin::HookEnvCommand;
use crate::internal::commands::builtin::HookInitCommand;
use crate::internal::commands::builtin::HookUuidCommand;
use crate::internal::commands::builtin::ScopeCommand;
use crate::internal::commands::builtin::StatusCommand;
use crate::internal::commands::builtin::TidyCommand;
use crate::internal::commands::builtin::UpCommand;
use crate::internal::commands::fromconfig::ConfigCommand;
use crate::internal::commands::frommakefile::MakefileCommand;
use crate::internal::commands::frompath::PathCommand;
use crate::internal::config;
use crate::internal::env::shell_is_interactive;
use crate::internal::user_interface::colors::StringColor;
use crate::omni_info;

lazy_static! {
    #[derive(Debug)]
    pub static ref COMMAND_LOADER_PER_PATH: Mutex<CommandLoaderPerPath> = Mutex::new(CommandLoaderPerPath::new());

    #[derive(Debug)]
    pub static ref COMMAND_LOADER: CommandLoader = command_loader(".");
}

pub fn command_loader(path: &str) -> CommandLoader {
    let path = std::fs::canonicalize(path)
        .unwrap_or(path.to_owned().into())
        .to_str()
        .unwrap()
        .to_owned();
    let mut command_loader_per_path = COMMAND_LOADER_PER_PATH.lock().unwrap();
    command_loader_per_path.get(&path).clone()
}

#[derive(Debug)]
pub struct CommandLoaderPerPath {
    loaders: HashMap<String, CommandLoader>,
}

impl CommandLoaderPerPath {
    fn new() -> Self {
        Self {
            loaders: HashMap::new(),
        }
    }

    pub fn get(&mut self, path: &str) -> &CommandLoader {
        if !self.loaders.contains_key(path) {
            self.loaders
                .insert(path.to_owned(), CommandLoader::new_with_path(path));
        }

        self.loaders.get(path).unwrap()
    }
}

#[derive(Debug, Clone)]
pub struct CommandLoader {
    pub commands: Vec<Command>,
}

impl CommandLoader {
    pub fn new_with_path(path: &str) -> Self {
        let mut commands = vec![];
        let mut seen = HashSet::new();

        // Load all builtins first
        commands.push(Command::BuiltinCd(CdCommand::new()));
        commands.push(Command::BuiltinClone(CloneCommand::new()));
        commands.push(Command::BuiltinConfigBootstrap(
            ConfigBootstrapCommand::new(),
        ));
        commands.push(Command::BuiltinConfigPathSwitch(
            ConfigPathSwitchCommand::new(),
        ));
        commands.push(Command::BuiltinHelp(HelpCommand::new()));
        commands.push(Command::BuiltinHook(HookCommand::new()));
        commands.push(Command::BuiltinHookEnv(HookEnvCommand::new()));
        commands.push(Command::BuiltinHookInit(HookInitCommand::new()));
        commands.push(Command::BuiltinHookUuid(HookUuidCommand::new()));
        commands.push(Command::BuiltinScope(ScopeCommand::new()));
        commands.push(Command::BuiltinStatus(StatusCommand::new()));
        commands.push(Command::BuiltinTidy(TidyCommand::new()));
        commands.push(Command::BuiltinUp(UpCommand::new()));

        // Add all the builtin to seen commands
        for command in commands.iter() {
            for name in command.all_names() {
                seen.insert(name);
            }
        }

        let mut add_fn = |command: Command| {
            let inserted = command
                .all_names()
                .iter()
                .filter(|&name| seen.insert(name.to_vec()))
                .count();
            if inserted > 0 {
                commands.push(command);
            }
        };

        // Look for all commands in the configuration
        for command in ConfigCommand::all() {
            add_fn(Command::FromConfig(command));
        }

        // Look for all commands in the path
        for command in PathCommand::all() {
            add_fn(Command::FromPath(command));
        }

        for command in MakefileCommand::all_from_path(path) {
            add_fn(Command::FromMakefile(command));
        }

        Self { commands }
    }

    pub fn to_serve(&self, argv: &[String]) -> Option<(&Command, Vec<String>, Vec<String>)> {
        let mut command: Option<&Command> = None;
        let mut cur_match_len = 0;

        for command_candidate in &self.commands {
            let match_len = command_candidate.serves(argv);
            if match_len > 0 && (command.is_none() || match_len > cur_match_len) {
                command = Some(command_candidate);
                cur_match_len = match_len;
            }
        }

        if let Some(command) = command {
            let called_as = argv[..cur_match_len].to_vec();
            let with_argv = argv[cur_match_len..].to_vec();
            Some((command, called_as, with_argv))
        } else {
            None
        }
    }

    pub fn has_subcommand_of(&self, argv: &[String]) -> bool {
        for command_candidate in &self.commands {
            if command_candidate.is_subcommand_of(argv) {
                return true;
            }
        }
        false
    }

    #[allow(dead_code)]
    pub fn sorted(&self) -> Vec<&Command> {
        let mut commands = self.commands.iter().collect::<Vec<&Command>>();
        commands.sort_by(|a, b| a.cmp_help(b));
        commands
    }

    pub fn complete(&self, comp_cword: usize, argv: Vec<String>, allow_delegate: bool) {
        // Prepare until which word we need to match
        let match_pos = comp_cword;

        #[derive(Debug, Clone)]
        struct MatchedCommand {
            command: Command,
            match_name: Vec<String>,
            match_level: f32,
        }
        let mut matched_commands = vec![];

        // Check how much each command matches until the match_pos
        for command in self.commands.iter() {
            for command_name in command.all_names() {
                let mut match_level: f32 = 0.0;
                let match_until = std::cmp::min(command_name.len(), match_pos + 1);
                for i in 0..match_until {
                    if argv.len() <= i {
                        break;
                    } else if command_name[i] == argv[i] {
                        match_level += 1.0;
                    } else if i == match_pos && command_name[i].starts_with(&argv[i]) {
                        match_level += 0.5;
                    } else {
                        match_level = -1.0;
                        break;
                    }
                }

                if match_level >= 0.0 {
                    matched_commands.push(MatchedCommand {
                        command: command.clone(),
                        match_name: command_name.clone(),
                        match_level,
                    });
                }
            }
        }

        // Get the highest matching score
        let max_match_level = matched_commands
            .iter()
            .fold(0.0, |acc: f32, x| acc.max(x.match_level));

        // Filter only the highest matching scores
        matched_commands.retain(|x| x.match_level == max_match_level);

        // If the score ends with .5, it means that we have a partial match, so we can
        // return the matching commands right away
        if max_match_level.fract() == 0.5 {
            for matched_command in matched_commands.iter() {
                println!("{}", matched_command.match_name[match_pos]);
            }
            return;
        }

        // If we have a full match, we also want to return it
        if max_match_level == match_pos as f32 + 1.0 {
            let matched_command = &matched_commands[0];
            println!("{}", matched_command.match_name[match_pos]);
            return;
        }

        // If we get here, and if there is a single command, then we can try and
        // delegate the autocompletion if supported by that command
        if allow_delegate && matched_commands.len() == 1 {
            let matched_command = &matched_commands[0];
            if matched_command.command.autocompletion() {
                // Set the environment variables that we need to pass to the
                // subcommand
                let new_comp_cword = comp_cword - matched_command.match_level as usize;

                // We want to edit the argv to remove the command name
                let new_argv = argv[matched_command.match_level as usize..].to_vec();

                matched_command
                    .command
                    .autocomplete(new_comp_cword, new_argv);
                return;
            }
        }

        // Finally, we can just return the list of commands that fit, if any
        for matched_command in matched_commands.iter() {
            if matched_command.match_name.len() <= match_pos {
                continue;
            }

            println!("{}", matched_command.match_name[match_pos]);
        }
    }

    pub fn find_command(&self, argv: &[String]) -> Option<(Command, Vec<String>, Vec<String>)> {
        let page_size = 7;

        // This preempt the score search if we are in interactive mode and the arguments
        // are prefix of an existing subcommand
        if shell_is_interactive() && !argv.is_empty() && self.has_subcommand_of(argv) {
            let mut subcommands = BTreeMap::new();
            for command in self.commands.iter() {
                command
                    .all_names_with_prefix(argv.to_vec())
                    .iter()
                    .for_each(|name| {
                        if !subcommands.contains_key(name) {
                            subcommands.insert(name.clone(), command.clone());
                        }
                    });
            }

            if !subcommands.is_empty() {
                // Convert the subcommands into two vectors, one with the names and
                // one with the commands; this is not the neatest way to do it, but
                // it's the easiest for now
                let mut sub_names = vec![];
                let mut sub_commands = vec![];
                for (name, command) in subcommands.iter() {
                    let full_name = argv
                        .iter()
                        .cloned()
                        .chain(name.iter().cloned())
                        .collect::<Vec<_>>()
                        .join(" ");
                    sub_names.push(full_name);
                    sub_commands.push(command.clone());
                }

                let question = if subcommands.len() > 1 {
                    requestty::Question::select("did_you_mean_command")
                        .ask_if_answered(true)
                        .on_esc(requestty::OnEsc::Terminate)
                        .message(format!(
                            "{} {}",
                            "omni:".light_cyan(),
                            "Did you mean?".yellow()
                        ))
                        .choices(sub_names.iter())
                        .should_loop(false)
                        .page_size(page_size)
                        .build()
                } else {
                    requestty::Question::confirm("did_you_mean_command")
                        .ask_if_answered(true)
                        .on_esc(requestty::OnEsc::Terminate)
                        .message(format!(
                            "{} {} {} {}",
                            "omni:".light_cyan(),
                            "Did you mean?".yellow(),
                            "·".light_black(),
                            sub_names[0].normal(),
                        ))
                        .default(true)
                        .build()
                };

                match requestty::prompt_one(question) {
                    Ok(answer) => match answer {
                        requestty::Answer::ListItem(listitem) => {
                            return Some((
                                sub_commands[listitem.index].clone(),
                                argv.to_vec(),
                                vec![],
                            ));
                        }
                        requestty::Answer::Bool(confirmed) => {
                            if confirmed {
                                return Some((sub_commands[0].clone(), argv.to_vec(), vec![]));
                            }
                        }
                        _ => {}
                    },
                    Err(err) => {
                        if page_size < sub_names.len() {
                            print!("\x1B[1A\x1B[2K"); // This clears the line, so there's no artifact left
                        }
                        println!("{}", format!("[✘] {:?}", err).red());
                    }
                }

                return None;
            }
        }

        let mut with_score = self
            .commands
            .iter()
            .map(|command| {
                // Take the base score
                let mut max_score: f64 = 0.0;
                let mut match_level: usize = 0;

                for command_name in command.all_names() {
                    for i in 0..argv.len() {
                        let argv = &argv[..argv.len() - i].to_vec();
                        let cmd = argv.join(" ");

                        let score =
                            normalized_damerau_levenshtein(cmd.as_str(), &command_name.join(" "));

                        if score > max_score {
                            max_score = score;
                            match_level = argv.len();
                        }
                    }
                }

                CommandScore {
                    score: max_score,
                    command: command.clone(),
                    match_level,
                }
            })
            .filter(|command| command.score > config(".").command_match_min_score)
            .collect::<Vec<_>>();

        if with_score.is_empty() {
            return None;
        }

        with_score.sort_by(|a, b| a.score.partial_cmp(&b.score).unwrap());
        with_score.reverse();

        if config(".").command_match_skip_prompt_if.enabled
            && with_score[0].score >= config(".").command_match_skip_prompt_if.first_min
            && (with_score.len() < 2
                || with_score[1].score <= config(".").command_match_skip_prompt_if.second_max)
        {
            omni_info!(format!("{}", with_score[0].command.flat_name()), "found");
            return with_score[0].to_return(argv);
        }

        if shell_is_interactive() {
            let question = if with_score.len() > 1 {
                requestty::Question::select("did_you_mean_command")
                    .ask_if_answered(true)
                    .on_esc(requestty::OnEsc::Terminate)
                    .message(format!(
                        "{} {}",
                        "omni:".light_cyan(),
                        "Did you mean?".yellow()
                    ))
                    .choices(with_score.iter().map(|found| found.command.flat_name()))
                    .should_loop(false)
                    .page_size(page_size)
                    .build()
            } else {
                requestty::Question::confirm("did_you_mean_command")
                    .ask_if_answered(true)
                    .on_esc(requestty::OnEsc::Terminate)
                    .message(format!(
                        "{} {} {} {}",
                        "omni:".light_cyan(),
                        "Did you mean?".yellow(),
                        "·".light_black(),
                        with_score[0].command.flat_name().normal(),
                    ))
                    .default(true)
                    .build()
            };

            match requestty::prompt_one(question) {
                Ok(answer) => {
                    match answer {
                        requestty::Answer::ListItem(listitem) => {
                            return with_score[listitem.index].to_return(argv);
                        }
                        requestty::Answer::Bool(confirmed) => {
                            if confirmed {
                                // println!("{}", format!("[✔] {}", with_score[0].abspath.to_str().unwrap()).green());
                                return with_score[0].to_return(argv);
                            }
                        }
                        _ => {}
                    }
                }
                Err(err) => {
                    if page_size < with_score.len() {
                        print!("\x1B[1A\x1B[2K"); // This clears the line, so there's no artifact left
                    }
                    println!("{}", format!("[✘] {:?}", err).red());
                }
            }
        }

        None
    }
}

#[derive(Debug)]
struct CommandScore {
    score: f64,
    command: Command,
    match_level: usize,
}

impl CommandScore {
    fn to_return(&self, argv: &[String]) -> Option<(Command, Vec<String>, Vec<String>)> {
        let cmd = self.command.clone();
        let called_as = argv[..self.match_level].to_vec();
        let argv = argv[self.match_level..].to_vec();
        Some((cmd, called_as, argv))
    }
}

impl From<CommandScore> for String {
    fn from(val: CommandScore) -> Self {
        val.command.flat_name()
    }
}

impl<'a> From<&'a mut CommandScore> for String {
    fn from(val: &'a mut CommandScore) -> Self {
        val.command.flat_name()
    }
}
