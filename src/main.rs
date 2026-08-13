use clap::Parser;

#[derive(Debug, Parser)]
#[command(version, about, long_about = None)]
struct Cli {}

fn main() {
    Cli::parse();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_args(values: &[&str]) -> clap::error::Result<Cli> {
        Cli::try_parse_from(std::iter::once(env!("CARGO_PKG_NAME")).chain(values.iter().copied()))
    }

    #[test]
    fn accepts_no_arguments() {
        assert!(parse_args(&[]).is_ok());
    }

    #[test]
    fn shows_help_and_version() {
        assert_eq!(
            parse_args(&["--help"]).unwrap_err().kind(),
            clap::error::ErrorKind::DisplayHelp
        );
        assert_eq!(
            parse_args(&["--version"]).unwrap_err().kind(),
            clap::error::ErrorKind::DisplayVersion
        );
    }

    #[test]
    fn rejects_unknown_arguments() {
        assert_eq!(
            parse_args(&["--unknown"]).unwrap_err().kind(),
            clap::error::ErrorKind::UnknownArgument
        );
    }
}
