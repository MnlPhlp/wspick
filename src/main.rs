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
    dirs: Option<Vec<String>>,
    open_cmd: String,
    editor: PathBuf,
    /// sort projects alphabetically
    sort: Option<bool>,
}
impl Projects {
    fn new() -> Result<Self> {
        Ok(Self {
            paths: Map::default(),
            dirs: Some(vec![]),
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
        let dir_paths = add_options_from_dirs(&mut config, &mut options)?;
        options.push("[new project]".into());
        options.push("[new dir]".into());
        options.push("[edit]".into());
        let menu = inquire::Select::new("select project:", options)
            .with_page_size(termsize::get().map(|size| size.rows - 3).unwrap_or(10) as usize);
        if let Some(selected) = menu.prompt_skippable()? {
            match config.paths.get(&selected) {
                None => {
                    if selected == "[new project]" {
                        path = Some(new_project(&mut config, &config_file, None)?)
                    } else if selected == "[new dir]" {
                        add_dir(&mut config, &config_file)?;
                    } else if selected == "[edit]" {
                        edit_project(&mut config, &config_file)?;
                    } else {
                        path = Some(get_path(
                            dir_paths
                                .get(&selected)
                                .expect("invalid option, this should never happen"),
                        ));
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

fn add_dir(config: &mut Projects, config_file: &PathBuf) -> Result<()> {
    let path = inquire::Text::new("directory path:")
        .with_validator(FileValidator)
        .prompt()?;
    if config.dirs.is_none() {
        config.dirs = Some(vec![])
    }
    config.dirs.as_mut().unwrap().push(path);
    sort_config(config);
    fs::write(config_file, toml::to_string(&config)?)?;
    Ok(())
}

fn add_options_from_dirs(
    config: &mut Projects,
    options: &mut Vec<String>,
) -> Result<Map<String, Value>> {
    let mut map = Map::new();
    if let Some(dirs) = config.dirs.as_ref() {
        for dir in dirs {
            let dir_path = PathBuf::from(dir);
            let dir_name = dir_path.file_name().map(|d| d.to_str());
            if dir_name.is_none() || dir_name.unwrap().is_none() {
                continue;
            }
            let paths = fs::read_dir(dir)?.filter(|f| {
                if f.is_err() {
                    return false;
                }
                if let Ok(ft) = f.as_ref().unwrap().file_type() {
                    return ft.is_dir();
                }
                return false;
            });
            for path in paths {
                if let Ok(path) = path.map(|p| p.path()) {
                    let path_str = path.to_str();
                    let name = path.file_name().map(|n| n.to_str());
                    if path_str.is_none() || name.is_none() || name.unwrap().is_none() {
                        continue;
                    }
                    let key = String::from(name.unwrap().unwrap());
                    options.push(key.clone());
                    map.insert(key, Value::String(String::from(path_str.unwrap())));
                }
            }
        }
        options.sort();
    }
    Ok(map)
}

fn update_config(config: &mut Projects, config_file: &PathBuf) -> Result<()> {
    let mut changed = false;
    if config.sort.is_none() {
        config.sort = Some(true);
        sort_config(config);
        changed = true;
    }
    if config.dirs.is_none() {
        config.dirs = Some(vec![]);
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
