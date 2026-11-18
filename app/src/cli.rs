//! Command line for the demo app.
//!
//! `--model <name>` resolves a model by directory or file stem inside the
//! directories listed in [`MODEL_ROOTS`], so switching between the sample corpus
//! and the rig under development does not need a long path.

use std::path::{Path, PathBuf};

use live2d_render::LookAtMode;

/// Directories searched by `--model <name>` and `--list`, in order.
///
/// Missing directories are skipped, so a checkout without the corpus still works.
const MODEL_ROOTS: &[&str] = &[
    "models",
    ".refs/models",
    ".refs/cubism-web-samples/Samples/Resources",
    "DeepSeek-fish-live2d/DeepSeek",
];

/// Used when no model is given on the command line.
const DEFAULT_MODEL: &str = "DeepSeek-fish-live2d/DeepSeek/大肥鱼.model3.json";

pub const USAGE: &str = "\
Live2D · iced 0.15-dev + mocari

USAGE:
    app                          open the default rig
    app <path/to/model3.json>    open that model
    app --model <name>           open a model found by name
    app --list                   list the models that can be named
    app --track <mode>           adapted (default) or viewer

OPTIONS:
    -m, --model <name>   match a model directory or file stem, case-insensitive
    -t, --track <mode>   adapted: follow the rig's own conventions;
                         viewer: reproduce the Cubism Viewer's mouse bindings
    -l, --list           print every model found in the search roots
    -h, --help           print this text

KEYS:
    R  next motion      E  next expression      mouse  tracking
";

#[derive(Debug)]
pub struct Args {
    pub model: PathBuf,
    pub look_at_mode: LookAtMode,
}

#[derive(Debug)]
pub enum Invocation {
    Run(Args),
    List,
    Help,
}

/// Every model reachable by name, sorted by name.
pub fn discover() -> Vec<(String, PathBuf)> {
    discover_in(MODEL_ROOTS.iter().map(PathBuf::from))
}

fn discover_in(roots: impl IntoIterator<Item = PathBuf>) -> Vec<(String, PathBuf)> {
    let mut found = Vec::new();

    for root in roots {
        let Ok(entries) = std::fs::read_dir(&root) else {
            continue;
        };

        for entry in entries.flatten() {
            let path = entry.path();
            let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
                continue;
            };

            let is_directory = path.is_dir();

            // A model is either a directory carrying `<dir>.model3.json`, or the
            // json file itself sitting in a search root.
            let candidate = if is_directory {
                path.join(format!("{file_name}.model3.json"))
            } else {
                path.clone()
            };

            if !is_model_json(&candidate) {
                continue;
            }

            let label = if is_directory {
                file_name.to_owned()
            } else {
                strip_model_suffix(file_name).to_owned()
            };

            found.push((label, candidate));
        }
    }

    found.sort_by(|left, right| {
        left.0
            .to_ascii_lowercase()
            .cmp(&right.0.to_ascii_lowercase())
    });
    found.dedup_by(|left, right| left.0.eq_ignore_ascii_case(&right.0));
    found
}

/// Whether `path` is a `*.model3.json` file.
fn is_model_json(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.to_ascii_lowercase().ends_with(".model3.json"))
        && path.is_file()
}

/// `大肥鱼.model3.json` -> `大肥鱼`, so `--model` takes the model's name.
fn strip_model_suffix(file_name: &str) -> &str {
    let trimmed = file_name
        .strip_suffix(".model3.json")
        .or_else(|| file_name.strip_suffix(".json"));

    trimmed.unwrap_or(file_name)
}

pub fn parse(args: impl IntoIterator<Item = String>) -> Result<Invocation, String> {
    parse_with(args, discover())
}

fn parse_with(
    args: impl IntoIterator<Item = String>,
    available: Vec<(String, PathBuf)>,
) -> Result<Invocation, String> {
    let mut wanted: Option<String> = None;
    let mut look_at_mode = LookAtMode::default();
    let mut args = args.into_iter();

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-h" | "--help" => return Ok(Invocation::Help),
            "-l" | "--list" => return Ok(Invocation::List),
            "-m" | "--model" => {
                wanted = Some(args.next().ok_or("--model needs a name")?);
            }
            "-t" | "--track" => {
                let raw = args.next().ok_or("--track needs a mode")?;
                look_at_mode = raw.parse()?;
            }
            other if other.starts_with('-') => {
                return Err(format!("unknown option {other:?}"));
            }
            other => wanted = Some(other.to_owned()),
        }
    }

    let model = match wanted {
        None => PathBuf::from(DEFAULT_MODEL),
        Some(wanted) => resolve(&wanted, &available)?,
    };

    Ok(Invocation::Run(Args {
        model,
        look_at_mode,
    }))
}

fn resolve(wanted: &str, available: &[(String, PathBuf)]) -> Result<PathBuf, String> {
    let path = Path::new(wanted);
    if path.is_file() {
        return Ok(path.to_path_buf());
    }

    let matches: Vec<&(String, PathBuf)> = available
        .iter()
        .filter(|(name, _)| name.eq_ignore_ascii_case(wanted))
        .collect();

    match matches.as_slice() {
        [single] => Ok(single.1.clone()),
        [] => {
            let names: Vec<&str> = available.iter().map(|(name, _)| name.as_str()).collect();

            if names.is_empty() {
                Err(format!(
                    "no model named {wanted:?}; no models found in the search roots either"
                ))
            } else {
                Err(format!(
                    "no model named {wanted:?}; available: {}",
                    names.join(", ")
                ))
            }
        }
        _ => Err(format!(
            "{wanted:?} is ambiguous: {}",
            matches
                .iter()
                .map(|(_, path)| path.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn corpus() -> Vec<(String, PathBuf)> {
        vec![
            (
                "Hiyori".to_owned(),
                PathBuf::from(
                    ".refs/cubism-web-samples/Samples/Resources/Hiyori/Hiyori.model3.json",
                ),
            ),
            (
                "Wanko".to_owned(),
                PathBuf::from(".refs/cubism-web-samples/Samples/Resources/Wanko/Wanko.model3.json"),
            ),
        ]
    }

    fn parse_args(args: &[&str]) -> Result<Invocation, String> {
        parse_with(args.iter().map(|arg| (*arg).to_owned()), corpus())
    }

    #[test]
    fn no_arguments_opens_the_default_rig() {
        let Ok(Invocation::Run(args)) = parse_args(&[]) else {
            panic!("expected a run invocation");
        };

        assert!(args.model.ends_with(DEFAULT_MODEL));
        assert_eq!(args.look_at_mode, LookAtMode::Adapted);
    }

    #[test]
    fn a_name_is_matched_case_insensitively() {
        let Ok(Invocation::Run(args)) = parse_args(&["--model", "hiyori"]) else {
            panic!("expected a run invocation");
        };

        assert!(args.model.ends_with("Hiyori.model3.json"));
    }

    #[test]
    fn an_unknown_name_lists_the_alternatives() {
        let error = parse_args(&["--model", "Nobody"]).unwrap_err();

        assert!(error.contains("available: Hiyori, Wanko"), "{error}");
    }

    #[test]
    fn the_look_at_mode_is_parsed() {
        let Ok(Invocation::Run(args)) = parse_args(&["-m", "Wanko", "-t", "viewer"]) else {
            panic!("expected a run invocation");
        };

        assert_eq!(args.look_at_mode, LookAtMode::ViewerDefault);
    }

    #[test]
    fn a_bad_look_at_mode_is_rejected() {
        assert!(parse_args(&["-t", "sideways"]).is_err());
    }

    #[test]
    fn list_and_help_short_circuit() {
        assert!(matches!(parse_args(&["--list"]), Ok(Invocation::List)));
        assert!(matches!(parse_args(&["-h"]), Ok(Invocation::Help)));
    }

    #[test]
    fn unknown_options_are_rejected() {
        assert!(parse_args(&["--frobnicate"]).is_err());
    }
}
