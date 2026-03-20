use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[repr(u8)]
pub enum Destructiveness {
    ReadOnly = 0,
    Modifying = 1,
    Destructive = 2,
    SystemAltering = 3,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[non_exhaustive]
pub enum CommandEffect {
    ReadsFs,
    WritesFs,
    ChangesCwd,
    MayExec,
    Network,
    MutatesEnv,
    MayExit,
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct KnownFlag {
    pub short: Option<&'static str>,
    pub long: Option<&'static str>,
    pub description: &'static str,
}

#[derive(Debug, Clone, Serialize)]
pub struct CommandSig {
    pub name: &'static str,
    pub destructiveness: Destructiveness,
    pub effects: &'static [CommandEffect],
    pub description: &'static str,
    pub known_flags: &'static [KnownFlag],
}

pub fn lookup_command(name: &str) -> Option<&'static CommandSig> {
    COMMANDS.iter().find(|c| c.name == name)
}

pub fn all_commands() -> &'static [CommandSig] {
    &COMMANDS
}

static COMMANDS: [CommandSig; 16] = [
    CommandSig {
        name: "echo",
        destructiveness: Destructiveness::ReadOnly,
        effects: &[],
        description: "Write arguments to standard output",
        known_flags: &[],
    },
    CommandSig {
        name: "printf",
        destructiveness: Destructiveness::ReadOnly,
        effects: &[],
        description: "Format and print data",
        known_flags: &[],
    },
    CommandSig {
        name: "cat",
        destructiveness: Destructiveness::ReadOnly,
        effects: &[CommandEffect::ReadsFs],
        description: "Concatenate and print files",
        known_flags: &[],
    },
    CommandSig {
        name: "grep",
        destructiveness: Destructiveness::ReadOnly,
        effects: &[CommandEffect::ReadsFs],
        description: "Search files for patterns",
        known_flags: &[],
    },
    CommandSig {
        name: "test",
        destructiveness: Destructiveness::ReadOnly,
        effects: &[CommandEffect::ReadsFs],
        description: "Evaluate conditional expressions",
        known_flags: &[],
    },
    CommandSig {
        name: "mkdir",
        destructiveness: Destructiveness::Modifying,
        effects: &[CommandEffect::WritesFs],
        description: "Create directories",
        known_flags: &[],
    },
    CommandSig {
        name: "chmod",
        destructiveness: Destructiveness::Modifying,
        effects: &[CommandEffect::WritesFs],
        description: "Change file mode bits",
        known_flags: &[
            KnownFlag { short: Some("-R"), long: Some("--recursive"), description: "Change files and directories recursively" },
        ],
    },
    CommandSig {
        name: "tee",
        destructiveness: Destructiveness::Modifying,
        effects: &[CommandEffect::WritesFs],
        description: "Read from stdin and write to stdout and files",
        known_flags: &[],
    },
    CommandSig {
        name: "cp",
        destructiveness: Destructiveness::Modifying,
        effects: &[CommandEffect::ReadsFs, CommandEffect::WritesFs],
        description: "Copy files and directories",
        known_flags: &[
            KnownFlag { short: Some("-r"), long: Some("--recursive"), description: "Copy directories recursively" },
        ],
    },
    CommandSig {
        name: "cd",
        destructiveness: Destructiveness::Modifying,
        effects: &[CommandEffect::ChangesCwd],
        description: "Change the working directory",
        known_flags: &[],
    },
    CommandSig {
        name: "curl",
        destructiveness: Destructiveness::Modifying,
        effects: &[CommandEffect::Network],
        description: "Transfer data from or to a server",
        known_flags: &[],
    },
    CommandSig {
        name: "rm",
        destructiveness: Destructiveness::Destructive,
        effects: &[CommandEffect::WritesFs],
        description: "Remove files or directories",
        known_flags: &[
            KnownFlag { short: Some("-r"), long: Some("--recursive"), description: "Remove directories and their contents recursively" },
            KnownFlag { short: Some("-f"), long: Some("--force"), description: "Ignore nonexistent files, never prompt" },
            KnownFlag { short: Some("-i"), long: None, description: "Prompt before every removal" },
        ],
    },
    CommandSig {
        name: "mv",
        destructiveness: Destructiveness::Destructive,
        effects: &[CommandEffect::ReadsFs, CommandEffect::WritesFs],
        description: "Move or rename files",
        known_flags: &[
            KnownFlag { short: Some("-f"), long: Some("--force"), description: "Do not prompt before overwriting" },
            KnownFlag { short: Some("-i"), long: Some("--interactive"), description: "Prompt before overwriting" },
        ],
    },
    CommandSig {
        name: "git",
        destructiveness: Destructiveness::Modifying,
        effects: &[CommandEffect::ReadsFs, CommandEffect::WritesFs, CommandEffect::Network, CommandEffect::MayExec],
        description: "Distributed version control system",
        known_flags: &[],
    },
    CommandSig {
        name: "docker",
        destructiveness: Destructiveness::SystemAltering,
        effects: &[CommandEffect::Network, CommandEffect::MayExec],
        description: "Container runtime",
        known_flags: &[],
    },
    CommandSig {
        name: "kubectl",
        destructiveness: Destructiveness::SystemAltering,
        effects: &[CommandEffect::Network, CommandEffect::MayExec],
        description: "Kubernetes command-line tool",
        known_flags: &[],
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lookup_rm() {
        let cmd = lookup_command("rm").expect("rm should be known");
        assert_eq!(cmd.destructiveness, Destructiveness::Destructive);
    }

    #[test]
    fn lookup_unknown() {
        assert!(lookup_command("some_custom_tool").is_none());
    }

    #[test]
    fn lookup_cat() {
        let cmd = lookup_command("cat").expect("cat should be known");
        assert_eq!(cmd.destructiveness, Destructiveness::ReadOnly);
    }

    #[test]
    fn all_commands_populated() {
        assert!(all_commands().len() >= 15);
    }

    #[test]
    fn destructiveness_ordering() {
        assert!(Destructiveness::ReadOnly < Destructiveness::Destructive);
        assert!(Destructiveness::Modifying < Destructiveness::SystemAltering);
    }

    #[test]
    fn rm_has_recursive_flag() {
        let cmd = lookup_command("rm").unwrap();
        assert!(
            cmd.known_flags.iter().any(|f| f.short == Some("-r")),
            "rm should have -r flag"
        );
    }

    #[test]
    fn lookup_kubectl() {
        let cmd = lookup_command("kubectl").expect("kubectl should be known");
        assert_eq!(cmd.destructiveness, Destructiveness::SystemAltering);
    }
}
