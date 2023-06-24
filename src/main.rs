use std::{fs, path::PathBuf, process::Command};

use anyhow::Result;
use serde_derive::{Deserialize, Serialize};
use toml::{map::Map, Value};

#[derive(Debug, Deserialize, Serialize)]
struct Projects {
    paths: Map<String, Value>,
    open_cmd: String,
    editor: PathBuf,
}
impl Projects {
    fn new() -> Result<Self> {
        Ok(Self {
            paths: Map::default(),
            open_cmd: String::from(""),
            editor: edit::get_editor()?,
        })
    }
}

fn main() -> Result<()> {
    // make sure config exists
    let dirs = directories::ProjectDirs::from("io.github", "mnlphlp", "wspick")
        .expect("home directory has to be found");
    let config_dir = dirs.config_dir();
    let config_file = config_dir.join("wspick.toml");
    if !config_file.try_exists()? {
        fs::create_dir_all(config_dir)?;
        fs::write(&config_file, toml::to_string(&Projects::new()?)?)?;
    }
    // load config
    let mut config: Projects = toml::from_str(&fs::read_to_string(&config_file)?)?;
    // build and show menu
    let mut path = None;
    while path.is_none() {
        let mut options: Vec<String> = config.paths.keys().cloned().collect();
        options.push("new".into());
        options.push("edit".into());
        let menu = inquire::Select::new("select project", options);
        if let Some(selected) = menu.prompt_skippable()? {
            match config.paths.get(&selected) {
                None => {
                    if selected == "new" {
                        path = Some(new_project(&mut config, &config_file)?)
                    } else if selected == "edit" {
                        edit_prject(&mut config, &config_file)?;
                    } else {
                        panic!("invalid option, this should never happen");
                    }
                }
                Some(val) => path = Some(get_path(val)),
            }
        } else {
            return Ok(());
        }
    }
    open_project(&config.open_cmd, &path.unwrap())?;
    Ok(())
}

fn open_project(cmd: &str, path: &str) -> Result<()> {
    if cmd.is_empty() {
        println!("{path}");
    } else {
        Command::new(cmd).arg(path).spawn()?.wait()?;
    }
    Ok(())
}

fn new_project(config: &mut Projects, config_file: &PathBuf) -> Result<String> {
    let name = inquire::Text::new("project name:").prompt()?;
    let path = inquire::Text::new("project path:").prompt()?;
    config
        .paths
        .insert(name.clone(), Value::String(path.clone()));
    fs::write(config_file, toml::to_string(&config)?)?;
    Ok(path)
}

fn edit_prject(config: &mut Projects, config_file: &PathBuf) -> Result<()> {
    Command::new(&config.editor)
        .arg(config_file)
        .spawn()?
        .wait()?;
    let new_config: Projects = toml::from_str(&fs::read_to_string(&config_file)?)?;
    config.paths = new_config.paths;
    config.editor = new_config.editor;
    config.open_cmd = new_config.open_cmd;
    Ok(())
}

fn get_path(path: &Value) -> String {
    match path {
        Value::String(path) => path.to_owned(),
        _ => panic!("get_path called with not a string"),
    }
}
