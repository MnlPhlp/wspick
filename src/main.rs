use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::Result;
use clap::Parser;
use inquire::{
    validator::{ErrorMessage, StringValidator, Validation},
    CustomUserError,
};
use serde_derive::{Deserialize, Serialize};
use toml::{map::Map, Value};

#[derive(Debug, Deserialize, Serialize)]
struct Projects {
    paths: Map<String, Value>,
    open_cmd: String,
    editor: PathBuf,
    /// sort projects alphabetically
    sort: Option<bool>,
}
impl Projects {
    fn new() -> Result<Self> {
        Ok(Self {
            paths: Map::default(),
            /// command to run with selected path as arg
            open_cmd: String::from(""),
            editor: edit::get_editor()?,
            sort: Some(true),
        })
    }
}

#[derive(Parser, Debug)]
#[command(version, about)]
struct Flags {
    /// always print selected path (ignores configured open_cmd)
    #[arg(short, long)]
    print: bool,

    /// chose [new], [edit] or a path directly, without opening the selector
    cmd_or_path: Option<String>,
    /// path for project if given after [new] command
    new_path: Option<String>,
}

fn main() -> Result<()> {
    let flags = Flags::parse();
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
    // add later added config items
    update_config(&mut config, &config_file)?;
    // check cmd args
    let mut path = None;
    if let Some(cmd) = flags.cmd_or_path {
        match cmd.as_str() {
            "new" => path = Some(new_project(&mut config, &config_file, flags.new_path)?),
            "edit" => edit_project(&mut config, &config_file)?,
            _ => path = Some(cmd),
        }
    }
    // build and show menu
    while path.is_none() {
        let mut options: Vec<String> = config.paths.keys().cloned().collect();
        options.push("[new]".into());
        options.push("[edit]".into());
        let menu = inquire::Select::new("select project", options)
            .with_page_size(termsize::get().map(|size| size.rows - 3).unwrap_or(10) as usize);
        if let Some(selected) = menu.prompt_skippable()? {
            match config.paths.get(&selected) {
                None => {
                    if selected == "[new]" {
                        path = Some(new_project(&mut config, &config_file, None)?)
                    } else if selected == "[edit]" {
                        edit_project(&mut config, &config_file)?;
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
    open_project(&config.open_cmd, &path.unwrap(), flags.print)?;
    Ok(())
}

fn update_config(config: &mut Projects, config_file: &PathBuf) -> Result<()> {
    let mut changed = false;
    if config.sort.is_none() {
        config.sort = Some(true);
        sort_config(config);
        changed = true;
    }
    if changed {
        fs::write(config_file, toml::to_string(&config)?)?;
    }
    Ok(())
}

fn open_project(cmd: &str, path: &str, print: bool) -> Result<()> {
    if print || cmd.is_empty() {
        println!("{path}");
    } else {
        Command::new(cmd).arg(path).spawn()?.wait()?;
    }
    Ok(())
}

#[derive(Clone)]
struct FileValidator;
impl StringValidator for FileValidator {
    fn validate(
        &self,
        input: &str,
    ) -> std::result::Result<inquire::validator::Validation, inquire::CustomUserError> {
        match Path::new(input).try_exists() {
            Ok(val) => {
                if val {
                    Ok(Validation::Valid)
                } else {
                    Ok(Validation::Invalid(ErrorMessage::Custom(format!(
                        "path '{input}' does not exist"
                    ))))
                }
            }
            Err(e) => Err(CustomUserError::from(e)),
        }
    }
}

fn new_project(
    config: &mut Projects,
    config_file: &PathBuf,
    path: Option<String>,
) -> Result<String> {
    let name = inquire::Text::new("project name:").prompt()?;
    let path = match path {
        Some(p) => p,
        None => inquire::Text::new("project path:")
            .with_validator(FileValidator)
            .prompt()?,
    };
    // store adjusted config
    config.paths.insert(name, Value::String(path.clone()));
    sort_config(config);
    fs::write(config_file, toml::to_string(&config)?)?;
    Ok(path)
}

fn sort_config(config: &mut Projects) {
    if config.sort.unwrap_or(false) {
        let mut new_paths = Map::with_capacity(config.paths.len());
        let mut keys = config.paths.keys().cloned().collect::<Vec<String>>();
        keys.sort();
        for k in keys {
            let val = config.paths.remove(&k).unwrap();
            new_paths.insert(k, val);
        }
        config.paths = new_paths;
    }
}

fn edit_project(config: &mut Projects, config_file: &PathBuf) -> Result<()> {
    Command::new(&config.editor)
        .arg(config_file)
        .spawn()?
        .wait()?;
    let new_config: Projects = toml::from_str(&fs::read_to_string(config_file)?)?;
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
