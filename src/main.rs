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
use serde_derive::{Deserialize, Serialize};

mod select;
use select::{select, text, text_with_validator};

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

    /// use alternative configuration file at `$HOME/.config/wspick/<name>.toml`
    #[arg(short, long)]
    config: Option<String>,

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
    let config_file = if let Some(name) = flags.config {
        config_dir.join(format!("{}.toml", name))
    } else {
        config_dir.join("wspick.toml")
    };
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
    // stack of currently opened sub menus (directories that only contain directories)
    let mut menu_stack: Vec<PathBuf> = vec![];
    while path.is_none() {
        let page_size = termsize::get()
            .map(|size| size.rows.saturating_sub(3).max(1))
            .unwrap_or(10) as usize;
        if let Some(current) = menu_stack.last().cloned() {
            // sub menu: show entries of the selected directory
            let mut options = vec![];
            let entries = add_options_from_dirs(
                &config,
                &mut options,
                std::slice::from_ref(&current),
                false,
            )?;
            options.push("[..]".into());
            // show the chain of opened sub menus, e.g. "group/nested"
            let chain = menu_stack
                .iter()
                .map(|p| p.file_name().map(|n| n.to_string_lossy().to_string()))
                .map(|n| n.unwrap_or_default())
                .collect::<Vec<_>>()
                .join(MENU_SUFFIX);
            let prompt = format!("select project in {chain}:");
            match select(&prompt, &options, page_size)? {
                None => {
                    // escape goes back one level
                    menu_stack.pop();
                }
                Some(selected) => match entries.get(&selected) {
                    Some(Entry::Project(p)) => path = Some(p.clone()),
                    Some(Entry::Menu(p)) => menu_stack.push(PathBuf::from(p)),
                    None => {
                        menu_stack.pop();
                    }
                },
            }
            continue;
        }
        let mut options: Vec<String> = config.paths.keys().cloned().collect();
        let dirs: Vec<PathBuf> = config
            .dirs
            .clone()
            .unwrap_or_default()
            .iter()
            .map(PathBuf::from)
            .collect();
        let entries = add_options_from_dirs(
            &config,
            &mut options,
            &dirs,
            config.exclude_proj_dirs.unwrap_or(false),
        )?;
        options.push("[new project]".into());
        options.push("[new dir]".into());
        options.push("[edit]".into());
        if let Some(selected) = select("select project:", &options, page_size)? {
            match config.paths.get(&selected) {
                None => {
                    if selected == "[new project]" {
                        path = Some(new_project(&mut config, &config_file, None)?)
                    } else if selected == "[new dir]" {
                        add_dir(&mut config, &config_file)?;
                    } else if selected == "[edit]" {
                        edit_project(&mut config, &config_file)?;
                    } else {
                        match entries
                            .get(&selected)
                            .expect("invalid option, this should never happen")
                        {
                            Entry::Project(p) => path = Some(p.clone()),
                            Entry::Menu(p) => menu_stack.push(PathBuf::from(p)),
                        }
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
    let mut config: Result<Projects, _> = toml::from_str(&fs::read_to_string(config_file)?);
    while let Err(ref err) = config {
        // display error and ask for action
        let choice = select(
            format!("config file is invalid: {err}\n\nwhat do you want to do?").as_str(),
            &[
                "edit".to_string(),
                "generate new".to_string(),
                "exit".to_string(),
            ],
            3,
        )?;
        match choice.as_deref().unwrap_or("exit") {
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
    let path = text_with_validator("directory path:", validate_path)?;
    if config.dirs.is_none() {
        config.dirs = Some(vec![])
    }
    config.dirs.as_mut().unwrap().push(path);
    sort_config(config);
    save_config(config, config_file)?;
    Ok(())
}

/// An entry in the selection menu that was discovered from a configured directory
#[derive(Debug, Clone)]
enum Entry {
    /// a project that can be opened
    Project(String),
    /// a directory that only contains other directories and is shown as sub menu
    Menu(String),
}

/// Suffix appended to sub menu entries so they can be told apart from projects
const MENU_SUFFIX: &str = "/";

/// Decide whether a directory should be shown as sub menu instead of a project.
///
/// A directory is a sub menu if it is not a git repository, contains at least one
/// directory and contains no (non hidden) files.
fn is_sub_menu(dir: &Path) -> bool {
    let Ok(read) = fs::read_dir(dir) else {
        return false;
    };
    let mut has_dir = false;
    for entry in read.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if name == ".git" {
            // git repositories are always projects
            return false;
        }
        if name.starts_with('.') {
            // ignore other hidden entries
            continue;
        }
        match entry.file_type() {
            Ok(ft) if ft.is_dir() => has_dir = true,
            // files (or symlinks to files) mark this as project
            Ok(_) => return false,
            Err(_) => return false,
        }
    }
    has_dir
}

/// Add all subdirectories of `dirs` to `options` and return a map from option name to entry.
///
/// If `exclude_proj_dirs` is set, directories that contain already configured projects
/// or searched directories are skipped.
fn add_options_from_dirs(
    config: &Projects,
    options: &mut Vec<String>,
    dirs: &[PathBuf],
    exclude_proj_dirs: bool,
) -> Result<HashMap<String, Entry>> {
    let mut map = HashMap::new();
    for dir in dirs {
        let dir_name = dir.file_name().map(|d| d.to_str());
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
        if exclude_proj_dirs {
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
                let path_str: String = path_str.unwrap().into();
                let (key, entry) = if is_sub_menu(&path) {
                    (
                        format!("{}{MENU_SUFFIX}", name.unwrap().unwrap()),
                        Entry::Menu(path_str),
                    )
                } else {
                    (
                        String::from(name.unwrap().unwrap()),
                        Entry::Project(path_str),
                    )
                };
                options.push(key.clone());
                map.insert(key, entry);
            }
        }
    }
    options.sort();
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

/// Check that the given input is an existing path.
fn validate_path(input: &str) -> Result<(), String> {
    match Path::new(input).try_exists() {
        Ok(true) => Ok(()),
        Ok(false) => Err(format!("path '{input}' does not exist")),
        Err(e) => Err(e.to_string()),
    }
}

fn new_project(
    config: &mut Projects,
    config_file: &PathBuf,
    path: Option<String>,
) -> Result<String> {
    let name = text("project name:")?;
    let path = match path {
        Some(p) => p,
        None => text_with_validator("project path:", validate_path)?,
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
