//! Arguments accepted by the guest's su client.

/// A shell invocation requested by an app.
#[derive(Debug, PartialEq, Eq)]
pub enum SuCommand {
    /// Run a shell, taking commands from standard input.
    Shell,
    /// Run a command through the shell.
    Command(String),
}

/// Parse the supported su forms without interpreting shell syntax.
pub fn parse(args: &[String]) -> Result<SuCommand, &'static str> {
    match args {
        [] => Ok(SuCommand::Shell),
        [flag, command] if flag == "-c" => Ok(SuCommand::Command(command.clone())),
        _ => Err("usage: su [-c command]"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fr35_su_accepts_a_shell_and_a_command() {
        assert_eq!(parse(&[]), Ok(SuCommand::Shell));
        assert_eq!(
            parse(&["-c".into(), "id; echo ok".into()]),
            Ok(SuCommand::Command("id; echo ok".into()))
        );
    }

    #[test]
    fn fr35_su_rejects_incomplete_or_unknown_options() {
        assert!(parse(&["-c".into()]).is_err());
        assert!(parse(&["--unknown".into()]).is_err());
        assert!(parse(&["-c".into(), "id".into(), "extra".into()]).is_err());
    }
}
