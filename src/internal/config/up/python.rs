use std::path::PathBuf;

use once_cell::sync::OnceCell;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use crate::internal::cache::Cache;
use crate::internal::cache::UpEnvironment;
use crate::internal::cache::UpEnvironments;
use crate::internal::cache::UpVersion;
use crate::internal::config::up::utils::version_from_config;
use crate::internal::config::up::utils::PrintProgressHandler;
use crate::internal::config::up::utils::ProgressHandler;
use crate::internal::config::up::utils::SpinnerProgressHandler;
use crate::internal::config::up::UpConfigAsdfBase;
use crate::internal::config::up::UpError;
use crate::internal::config::up::ASDF_PATH;
use crate::internal::user_interface::StringColor;
use crate::internal::workdir;
use crate::internal::ConfigValue;
use crate::internal::ENV;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct UpConfigPython {
    pub version: String,
    pub with_venv: bool,
    #[serde(skip)]
    pub asdf_base: OnceCell<UpConfigAsdfBase>,
}

impl UpConfigPython {
    pub fn from_config_value(config_value: Option<&ConfigValue>) -> Self {
        let version = version_from_config(config_value);
        let with_venv = with_venv_from_config(config_value);

        Self {
            asdf_base: OnceCell::new(),
            version,
            with_venv,
        }
    }

    pub fn up(&self, progress: Option<(usize, usize)>) -> Result<(), UpError> {
        self.asdf_base().up(progress)?;

        let desc = "python (venv)".to_string().light_blue();
        let progress_handler: Box<dyn ProgressHandler> = if ENV.interactive_shell {
            Box::new(SpinnerProgressHandler::new(desc, progress))
        } else {
            Box::new(PrintProgressHandler::new(desc, progress))
        };
        let progress_handler: Option<Box<&dyn ProgressHandler>> =
            Some(Box::new(progress_handler.as_ref()));
        let progress_handler = progress_handler.as_ref();

        if self.with_venv {
            self.update_cache(progress_handler.cloned());
            if !self.venv_present() {
                let msg = format!("working on venv for {}", self.version())
                    .to_string()
                    .green();
                progress_handler.map(|ph| ph.success_with_message(msg));
                self.venv_setup().unwrap();
            } else {
                let msg = "venv already set up".to_string().light_black();
                progress_handler.map(|ph| ph.success_with_message(msg));
            }
        }

        Ok(())
    }

    pub fn down(&self, progress: Option<(usize, usize)>) -> Result<(), UpError> {
        if self.venv_present() {
            std::fs::remove_dir_all(self.venv_dir()).unwrap()
        }
        self.asdf_base().down(progress)
    }

    pub fn asdf_base(&self) -> &UpConfigAsdfBase {
        self.asdf_base
            .get_or_init(|| UpConfigAsdfBase::new("python", self.version.as_ref()))
    }

    fn version(&self) -> String {
        // TODO - this should be infallable here?
        self.asdf_base().version(None).unwrap().to_string()
    }

    fn update_cache(&self, progress_handler: Option<Box<&dyn ProgressHandler>>) {
        progress_handler
            .clone()
            .map(|progress_handler| progress_handler.progress("updating cache".to_string()));

        let result = Cache::exclusive(|cache| {
            let workdir = workdir(".");
            let repo_id = match workdir.id() {
                Some(repo_id) => repo_id,
                None => return false,
            };

            let mut up_env = cache.up_environments.as_ref().unwrap().env.clone();
            let repo_up_env = match up_env.get_mut(&repo_id) {
                Some(env) => env,
                None => {
                    up_env.insert(repo_id.clone(), UpEnvironment::new());
                    up_env.get_mut(&repo_id).unwrap()
                }
            };

            let found = repo_up_env
                .versions
                .iter()
                .any(|v| v.tool == "python-venv" && v.version == self.version());

            if !found {
                repo_up_env
                    .env_vars
                    .insert("VIRTUAL_ENV".to_string(), self.venv_name().to_string());
                repo_up_env.env_vars.insert(
                    "__omni_python_venv_path".to_string(),
                    self.venv_dir().to_str().unwrap().to_string(),
                );

                repo_up_env.versions.push(UpVersion {
                    tool: "python-venv".to_string(),
                    version: self.version().clone(),
                });

                cache.up_environments = Some(UpEnvironments {
                    env: up_env.clone(),
                    updated_at: OffsetDateTime::now_utc(),
                });

                true
            } else {
                false
            }
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

    fn venv_dir(&self) -> PathBuf {
        // TODO what are the fail cases?
        let workdir_root = PathBuf::from(workdir(".").root().unwrap());
        workdir_root.join(".omni/venv")
    }

    fn venv_name(&self) -> String {
        // TODO what are the fail cases?
        let workdir_root = PathBuf::from(workdir(".").root().unwrap());
        workdir_root
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .to_string()
    }

    fn venv_present(&self) -> bool {
        let dir = self.venv_dir();
        let dir_exists = dir.try_exists().unwrap();
        let pyvenv_exists = dir.join("pyvenv.cfg").try_exists().unwrap();
        if dir_exists ^ pyvenv_exists {
            panic!("a venv dir exists, but it was not set up");
        }
        pyvenv_exists
    }

    fn venv_setup(&self) -> std::io::Result<()> {
        let tool_prefix =
            PathBuf::from(format!("{}/installs/python/{}", *ASDF_PATH, self.version()));
        let python_path = tool_prefix.join("bin").join("python3");
        let venv_dir = self.venv_dir();
        let venv_bin = &venv_dir.join("bin");

        std::process::Command::new(python_path)
            .args(["-m", "venv", venv_dir.to_str().unwrap()])
            .output()
            .expect("failed to create venv");

        // The venv activate scripts can break a given shell session since the activate/deactivate
        // stashes and unstashes environment values (include PATH) outside of omni's dynenv.
        for script in ["activate", "activate.csh", "activate.fish", "Activate.ps1"] {
            std::fs::remove_file(&venv_bin.join(script))?;
        }

        Ok(())
    }
}

fn with_venv_from_config(value: Option<&ConfigValue>) -> bool {
    if let Some(with_venv) = value.and_then(|val| val.get("with_venv")) {
        with_venv.as_bool().unwrap_or(false)
    } else {
        false
    }
}
