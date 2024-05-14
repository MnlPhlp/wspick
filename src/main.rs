use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::Result;
use clap::Parser;
use doc_consts::DocConsts;
use indexmap::IndexMap;
use inquire::{
    validator::{ErrorMessage, StringValidator, Validation},
    CustomUserError,
};
use serde_derive::{Deserialize, Serialize};

#[derive(Debug, Deserialize, Serialize, DocConsts)]
struct Projects {
    /// Directories to search for projects
    dirs: Option<Vec<String>>,
    /// command to run with selected path as arg
    open_cmd: String,
    /// editor to open config with
    editor: String,
    /// sort projects alphabetically
    sort: Option<bool>,
    /// exclude directories that contain projects from automatic list
    exclude_proj_dirs: Option<bool>,
    /// Paths to specific projects
    paths: IndexMap<String, String>,
}
impl Projects {
    fn new() -> Self {
        Self {
            paths: IndexMap::default(),
            dirs: Some(vec![]),
            open_cmd: String::from(""),
            editor: edit::get_editor()
                .map(|e| e.to_str().unwrap_or("").into())
                .unwrap_or("".into()),
            sort: Some(true),
            exclude_proj_dirs: Some(false),
        }
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
        save_config(&Projects::new(), &config_file)?;
    }
    // load config
    let mut config = load_config(&config_file)?;
    // add later added config items
    update_config(&mut config, &config_file)?;
    // check cmd args#
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
                        path = Some(
                            dir_paths
                                .get(&selected)
                                .expect("invalid option, this should never happen")
                                .clone(),
                        );
                    }
                }
                Some(val) => path = Some(val.clone()),
            }
        } else {
            return Ok(());
        }
    }
    open_project(&config.open_cmd, &path.unwrap(), flags.print)?;
    Ok(())
}

fn load_config(config_file: &PathBuf) -> Result<Projects> {
    let mut config: Result<Projects, _> = toml::from_str(&fs::read_to_string(&config_file)?);
    while let Err(ref err) = config {
        // display error and ask for action
        match inquire::Select::new(
            format!("config file is invalid: {err}\n\nwhat do you want to do?").as_str(),
            vec!["edit", "generate new", "exit"],
        )
        .prompt()?
        {
            "edit" => {
                let mut edited = Projects::new();
                if edit_project(&mut edited, config_file).is_ok() {
                    config = Ok(edited)
                };
            }
            "generate new" => {
                // generate new empty configuration
                save_config(&Projects::new(), config_file)?;
                config = Ok(Projects::new())
            }
            "exit" => std::process::exit(1),
            _ => (),
        }
    }
    Ok(config?)
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
    save_config(config, config_file)?;
    Ok(())
}

fn add_options_from_dirs(
    config: &mut Projects,
    options: &mut Vec<String>,
) -> Result<HashMap<String, String>> {
    let mut map = HashMap::new();
    if let Some(dirs) = config.dirs.as_ref() {
        for dir in dirs {
            let dir_path = PathBuf::from(dir);
            let dir_name = dir_path.file_name().map(|d| d.to_str());
            if dir_name.is_none() || dir_name.unwrap().is_none() {
                continue;
            }
            // filter for directories
            let mut paths = fs::read_dir(dir)?
                .filter(|f| {
                    if f.is_err() {
                        return false;
                    }
                    if let Ok(ft) = f.as_ref().unwrap().file_type() {
                        return ft.is_dir();
                    }
                    false
                })
                .collect::<Vec<_>>();
            if let Some(true) = config.exclude_proj_dirs {
                // filter out directories that contain projects
                paths.retain(|p| {
                    if let Ok(p) = p {
                        let name = p.file_name().to_string_lossy().to_string();
                        // filter custom project paths
                        for proj in config.paths.values() {
                            if proj.contains(&name) {
                                return false;
                            }
                        }
                        // filter searched dirs
                        if let Some(dirs) = &config.dirs {
                            for dir in dirs {
                                if dir.contains(&name) {
                                    return false;
                                }
                            }
                        }
                    }
                    true
                });
            }
            for path in paths {
                if let Ok(path) = path.map(|p| p.path()) {
                    let path_str = path.to_str();
                    let name = path.file_name().map(|n| n.to_str());
                    if path_str.is_none()
                        || name.is_none()
                        || name.unwrap().is_none()
                        || name.unwrap().unwrap().starts_with('.')
                    {
                        continue;
                    }
                    let key = String::from(name.unwrap().unwrap());
                    options.push(key.clone());
                    map.insert(key, path_str.unwrap().into());
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
    if config.exclude_proj_dirs.is_none() {
        config.exclude_proj_dirs = Some(false);
        changed = true;
    }
    if changed {
        save_config(config, config_file)?;
    }
    Ok(())
}

fn save_config(config: &Projects, config_file: &PathBuf) -> Result<()> {
    let doc = toml::ser::to_string_pretty(config)?;
    let mut doc_commented = vec![];
    // add comments
    for line in doc.lines() {
        match &line[..line.find(' ').unwrap_or(line.len())] {
            "open_cmd" => {
                doc_commented.push(format!("# {}", Projects::get_docs().open_cmd));
            }
            "sort" => {
                doc_commented.push(format!("# {}", Projects::get_docs().sort));
            }
            "exclude_proj_dirs" => {
                doc_commented.push(format!("# {}", Projects::get_docs().exclude_proj_dirs));
            }
            "[paths]" => {
                doc_commented.push(format!("# {}", Projects::get_docs().paths));
            }
            "dirs" => {
                doc_commented.push(format!("# {}", Projects::get_docs().dirs));
            }
            "editor" => {
                doc_commented.push(format!("# {}", Projects::get_docs().editor));
            }
            _ => (),
        }
        doc_commented.push(line.to_string())
    }
    fs::create_dir_all(config_file.parent().unwrap())?;
    fs::write(config_file, doc_commented.join("\n"))?;
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
    config.paths.insert(name, path.clone());
    sort_config(config);
    save_config(config, config_file)?;
    Ok(path)
}

fn sort_config(config: &mut Projects) {
    if config.sort.unwrap_or(false) {
        let mut new_paths = IndexMap::with_capacity(config.paths.len());
        let mut keys = config.paths.keys().cloned().collect::<Vec<String>>();
        keys.sort();
        for k in keys {
            let val = config.paths.swap_remove(&k).unwrap();
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
    let new_config = load_config(config_file)?;
    config.paths = new_config.paths;
    config.editor = new_config.editor;
    config.open_cmd = new_config.open_cmd;
    config.sort = new_config.sort;
    config.dirs = new_config.dirs;
    config.exclude_proj_dirs = new_config.exclude_proj_dirs;
    Ok(())
}
