//! Command line for the demo app: the model to open and the look-at convention.

use std::path::PathBuf;

use iced_live2d::LookAtMode;

pub const USAGE: &str = "\
Live2D · iced + mocari

USAGE:
    app <model.model3.json> [--track adapted|viewer]

OPTIONS:
    -t, --track <mode>   adapted: follow the rig's own conventions (default);
                         viewer: reproduce the Cubism Viewer's mouse bindings
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
    Help,
}

pub fn parse(args: impl IntoIterator<Item = String>) -> Result<Invocation, String> {
    let mut model: Option<PathBuf> = None;
    let mut look_at_mode = LookAtMode::default();
    let mut args = args.into_iter();

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-h" | "--help" => return Ok(Invocation::Help),
            "-t" | "--track" => {
                look_at_mode = args.next().ok_or("--track needs a mode")?.parse()?;
            }
            other if other.starts_with('-') => {
                return Err(format!("unknown option {other:?}"));
            }
            other if model.is_some() => {
                return Err(format!("unexpected extra path {other:?}"));
            }
            other => model = Some(PathBuf::from(other)),
        }
    }

    let model = model.ok_or("no model given - pass the path to a .model3.json")?;

    if !model.is_file() {
        return Err(format!("{} is not a file", model.display()));
    }

    Ok(Invocation::Run(Args { model, look_at_mode }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_args(args: &[&str]) -> Result<Invocation, String> {
        parse(args.iter().map(|arg| (*arg).to_owned()))
    }

    #[test]
    fn the_path_argument_is_the_model() {
        let Ok(Invocation::Run(args)) = parse_args(&["Cargo.toml"]) else {
            panic!("expected a run invocation");
        };

        assert_eq!(args.model, PathBuf::from("Cargo.toml"));
        assert_eq!(args.look_at_mode, LookAtMode::Adapted);
    }

    #[test]
    fn no_path_is_an_error() {
        let error = parse_args(&[]).unwrap_err();

        assert!(error.contains("no model given"), "{error}");
    }

    #[test]
    fn a_second_path_is_rejected() {
        let error = parse_args(&["Cargo.toml", "Cargo.lock"]).unwrap_err();

        assert!(error.contains("unexpected extra path"), "{error}");
    }

    #[test]
    fn a_missing_file_is_rejected() {
        let error = parse_args(&["nope.model3.json"]).unwrap_err();

        assert!(error.contains("is not a file"), "{error}");
    }

    #[test]
    fn the_look_at_mode_is_parsed() {
        let Ok(Invocation::Run(args)) = parse_args(&["--track", "viewer", "Cargo.toml"]) else {
            panic!("expected a run invocation");
        };

        assert_eq!(args.look_at_mode, LookAtMode::ViewerDefault);
    }

    #[test]
    fn a_bad_look_at_mode_is_rejected() {
        assert!(parse_args(&["-t", "sideways", "Cargo.toml"]).is_err());
    }

    #[test]
    fn help_short_circuits_without_a_path() {
        assert!(matches!(parse_args(&["-h"]), Ok(Invocation::Help)));
    }

    #[test]
    fn unknown_options_are_rejected() {
        assert!(parse_args(&["--frobnicate", "Cargo.toml"]).is_err());
    }
}
