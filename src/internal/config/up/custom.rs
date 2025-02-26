use serde::Deserialize;
use serde::Serialize;
use tokio::process::Command as TokioCommand;

use crate::internal::config::up::utils::run_progress;
use crate::internal::config::up::utils::PrintProgressHandler;
use crate::internal::config::up::utils::ProgressHandler;
use crate::internal::config::up::utils::RunConfig;
use crate::internal::config::up::utils::SpinnerProgressHandler;
use crate::internal::config::up::UpError;
use crate::internal::config::ConfigValue;
use crate::internal::env::shell_is_interactive;
use crate::internal::user_interface::StringColor;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct UpConfigCustom {
    pub meet: String,
    pub met: Option<String>,
    pub unmeet: Option<String>,
    pub name: Option<String>,
    pub dir: Option<String>,
}

impl UpConfigCustom {
    pub fn from_config_value(config_value: Option<&ConfigValue>) -> Self {
        let mut meet = None;
        let mut met = None;
        let mut unmeet = None;
        let mut name = None;
        let mut dir = None;

        if let Some(config_value) = config_value {
            if let Some(value) = config_value.get_as_str_forced("meet") {
                meet = Some(value.to_string());
            }
            if let Some(value) = config_value.get_as_str_forced("met?") {
                met = Some(value.to_string());
            }
            if let Some(value) = config_value.get_as_str_forced("unmeet") {
                unmeet = Some(value.to_string());
            }
            if let Some(value) = config_value.get_as_str_forced("name") {
                name = Some(value.to_string());
            }
            if let Some(value) = config_value.get_as_str_forced("dir") {
                dir = Some(value.to_string());
            }
        }

        if meet.is_none() {
            meet = Some("".to_string());
        }

        UpConfigCustom {
            meet: meet.unwrap(),
            met,
            unmeet,
            name,
            dir,
        }
    }

    pub fn dir(&self) -> Option<String> {
        self.dir.as_ref().map(|dir| dir.to_string())
    }

    pub fn up(&self, progress: Option<(usize, usize)>) -> Result<(), UpError> {
        let name = if let Some(name) = &self.name {
            name.to_string()
        } else {
            self.meet
                .split_whitespace()
                .next()
                .unwrap_or("custom")
                .to_string()
        };
        let desc = format!("{}:", name).light_blue();

        let progress_handler: Box<dyn ProgressHandler> = if shell_is_interactive() {
            Box::new(SpinnerProgressHandler::new(desc, progress))
        } else {
            Box::new(PrintProgressHandler::new(desc, progress))
        };
        let progress_handler: Option<&dyn ProgressHandler> = Some(progress_handler.as_ref());

        if self.met().unwrap_or(false) {
            if let Some(progress_handler) = progress_handler {
                progress_handler.success_with_message("skipping (already met)".light_black())
            }
            return Ok(());
        }

        if let Err(err) = self.meet(progress_handler) {
            if let Some(progress_handler) = progress_handler {
                progress_handler.error_with_message(format!("{}", err).light_red())
            }
            return Err(UpError::StepFailed(name, progress));
        }

        if let Some(progress_handler) = progress_handler {
            progress_handler.success()
        }

        Ok(())
    }

    pub fn down(&self, progress: Option<(usize, usize)>) -> Result<(), UpError> {
        let name = if let Some(name) = &self.name {
            name.to_string()
        } else {
            self.unmeet
                .clone()
                .unwrap_or("custom".to_string())
                .split_whitespace()
                .next()
                .unwrap_or("custom")
                .to_string()
        };

        let spinner_progress_handler;
        let mut progress_handler: Option<&dyn ProgressHandler> = None;
        if shell_is_interactive() {
            spinner_progress_handler = Box::new(SpinnerProgressHandler::new(
                format!("{}:", name).light_blue(),
                progress,
            ));
            progress_handler = Some(spinner_progress_handler.as_ref());
        }

        if let Some(_unmeet) = &self.unmeet {
            if !self.met().unwrap_or(true) {
                if let Some(progress_handler) = progress_handler {
                    progress_handler.success_with_message("skipping (not met)".light_black())
                }
                return Ok(());
            }

            if let Some(progress_handler) = progress_handler {
                progress_handler.progress("reverting".light_black())
            }

            if let Err(err) = self.unmeet(progress_handler) {
                if let Some(progress_handler) = progress_handler {
                    progress_handler.error_with_message(format!("{}", err).light_red())
                }
                return Err(err);
            }
        }

        if let Some(progress_handler) = progress_handler {
            progress_handler.success()
        }

        Ok(())
    }

    fn met(&self) -> Option<bool> {
        if let Some(met) = &self.met {
            let mut command = std::process::Command::new("bash");
            command.arg("-c");
            command.arg(met);
            command.stdout(std::process::Stdio::null());
            command.stderr(std::process::Stdio::null());

            let output = command.output().unwrap();
            Some(output.status.success())
        } else {
            None
        }
    }

    fn meet(&self, progress_handler: Option<&dyn ProgressHandler>) -> Result<(), UpError> {
        if !self.meet.is_empty() {
            // eprintln!("{}", format!("$ {}", self.meet).light_black());
            if let Some(progress_handler) = progress_handler {
                progress_handler.progress("running (meet) command".to_string())
            }

            let mut command = TokioCommand::new("bash");
            command.arg("-c");
            command.arg(&self.meet);
            command.stdout(std::process::Stdio::piped());
            command.stderr(std::process::Stdio::piped());

            run_progress(&mut command, progress_handler, RunConfig::default())?;
        }

        Ok(())
    }

    fn unmeet(&self, progress_handler: Option<&dyn ProgressHandler>) -> Result<(), UpError> {
        if let Some(unmeet) = &self.unmeet {
            // eprintln!("{}", format!("$ {}", unmeet).light_black());
            if let Some(progress_handler) = progress_handler {
                progress_handler.progress("running (unmeet) command".to_string())
            }

            let mut command = TokioCommand::new("bash");
            command.arg("-c");
            command.arg(unmeet);
            command.stdout(std::process::Stdio::piped());
            command.stderr(std::process::Stdio::piped());

            run_progress(&mut command, progress_handler, RunConfig::default())?;
        }

        Ok(())
    }
}
