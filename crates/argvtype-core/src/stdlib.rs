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

/// Bitflags for efficient effect set operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub struct EffectSet(u16);

impl EffectSet {
    pub const NONE: EffectSet = EffectSet(0);
    pub const READS_FS: EffectSet = EffectSet(1 << 0);
    pub const WRITES_FS: EffectSet = EffectSet(1 << 1);
    pub const CHANGES_CWD: EffectSet = EffectSet(1 << 2);
    pub const MAY_EXEC: EffectSet = EffectSet(1 << 3);
    pub const NETWORK: EffectSet = EffectSet(1 << 4);
    pub const MUTATES_ENV: EffectSet = EffectSet(1 << 5);
    pub const MAY_EXIT: EffectSet = EffectSet(1 << 6);
    pub const MAY_SOURCE: EffectSet = EffectSet(1 << 7);
    pub const MAY_SPLIT: EffectSet = EffectSet(1 << 8);
    pub const MAY_GLOB: EffectSet = EffectSet(1 << 9);

    /// Conservative default for unknown external commands.
    pub const UNKNOWN_EXTERNAL: EffectSet = EffectSet(
        Self::MAY_EXEC.0 | Self::READS_FS.0 | Self::WRITES_FS.0,
    );

    pub const fn contains(self, other: EffectSet) -> bool {
        (self.0 & other.0) == other.0
    }

    pub const fn union(self, other: EffectSet) -> EffectSet {
        EffectSet(self.0 | other.0)
    }

    pub const fn intersects(self, other: EffectSet) -> bool {
        (self.0 & other.0) != 0
    }

    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// Returns true if this effect set could invalidate path proofs
    /// (writes_fs, changes_cwd, may_exec with unknown effects, may_source).
    pub const fn invalidates_path_proofs(self) -> bool {
        self.intersects(EffectSet(
            Self::WRITES_FS.0 | Self::CHANGES_CWD.0 | Self::MAY_SOURCE.0,
        ))
    }

    /// Convert from a CommandEffect enum variant.
    pub const fn from_command_effect(effect: CommandEffect) -> EffectSet {
        match effect {
            CommandEffect::ReadsFs => Self::READS_FS,
            CommandEffect::WritesFs => Self::WRITES_FS,
            CommandEffect::ChangesCwd => Self::CHANGES_CWD,
            CommandEffect::MayExec => Self::MAY_EXEC,
            CommandEffect::Network => Self::NETWORK,
            CommandEffect::MutatesEnv => Self::MUTATES_ENV,
            CommandEffect::MayExit => Self::MAY_EXIT,
        }
    }

    /// Convert from a `#@sig` effect name string.
    pub fn from_effect_name(name: &str) -> Option<EffectSet> {
        match name {
            "reads_fs" => Some(Self::READS_FS),
            "writes_fs" => Some(Self::WRITES_FS),
            "changes_cwd" => Some(Self::CHANGES_CWD),
            "may_exec" => Some(Self::MAY_EXEC),
            "network" => Some(Self::NETWORK),
            "mutates_env" => Some(Self::MUTATES_ENV),
            "may_exit" => Some(Self::MAY_EXIT),
            "may_source" => Some(Self::MAY_SOURCE),
            "may_split" => Some(Self::MAY_SPLIT),
            "may_glob" => Some(Self::MAY_GLOB),
            _ => None,
        }
    }
}

/// Compute the EffectSet for a known command from its CommandSig.
pub fn command_effects(sig: &CommandSig) -> EffectSet {
    let mut set = EffectSet::NONE;
    for &effect in sig.effects {
        set = set.union(EffectSet::from_command_effect(effect));
    }
    set
}

/// Look up the EffectSet for a command by name.
/// Returns UNKNOWN_EXTERNAL for unknown commands, NONE for builtins without effects.
pub fn lookup_effects(name: &str) -> EffectSet {
    // Builtins with known effects
    match name {
        "cd" => EffectSet::CHANGES_CWD,
        "source" | "." => EffectSet::MAY_SOURCE.union(EffectSet::MAY_EXEC),
        "eval" => EffectSet::MAY_EXEC.union(EffectSet::MAY_SOURCE),
        "exec" => EffectSet::MAY_EXEC.union(EffectSet::MAY_EXIT),
        "exit" | "return" => EffectSet::MAY_EXIT,
        "export" | "unset" | "declare" | "local" | "readonly" => EffectSet::MUTATES_ENV,
        // Safe builtins
        "echo" | "printf" | "true" | "false" | ":" | "test" | "[" | "[[" => EffectSet::NONE,
        "read" | "mapfile" | "readarray" => EffectSet::MUTATES_ENV,
        "shift" | "set" => EffectSet::MUTATES_ENV,
        _ => {
            // Check the command library
            if let Some(sig) = lookup_command(name) {
                command_effects(sig)
            } else {
                EffectSet::UNKNOWN_EXTERNAL
            }
        }
    }
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

    // EffectSet tests

    #[test]
    fn effect_set_empty() {
        assert!(EffectSet::NONE.is_empty());
        assert!(!EffectSet::READS_FS.is_empty());
    }

    #[test]
    fn effect_set_union() {
        let set = EffectSet::READS_FS.union(EffectSet::WRITES_FS);
        assert!(set.contains(EffectSet::READS_FS));
        assert!(set.contains(EffectSet::WRITES_FS));
        assert!(!set.contains(EffectSet::MAY_EXEC));
    }

    #[test]
    fn effect_set_intersects() {
        let set = EffectSet::READS_FS.union(EffectSet::WRITES_FS);
        assert!(set.intersects(EffectSet::READS_FS));
        assert!(!set.intersects(EffectSet::MAY_EXEC));
    }

    #[test]
    fn effect_set_invalidates_path_proofs() {
        assert!(EffectSet::WRITES_FS.invalidates_path_proofs());
        assert!(EffectSet::CHANGES_CWD.invalidates_path_proofs());
        assert!(EffectSet::MAY_SOURCE.invalidates_path_proofs());
        assert!(!EffectSet::READS_FS.invalidates_path_proofs());
        assert!(!EffectSet::MAY_EXEC.invalidates_path_proofs());
    }

    #[test]
    fn effect_set_from_name() {
        assert_eq!(EffectSet::from_effect_name("reads_fs"), Some(EffectSet::READS_FS));
        assert_eq!(EffectSet::from_effect_name("may_exec"), Some(EffectSet::MAY_EXEC));
        assert_eq!(EffectSet::from_effect_name("unknown_thing"), None);
    }

    #[test]
    fn lookup_effects_cd() {
        let effects = lookup_effects("cd");
        assert!(effects.contains(EffectSet::CHANGES_CWD));
    }

    #[test]
    fn lookup_effects_echo() {
        let effects = lookup_effects("echo");
        assert!(effects.is_empty());
    }

    #[test]
    fn lookup_effects_rm() {
        let effects = lookup_effects("rm");
        assert!(effects.contains(EffectSet::WRITES_FS));
    }

    #[test]
    fn lookup_effects_unknown() {
        let effects = lookup_effects("some_random_tool");
        assert_eq!(effects, EffectSet::UNKNOWN_EXTERNAL);
        assert!(effects.contains(EffectSet::MAY_EXEC));
    }

    #[test]
    fn lookup_effects_source() {
        let effects = lookup_effects("source");
        assert!(effects.contains(EffectSet::MAY_SOURCE));
    }

    #[test]
    fn command_effects_from_sig() {
        let sig = lookup_command("rm").unwrap();
        let effects = command_effects(sig);
        assert!(effects.contains(EffectSet::WRITES_FS));
        assert!(!effects.contains(EffectSet::READS_FS));
    }
}
