use std::collections::HashMap;
use std::path::Path;

use serde::Deserialize;
use serde::Serialize;
use time::OffsetDateTime;
use tokio::process::Command as TokioCommand;

use crate::internal::cache::UpEnvironment;
use crate::internal::cache::UpEnvironments;
use crate::internal::commands::utils::abs_path;
use crate::internal::config::up::utils::run_progress;
use crate::internal::config::up::utils::PrintProgressHandler;
use crate::internal::config::up::utils::ProgressHandler;
use crate::internal::config::up::utils::RunConfig;
use crate::internal::config::up::utils::SpinnerProgressHandler;
use crate::internal::config::up::UpError;
use crate::internal::config::ConfigValue;
use crate::internal::user_interface::StringColor;
use crate::internal::workdir;
use crate::internal::Cache;
use crate::internal::ENV;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct UpConfigBundler {
    pub gemfile: Option<String>,
    pub path: Option<String>,
}

impl UpConfigBundler {
    pub fn from_config_value(config_value: Option<&ConfigValue>) -> Self {
        let mut gemfile = None;
        let mut path = Some("vendor/bundle".to_string());
        if let Some(config_value) = config_value {
            if let Some(config_value) = config_value.as_table() {
                if let Some(value) = config_value.get("gemfile") {
                    gemfile = Some(value.as_str().unwrap().to_string());
                }
                if let Some(value) = config_value.get("path") {
                    path = Some(value.as_str().unwrap().to_string());
                }
            } else {
                gemfile = Some(config_value.as_str().unwrap().to_string());
            }
        }

        UpConfigBundler {
            gemfile: gemfile,
            path: path,
        }
    }

    fn update_cache(&self, progress_handler: Option<Box<&dyn ProgressHandler>>) {
        progress_handler
            .clone()
            .map(|progress_handler| progress_handler.progress("updating cache".to_string()));

        let result = Cache::exclusive(|cache| {
            let workdir = workdir(".");
            let repo_id = workdir.id();
            if repo_id.is_none() {
                return false;
            }
            let repo_id = repo_id.unwrap();

            // Update the repository up cache
            let mut up_env = HashMap::new();
            if let Some(up_cache) = &cache.up_environments {
                up_env = up_cache.env.clone();
            }

            if !up_env.contains_key(&repo_id) {
                up_env.insert(repo_id.clone(), UpEnvironment::new());
            }
            let repo_up_env = up_env.get_mut(&repo_id).unwrap();

            repo_up_env
                .env_vars
                .insert("BUNDLE_GEMFILE".to_string(), self.gemfile_abs_path());

            cache.up_environments = Some(UpEnvironments {
                env: up_env.clone(),
                updated_at: OffsetDateTime::now_utc(),
            });

            true
        });

        if let Err(err) = result {
            progress_handler.clone().map(|progress_handler| {
                progress_handler.progress(format!("failed to update cache: {}", err))
            });
        } else {
            progress_handler
                .clone()
                .map(|progress_handler| progress_handler.progress("updated cache".to_string()));
        }
    }

    pub fn up(&self, progress: Option<(usize, usize)>) -> Result<(), UpError> {
        let desc = format!("install Gemfile dependencies:").light_blue();
        let progress_handler: Box<dyn ProgressHandler> = if ENV.interactive_shell {
            Box::new(SpinnerProgressHandler::new(desc, progress))
        } else {
            Box::new(PrintProgressHandler::new(desc, progress))
        };
        let progress_handler: Option<Box<&dyn ProgressHandler>> =
            Some(Box::new(progress_handler.as_ref()));

        if let Some(path) = &self.path {
            progress_handler.clone().map(|progress_handler| {
                progress_handler.progress("setting bundle path".to_string())
            });

            let mut bundle_config = TokioCommand::new("bundle");
            bundle_config.arg("config");
            bundle_config.arg("--local");
            bundle_config.arg("path");
            bundle_config.arg(path);
            bundle_config.stdout(std::process::Stdio::piped());
            bundle_config.stderr(std::process::Stdio::piped());

            run_progress(
                &mut bundle_config,
                progress_handler.clone(),
                RunConfig::default(),
            )?;
        }

        progress_handler
            .clone()
            .map(|progress_handler| progress_handler.progress("installing bundle".to_string()));

        let mut bundle_install = TokioCommand::new("bundle");
        bundle_install.arg("install");
        if let Some(gemfile) = &self.gemfile {
            bundle_install.arg("--gemfile");
            bundle_install.arg(gemfile);
        }
        bundle_install.stdout(std::process::Stdio::piped());
        bundle_install.stderr(std::process::Stdio::piped());

        let result = run_progress(
            &mut bundle_install,
            progress_handler.clone(),
            RunConfig::default(),
        );

        if let Err(err) = &result {
            progress_handler.clone().map(|progress_handler| {
                progress_handler.error_with_message(format!("bundle install failed: {}", err))
            });
            return result;
        }

        self.update_cache(progress_handler.clone());

        progress_handler
            .clone()
            .map(|progress_handler| progress_handler.success());

        Ok(())
    }

    pub fn down(&self, progress: Option<(usize, usize)>) -> Result<(), UpError> {
        let desc = format!("remove Gemfile dependencies:").light_blue();
        let progress_handler: Box<dyn ProgressHandler> = if ENV.interactive_shell {
            Box::new(SpinnerProgressHandler::new(desc, progress))
        } else {
            Box::new(PrintProgressHandler::new(desc, progress))
        };
        let progress_handler: Option<Box<&dyn ProgressHandler>> =
            Some(Box::new(progress_handler.as_ref()));

        // Check if path exists, and if so delete it
        if self.path.is_some() && Path::new(&self.path.clone().unwrap()).exists() {
            let path = self.path.clone().unwrap();
            let path = abs_path(&path).to_str().unwrap().to_string();

            progress_handler.clone().map(|progress_handler| {
                progress_handler.progress(format!("removing {}", path).to_string())
            });

            if let Err(err) = std::fs::remove_dir_all(&path) {
                progress_handler.clone().map(|progress_handler| {
                    progress_handler.error_with_message(format!(
                        "failed to remove {}: {}",
                        path,
                        err.to_string()
                    ))
                });
                return Err(UpError::Exec(format!(
                    "failed to remove {}: {}",
                    path,
                    err.to_string()
                )));
            }

            // Cleanup the parents as long as they are empty directories
            let mut parent = Path::new(&path);
            while let Some(path) = parent.parent() {
                if let Err(_err) = std::fs::remove_dir(path) {
                    break;
                }
                parent = path;
            }

            progress_handler
                .clone()
                .map(|progress_handler| progress_handler.success());
        } else {
            progress_handler.clone().map(|progress_handler| {
                progress_handler
                    .success_with_message("skipping (nothing to do)".to_string().light_black())
            });
        }

        Ok(())
    }

    fn gemfile_abs_path(&self) -> String {
        let gemfile = if let Some(gemfile) = &self.gemfile {
            gemfile.clone()
        } else {
            "Gemfile".to_string()
        };

        // make a path from the str
        let gemfile = Path::new(&gemfile);

        abs_path(gemfile).to_str().unwrap().to_string()
    }
}
