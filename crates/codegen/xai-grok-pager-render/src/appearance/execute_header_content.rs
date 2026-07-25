//! Which fields an Execute/Bash tool header renders.

/// User-facing content policy for grok-pi Bash/run cards.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ExecuteHeaderContent {
    /// Render the syntax-highlighted command and suppress the task name.
    CommandOnly,
    /// Render the task name, falling back to the command when absent.
    #[default]
    TaskName,
    /// Render the task name followed by the syntax-highlighted command.
    TaskNameAndCommand,
}

impl ExecuteHeaderContent {
    pub const fn as_canonical(self) -> &'static str {
        match self {
            Self::CommandOnly => "command_only",
            Self::TaskName => "task_name",
            Self::TaskNameAndCommand => "task_name_and_command",
        }
    }

    pub fn from_canonical(value: &str) -> Option<Self> {
        match value {
            "command_only" => Some(Self::CommandOnly),
            "task_name" => Some(Self::TaskName),
            "task_name_and_command" => Some(Self::TaskNameAndCommand),
            _ => None,
        }
    }

    pub const fn shows_task_name(self) -> bool {
        !matches!(self, Self::CommandOnly)
    }

    pub const fn shows_command(self) -> bool {
        matches!(self, Self::CommandOnly | Self::TaskNameAndCommand)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonicals_round_trip() {
        for value in [
            ExecuteHeaderContent::CommandOnly,
            ExecuteHeaderContent::TaskName,
            ExecuteHeaderContent::TaskNameAndCommand,
        ] {
            assert_eq!(
                ExecuteHeaderContent::from_canonical(value.as_canonical()),
                Some(value)
            );
        }
        assert_eq!(ExecuteHeaderContent::from_canonical("unknown"), None);
        assert_eq!(
            ExecuteHeaderContent::default(),
            ExecuteHeaderContent::TaskName
        );
    }
}
